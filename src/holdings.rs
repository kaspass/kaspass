//! What a wallet holds: KRC-721 NFTs (KaspaCom's indexer), KRC-20 token balances (Kasplex) and
//! KCC-20 token balances (covenant tokens on Kaspa itself, read from kascov's index), plus holder
//! rules over several assets ("at least 1 NACHO NFT, or 100,000 NACHO tokens").
//!
//! Covenants can't see these assets: they live in indexers, so the server reads them and decides.
//! Any read that fails answers None, so an indexer outage never counts as "holds nothing".

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a holdings read is reused (KASPASS_HOLDINGS_TTL_S, default 60 s).
fn ttl() -> Duration {
    static T: OnceLock<Duration> = OnceLock::new();
    *T.get_or_init(|| Duration::from_secs(crate::test_switch("KASPASS_HOLDINGS_TTL_S").and_then(|v| v.parse().ok()).unwrap_or(60)))
}
const MAX_PAGES: usize = 200; // 10,000 NFTs per wallet per collection
const MAX_CACHE: usize = 20_000;

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"))
}

type NftCache = Mutex<HashMap<String, (Instant, Vec<(String, bool)>)>>;
type TokenCache = Mutex<HashMap<String, (Instant, f64)>>;

fn nft_cache() -> &'static NftCache {
    static C: OnceLock<NftCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn token_cache() -> &'static TokenCache {
    static C: OnceLock<TokenCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The KRC-721 indexer for a network (KASPASS_KRC721_API overrides it, for tests).
pub fn nft_indexer(network: &str) -> Option<String> {
    if let Some(u) = crate::test_switch("KASPASS_KRC721_API") {
        return Some(u.trim_end_matches('/').to_string());
    }
    match network {
        "mainnet" => Some("https://krc721-indexer.kaspa.com".into()),
        "testnet-10" => Some("https://dev-krc721-indexer.kaspa.com".into()),
        _ => None,
    }
}

/// The KCC-20 index (kascov) for a network (KASPASS_KCC20_API overrides it, for tests).
pub fn kcc20_api(network: &str) -> Option<String> {
    if let Some(u) = crate::test_switch("KASPASS_KCC20_API") {
        return Some(u.trim_end_matches('/').to_string());
    }
    match network {
        "mainnet" | "testnet-10" => Some(format!("https://kascov.io/data/{network}")),
        _ => None,
    }
}

/// A wallet's balance of the KCC-20 token with covenant id `token_id`, in whole tokens (the
/// token's declared decimals, else `dec`). KCC-20 tokens are covenant coins on Kaspa itself; their
/// owner is written inside each coin, so they can't be found by address on a node: kascov
/// indexes them. None if it couldn't be read.
pub async fn kcc20_balance(network: &str, token_id: &str, dec: Option<u32>, address: &str, fresh: bool) -> Option<f64> {
    if wrong_network(network, address) {
        return Some(0.0);
    }
    let base = kcc20_api(network)?;
    let key = format!("{base}|{token_id}|{address}");
    if !fresh {
        if let Some((t, v)) = token_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if t.elapsed() < ttl() {
                return Some(*v);
            }
        }
    }
    let url = format!("{base}/addr/{address}.json");
    let resp = match http().get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("holdings: KCC-20 index: {}", e.without_url());
            return None;
        }
    };
    if !resp.status().is_success() {
        eprintln!("holdings: KCC-20 index answered {}", resp.status());
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    let rows = v["token_holdings"].as_array()?;
    let mut found = 0.0;
    for r in rows.iter().filter(|r| r["token_id"].as_str().map(str::to_lowercase).as_deref() == Some(token_id)) {
        let raw = r["balance_str"].as_str().and_then(|b| b.parse::<f64>().ok()).or_else(|| r["balance"].as_f64()).unwrap_or(0.0);
        let d = r["claimed_decimals"].as_u64().or_else(|| r["listed_decimals"].as_u64()).map(|d| d as u32).or(dec).unwrap_or(0).min(30);
        found += raw / 10f64.powi(d as i32);
    }
    remember(token_cache(), key, &found);
    Some(found)
}

