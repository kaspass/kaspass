//! v5 prices: plans priced in a currency (CAD, USD or EUR), charged in KAS at today's rate.
//!
//! A v5 plan's on-chain price is a KAS ceiling (the most a period can ever cost). The server
//! reads KAS/USD and USD to the currency from Kraken every minute and signs a discount off the
//! ceiling so the buyer pays today's price. No fresh rate means no quote: purchases wait, and
//! auto-renewals wait and retry instead of charging the ceiling.

use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// What a v5 plan can be priced in: a currency, or KAS itself (then the on-chain price is the
/// real price, with no ceiling above it and no exchange rate needed).
pub const PLAN_CURRENCIES: &[&str] = &["CAD", "USD", "EUR", "KAS"];

/// A quote may use rates read at most this long ago.
pub const RATE_MAX_AGE_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Rates {
    pub kas_usd: f64,
    /// CAD per USD.
    pub usd_cad: f64,
    /// USD per EUR.
    pub eur_usd: f64,
    /// When Kraken was last read successfully (wall clock, ms).
    pub at_ms: i64,
}

static RATES: RwLock<Option<Rates>> = RwLock::new(None);

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Today's rates, if read recently enough to quote with.
pub fn fresh() -> Option<Rates> {
    let r = (*RATES.read().unwrap_or_else(|e| e.into_inner()))?;
    (wall_ms() - r.at_ms <= RATE_MAX_AGE_MS && r.kas_usd > 0.0).then_some(r)
}

/// The last rates read, however old (for display only).
pub fn last() -> Option<Rates> {
    *RATES.read().unwrap_or_else(|e| e.into_inner())
}

impl Rates {
    /// USD value of `amount` in `currency`.
    fn usd(&self, currency: &str, amount: f64) -> Option<f64> {
        match currency {
            "USD" => Some(amount),
            "CAD" if self.usd_cad > 0.0 => Some(amount / self.usd_cad),
            "EUR" if self.eur_usd > 0.0 => Some(amount * self.eur_usd),
            _ => None,
        }
    }

    /// KAS for `amount` in `currency`.
    /// No exchange rates: only KAS amounts convert (for plans priced in KAS).
    pub fn kas_only() -> Rates {
        Rates { kas_usd: f64::NAN, usd_cad: f64::NAN, eur_usd: f64::NAN, at_ms: 0 }
    }

    pub fn kas(&self, currency: &str, amount: f64) -> Option<f64> {
        if currency == "KAS" {
            return Some(amount);
        }
        self.usd(currency, amount).map(|u| u / self.kas_usd).filter(|k| k.is_finite())
    }
}

/// A second, independent reading of KAS in USD (CoinGecko, then the Kaspa REST API), used only
/// to check Kraken's. None when neither answers.
async fn second_kas_usd(http: &reqwest::Client) -> Option<f64> {
    let cg = async {
        let v: serde_json::Value = http.get("https://api.coingecko.com/api/v3/simple/price?ids=kaspa&vs_currencies=usd").send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
        v["kaspa"]["usd"].as_f64()
    };
    if let Some(x) = cg.await.filter(|x| *x > 0.0) {
        return Some(x);
    }
    let v: serde_json::Value = http.get("https://api.kaspa.org/info/price").send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    v["price"].as_f64().filter(|x| *x > 0.0)
}

/// Two readings this far apart (as a fraction) mean one of them is wrong: no quotes until they agree.
pub const MAX_SOURCE_GAP: f64 = 0.05;
/// A move bigger than this from the last accepted rate must hold for `HOLD_MS` before it's used.
pub const MAX_JUMP: f64 = 0.20;
pub const HOLD_MS: i64 = 5 * 60 * 1000;

fn gap(a: f64, b: f64) -> f64 {
    (a / b - 1.0).abs()
}

