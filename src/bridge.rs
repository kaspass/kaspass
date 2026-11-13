//! Migrations through the KAT Bridge (KRC-20 to KCC-20), for holder rules like "migrated at least
//! 1,000,000 NACHO" or "burned at least 100,000 NACHO". A migration is permanent history tied to
//! the wallets that made it, so it can't be moved to a fresh wallet to claim again.
//!
//! How a migration shows up (all public):
//! - the bridge's setup, `GET {bridge}/v1/tokens`: the KRC-20 `vault` deposits go to, the
//!   `wallet` that pays out, and each token's KCC-20 covenant id and decimals;
//! - the deposit: an accepted KRC-20 `transfer` to the vault (Kasplex `oplist` of the vault);
//! - the payout: a KCC-20 transition from the bridge's wallet to the member (kascov's event list
//!   of the token, read forward by `after_seq`), usually within a minute or two;
//! - the burn isn't written anywhere, but it's exact: the payout is the deposit times
//!   (100 - p) / 100, rounded down, for a whole percent p, once both are in the same decimals.
//!
//! The deposits and payouts are kept in a small file per token next to the site
//! (`bridge-<TICK>.json`) and read forward from where they left off.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const BURN_ADDRESS: &str = "kaspa:qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqkx9awp4e";
/// A payout matches a deposit made at most this long before it (or a little after, clocks differ).
const MATCH_BEFORE_MS: i64 = 20 * 60_000;
const MATCH_AFTER_MS: i64 = 2 * 60_000;
/// Pages read per refresh, so a first read can't run away (it continues on the next refresh).
const MAX_PAGES: usize = 60;

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"))
}

fn bridge_api() -> String {
    crate::test_switch("KASPASS_BRIDGE_API").unwrap_or_else(|| "https://kcc-api-production.up.railway.app".into()).trim_end_matches('/').to_string()
}