/// The Kasplex KRC-20 API for a network (KASPASS_KRC20_API overrides it, for tests).
pub fn token_api(network: &str) -> Option<String> {
    if let Some(u) = crate::test_switch("KASPASS_KRC20_API") {
        return Some(u.trim_end_matches('/').to_string());
    }
    match network {
        "mainnet" => Some("https://api.kasplex.org/v1/krc20".into()),
        "testnet-10" => Some("https://tn10api.kasplex.org/v1/krc20".into()),
        _ => None,
    }
}

fn wrong_network(network: &str, address: &str) -> bool {
    !address.starts_with(if network.starts_with("testnet") { "kaspatest:" } else { "kaspa:" })
}

fn remember<T: Clone>(cache: &Mutex<HashMap<String, (Instant, T)>>, key: String, v: &T) {
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() >= MAX_CACHE {
        c.retain(|_, (t, _)| t.elapsed() < ttl());
        if c.len() >= MAX_CACHE {
            c.clear();
        }
    }
    c.insert(key, (Instant::now(), v.clone()));
}

/// (token id, listed for sale) for every NFT of `tick` this wallet holds, from the indexer at
/// `base`. None if the indexer couldn't be read.
pub async fn nfts_at(base: &str, network: &str, tick: &str, address: &str, fresh: bool) -> Option<Vec<(String, bool)>> {
    if wrong_network(network, address) {
        return Some(Vec::new());
    }
    let key = format!("{base}|{network}|{}|{address}", tick.to_uppercase());
    if !fresh {
        if let Some((t, v)) = nft_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if t.elapsed() < ttl() {
                return Some(v.clone());
            }
        }
    }
    let mut out = Vec::new();
    let mut offset: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let url = format!("{base}/api/v1/krc721/{network}/address/{address}/{tick}");
        let mut req = http().get(&url);
        if let Some(o) = &offset {
            req = req.query(&[("offset", o)]);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("holdings: NFT indexer: {}", e.without_url());
                return None;
            }
        };
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // A wallet that never held the collection can come back as "not found"; that's zero.
            if status.as_u16() == 404 || v["message"].as_str().is_some_and(|m| m.to_lowercase().contains("not found")) {
                break;
            }
            eprintln!("holdings: NFT indexer answered {status}");
            return None;
        }
        for t in v["result"].as_array().into_iter().flatten() {
            let id = t["tokenId"].as_str().map(String::from).or_else(|| t["tokenId"].as_u64().map(|n| n.to_string()));
            if let Some(id) = id {
                let state = t["status"]["state"].as_str().unwrap_or("unlisted");
                if id.len() <= 64 {
                    out.push((id, state != "unlisted"));
                }
            }
        }
        if out.len() > 20_000 {
            break;
        }
        match &v["next"] {
            Value::Null => break,
            n => {
                let n = n.as_str().map(String::from).unwrap_or_else(|| n.to_string());
                if n.is_empty() || offset.as_deref() == Some(n.as_str()) {
                    break;
                }
                offset = Some(n);
            }
        }
    }
    remember(nft_cache(), key, &out);
    Some(out)
}

/// NFTs of `tick` on `network`, from the default indexer.
pub async fn nfts(network: &str, tick: &str, address: &str, fresh: bool) -> Option<Vec<(String, bool)>> {
    let base = nft_indexer(network)?;
    nfts_at(&base, network, tick, address, fresh).await
}