/// The checks every new reading passes before quotes use it. `last` is the rate in use,
/// `pending` a big move waiting out its hold (value, first seen). Returns what to do.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Use,
    /// Kraken and the second source disagree.
    Disagree(f64),
    /// A big move: wait until it has held for `HOLD_MS`.
    Hold,
}

pub fn judge(new: f64, second: Option<f64>, last: Option<f64>, pending: &mut Option<(f64, i64)>, now: i64) -> Verdict {
    if let Some(x) = second {
        if gap(new, x) > MAX_SOURCE_GAP {
            return Verdict::Disagree(x);
        }
    }
    match last {
        Some(l) if gap(new, l) > MAX_JUMP => match *pending {
            Some((p, since)) if gap(new, p) <= MAX_SOURCE_GAP => {
                if now - since >= HOLD_MS {
                    *pending = None;
                    Verdict::Use
                } else {
                    Verdict::Hold
                }
            }
            _ => {
                *pending = Some((new, now));
                Verdict::Hold
            }
        },
        _ => {
            *pending = None;
            Verdict::Use
        }
    }
}

/// Plausible ranges for the two currency rates: a reading outside them is a fault, not news.
pub const USD_CAD_RANGE: (f64, f64) = (1.0, 2.0);
pub const EUR_USD_RANGE: (f64, f64) = (0.8, 1.6);
/// The currency rates move slowly: a move bigger than this must hold for `HOLD_MS` as well.
pub const MAX_FX_JUMP: f64 = 0.05;
/// How long Kraken alone may decide after the second source stops answering.
pub const SECOND_GRACE_MS: i64 = 15 * 60 * 1000;
/// A rate saved before a restart is compared with for this long.
pub const SAVED_MAX_AGE_MS: i64 = 3 * 24 * 3600 * 1000;
const LAST_FILE: &str = "rates-last.json";

pub fn fx_in_range(r: &Rates) -> bool {
    (USD_CAD_RANGE.0..=USD_CAD_RANGE.1).contains(&r.usd_cad) && (EUR_USD_RANGE.0..=EUR_USD_RANGE.1).contains(&r.eur_usd)
}

/// What every reading is checked against: the last accepted rates (kept on disk across
/// restarts), big moves waiting out their hold, and when the second source last answered.
pub struct Guard {
    pub last: Option<Rates>,
    pending: Option<(f64, i64)>,
    pending_fx: Option<(f64, f64, i64)>,
    second_seen_ms: i64,
}

#[derive(Debug, PartialEq)]
pub enum Check {
    Use,
    Disagree(f64),
    Hold,
    /// A currency rate outside its plausible range.
    OddFx,
    /// No second reading to compare with (the first after a restart, or for too long).
    NoSecond,
}

impl Guard {
    pub fn new(last: Option<Rates>, now: i64) -> Self {
        Guard { last, pending: None, pending_fx: None, second_seen_ms: now }
    }

    pub fn check(&mut self, r: &Rates, second: Option<f64>, now: i64) -> Check {
        if !fx_in_range(r) {
            return Check::OddFx;
        }
        if second.is_some() {
            self.second_seen_ms = now;
        } else if self.last.is_none() || now - self.second_seen_ms > SECOND_GRACE_MS {
            return Check::NoSecond;
        }
        match judge(r.kas_usd, second, self.last.map(|l| l.kas_usd), &mut self.pending, now) {
            Verdict::Use => {}
            Verdict::Disagree(x) => return Check::Disagree(x),
            Verdict::Hold => return Check::Hold,
        }
        if let Some(l) = self.last {
            if gap(r.usd_cad, l.usd_cad) > MAX_FX_JUMP || gap(r.eur_usd, l.eur_usd) > MAX_FX_JUMP {
                return match self.pending_fx {
                    Some((c, e, since)) if gap(r.usd_cad, c) <= 0.01 && gap(r.eur_usd, e) <= 0.01 => {
                        if now - since >= HOLD_MS {
                            self.pending_fx = None;
                            Check::Use
                        } else {
                            Check::Hold
                        }
                    }
                    _ => {
                        self.pending_fx = Some((r.usd_cad, r.eur_usd, now));
                        Check::Hold
                    }
                };
            }
        }
        self.pending_fx = None;
        Check::Use
    }
}

