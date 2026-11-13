//! Self-serve free trials (levels launched with minter_v5t): a member starts the plan's free trial
//! themselves. Their first period is set aside in their own pass, not paid to anyone; the keeper
//! charges it from the pass only at the very end of the trial, and "Cancel my trial" takes it back.
//! The set-aside money stays the member's: they can take it back at any time, so a trial costs
//! them only network fees. What limits free trials is the server's checks, before it approves one
//! (the voucher key co-signs):
//!   * one trial per wallet on this site (trial-claims.json, plus every trial on chain);
//!   * the wallet is new to this site (it has never held a pass here);
//!   * on mainnet, the wallet sent a payment at least 30 days ago (a public indexer);
//!   * at most a set number of new trials per plan per day (the owner is told when it's reached);
//!   * no trials on a plan with a limited number of passes (each trial would use one up).

use super::*;

const CLAIMS_FILE: &str = "trial-claims.json";
const CANCELS_FILE: &str = "trial-cancels.json";
/// How old a wallet must be (a payment it sent) to start a free trial, on mainnet.
pub const MIN_AGE_DAYS: i64 = 30;
const DAY_MS: i64 = 86_400_000;

/// How long before a trial ends the keeper charges the first period (members can cancel until then).
pub fn charge_window_ms() -> i64 {
    crate::test_switch("KASPASS_TRIAL_CHARGE_S").and_then(|v| v.parse::<i64>().ok()).map(|s| s * 1000).unwrap_or(15 * 60 * 1000)
}