static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Where the per-token files live (the site's folder). Called once at start.
pub fn set_dir(dir: &std::path::Path) {
    let _ = DIR.set(dir.to_path_buf());
}

#[derive(Clone, Debug)]
struct Setup {
    vault: String,
    reserve: String,
    covenant_id: String,
    kcc_dec: u32,
    src_dec: u32,
}

async fn setup(tick: &str) -> Option<Setup> {
    static CACHE: OnceLock<tokio::sync::Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    let v = {
        let mut g = cache.lock().await;
        match g.as_ref() {
            Some((t, v)) if t.elapsed() < Duration::from_secs(3600) => v.clone(),
            _ => {
                let r = http().get(format!("{}/v1/tokens", bridge_api())).send().await.ok()?;
                if !r.status().is_success() {
                    return None;
                }
                let v: Value = r.json().await.ok()?;
                *g = Some((Instant::now(), v.clone()));
                v
            }
        }
    };
    let t = v["tokens"].as_array()?.iter().find(|t| t["symbol"].as_str().map(|s| s.eq_ignore_ascii_case(tick)) == Some(true))?;
    let src = t["sources"].as_array()?.iter().find(|s| s["route"] == "krc20")?;
    Some(Setup {
        vault: v["krc20"]["vault"].as_str()?.to_string(),
        reserve: v["wallet"].as_str()?.to_string(),
        covenant_id: t["covenantId"].as_str()?.to_lowercase(),
        kcc_dec: t["kccDecimals"].as_u64()? as u32,
        src_dec: src["sourceDecimals"].as_u64().unwrap_or(8) as u32,
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Deposit {
    pub from: String,
    /// In the KRC-20 token's own decimals.
    pub amt: String,
    pub time_ms: i64,
    pub txid: String,
    pub score: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Payout {
    pub to: String,
    /// In the KCC-20 token's decimals.
    pub amt: String,
    pub time_ms: i64,
    pub txid: String,
    pub seq: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Book {
    vault: String,
    covenant_id: String,
    /// Kasplex op score of the newest deposit read, and the oldest (for the first read, which
    /// goes backwards and may take several refreshes).
    newest_score: u64,
    oldest_cursor: Option<String>,
    first_read_done: bool,
    deposits: Vec<Deposit>,
    next_seq: u64,
    payouts: Vec<Payout>,
}

/// One migration: what left the KRC-20 side and what arrived on the KCC-20 side.
#[derive(Clone, Debug, PartialEq)]
pub struct Migration {
    /// "mig:<deposit txid>": the same before and after its payout is matched.
    pub id: String,
    pub from: String,
    pub to: Option<String>,
    pub time_ms: i64,
    /// Whole tokens.
    pub sent: f64,
    pub burned: f64,
}

type Books = tokio::sync::Mutex<HashMap<String, (Instant, Book)>>;
fn books() -> &'static Books {
    static B: OnceLock<Books> = OnceLock::new();
    B.get_or_init(Default::default)
}

fn book_path(tick: &str) -> Option<std::path::PathBuf> {
    DIR.get().map(|d| d.join(format!("bridge-{}.json", tick.to_uppercase())))
}

fn kasplex_base() -> String {
    crate::test_switch("KASPASS_KRC20_API").unwrap_or_else(|| "https://api.kasplex.org/v1/krc20".into()).trim_end_matches('/').to_string()
}

fn kascov_base() -> String {
    crate::test_switch("KASPASS_KCC20_API").unwrap_or_else(|| "https://kascov.io/data/mainnet".into()).trim_end_matches('/').to_string()
}

/// One page of a wallet's KRC-20 ops for `tick`, newest first. (ops, next cursor)
async fn oplist(address: &str, tick: &str, next: Option<&str>) -> Option<(Vec<Value>, Option<String>)> {
    let mut q = vec![("address", address.to_string()), ("tick", tick.to_string())];
    if let Some(n) = next {
        q.push(("next", n.to_string()));
    }
    let r = http().get(format!("{}/oplist", kasplex_base())).query(&q).send().await.ok()?;
    if !r.status().is_success() {
        eprintln!("bridge: Kasplex answered {}", r.status());
        return None;
    }
    let v: Value = r.json().await.ok()?;
    let ops = v["result"].as_array().cloned().unwrap_or_default();
    let next = v["next"].as_str().filter(|s| !s.is_empty()).map(String::from).or_else(|| v["next"].as_u64().map(|n| n.to_string()));
    Some((ops, next))
}

fn deposit_of(op: &Value, vault: &str) -> Option<Deposit> {
    if op["op"] != "transfer" || op["opAccept"] != "1" || op["to"].as_str() != Some(vault) {
        return None;
    }
    Some(Deposit {
        from: op["from"].as_str()?.to_string(),
        amt: op["amt"].as_str()?.to_string(),
        time_ms: op["mtsAdd"].as_str().and_then(|t| t.parse().ok()).unwrap_or(0),
        txid: op["hashRev"].as_str()?.to_string(),
        score: op["opScore"].as_str().and_then(|t| t.parse().ok()).unwrap_or(0),
    })
}

/// Reads new deposits and payouts for `tick` (at most every 30 s unless `fresh`).
async fn refresh(tick: &str, fresh: bool) -> Option<()> {
    let tick = tick.to_uppercase();
    let st = setup(&tick).await?;
    let mut all = books().lock().await;
    let entry = all.entry(tick.clone()).or_insert_with(|| {
        let b = book_path(&tick).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Book>(&t).ok()).unwrap_or_default();
        (Instant::now() - Duration::from_secs(3600), b)
    });
    if !fresh && entry.0.elapsed() < Duration::from_secs(30) && entry.1.first_read_done && entry.1.next_seq > 0 {
        return Some(());
    }
    let book = &mut entry.1;
    if book.vault != st.vault || book.covenant_id != st.covenant_id {
        *book = Book { vault: st.vault.clone(), covenant_id: st.covenant_id.clone(), ..Default::default() };
    }
    let mut pages = 0;
    let score_of = |op: &Value| op["opScore"].as_str().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
    // 1. Deposits since the last read, newest first, until one already read.
    if book.newest_score > 0 {
        let mut cursor: Option<String> = None;
        let mut top = book.newest_score;
        'new: while pages < MAX_PAGES {
            let (ops, next) = oplist(&st.vault, &tick, cursor.as_deref()).await?;
            pages += 1;
            if ops.is_empty() {
                break;
            }
            for op in &ops {
                let score = score_of(op);
                if score <= book.newest_score {
                    break 'new;
                }
                top = top.max(score);
                if let Some(d) = deposit_of(op, &st.vault) {
                    if !book.deposits.iter().any(|x| x.txid == d.txid) {
                        book.deposits.push(d);
                    }
                }
            }
            match next {
                Some(n) if Some(&n) != cursor.as_ref() => cursor = Some(n),
                _ => break,
            }
        }
        book.newest_score = top;
    }
    // 2. The first read, going back in time; it picks up where it stopped on the next refresh.
    if !book.first_read_done {
        let mut cursor = book.oldest_cursor.clone();
        while pages < MAX_PAGES {
            let (ops, next) = oplist(&st.vault, &tick, cursor.as_deref()).await?;
            pages += 1;
            for op in &ops {
                book.newest_score = book.newest_score.max(score_of(op));
                if let Some(d) = deposit_of(op, &st.vault) {
                    if !book.deposits.iter().any(|x| x.txid == d.txid) {
                        book.deposits.push(d);
                    }
                }
            }
            match next {
                Some(n) if !ops.is_empty() && Some(&n) != cursor.as_ref() => {
                    cursor = Some(n.clone());
                    book.oldest_cursor = Some(n);
                }
                _ => {
                    book.first_read_done = true;
                    book.oldest_cursor = None;
                    break;
                }
            }
        }
    }
    // New payouts: the token's events, forward.
    let mut p = 0;
    while p < MAX_PAGES {
        let url = format!("{}/token/{}", kascov_base(), st.covenant_id);
        let r = http().get(&url).query(&[("after_seq", book.next_seq.saturating_sub(1).to_string())]).send().await.ok()?;
        if !r.status().is_success() {
            eprintln!("bridge: kascov answered {}", r.status());
            return None;
        }
        let v: Value = r.json().await.ok()?;
        let events = v["events"].as_array().cloned().unwrap_or_default();
        let tip_daa = v["tip_daa"].as_i64().unwrap_or(0);
        let tip_ms = v["tip_at_ms"].as_i64().unwrap_or(0);
        let mut max_seq = None;
        for e in &events {
            let seq = e["seq"].as_u64().unwrap_or(0);
            if seq < book.next_seq {
                continue;
            }
            max_seq = Some(max_seq.unwrap_or(0).max(seq));
            let from = e["owner_from_address"].as_str();
            let to = e["owner_to_address"].as_str();
            if from == Some(st.reserve.as_str()) && to.is_some_and(|t| t != st.reserve) {
                let daa = e["accepting_daa"].as_i64().unwrap_or(tip_daa);
                book.payouts.push(Payout {
                    to: to.unwrap().to_string(),
                    amt: e["amount_str"].as_str().map(String::from).unwrap_or_else(|| e["amount"].to_string()),
                    // About ten blocks a second on Kaspa.
                    time_ms: tip_ms - (tip_daa - daa) * 100,
                    txid: e["txid"].as_str().unwrap_or_default().to_string(),
                    seq,
                });
            }
        }
        p += 1;
        match max_seq {
            Some(m) => book.next_seq = m + 1,
            None => break,
        }
        if events.len() < 50 {
            break;
        }
    }
    entry.0 = Instant::now();
    if let Some(path) = book_path(&tick) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string(&entry.1).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
    Some(())
}

fn pow10(n: u32) -> u128 {
    10u128.pow(n.min(36))
}

/// The whole percent burned, if `payout` is `deposit` less a whole-percent burn (both in KCC
/// decimals after converting).
fn burn_percent(dep_conv: u128, payout: u128) -> Option<u32> {
    (0..=99u32).find(|p| dep_conv * (100 - *p as u128) / 100 == payout)
}

/// Pairs payouts with deposits. Each deposit and payout is used once.
fn migrations(book: &Book, st: &Setup) -> Vec<Migration> {
    let conv = |amt: &str| -> Option<u128> {
        let raw: u128 = amt.parse().ok()?;
        Some(if st.kcc_dec >= st.src_dec { raw * pow10(st.kcc_dec - st.src_dec) } else { raw / pow10(st.src_dec - st.kcc_dec) })
    };
    let mut deposits: Vec<&Deposit> = book.deposits.iter().collect();
    deposits.sort_by_key(|d| d.time_ms);
    let mut payouts: Vec<&Payout> = book.payouts.iter().collect();
    payouts.sort_by_key(|p| p.time_ms);
    let mut used = HashSet::new();
    let mut out = Vec::new();
    let tok = |x: u128| x as f64 / pow10(st.kcc_dec) as f64;
    for p in &payouts {
        let Ok(got) = p.amt.parse::<u128>() else { continue };
        let best = deposits
            .iter()
            .filter(|d| !used.contains(&d.txid) && d.time_ms >= p.time_ms - MATCH_BEFORE_MS && d.time_ms <= p.time_ms + MATCH_AFTER_MS)
            .filter_map(|d| conv(&d.amt).and_then(|c| burn_percent(c, got).map(|_| (d, c))))
            .min_by_key(|(d, _)| (p.time_ms - d.time_ms).abs());
        if let Some((d, c)) = best {
            used.insert(d.txid.clone());
            out.push(Migration { id: format!("mig:{}", d.txid), from: d.from.clone(), to: Some(p.to.clone()), time_ms: d.time_ms, sent: tok(c), burned: tok(c - got) });
        }
    }
    // Deposits with no payout (yet): still sent into the bridge, burn unknown.
    for d in deposits.iter().filter(|d| !used.contains(&d.txid)) {
        if let Some(c) = conv(&d.amt) {
            out.push(Migration { id: format!("mig:{}", d.txid), from: d.from.clone(), to: None, time_ms: d.time_ms, sent: tok(c), burned: 0.0 });
        }
    }
    out
}

/// Reads `tick`'s whole bridge history in the background (a first read can take a few rounds),
/// so the first member to check doesn't wait for it.
pub fn warm(tick: &str) {
    let tick = tick.to_uppercase();
    tokio::spawn(async move {
        for _ in 0..40 {
            if refresh(&tick, true).await.is_none() {
                return;
            }
            let done = books().lock().await.get(&tick).is_some_and(|(_, b)| b.first_read_done);
            if done {
                return;
            }
        }
    });
}

/// A member's migrations of `tick` (as the sender or the receiver), since `since_ms`, leaving out
/// ids in `used`. None if a source couldn't be read.
pub async fn of(tick: &str, wallets: &[String], since_ms: Option<i64>, used: &HashSet<String>, fresh: bool) -> Option<Vec<Migration>> {
    let tick_u = tick.to_uppercase();
    refresh(&tick_u, fresh).await?;
    let st = setup(&tick_u).await?;
    let all = books().lock().await;
    let book = &all.get(&tick_u)?.1;
    Some(
        migrations(book, &st)
            .into_iter()
            .filter(|m| wallets.contains(&m.from) || m.to.as_ref().is_some_and(|t| wallets.contains(t)))
            .filter(|m| since_ms.is_none_or(|s| m.time_ms >= s))
            .filter(|m| !used.contains(&m.id))
            .collect(),
    )
}

/// Plain KRC-20 burns of `tick` (transfers to the all-zero address) from these wallets: (id, whole tokens).
pub async fn plain_burns(tick: &str, wallets: &[String], since_ms: Option<i64>, used: &HashSet<String>) -> Option<Vec<(String, f64)>> {
    let st = setup(tick).await?;
    let mut out = Vec::new();
    for w in wallets {
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let (ops, next) = oplist(w, tick, cursor.as_deref()).await?;
            for op in &ops {
                if op["op"] == "transfer" && op["opAccept"] == "1" && op["from"].as_str() == Some(w.as_str()) && op["to"].as_str() == Some(BURN_ADDRESS) {
                    let t: i64 = op["mtsAdd"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0);
                    let id = format!("burn:{}", op["hashRev"].as_str().unwrap_or_default());
                    if since_ms.is_none_or(|s| t >= s) && !used.contains(&id) {
                        let raw: u128 = op["amt"].as_str().and_then(|a| a.parse().ok()).unwrap_or(0);
                        out.push((id, raw as f64 / pow10(st.src_dec) as f64));
                    }
                }
            }
            match next {
                Some(n) if Some(&n) != cursor.as_ref() && !ops.is_empty() => cursor = Some(n),
                _ => break,
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burns_and_matching() {
        let st = Setup { vault: "v".into(), reserve: "r".into(), covenant_id: "c".into(), kcc_dec: 6, src_dec: 8 };
        // Three made-up migrations: 0%, 10% and 50% burned.
        let dep = |from: &str, tokens: u128, t: i64, id: &str| Deposit { from: from.into(), amt: (tokens * 100_000_000).to_string(), time_ms: t, txid: id.into(), score: 0 };
        let pay = |to: &str, tokens: u128, t: i64, id: &str| Payout { to: to.into(), amt: (tokens * 1_000_000).to_string(), time_ms: t, txid: id.into(), seq: 0 };
        let book = Book {
            deposits: vec![dep("A", 300_000, 1_000, "d1"), dep("A", 10_000_000, 100_000, "d2"), dep("A", 5_000_000, 200_000, "d3"), dep("C", 7, 900_000, "d4")],
            payouts: vec![pay("A", 300_000, 50_000, "p1"), pay("B", 9_000_000, 150_000, "p2"), pay("A", 2_500_000, 260_000, "p3")],
            ..Default::default()
        };
        let m = migrations(&book, &st);
        let by = |id: &str| m.iter().find(|x| x.id == id).cloned().unwrap();
        assert_eq!((by("mig:d1").sent, by("mig:d1").burned), (300_000.0, 0.0));
        assert_eq!((by("mig:d2").sent, by("mig:d2").burned, by("mig:d2").to.as_deref()), (10_000_000.0, 1_000_000.0, Some("B")));
        assert_eq!((by("mig:d3").sent, by("mig:d3").burned), (5_000_000.0, 2_500_000.0));
        // A deposit with no payout still counts as sent (same id once its payout shows up).
        assert_eq!((by("mig:d4").sent, by("mig:d4").to.clone()), (7.0, None));
        assert_eq!(burn_percent(1_000_000, 900_000), Some(10));
        assert_eq!(burn_percent(1_000_000, 900_001), None);
    }
}