fn load_last(site: &std::path::Path) -> Option<Rates> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(site.join(LAST_FILE)).ok()?).ok()?;
    let r = Rates { kas_usd: v["kas_usd"].as_f64()?, usd_cad: v["usd_cad"].as_f64()?, eur_usd: v["eur_usd"].as_f64()?, at_ms: v["at_ms"].as_i64()? };
    (r.kas_usd > 0.0 && fx_in_range(&r) && wall_ms() - r.at_ms < SAVED_MAX_AGE_MS).then_some(r)
}

fn save_last(site: &std::path::Path, r: &Rates) {
    let v = serde_json::json!({ "kas_usd": r.kas_usd, "usd_cad": r.usd_cad, "eur_usd": r.eur_usd, "at_ms": r.at_ms });
    let _ = write_atomic(&site.join(LAST_FILE), v.to_string().as_bytes());
}

/// Reads Kraken once: the middle of the best bid and ask (`a`, `b`) of KAS/USD, USD/CAD and EUR/USD.
async fn read_kraken(http: &reqwest::Client) -> anyhow::Result<Rates> {
    let v: serde_json::Value = http.get("https://api.kraken.com/0/public/Ticker?pair=KASUSD,USDCAD,EURUSD").send().await?.error_for_status()?.json().await?;
    if let Some(e) = v["error"].as_array().filter(|e| !e.is_empty()) {
        anyhow::bail!("kraken: {}", serde_json::Value::Array(e.clone()));
    }
    let res = v["result"].as_object().ok_or_else(|| anyhow::anyhow!("kraken: no result"))?;
    // The middle of the order book, not the last trade: one odd trade can't move it.
    let num = |t: &serde_json::Value, k: &str| -> Option<f64> { t[k][0].as_str().and_then(|x| x.parse::<f64>().ok()).filter(|x| *x > 0.0) };
    let last = |pick: &dyn Fn(&str) -> bool| -> Option<f64> {
        res.iter().find(|(k, _)| pick(k)).and_then(|(_, t)| match (num(t, "a"), num(t, "b")) {
            (Some(a), Some(b)) if a >= b => Some((a + b) / 2.0),
            _ => num(t, "c"),
        })
    };
    let kas_usd = last(&|k| k.contains("KAS")).ok_or_else(|| anyhow::anyhow!("kraken: no KAS/USD"))?;
    let usd_cad = last(&|k| k.contains("CAD")).ok_or_else(|| anyhow::anyhow!("kraken: no USD/CAD"))?;
    let eur_usd = last(&|k| k.contains("EUR")).ok_or_else(|| anyhow::anyhow!("kraken: no EUR/USD"))?;
    anyhow::ensure!(kas_usd > 0.0 && usd_cad > 0.0 && eur_usd > 0.0, "kraken: a rate is zero");
    Ok(Rates { kas_usd, usd_cad, eur_usd, at_ms: wall_ms() })
}