/// The most new free trials one plan approves in 24 hours (`KASPASS_TRIAL_DAILY_MAX`, default 20).
pub fn daily_max() -> usize {
    std::env::var("KASPASS_TRIAL_DAILY_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(20)
}

#[derive(Serialize, Deserialize, Clone)]
struct Claim {
    level: String,
    at_ms: i64,
    txid: String,
    /// Approved and being sent; becomes a claim once sent, or is removed if the send failed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pending: bool,
}

/// One lock around reading and writing the claims, so two submits can't both pass the check.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The claims file. A file that's there but can't be read is an error: no trials until it's fixed.
fn claims(home: &std::path::Path) -> anyhow::Result<HashMap<String, Claim>> {
    match std::fs::read_to_string(home.join(CLAIMS_FILE)) {
        Ok(t) => Ok(serde_json::from_str(&t)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_claims(home: &std::path::Path, all: &HashMap<String, Claim>) -> std::io::Result<()> {
    crate::prices::write_atomic(&home.join(CLAIMS_FILE), serde_json::to_string(all).unwrap_or_default().as_bytes())
}

fn claims_broken() -> ApiError {
    eprintln!("trial: {CLAIMS_FILE} can't be read; free trials are paused until it's fixed or restored");
    (StatusCode::SERVICE_UNAVAILABLE, "free trials are paused on this site right now. You can still join.".into())
}

/// Trials prepared and waiting for the wallet's signature: txid -> (owner hash, level, when).
static PENDING: std::sync::LazyLock<std::sync::Mutex<HashMap<String, (String, String, i64)>>> = std::sync::LazyLock::new(Default::default);

/// Every free trial on chain on this site (any plan): (owner hash, level, txid, time).
async fn trials_on_chain(s: &AppState) -> Vec<(String, String, String, i64)> {
    let mut out = Vec::new();
    for l in s.levels.iter() {
        let snap = l.snap.read().await;
        out.extend(snap.events.iter().filter(|e| e.kind == "self_trial").map(|e| (e.owner_hash.clone(), l.card.covenant_id.clone(), e.txid.clone(), e.time_ms)));
    }
    out
}

/// How many free trials this plan approved in the last 24 hours (on chain, or being sent).
fn trials_today(all: &HashMap<String, Claim>, chain: &[(String, String, String, i64)], level: &str) -> usize {
    let since = wall_ms() - DAY_MS;
    let mut ids: std::collections::HashSet<&str> = all.values().filter(|c| c.level == level && c.at_ms >= since).map(|c| c.txid.as_str()).collect();
    ids.extend(chain.iter().filter(|(_, l, _, t)| l == level && *t >= since).map(|(_, _, id, _)| id.as_str()));
    ids.len()
}

/// Called by `/v1/submit` before sending: checks the wallet again and records the trial as being
/// sent, all under one lock. A prepared trial for a wallet that has had one since is refused.
pub async fn reserve(s: &AppState, txid: &str) -> Result<(), ApiError> {
    let Some((owner, level, _)) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).get(txid).cloned() else { return Ok(()) };
    let chain = trials_on_chain(s).await;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = claims(&s.home).map_err(|_| claims_broken())?;
    if all.get(&owner).is_some_and(|c| c.txid != txid) || chain.iter().any(|(o, _, _, _)| *o == owner) {
        return Err(bad("you've already had a free trial here"));
    }
    if trials_today(&all, &chain, &level) >= daily_max() {
        return Err(bad("this plan has given out all its free trials for today. Please try again tomorrow, or join now."));
    }
    all.insert(owner, Claim { level, at_ms: wall_ms(), txid: txid.to_string(), pending: true });
    save_claims(&s.home, &all).map_err(|e| {
        eprintln!("trial: can't save {CLAIMS_FILE} ({e})");
        claims_broken()
    })
}

/// Called by `/v1/submit` after sending: the trial now counts for this wallet.
pub fn commit(home: &std::path::Path, txid: &str) {
    let Some((owner, _, _)) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).remove(txid) else { return };
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(mut all) = claims(home) else { return };
    if let Some(c) = all.get_mut(&owner).filter(|c| c.txid == txid) {
        c.pending = false;
        if let Err(e) = save_claims(home, &all) {
            eprintln!("trial: can't save {CLAIMS_FILE} ({e}); the trial is still counted from the chain");
        }
    }
}

/// Called by `/v1/submit` when sending failed: the wallet can try again. (If the network took it
/// after all, the trial shows up on chain and counts from there.)
pub fn release(home: &std::path::Path, txid: &str) {
    let Some((owner, _, _)) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).get(txid).cloned() else { return };
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(mut all) = claims(home) else { return };
    if all.get(&owner).is_some_and(|c| c.txid == txid && c.pending) {
        all.remove(&owner);
        let _ = save_claims(home, &all);
    }
}

fn cancels(home: &std::path::Path) -> HashMap<String, i64> {
    std::fs::read_to_string(home.join(CANCELS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The member pressed "Cancel my trial" (signed in as the pass's owner): from now on the keeper
/// leaves that trial alone, even if taking the money back hasn't gone through yet.
/// `false` takes it back (they added money to keep the plan after all).
pub fn set_cancelled(home: &std::path::Path, level: &str, owner_hash: &str, on: bool) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = cancels(home);
    let now = wall_ms();
    all.retain(|_, t| now - *t < 90 * DAY_MS);
    let key = format!("{level}:{owner_hash}");
    let changed = if on { all.insert(key, now).is_none() } else { all.remove(&key).is_some() };
    if changed {
        if let Err(e) = crate::prices::write_atomic(&home.join(CANCELS_FILE), serde_json::to_string(&all).unwrap_or_default().as_bytes()) {
            eprintln!("trial: can't save {CANCELS_FILE} ({e})");
        }
    }
}

/// Whether the member cancelled this trial (the keeper skips it).
pub fn cancelled(home: &std::path::Path, level: &str, owner_hash: &str) -> bool {
    cancels(home).contains_key(&format!("{level}:{owner_hash}"))
}

/// From one page of the indexer's answer: Some(true) when `address` sent a payment before
/// `cutoff`, Some(false) when it didn't, None when some times are missing (can't tell).
fn spent_before(page: &serde_json::Value, address: &str, cutoff: i64) -> Option<bool> {
    let mut unsure = false;
    for t in page.as_array().map(Vec::as_slice).unwrap_or_default() {
        let spent = t["inputs"].as_array().is_some_and(|ins| ins.iter().any(|i| i["previous_outpoint_address"].as_str() == Some(address)));
        if !spent {
            continue;
        }
        match t["block_time"].as_i64() {
            Some(bt) if bt < cutoff => return Some(true),
            Some(_) => {}
            None => unsure = true,
        }
    }
    (!unsure).then_some(false)
}

fn http() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().expect("http client"))
}

/// Whether `address` sent a payment at least `days` ago, from a public Kaspa indexer. Receiving
/// doesn't count: one payment can give any number of new addresses a history, but each address
/// must spend for itself. Cached for a day when old enough, ten minutes when not. Err = can't tell.
async fn old_enough(network: &str, address: &str, days: i64) -> anyhow::Result<bool> {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, (bool, std::time::Instant)>>> = std::sync::LazyLock::new(Default::default);
    if let Some((ok, t)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(address).cloned() {
        if t.elapsed() < std::time::Duration::from_secs(if ok { 86_400 } else { 600 }) {
            return Ok(ok);
        }
    }
    let base = crate::test_switch("KASPASS_KASPA_API").unwrap_or_else(|| match network {
        "mainnet" => "https://api.kaspa.org".into(),
        _ => "https://api-tn10.kaspa.org".into(),
    });
    let cutoff = wall_ms() - days * DAY_MS;
    let url = format!("{}/addresses/{}/full-transactions-page", base.trim_end_matches('/'), address);
    let mut before = cutoff;
    let mut ok = None;
    // Up to three pages of 100 transactions from before the cutoff.
    for _ in 0..3 {
        let r = http()
            .get(&url)
            .query(&[("limit", "100"), ("before", &before.to_string()), ("resolve_previous_outpoints", "light")])
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
        let next = r.headers().get("x-next-page-before").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<i64>().ok());
        let page: serde_json::Value = r.json().await.map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
        match spent_before(&page, address, cutoff) {
            Some(true) => {
                ok = Some(true);
                break;
            }
            None => anyhow::bail!("the indexer left out some transaction times"),
            Some(false) => ok = Some(false),
        }
        match next {
            Some(n) if n > 0 && n < before && page.as_array().is_some_and(|a| !a.is_empty()) => before = n,
            _ => break,
        }
    }
    let ok = ok.unwrap_or(false);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() >= 5_000 {
        c.retain(|_, (_, t)| t.elapsed() < std::time::Duration::from_secs(600));
        if c.len() >= 5_000 {
            c.clear();
        }
    }
    c.insert(address.to_string(), (ok, std::time::Instant::now()));
    Ok(ok)
}

/// The first period to prepare into the trial pass: today's price for one period (with the usual
/// prepaid cushion for KAS moves), and what that is in the plan's currency.
async fn first_period(s: &AppState, shop: &crate::shop::Shop) -> Result<(u64, String, f64), ApiError> {
    let lane = current_lane(s).await?;
    let price = crate::prices::level_prices(&s.home).get(&s.card.covenant_id).cloned();
    let (due, currency, amount) = match price {
        Some(p) if p.currency != "KAS" => {
            let rates = crate::prices::fresh().ok_or((StatusCode::SERVICE_UNAVAILABLE, "today's price isn't available right now; please try again in a minute".to_string()))?;
            let kas_today = rates.kas(&p.currency, p.amount).ok_or((StatusCode::SERVICE_UNAVAILABLE, "today's price isn't available right now".to_string()))?;
            let max_ppm = s.card.settings.v4.as_ref().map(|v| v.max_discount_bps * 100).unwrap_or(0);
            (crate::discounted_ppm(lane.lane.price, crate::prices::ppm_for(lane.lane.price, kas_today, max_ppm)), p.currency, p.amount)
        }
        _ => (lane.lane.price, "KAS".into(), lane.lane.price as f64 / crate::SOMPI as f64),
    };
    Ok((shop.prepaid_margin(due), currency, amount))
}

/// Why this wallet can't start a trial here, in plain words (None = it can).
async fn blocker(s: &AppState, address: &str) -> Option<(&'static str, String)> {
    let owner = owner_hash_of(address).ok()?.to_string();
    let chain = trials_on_chain(s).await;
    let all = match claims(&s.home) {
        Ok(a) => a,
        Err(_) => return Some(("paused", claims_broken().1)),
    };
    if all.contains_key(&owner) || chain.iter().any(|(o, _, _, _)| *o == owner) {
        return Some(("used", "You've already had a free trial here.".into()));
    }
    // New members only: anyone who holds a pass here, or ever has, can join or renew instead.
    for l in s.levels.iter() {
        let snap = l.snap.read().await;
        if snap.passes.iter().any(|p| p.owner_hash == owner) || snap.events.iter().any(|e| e.owner_hash == owner) {
            return Some(("member", "Free trials are for new members. You can join or renew below.".into()));
        }
    }
    if trials_today(&all, &chain, &s.card.covenant_id) >= daily_max() {
        return Some(("today", "This plan has given out all its free trials for today. Please try again tomorrow, or join now.".into()));
    }
    if s.card.network == "mainnet" {
        match old_enough(&s.card.network, address, MIN_AGE_DAYS).await {
            Ok(true) => {}
            Ok(false) => return Some(("too_new", format!("Free trials are for wallets that sent a payment at least {MIN_AGE_DAYS} days ago, and this one hasn't yet. You can still join now."))),
            Err(e) => {
                eprintln!("trial: can't check a wallet's age ({e:#})");
                return Some(("cant_check", "We can't check your wallet right now. Please try again in a minute.".into()));
            }
        }
    }
    None
}

/// Whether free trials can run on this plan at all right now (a reason when they can't).
async fn plan_blocker(s: &AppState) -> Option<String> {
    let lane = current_lane(s).await.ok()?;
    if lane.lane.remaining > 0 {
        return Some("free trials are off on this plan because it has a limited number of passes".into());
    }
    if s.card.settings.trial_ms <= charge_window_ms() {
        return Some("this plan's free trial is too short to start here".into());
    }
    None
}

/// Tells the owner, once a day per plan, that today's free trials have all been given out.
async fn tell_owner_full(s: &AppState) {
    static TOLD: std::sync::LazyLock<std::sync::Mutex<HashMap<String, i64>>> = std::sync::LazyLock::new(Default::default);
    let now = wall_ms();
    {
        let mut t = TOLD.lock().unwrap_or_else(|e| e.into_inner());
        if t.get(&s.card.covenant_id).is_some_and(|at| now - at < DAY_MS) {
            return;
        }
        t.insert(s.card.covenant_id.clone(), now);
    }
    eprintln!("trial: {} reached its {} free trials for today", short_id(&s.card.covenant_id), daily_max());
    let text = format!(
        "🆓 {} has given out {} free trials in the last 24 hours, the most allowed in a day. New trials wait until tomorrow; people can still join. If this looks wrong, someone may be collecting free trials with many wallets.",
        s.level_name(),
        daily_max()
    );
    if let Some(owner) = s.shop.as_ref().and_then(|sh| sh.record()).and_then(|r| owner_hash_of(&r.owner).ok()) {
        dm_owner(s, &owner.to_string(), &text).await;
    }
}

#[derive(Deserialize)]
pub struct InfoQuery {
    #[serde(default)]
    address: Option<String>,
}

/// `GET /v1/trial?level=&address=`: whether the plan offers a self-serve trial, how long, what the
/// first period costs, and (for the signed-in wallet) whether it can start one.
pub async fn info(State(s): State<AppState>, headers: HeaderMap, Query(lq): Query<LevelQ>, Query(q): Query<InfoQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let Some(shop) = s.shop.clone().filter(|sh| sh.self_trial()) else {
        return Ok(Json(serde_json::json!({ "offered": false })));
    };
    if plan_blocker(&s).await.is_some() {
        return Ok(Json(serde_json::json!({ "offered": false })));
    }
    let mut out = serde_json::json!({
        "offered": true,
        "trial_ms": s.card.settings.trial_ms,
        "min_age_days": if s.card.network == "mainnet" { MIN_AGE_DAYS } else { 0 },
        "rule_on_mainnet_days": MIN_AGE_DAYS,
        "charge_window_ms": charge_window_ms(),
    });
    if let Ok((sompi, currency, amount)) = first_period(&s, &shop).await {
        out["first_period"] = serde_json::json!({ "sompi": sompi, "currency": currency, "amount": amount });
    }
    if let Some(a) = q.address.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        if signed_in_as(&s, &headers, a).is_ok() {
            match blocker(&s, a).await {
                None => out["eligible"] = serde_json::json!(true),
                Some(("today", text)) => {
                    tell_owner_full(&s).await;
                    out["eligible"] = serde_json::json!(false);
                    out["reason"] = serde_json::json!("today");
                    out["reason_text"] = serde_json::json!(text);
                }
                Some((reason, text)) => {
                    out["eligible"] = serde_json::json!(false);
                    out["reason"] = serde_json::json!(reason);
                    out["reason_text"] = serde_json::json!(text);
                }
            }
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct StartRequest {
    address: String,
}

/// `POST /v1/trial/prepare?level= {address}` (session header): the transaction that starts the
/// trial, for the member's wallet to sign; then `/v1/submit` as for any purchase.
pub async fn prepare(State(s): State<AppState>, headers: HeaderMap, Query(lq): Query<LevelQ>, Json(req): Json<StartRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let address = req.address.trim().to_string();
    signed_in_as(&s, &headers, &address)?;
    let shop = shop_of(&s)?;
    if !shop.self_trial() {
        return Err(bad("this plan doesn't offer a free trial"));
    }
    if let Some(why) = plan_blocker(&s).await {
        return Err(bad(why));
    }
    if let Some((reason, text)) = blocker(&s, &address).await {
        if reason == "today" {
            tell_owner_full(&s).await;
        }
        return Err(bad(text));
    }
    no_duplicate(&s, &address).await?;
    let (prepaid, currency, amount) = first_period(&s, &shop).await?;
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        let lane = current_lane(&s).await?;
        if lane.lane.remaining >= 0 {
            return Err(bad("free trials are off on this plan because it has a limited number of passes"));
        }
        shop.prepare_self_trial(&chain, &address, &lane, prepaid).await.map_err(bad)?
    };
    let owner = owner_hash_of(&address).map(|h| h.to_string()).unwrap_or_default();
    {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let now = wall_ms();
        p.retain(|_, (_, _, t)| now - *t < 3_600_000);
        p.insert(quote.txid(), (owner, s.card.covenant_id.clone(), now));
    }
    out["price"] = serde_json::json!({ "currency": currency, "amount": amount, "kas_today": prepaid as f64 / crate::SOMPI as f64, "trial": true });
    let id = store_quote(&s, quote);
    out["quote"] = serde_json::json!(id);
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::spent_before;
    use serde_json::json;

    #[test]
    fn only_spending_counts_as_history() {
        let me = "kaspa:me";
        let cutoff = 1_000;
        // Received long ago, never spent: not old enough.
        let got = json!([{ "block_time": 10, "inputs": [{ "previous_outpoint_address": "kaspa:friend" }] }]);
        assert_eq!(spent_before(&got, me, cutoff), Some(false));
        // Spent long ago: old enough.
        let sent = json!([{ "block_time": 10, "inputs": [{ "previous_outpoint_address": me }] }]);
        assert_eq!(spent_before(&sent, me, cutoff), Some(true));
        // Spent, but only recently: not old enough.
        let recent = json!([{ "block_time": 5_000, "inputs": [{ "previous_outpoint_address": me }] }]);
        assert_eq!(spent_before(&recent, me, cutoff), Some(false));
        // Spent with no time given: can't tell.
        let no_time = json!([{ "inputs": [{ "previous_outpoint_address": me }] }]);
        assert_eq!(spent_before(&no_time, me, cutoff), None);
        // Nothing at all.
        assert_eq!(spent_before(&json!([]), me, cutoff), Some(false));
    }
}