/// A wallet's KRC-20 balance of `tick`, in whole tokens (decimals applied). None if the API
/// couldn't be read.
pub async fn token_balance(network: &str, tick: &str, address: &str, fresh: bool) -> Option<f64> {
    if wrong_network(network, address) {
        return Some(0.0);
    }
    let base = token_api(network)?;
    let tick_u = tick.to_uppercase();
    let key = format!("{base}|{tick_u}|{address}");
    if !fresh {
        if let Some((t, v)) = token_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if t.elapsed() < ttl() {
                return Some(*v);
            }
        }
    }
    // The wallet's token list (`result[]` with `tick`, `balance` as an integer string, `dec`).
    let mut next: Option<String> = None;
    let mut found = 0.0;
    for _ in 0..50 {
        let url = format!("{base}/address/{address}/tokenlist");
        let mut req = http().get(&url);
        if let Some(n) = &next {
            req = req.query(&[("next", n)]);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("holdings: KRC-20 API: {}", e.without_url());
                return None;
            }
        };
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            if status.as_u16() == 404 {
                break;
            }
            eprintln!("holdings: KRC-20 API answered {status}");
            return None;
        }
        let list = v["result"].as_array().cloned().unwrap_or_default();
        let mut hit = false;
        for t in &list {
            if t["tick"].as_str().map(|x| x.to_uppercase()) != Some(tick_u.clone()) {
                continue;
            }
            found = amount_of(t);
            hit = true;
        }
        if hit {
            break;
        }
        match v["next"].as_str().filter(|n| !n.is_empty()) {
            Some(n) if next.as_deref() != Some(n) && !list.is_empty() => next = Some(n.to_string()),
            _ => break,
        }
    }
    remember(token_cache(), key, &found);
    Some(found)
}

/// `balance` (an integer string or number) divided by 10^`dec` (default 8).
fn amount_of(t: &Value) -> f64 {
    let raw = t["balance"].as_str().and_then(|b| b.parse::<f64>().ok()).or_else(|| t["balance"].as_f64()).unwrap_or(0.0);
    let dec = t["dec"].as_str().and_then(|d| d.parse::<i32>().ok()).or_else(|| t["dec"].as_i64().map(|d| d as i32)).unwrap_or(8).clamp(0, 30);
    raw / 10f64.powi(dec)
}

// ---------------------------------------------------------------------------
// Holder rules over several assets

/// One asset requirement: hold at least `min` NFTs of collection `tick` ("nft"), or at least
/// `min` whole tokens of KRC-20 `tick` ("token").
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Need {
    /// "nft" (KRC-721), "token" (KRC-20) or "kcc20" (a KCC-20 covenant token).
    pub kind: String,
    /// The ticker (for KCC-20, just the name shown, e.g. NACHO).
    pub tick: String,
    pub min: f64,
    /// KCC-20 only: the token's covenant id (64 hex characters), which is what's checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// KCC-20 only: decimals to use if the token doesn't declare them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dec: Option<u32>,
    /// "migrated" and "burned" only: count only what happened from this time on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    /// "burned" only: also count plain KRC-20 transfers to the burn address.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plain: bool,
}

impl Need {
    /// What identifies the asset: "nft:TICK", "token:TICK" or "kcc20:<covenant id>".
    pub fn key(&self) -> String {
        match (&self.kind[..], &self.id) {
            ("kcc20", Some(id)) => format!("kcc20:{id}"),
            _ => format!("{}:{}", self.kind, self.tick),
        }
    }
}

/// Several requirements: all of them, or any one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Needs {
    /// "all" or "any".
    #[serde(default = "all", rename = "match")]
    pub mode: String,
    pub list: Vec<Need>,
}

fn all() -> String {
    "all".into()
}

pub const MAX_NEEDS: usize = 6;