/// Keeps the rates fresh: Kraken every minute. In the simulation, `sim-rates.json` in the site
/// folder sets them (`{"kas_usd":..,"usd_cad":..,"eur_usd":..}`); no file = the feed is down.
pub fn start(site: std::path::PathBuf, sim: bool) {
    tokio::spawn(async move {
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().expect("http client");
        let mut failing = false;
        let mut pending: Option<(f64, i64)> = None;
        let mut guard = Guard::new(if sim { None } else { load_last(&site) }, wall_ms());
        let mut said = String::new();
        loop {
            let got = if sim {
                std::fs::read_to_string(site.join("sim-rates.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .map(|v| Rates { kas_usd: v["kas_usd"].as_f64().unwrap_or(0.0), usd_cad: v["usd_cad"].as_f64().unwrap_or(0.0), eur_usd: v["eur_usd"].as_f64().unwrap_or(0.0), at_ms: wall_ms() })
                    .ok_or_else(|| anyhow::anyhow!("no sim-rates.json"))
            } else {
                read_kraken(&http).await
            };
            match got {
                Ok(r) => {
                    let second = if sim {
                        std::fs::read_to_string(site.join("sim-rates.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["second_kas_usd"].as_f64())
                    } else {
                        second_kas_usd(&http).await
                    };
                    // The simulator moves its rates on purpose; the holds are covered by unit tests.
                    let check = if sim {
                        if !fx_in_range(&r) {
                            Check::OddFx
                        } else {
                            match judge(r.kas_usd, second, None, &mut pending, wall_ms()) {
                                Verdict::Disagree(x) => Check::Disagree(x),
                                _ => Check::Use,
                            }
                        }
                    } else {
                        guard.check(&r, second, wall_ms())
                    };
                    let last = guard.last.map(|x| x.kas_usd);
                    let note = match &check {
                        Check::Use => String::new(),
                        Check::Disagree(x) => format!("prices: Kraken says 1 KAS = {} USD but a second source says {x}; quotes wait until they agree", r.kas_usd),
                        Check::Hold => format!("prices: a rate moved a lot (KAS {} to {} USD, USD/CAD {}, EUR/USD {}); waiting 5 minutes to be sure before quoting at it", last.unwrap_or_default(), r.kas_usd, r.usd_cad, r.eur_usd),
                        Check::OddFx => format!("prices: Kraken's currency rates look wrong (USD/CAD {}, EUR/USD {}); quotes wait until they're back in range", r.usd_cad, r.eur_usd),
                        Check::NoSecond => "prices: no second price source is answering; quotes wait until one does".to_string(),
                    };
                    let verdict = if check == Check::Use { Verdict::Use } else { Verdict::Hold };
                    if !note.is_empty() && note != said {
                        eprintln!("{note}");
                    }
                    said = note;
                    if verdict == Verdict::Use {
                        if failing {
                            println!("prices: Kraken answers again (1 KAS = {} USD)", r.kas_usd);
                        }
                        failing = false;
                        if !sim {
                            guard.last = Some(r);
                            save_last(&site, &r);
                        }
                        *RATES.write().unwrap_or_else(|e| e.into_inner()) = Some(r);
                    }
                }
                Err(e) => {
                    if !failing {
                        eprintln!("prices: can't read rates ({e:#}); quotes wait until they're back");
                    }
                    failing = true;
                    if sim {
                        // A removed file means the feed is down right now.
                        *RATES.write().unwrap_or_else(|e| e.into_inner()) = None;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(if sim { 2 } else { 60 })).await;
        }
    });
}

/// A plan's price in its currency, set by the creator (level-prices.json, by covenant id).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LevelPrice {
    pub currency: String,
    pub amount: f64,
    /// A first-purchase price until a moment (ms), e.g. a founding deal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub founding: Option<Founding>,
    /// A lifetime pass, in the same currency (never renews).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<f64>,
    /// The commitment term (its length is fixed in the sales lane), in the same currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Founding {
    pub amount: f64,
    pub until_ms: i64,
}

pub const PRICES_FILE: &str = "level-prices.json";

pub fn level_prices(home: &Path) -> HashMap<String, LevelPrice> {
    std::fs::read_to_string(home.join(PRICES_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_level_prices(home: &Path, all: &HashMap<String, LevelPrice>) -> std::io::Result<()> {
    write_atomic(&home.join(PRICES_FILE), serde_json::to_string_pretty(all).unwrap_or_default().as_bytes())
}

/// What each pass was paid in its plan's currency, to value an upgrade credit (price-log.json,
/// key "covenant:owner_hash:paid_sompi", the pass's total paid after that payment).
pub const PAID_FILE: &str = "price-log.json";

pub fn paid_log(home: &Path) -> HashMap<String, (String, f64)> {
    std::fs::read_to_string(home.join(PAID_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Read, change and write the logs one at a time.
static LOGS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn log_paid(home: &Path, covenant: &str, owner_hash: &str, paid_sompi: i64, currency: &str, amount: f64) {
    let _g = LOGS.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = paid_log(home);
    all.insert(format!("{covenant}:{owner_hash}:{paid_sompi}"), (currency.to_string(), amount));
    // Keep it small: the newest 50,000 entries are plenty.
    if all.len() > 50_000 {
        let n = all.len() - 50_000;
        let drop: Vec<String> = all.keys().take(n).cloned().collect();
        for k in drop {
            all.remove(&k);
        }
    }
    let _ = write_atomic(&home.join(PAID_FILE), serde_json::to_string(&all).unwrap_or_default().as_bytes());
}

/// v5: what each payment was quoted, by transaction id (`payments-log.json`), for the owner's
/// payments export. A transaction id doesn't cover signatures, so it's known when the quote is made.
pub const PAYMENTS_FILE: &str = "payments-log.json";

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct PayNote {
    pub currency: String,
    /// What was charged, in the currency, after any credit.
    pub amount: f64,
    /// Upgrade credit, in the currency.
    #[serde(default)]
    pub credit: f64,
    /// "purchase", "upgrade", "renewal", "auto_renewal" or "promo" (a promotion's free pass,
    /// paid by the keeper wallet).
    pub kind: String,
}

/// Notes for transactions a wallet hasn't sent yet, written by `commit` once `/v1/submit` sends
/// them, so a quote nobody pays leaves nothing on disk.
enum Later {
    Pay(PayNote),
    Paid { covenant: String, owner: String, paid: i64, currency: String, amount: f64 },
}

static LATER: std::sync::Mutex<Vec<(String, i64, Later)>> = std::sync::Mutex::new(Vec::new());

fn later(txid: &str, l: Later) {
    let mut v = LATER.lock().unwrap_or_else(|e| e.into_inner());
    let now = wall_ms();
    v.retain(|(_, t, _)| now - t < 3_600_000);
    if v.len() >= 20_000 {
        v.remove(0);
    }
    v.push((txid.to_string(), now, l));
}

/// `log_payment`, once the transaction is sent.
pub fn later_payment(txid: &str, note: PayNote) {
    later(txid, Later::Pay(note));
}

/// `log_paid`, once the transaction is sent.
pub fn later_paid(txid: &str, covenant: &str, owner_hash: &str, paid_sompi: i64, currency: &str, amount: f64) {
    later(txid, Later::Paid { covenant: covenant.into(), owner: owner_hash.into(), paid: paid_sompi, currency: currency.into(), amount });
}

/// Writes the notes kept for `txid` (it has just been sent).
pub fn commit(home: &Path, txid: &str) {
    let mine: Vec<Later> = {
        let mut v = LATER.lock().unwrap_or_else(|e| e.into_inner());
        let (m, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *v).into_iter().partition(|(t, _, _)| t == txid);
        *v = rest;
        m.into_iter().map(|(_, _, l)| l).collect()
    };
    for l in mine {
        match l {
            Later::Pay(n) => log_payment(home, txid, n),
            Later::Paid { covenant, owner, paid, currency, amount } => log_paid(home, &covenant, &owner, paid, &currency, amount),
        }
    }
}

/// One writer at a time for the small JSON files, through a temporary file and a rename, so a
/// reader never sees half a file and two writers never mix.
static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let _g = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

pub fn payments_log(home: &Path) -> HashMap<String, PayNote> {
    std::fs::read_to_string(home.join(PAYMENTS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn log_payment(home: &Path, txid: &str, note: PayNote) {
    let _g = LOGS.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = payments_log(home);
    all.insert(txid.to_string(), note);
    let _ = write_atomic(&home.join(PAYMENTS_FILE), serde_json::to_string(&all).unwrap_or_default().as_bytes());
}

/// The discount (in millionths of the ceiling, rounded down) that brings `ceiling_sompi` to
/// `kas_today`, so the buyer pays today's price or a hair more, never less. Capped at `max_ppm`.
pub fn ppm_for(ceiling_sompi: u64, kas_today: f64, max_ppm: u64) -> u64 {
    if ceiling_sompi == 0 {
        return 0;
    }
    let ceiling = ceiling_sompi as f64 / 100_000_000.0;
    let frac = 1.0 - kas_today / ceiling;
    if frac <= 0.0 {
        return 0;
    }
    ((frac * 1_000_000.0).floor() as u64).min(max_ppm)
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    fn rates(kas: f64, cad: f64, eur: f64) -> Rates {
        Rates { kas_usd: kas, usd_cad: cad, eur_usd: eur, at_ms: 0 }
    }

    #[test]
    fn guard_checks_currency_rates_and_the_second_source() {
        let m = 60_000;
        // First reading after a restart with nothing saved: needs a second source.
        let mut g = Guard::new(None, 0);
        assert_eq!(g.check(&rates(0.1, 1.38, 1.08), None, 0), Check::NoSecond);
        assert_eq!(g.check(&rates(0.1, 1.38, 1.08), Some(0.101), 0), Check::Use);
        g.last = Some(rates(0.1, 1.38, 1.08));
        // Out of range currency rates are refused.
        assert_eq!(g.check(&rates(0.1, 13.8, 1.08), Some(0.1), m), Check::OddFx);
        assert_eq!(g.check(&rates(0.1, 1.38, 0.1), Some(0.1), m), Check::OddFx);
        // A big currency move must hold for 5 minutes.
        assert_eq!(g.check(&rates(0.1, 1.60, 1.08), Some(0.1), m), Check::Hold);
        assert_eq!(g.check(&rates(0.1, 1.60, 1.08), Some(0.1), 3 * m), Check::Hold);
        assert_eq!(g.check(&rates(0.1, 1.60, 1.08), Some(0.1), 7 * m), Check::Use);
        // Kraken alone is fine for a while, not for long.
        let mut g = Guard::new(Some(rates(0.1, 1.38, 1.08)), 0);
        assert_eq!(g.check(&rates(0.1, 1.38, 1.08), None, 10 * m), Check::Use);
        assert_eq!(g.check(&rates(0.1, 1.38, 1.08), None, 16 * m), Check::NoSecond);
        assert_eq!(g.check(&rates(0.1, 1.38, 1.08), Some(0.1), 17 * m), Check::Use);
    }

    #[test]
    fn a_second_source_that_disagrees_stops_quotes() {
        let mut p = None;
        assert_eq!(judge(0.10, Some(0.101), Some(0.1), &mut p, 0), Verdict::Use);
        assert!(matches!(judge(5.0, Some(0.1), Some(0.1), &mut p, 0), Verdict::Disagree(_)));
    }

    #[test]
    fn a_big_move_waits_five_minutes() {
        let mut p = None;
        assert_eq!(judge(0.5, None, Some(0.1), &mut p, 0), Verdict::Hold);
        assert_eq!(judge(0.51, None, Some(0.1), &mut p, 60_000), Verdict::Hold);
        assert_eq!(judge(0.5, None, Some(0.1), &mut p, HOLD_MS), Verdict::Use);
        // A spike that goes away starts over.
        let mut p = None;
        assert_eq!(judge(0.5, None, Some(0.1), &mut p, 0), Verdict::Hold);
        assert_eq!(judge(0.9, None, Some(0.1), &mut p, HOLD_MS), Verdict::Hold);
        // Small moves pass straight through.
        assert_eq!(judge(0.11, None, Some(0.1), &mut None, 0), Verdict::Use);
    }
}