impl Needs {
    /// Cleans up and checks a rule set from the dashboard.
    pub fn checked(mut self) -> Result<Needs> {
        self.mode = match self.mode.trim().to_lowercase().as_str() {
            "any" => "any".into(),
            "all" | "" => "all".into(),
            _ => bail!("match must be \"all\" or \"any\""),
        };
        if self.list.is_empty() {
            bail!("add at least one thing holders must hold");
        }
        if self.list.len() > MAX_NEEDS {
            bail!("{MAX_NEEDS} requirements at most");
        }
        for n in &mut self.list {
            n.kind = n.kind.trim().to_lowercase();
            n.tick = n.tick.trim().to_uppercase();
            if !["nft", "token", "kcc20", "migrated", "burned"].contains(&n.kind.as_str()) {
                bail!("each requirement is \"nft\" (a KRC-721 collection), \"token\" (a KRC-20 token), \"kcc20\" (a KCC-20 token), \"migrated\" or \"burned\" (through the KAT Bridge)");
            }
            if n.kind == "migrated" || n.kind == "burned" {
                if n.since_ms.is_some_and(|t| t <= 0) {
                    n.since_ms = None;
                }
                if n.kind != "burned" {
                    n.plain = false;
                }
            } else {
                n.since_ms = None;
                n.plain = false;
            }
            if n.kind == "kcc20" {
                let id = n.id.as_deref().unwrap_or_default().trim().to_lowercase();
                if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                    bail!("a KCC-20 token needs its covenant id (64 letters and digits, from the token's official page)");
                }
                n.id = Some(id);
                if n.tick.is_empty() {
                    n.tick = "KCC-20".into();
                }
                if n.dec.is_some_and(|d| d > 30) {
                    bail!("decimals must be 30 or fewer");
                }
            } else {
                n.id = None;
                n.dec = None;
            }
            if n.tick.is_empty() || n.tick.len() > 20 || !n.tick.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                bail!("\"{}\" isn't a valid ticker", n.tick);
            }
            if !n.min.is_finite() || n.min <= 0.0 || n.min > 1e15 {
                bail!("the amount for {} must be more than zero", n.tick);
            }
            if n.kind == "nft" && (n.min.fract() != 0.0 || n.min > 1000.0) {
                bail!("the NFT count for {} must be a whole number from 1 to 1000", n.tick);
            }
        }
        let mut seen = HashSet::new();
        for n in &self.list {
            if !seen.insert(n.key()) {
                bail!("{} {} is listed twice; keep one line for it", n.tick, if n.kind == "nft" { "NFTs" } else { "tokens" });
            }
        }
        Ok(self)
    }

    /// Plain words for the page: "at least 1 NACHO NFT, or at least 100,000 NACHO tokens".
    pub fn describe(&self) -> String {
        let parts: Vec<String> = self.list.iter().map(Need::describe).collect();
        parts.join(if self.mode == "any" { ", or " } else { ", and " })
    }
}

impl Need {
    pub fn describe(&self) -> String {
        let n = fmt_amount(self.min);
        if self.kind == "nft" {
            format!("at least {n} {} NFT{}", self.tick, if self.min == 1.0 { "" } else { "s" })
        } else if self.kind == "kcc20" {
            format!("at least {n} {} (KCC-20) token{}", self.tick, if self.min == 1.0 { "" } else { "s" })
        } else if self.kind == "migrated" || self.kind == "burned" {
            let since = self.since_ms.map(|t| format!(" since {}", day(t))).unwrap_or_default();
            let what = if self.kind == "migrated" { "migrated at least" } else { "burned at least" };
            let to = if self.kind == "migrated" { " to KCC-20" } else if self.plain { " (bridge burns or plain burns)" } else { " when migrating" };
            format!("{what} {n} {}{to}{since}", self.tick)
        } else {
            format!("at least {n} {} token{}", self.tick, if self.min == 1.0 { "" } else { "s" })
        }
    }
}

/// "Nov 10, 2026" (UTC) from a time in milliseconds.
pub fn day(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d}, {y}", M[(m - 1) as usize])
}

fn fmt_amount(x: f64) -> String {
    let whole = x.trunc() as u128;
    let s = whole.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    let frac = x - x.trunc();
    if frac > 0.0 {
        let f = format!("{:.4}", frac);
        out.push_str(f.trim_start_matches('0').trim_end_matches('0'));
    }
    out
}

/// One requirement checked against a member's wallets.
#[derive(Serialize, Clone, Debug)]
pub struct Row {
    pub text: String,
    pub kind: String,
    pub tick: String,
    /// The asset's key (see `Need::key`).
    pub key: String,
    pub need: f64,
    pub have: f64,
    pub ok: bool,
}

/// A rule set checked against a member's wallets.
#[derive(Serialize, Clone, Debug)]
pub struct Checked {
    pub ok: bool,
    pub rows: Vec<Row>,
    /// For each NFT requirement that's met: the collection and the token ids that count (not
    /// listed for sale, each id once), in a stable order.
    #[serde(skip)]
    pub nft_ids: Vec<(String, Vec<String>)>,
    /// For each met "migrated" or "burned" requirement: its key and the migrations and burns
    /// that count ("mig:<deposit txid>", "burn:<txid>").
    #[serde(skip)]
    pub history_ids: Vec<(String, Vec<String>)>,
}

/// Checks `needs` against these wallets, skipping NFT ids in `used` ("TICK:id"). None if any
/// indexer couldn't be read. `fresh` reads the indexers now instead of using the last minute's answer.
pub async fn check(network: &str, needs: &Needs, wallets: &[String], used: &HashSet<String>, fresh: bool) -> Option<Checked> {
    let mut rows = Vec::new();
    let mut nft_ids = Vec::new();
    let mut history_ids = Vec::new();
    for n in &needs.list {
        if n.kind == "migrated" || n.kind == "burned" {
            let migs = crate::bridge::of(&n.tick, wallets, n.since_ms, used, fresh).await?;
            let (have, ids): (f64, Vec<String>) = if n.kind == "migrated" {
                (migs.iter().map(|m| m.sent).sum(), migs.iter().map(|m| m.id.clone()).collect())
            } else {
                let mut total: f64 = migs.iter().map(|m| m.burned).sum();
                let mut ids: Vec<String> = migs.iter().filter(|m| m.burned > 0.0).map(|m| m.id.clone()).collect();
                if n.plain {
                    for (id, amt) in crate::bridge::plain_burns(&n.tick, wallets, n.since_ms, used).await? {
                        total += amt;
                        ids.push(id);
                    }
                }
                (total, ids)
            };
            let ok = have + 1e-9 >= n.min;
            if ok {
                history_ids.push((n.key(), ids));
            }
            rows.push(Row { text: n.describe(), kind: n.kind.clone(), tick: n.tick.clone(), key: n.key(), need: n.min, have, ok });
            continue;
        }
        let (have, ids) = if n.kind == "nft" {
            let mut seen = HashSet::new();
            let mut ids = Vec::new();
            for w in wallets {
                for (id, listed) in nfts(network, &n.tick, w, fresh).await? {
                    if listed || used.contains(&format!("{}:{id}", n.tick)) || !seen.insert(id.clone()) {
                        continue;
                    }
                    ids.push(id);
                }
            }
            ids.sort_by(|a, b| match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                _ => a.cmp(b),
            });
            (ids.len() as f64, Some(ids))
        } else if n.kind == "kcc20" {
            let id = n.id.as_deref()?;
            let mut total = 0.0;
            for w in wallets {
                total += kcc20_balance(network, id, n.dec, w, fresh).await?;
            }
            (total, None)
        } else {
            let mut total = 0.0;
            for w in wallets {
                total += token_balance(network, &n.tick, w, fresh).await?;
            }
            (total, None)
        };
        let ok = have + 1e-9 >= n.min;
        if let (true, Some(ids)) = (ok, ids) {
            nft_ids.push((n.tick.clone(), ids));
        }
        rows.push(Row { text: n.describe(), kind: n.kind.clone(), tick: n.tick.clone(), key: n.key(), need: n.min, have, ok });
    }
    let ok = if needs.mode == "any" { rows.iter().any(|r| r.ok) } else { rows.iter().all(|r| r.ok) };
    Some(Checked { ok, rows, nft_ids, history_ids })
}

/// Parses `{"kind": "nft", "tick": "NACHO", "min": 1}` from a club rule's value.
pub fn need_from(kind: &str, v: &Value) -> Result<Need> {
    let tick = v["tick"].as_str().map(String::from).or_else(|| (kind == "kcc20").then(String::new)).ok_or_else(|| anyhow!("{kind} needs a \"tick\""))?;
    let min = v["min"].as_f64().unwrap_or(1.0);
    let id = v["id"].as_str().map(String::from);
    let dec = v["dec"].as_u64().map(|d| d as u32);
    let n = Needs { mode: "all".into(), list: vec![Need { kind: kind.into(), tick, min, id, dec, since_ms: None, plain: false }] }.checked()?;
    Ok(n.list.into_iter().next().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_are_checked_and_described() {
        let n: Needs = serde_json::from_value(serde_json::json!({"match": "ANY", "list": [{"kind": "NFT", "tick": "nacho", "min": 1}, {"kind": "token", "tick": "Nacho", "min": 100000}]})).unwrap();
        let n = n.checked().unwrap();
        assert_eq!(n.mode, "any");
        assert_eq!(n.list[0].tick, "NACHO");
        assert_eq!(n.describe(), "at least 1 NACHO NFT, or at least 100,000 NACHO tokens");
        let bad = Needs { mode: "all".into(), list: vec![Need { kind: "nft".into(), tick: "X".into(), min: 1.5, id: None, dec: None, since_ms: None, plain: false }] };
        assert!(bad.checked().is_err());
        let bad = Needs { mode: "all".into(), list: vec![Need { kind: "coin".into(), tick: "X".into(), min: 1.0, id: None, dec: None, since_ms: None, plain: false }] };
        assert!(bad.checked().is_err());
        assert!(Needs { mode: "all".into(), list: vec![] }.checked().is_err());
        let twice: Needs = serde_json::from_value(serde_json::json!({"list": [{"kind": "nft", "tick": "A", "min": 1}, {"kind": "nft", "tick": "a", "min": 2}]})).unwrap();
        assert!(twice.checked().is_err());
        assert_eq!(fmt_amount(1234567.5), "1,234,567.5");
        let k: Needs = serde_json::from_value(serde_json::json!({"list": [{"kind": "kcc20", "tick": "NACHO", "min": 1000, "id": "41D44970ba6307bcec418f677fd253778d0ffa47750d179dff647061aa6c0d40"}, {"kind": "token", "tick": "NACHO", "min": 5}]})).unwrap();
        let k = k.checked().unwrap();
        assert_eq!(k.list[0].key(), "kcc20:41d44970ba6307bcec418f677fd253778d0ffa47750d179dff647061aa6c0d40");
        assert_eq!(k.describe(), "at least 1,000 NACHO (KCC-20) tokens, and at least 5 NACHO tokens");
        let h: Needs = serde_json::from_value(serde_json::json!({"match": "any", "list": [{"kind": "migrated", "tick": "nacho", "min": 1000000, "since_ms": 1794268800000u64}, {"kind": "burned", "tick": "NACHO", "min": 100000, "plain": true}]})).unwrap();
        let h = h.checked().unwrap();
        assert_eq!(h.describe(), "migrated at least 1,000,000 NACHO to KCC-20 since Nov 10, 2026, or burned at least 100,000 NACHO (bridge burns or plain burns)");
        assert_eq!(day(0), "Jan 1, 1970");
        let bad: Needs = serde_json::from_value(serde_json::json!({"list": [{"kind": "kcc20", "tick": "NACHO", "min": 1}]})).unwrap();
        assert!(bad.checked().is_err());
    }

    #[test]
    fn token_amounts_use_decimals() {
        assert_eq!(amount_of(&serde_json::json!({"balance": "250000000", "dec": "8"})), 2.5);
        assert_eq!(amount_of(&serde_json::json!({"balance": "7", "dec": 0})), 7.0);
        assert_eq!(amount_of(&serde_json::json!({"balance": "100000000"})), 1.0);
    }
}
