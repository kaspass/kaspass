//! Holder promotions ("NACHO holders: your first month is free"). The creator sets one campaign
//! in the dashboard: which plan, who qualifies (what they must hold), when it runs and how many
//! claims it allows. A qualifying member signs in and presses "Claim my free month"; the server's
//! keeper wallet buys them a normal pass on that plan at about 1 KAS (which goes to the creator's
//! vault) and pays its refundable deposit. Nobody approves each claim.
//!
//! Each NFT token id claims once per campaign (wherever it moves afterwards), and each member
//! (a wallet and the wallets linked to it) once per campaign. Holdings come from indexers
//! (see holdings.rs); covenants can't see them, so this server decides who qualifies.

use super::*;
use crate::holdings::Needs;

const PROMO_FILE: &str = "promo.json";
const CLAIMS_FILE: &str = "promo-claims.json";
/// What each free pass sends to the creator's vault (a near-zero payment would make an output too
/// small for Kaspa's storage-mass rules).
const CHARGE_KAS: f64 = 1.0;
/// Kept in the keeper on top of each claim's cost, for its network fee.
const KEEPER_SPARE: u64 = crate::SOMPI / 2;
/// Never spent on promotions: left for auto-renewal fees.
const KEEPER_FLOOR: u64 = crate::SOMPI;
/// Claims of past campaigns kept (the running campaign's are always kept).
const MAX_OLD_CLAIMS: usize = 20_000;
/// A claim whose transaction may or may not have gone through counts until this long has passed
/// without its pass showing up on chain.
const UNSURE_MS: i64 = 5 * 60_000;
/// The most claims one campaign can allow.
const MAX_CAMPAIGN_CLAIMS: u64 = 50_000;
const MAX_LABEL: usize = 80;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Promo {
    /// A new id for each campaign, so token ids and wallets can claim again in the next one.
    pub id: String,
    /// What the plan's card says, e.g. "NACHO holders: your first month is free".
    pub label: String,
    /// The plan (level covenant id) whose pass is given.
    pub level: String,
    pub needs: Needs,
    pub start_ms: i64,
    pub end_ms: i64,
    /// 0 = no limit.
    #[serde(default)]
    pub max_claims: u64,
    /// Ended early by the creator.
    #[serde(default)]
    pub ended: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Claim {
    pub campaign: String,
    pub wallet: String,
    /// NFTs used, as "TICK:id".
    #[serde(default)]
    pub nfts: Vec<String>,
    pub txid: String,
    pub at_ms: i64,
    pub level: String,
    /// What went to the vault.
    pub charge_sompi: u64,
    /// The campaign's line when it was claimed ("NACHO holders: your first month is free").
    #[serde(default)]
    pub label: String,
    /// Every wallet of the member when they claimed (their wallet and the linked ones).
    #[serde(default)]
    pub group: Vec<String>,
    /// "done"; "pending" (being sent) or "unsure" (sending reported an error, but it may have
    /// gone through). Both count until UNSURE_MS passes without the pass showing up.
    #[serde(default = "done_status")]
    pub status: String,
}

fn done_status() -> String {
    "done".into()
}

impl Claim {
    fn covers(&self, wallets: &[String]) -> bool {
        wallets.contains(&self.wallet) || self.group.iter().any(|w| wallets.contains(w))
    }
}

pub fn read_promo(home: &std::path::Path) -> Option<Promo> {
    std::fs::read_to_string(home.join(PROMO_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn save_promo(home: &std::path::Path, p: &Promo) -> std::io::Result<()> {
    let tmp = home.join("promo.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(p).unwrap_or_default())?;
    std::fs::rename(tmp, home.join(PROMO_FILE))
}

/// Every claim on record. An error (never an empty list) if the file is there but can't be read,
/// so a damaged file can't reset the limits.
pub fn read_claims(home: &std::path::Path) -> Result<Vec<Claim>, String> {
    match std::fs::read_to_string(home.join(CLAIMS_FILE)) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("promo-claims.json can't be read ({e})")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("promo-claims.json can't be read ({e})")),
    }
}

fn write_claims(home: &std::path::Path, all: &[Claim]) -> std::io::Result<()> {
    let tmp = home.join("promo-claims.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(all).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, home.join(CLAIMS_FILE))
}

/// Changes the claims file; past campaigns' claims are trimmed to MAX_OLD_CLAIMS, the running
/// campaign's are never dropped.
fn edit_claims(home: &std::path::Path, current: &str, f: impl FnOnce(&mut Vec<Claim>)) -> Result<(), String> {
    let mut all = read_claims(home)?;
    f(&mut all);
    let old = all.iter().filter(|c| c.campaign != current).count();
    if old > MAX_OLD_CLAIMS {
        let mut drop = old - MAX_OLD_CLAIMS;
        all.retain(|c| {
            if drop > 0 && c.campaign != current {
                drop -= 1;
                false
            } else {
                true
            }
        });
    }
    write_claims(home, &all).map_err(|e| format!("couldn't save promo-claims.json ({e})"))
}

/// Starts reading the bridge history a campaign's "migrated" or "burned" rules need.
pub fn warm_history(p: &Promo) {
    for n in p.needs.list.iter().filter(|n| n.kind == "migrated" || n.kind == "burned") {
        crate::bridge::warm(&n.tick);
    }
}

/// Why a campaign isn't taking claims right now (None = it is).
fn closed_reason(p: &Promo, claims: usize, now: i64) -> Option<&'static str> {
    if p.ended {
        Some("ended")
    } else if now < p.start_ms {
        Some("not_started")
    } else if now >= p.end_ms {
        Some("ended")
    } else if p.max_claims > 0 && claims as u64 >= p.max_claims {
        Some("all_claimed")
    } else {
        None
    }
}

/// One claim at a time, so two tabs (or two wallets with the same NFT) can't both get through.
static CLAIMING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Told the creator the keeper ran low (once per campaign until it's topped up).
static TOLD_LOW: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The keeper's balance, read from the chain at most every 20 s for the public page (`fresh`
/// reads it now: claims and the dashboard).
static KEEPER_SEEN: std::sync::Mutex<Option<(std::time::Instant, u64)>> = std::sync::Mutex::new(None);

async fn keeper_sompi_at(s: &AppState, fresh: bool) -> Option<u64> {
    let k = s.shop.as_ref()?.keeper.clone()?;
    if !fresh {
        if let Some((t, b)) = *KEEPER_SEEN.lock().unwrap_or_else(|e| e.into_inner()) {
            if t.elapsed() < std::time::Duration::from_secs(20) {
                return Some(b);
            }
        }
    }
    let b: u64 = {
        let chain = s.chain.lock().await;
        chain.utxos_at(&k.spk).await.ok()?.iter().filter(|u| u.entry.covenant_id.is_none()).map(|u| u.entry.amount).sum()
    };
    *KEEPER_SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now(), b));
    Some(b)
}

async fn keeper_sompi(s: &AppState) -> Option<u64> {
    keeper_sompi_at(s, true).await
}

/// What one claim takes from the keeper: the vault payment, the pass's deposit, a little spare.
/// The vault payment is about 1 KAS, or more if the plan's largest discount can't bring its
/// ceiling that low (then it's the ceiling less the largest discount).
async fn claim_cost(s: &AppState) -> u64 {
    let charge = match current_lane(s).await {
        Ok(l) => crate::discounted_ppm(l.lane.price, promo_ppm(s, l.lane.price)),
        Err(_) => (CHARGE_KAS * crate::SOMPI as f64) as u64,
    };
    charge + s.card.settings.bond + KEEPER_SPARE
}

/// The discount (millionths off the plan's KAS ceiling) that brings a pass to about 1 KAS.
fn promo_ppm(s: &AppState, ceiling: u64) -> u64 {
    let max_ppm = s.card.settings.v4.as_ref().map(|v| v.max_discount_bps * 100).unwrap_or(0);
    crate::prices::ppm_for(ceiling, CHARGE_KAS, max_ppm)
}

/// The campaign's claims that count: done ones, and pending or unsure ones until UNSURE_MS has
/// passed without their pass showing up on chain.
async fn campaign_claims(s: &AppState, id: &str) -> Result<Vec<Claim>, String> {
    let now = wall_ms();
    let mut out = Vec::new();
    for c in read_claims(&s.home)?.into_iter().filter(|c| c.campaign == id) {
        if c.status == "done" || now - c.at_ms < UNSURE_MS || seen_on_chain(s, &c).await {
            out.push(c);
        }
    }
    Ok(out)
}

async fn seen_on_chain(s: &AppState, c: &Claim) -> bool {
    if c.txid.is_empty() {
        return false;
    }
    let Some(l) = s.levels.iter().find(|l| l.card.covenant_id == c.level) else { return true };
    let snap = l.snap.read().await;
    // Not read yet: keep counting it, to be safe.
    !snap.synced || snap.events.iter().any(|e| e.txid == c.txid) || snap.passes.iter().any(|p| p.txid == c.txid)
}

fn unreadable(e: String) -> ApiError {
    eprintln!("promo: {e}");
    (StatusCode::SERVICE_UNAVAILABLE, "free passes are paused: the site's claims list needs a look. Try again later.".to_string())
}

fn can_give(s: &AppState) -> bool {
    s.shop.as_ref().is_some_and(|sh| sh.keeper.is_some() && sh.can_discount())
}

/// Does any of these wallets hold an active pass on the promotion's plan, or on a plan that gives
/// the same access or more? None while the chain is still being read.
async fn has_access(s: &AppState, level: &str, wallets: &[String]) -> Option<bool> {
    let owners: Vec<String> = wallets.iter().filter_map(|a| owner_hash_of(a).ok().map(|h| h.to_string())).collect();
    let ar = access_ranks(&s.home, &s.levels);
    let at = s.levels.iter().position(|l| l.card.covenant_id == level)?;
    for (i, l) in s.levels.iter().enumerate() {
        if ar[i] < ar[at] {
            continue;
        }
        let snap = l.snap.read().await;
        if !snap.synced {
            return None;
        }
        if snap.passes.iter().any(|p| p.active && owners.contains(&p.owner_hash)) {
            return Some(true);
        }
    }
    Some(false)
}

/// The public view of a campaign (for the plan's card).
fn public_view(s: &AppState, p: &Promo, claims: usize, now: i64, paused: bool) -> serde_json::Value {
    let rank = s.levels.iter().position(|l| l.card.covenant_id == p.level);
    let closed = closed_reason(p, claims, now);
    serde_json::json!({
        "active": closed.is_none() && rank.is_some() && !paused,
        "closed": closed,
        "paused": paused,
        "label": p.label,
        "level": p.level,
        "level_name": rank.map(|i| level_name(&s.home, &s.levels, i)),
        "requirement": p.needs.describe(),
        "needs": p.needs,
        "start_ms": p.start_ms,
        "end_ms": p.end_ms,
        "claims_left": (p.max_claims > 0).then(|| p.max_claims.saturating_sub(claims as u64)),
        "once_per_nft": p.needs.list.iter().any(|n| n.kind == "nft"),
    })
}

/// `GET /v1/promo`: the running campaign, if any (`active: false` otherwise).
pub async fn info(State(s): State<AppState>) -> Json<serde_json::Value> {
    let Some(p) = read_promo(&s.home) else { return Json(serde_json::json!({ "active": false })) };
    let Ok(ls) = s.at(Some(&p.level)) else { return Json(serde_json::json!({ "active": false })) };
    let Ok(claims) = campaign_claims(&ls, &p.id).await else { return Json(serde_json::json!({ "active": false, "paused": true })) };
    let paused = !can_give(&ls)
        || match keeper_sompi_at(&ls, false).await {
            Some(b) => b < claim_cost(&ls).await + KEEPER_FLOOR,
            None => true,
        };
    Json(public_view(&ls, &p, claims.len(), wall_ms(), paused))
}

#[derive(Deserialize)]
pub struct SessionBody {
    session: String,
}

/// Where a member stands: eligible, already claimed, already a member, or what's missing.
struct Standing {
    wallet: String,
    group: Vec<String>,
    checked: crate::holdings::Checked,
    claimed: Option<Claim>,
    member: bool,
}

async fn standing(s: &AppState, p: &Promo, session: &str, fresh: bool) -> Result<Standing, ApiError> {
    let wallet = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let group = s.wallets.group_of(&wallet);
    let claims = campaign_claims(s, &p.id).await.map_err(unreadable)?;
    let claimed = claims.iter().find(|c| c.covers(&group)).cloned();
    let used: std::collections::HashSet<String> = claims.iter().flat_map(|c| c.nfts.iter().cloned()).collect();
    let member = has_access(s, &p.level, &group).await.ok_or((StatusCode::SERVICE_UNAVAILABLE, "still reading the chain, try again in a moment".to_string()))?;
    let down = || (StatusCode::SERVICE_UNAVAILABLE, "couldn't read what your wallet holds right now (the NFT or token indexer didn't answer). Try again in a minute.".to_string());
    let checked = tokio::time::timeout(std::time::Duration::from_secs(60), crate::holdings::check(&s.card.network, &p.needs, &group, &used, fresh)).await.map_err(|_| down())?.ok_or_else(down)?;
    Ok(Standing { wallet, group, checked, claimed, member })
}

fn standing_json(st: &Standing) -> serde_json::Value {
    let reason = if st.claimed.is_some() {
        Some("already_claimed")
    } else if st.member {
        Some("already_member")
    } else if !st.checked.ok {
        Some("not_holding")
    } else {
        None
    };
    serde_json::json!({
        "wallet": st.wallet,
        "wallets_checked": st.group.len(),
        "eligible": reason.is_none(),
        "reason": reason,
        "rows": st.checked.rows,
        "claimed_txid": st.claimed.as_ref().map(|c| c.txid.clone()),
    })
}

/// `POST /v1/promo/check {session}`: does the signed-in member qualify?
pub async fn check_me(State(s): State<AppState>, Json(req): Json<SessionBody>) -> Result<Json<serde_json::Value>, ApiError> {
    let p = read_promo(&s.home).ok_or((StatusCode::NOT_FOUND, "there's no promotion running".to_string()))?;
    let ls = s.at(Some(&p.level))?;
    let st = standing(&ls, &p, &req.session, false).await?;
    let mut out = standing_json(&st);
    let closed = closed_reason(&p, campaign_claims(&ls, &p.id).await.map_err(unreadable)?.len(), wall_ms());
    out["closed"] = serde_json::json!(closed);
    if closed.is_some() {
        out["eligible"] = serde_json::json!(false);
    }
    if !can_give(&ls) {
        out["eligible"] = serde_json::json!(false);
        out["closed"] = serde_json::json!("paused");
    }
    Ok(Json(out))
}

/// Which NFTs a claim uses: in "all" mode the first `min` unused ids of each NFT requirement; in
/// "any" mode nothing if a token requirement is met, else the first met NFT requirement's.
fn nfts_to_use(p: &Promo, c: &crate::holdings::Checked) -> Vec<String> {
    let take = |tick: &str| -> Vec<String> {
        let need = p.needs.list.iter().find(|n| n.kind == "nft" && n.tick == tick).map(|n| n.min as usize).unwrap_or(1);
        c.nft_ids.iter().find(|(t, _)| t == tick).map(|(t, ids)| ids.iter().take(need).map(|id| format!("{t}:{id}")).collect()).unwrap_or_default()
    };
    // Migrations and burns that count are all used (each once per campaign).
    let hist = |key: &str| -> Vec<String> { c.history_ids.iter().find(|(k, _)| k == key).map(|(_, ids)| ids.clone()).unwrap_or_default() };
    if p.needs.mode == "any" {
        // A met holding (tokens) uses nothing; then history; then NFTs.
        if c.rows.iter().any(|r| r.ok && (r.kind == "token" || r.kind == "kcc20")) {
            return Vec::new();
        }
        if let Some(r) = c.rows.iter().find(|r| r.ok && (r.kind == "migrated" || r.kind == "burned")) {
            return hist(&r.key);
        }
        return c.rows.iter().find(|r| r.ok && r.kind == "nft").map(|r| take(&r.tick)).unwrap_or_default();
    }
    let mut out: Vec<String> = c.rows.iter().filter(|r| r.kind == "nft").flat_map(|r| take(&r.tick)).collect();
    for r in c.rows.iter().filter(|r| r.kind == "migrated" || r.kind == "burned") {
        for id in hist(&r.key) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// `POST /v1/promo/claim {session}`: checks the member and, if they qualify, has the keeper buy
/// their pass. Returns the txid.
pub async fn claim(State(s): State<AppState>, Json(req): Json<SessionBody>) -> Result<Json<serde_json::Value>, ApiError> {
    let p = read_promo(&s.home).ok_or((StatusCode::NOT_FOUND, "there's no promotion running".to_string()))?;
    let ls = s.at(Some(&p.level))?;
    let shop = shop_of(&ls)?;
    if !can_give(&ls) {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "this site can't give free passes right now".to_string()));
    }
    let open = |n: usize| match closed_reason(&p, n, wall_ms()) {
        Some("not_started") => Err((StatusCode::CONFLICT, "this promotion hasn't started yet".to_string())),
        Some("all_claimed") => Err((StatusCode::CONFLICT, "every free pass in this promotion has been claimed".to_string())),
        Some(_) => Err((StatusCode::CONFLICT, "this promotion has ended".to_string())),
        None => Ok(()),
    };
    open(campaign_claims(&ls, &p.id).await.map_err(unreadable)?.len())?;
    // What the member holds, read fresh from the indexers (before taking the claim lock, so a
    // slow indexer holds up only this member).
    let st = standing(&ls, &p, &req.session, true).await?;
    let refuse = |st: &Standing| -> Result<(), ApiError> {
        if st.claimed.is_some() {
            return Err((StatusCode::CONFLICT, "you've already claimed this promotion".to_string()));
        }
        if st.member {
            return Err((StatusCode::CONFLICT, "you already have this plan (or one that includes it), so there's nothing to claim".to_string()));
        }
        Ok(())
    };
    refuse(&st)?;
    if !st.checked.ok {
        let missing: Vec<String> = st.checked.rows.iter().filter(|r| !r.ok).map(|r| r.text.clone()).collect();
        return Err((StatusCode::FORBIDDEN, format!("your wallets don't qualify yet (needs {})", if p.needs.mode == "any" { p.needs.describe() } else { missing.join(", and ") })));
    }
    let nfts = nfts_to_use(&p, &st.checked);

    // One claim at a time from here: check again against the claims made meanwhile.
    let _one = CLAIMING.lock().await;
    let claims = campaign_claims(&ls, &p.id).await.map_err(unreadable)?;
    open(claims.len())?;
    if claims.iter().any(|c| c.covers(&st.group)) {
        return Err((StatusCode::CONFLICT, "you've already claimed this promotion".to_string()));
    }
    if claims.iter().any(|c| c.nfts.iter().any(|n| nfts.contains(n))) {
        return Err((StatusCode::CONFLICT, "one of your NFTs was just used for a claim; try again".to_string()));
    }
    if has_access(&ls, &p.level, &st.group).await != Some(false) {
        return Err((StatusCode::CONFLICT, "you already have this plan (or one that includes it), so there's nothing to claim".to_string()));
    }
    no_duplicate(&ls, &st.wallet).await?;
    // The keeper pays: stop (and tell the creator once) if it can't.
    let bal = keeper_sompi(&ls).await.unwrap_or(0);
    let cost = claim_cost(&ls).await;
    if bal < cost + KEEPER_FLOOR {
        let first = TOLD_LOW.lock().unwrap_or_else(|e| e.into_inner()).replace(p.id.clone()).as_deref() != Some(p.id.as_str());
        if first {
            let owner = owner_hash_of(&site_owner(&ls)).map(|h| h.to_string()).unwrap_or_default();
            dm_owner(&ls, &owner, &format!("Your promotion \"{}\" is paused: the keeper wallet needs a top-up (each free pass uses about {} KAS). Its address is in your dashboard › Promotions.", p.label, kas4(cost))).await;
        }
        return Err((StatusCode::SERVICE_UNAVAILABLE, "free passes are paused for a moment while the site tops up. Try again later.".to_string()));
    }
    *TOLD_LOW.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let lane = current_lane(&ls).await?;
    if lane.lane.remaining == 0 {
        return Err((StatusCode::CONFLICT, "this plan is sold out".to_string()));
    }
    let ppm = promo_ppm(&ls, lane.lane.price);
    // Recorded before anything is sent: if the record can't be saved, nothing is sent.
    let now = wall_ms();
    let rec = Claim { campaign: p.id.clone(), wallet: st.wallet.clone(), nfts: nfts.clone(), txid: String::new(), at_ms: now, level: p.level.clone(), charge_sompi: 0, group: st.group.clone(), status: "pending".into(), label: p.label.clone() };
    edit_claims(&s.home, &p.id, |all| all.push(rec)).map_err(unreadable)?;
    let mine = |c: &Claim| c.campaign == p.id && c.wallet == st.wallet && c.at_ms == now && c.status == "pending";
    let res = {
        let mut chain = ls.chain.lock().await;
        shop.promo_buy(&mut chain, &st.wallet, &lane, ppm).await
    };
    let (txid, charge) = match res {
        Ok(x) => x,
        Err((None, e)) => {
            // Nothing was sent: the claim doesn't count.
            let _ = edit_claims(&s.home, &p.id, |all| all.retain(|c| !mine(c)));
            return Err((StatusCode::BAD_GATEWAY, format!("the free pass couldn't be sent ({e:#}); nothing was used up, try again in a minute")));
        }
        Err((Some(txid), e)) => {
            eprintln!("promo: sending a free pass reported an error (tx {txid}): {e:#}");
            let _ = edit_claims(&s.home, &p.id, |all| {
                if let Some(c) = all.iter_mut().find(|c| mine(c)) {
                    c.txid = txid.clone();
                    c.status = "unsure".into();
                }
            });
            return Err((StatusCode::BAD_GATEWAY, "the network didn't confirm your free pass yet. If it doesn't show up in a few minutes, try again.".to_string()));
        }
    };
    members::remember(&s.home, &st.wallet);
    let _ = edit_claims(&s.home, &p.id, |all| {
        if let Some(c) = all.iter_mut().find(|c| mine(c)) {
            c.txid = txid.clone();
            c.charge_sompi = charge;
            c.status = "done".into();
        }
    });
    {
        let next = FoundLane { txid: txid.clone(), index: 0, value: lane.value, lane: crate::Lane { remaining: if lane.lane.remaining > 0 { lane.lane.remaining - 1 } else { lane.lane.remaining }, ..lane.lane } };
        let mut snap = ls.snap.write().await;
        snap.lanes.retain(|l| !(l.txid == lane.txid && l.index == lane.index));
        snap.lanes.insert(0, next);
    }
    let currency = crate::prices::level_prices(&s.home).get(&p.level).map(|x| x.currency.clone()).unwrap_or_else(|| "KAS".into());
    crate::prices::log_payment(&s.home, &txid, crate::prices::PayNote { currency: currency.clone(), amount: 0.0, credit: 0.0, kind: "promo".into() });
    // Paid nothing in the plan's currency, so it earns no upgrade credit on a higher plan.
    crate::prices::log_paid(&s.home, &p.level, &owner_hash_of(&st.wallet).map(|h| h.to_string()).unwrap_or_default(), charge as i64, &currency, 0.0);
    ls.recent_buys.lock().unwrap_or_else(|e| e.into_inner()).insert(format!("{}:{}", ls.card.covenant_id, st.wallet), std::time::Instant::now());
    println!("promo: gave a free pass ({} KAS to the vault, paid by the keeper), tx {txid}", kas4(charge));
    Ok(Json(serde_json::json!({ "txid": txid, "level": p.level, "until_ms": now + ls.card.settings.period_ms, "nfts_used": nfts })))
}

// ---- Creator ------------------------------------------------------------------

/// `GET /v1/creator/promo?session=`: the campaign, its claims, and the keeper's balance.
pub async fn creator_get(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    let p = read_promo(&s.home);
    let lv = p.as_ref().and_then(|p| s.at(Some(&p.level)).ok()).unwrap_or_else(|| s.clone());
    let keeper = lv.shop.as_ref().and_then(|sh| sh.keeper.as_ref()).map(|w| w.addr.to_string());
    let bal = keeper_sompi(&lv).await;
    let cost = claim_cost(&lv).await;
    let claims: Vec<Claim> = match &p {
        Some(p) => campaign_claims(&lv, &p.id).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
        None => Vec::new(),
    };
    let levels: Vec<serde_json::Value> = s
        .levels
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.card.placeholder && is_v5(&l.card))
        .map(|(i, l)| serde_json::json!({ "level": l.card.covenant_id, "name": level_name(&s.home, &s.levels, i), "period_ms": l.card.settings.period_ms }))
        .collect();
    let view = p.as_ref().map(|p| public_view(&lv, p, claims.len(), wall_ms(), !can_give(&lv) || bal.is_none_or(|b| b < cost + KEEPER_FLOOR)));
    let mut recent: Vec<&Claim> = claims.iter().collect();
    recent.reverse();
    recent.truncate(200);
    Ok(Json(serde_json::json!({
        "promo": p,
        "view": view,
        "claims": recent,
        "claims_count": claims.len(),
        "keeper": keeper,
        "keeper_sompi": bal,
        "cost_per_claim_sompi": cost,
        "vault_per_claim_sompi": cost.saturating_sub(lv.card.settings.bond + KEEPER_SPARE),
        "deposit_sompi": lv.card.settings.bond,
        "spare_sompi": KEEPER_SPARE,
        "claims_affordable": bal.map(|b| b.saturating_sub(KEEPER_FLOOR) / cost.max(1)),
        "keeper_floor_sompi": KEEPER_FLOOR,
        "can_discount": can_give(&lv),
        "levels": levels,
        "network": s.card.network,
    })))
}

#[derive(Deserialize)]
pub struct PromoRequest {
    session: String,
    label: String,
    level: String,
    needs: Needs,
    start_ms: i64,
    end_ms: i64,
    #[serde(default)]
    max_claims: u64,
    /// Start a new campaign (token ids and wallets can claim again) instead of editing this one.
    #[serde(default)]
    new_campaign: bool,
}

/// `POST /v1/creator/promo`: sets the campaign (or edits the running one).
pub async fn creator_set(State(s): State<AppState>, Json(req): Json<PromoRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let label: String = req.label.chars().filter(|c| !c.is_control()).take(MAX_LABEL).collect::<String>().trim().to_string();
    if label.is_empty() {
        return Err(bad("write the line the plan's card shows, e.g. \"NACHO holders: your first month is free\""));
    }
    let lv = s.at(Some(req.level.trim()))?;
    if lv.card.placeholder || !is_v5(&lv.card) {
        return Err(bad("choose one of this site's plans"));
    }
    if lv.shop.as_ref().and_then(|sh| sh.keeper.as_ref()).is_none() {
        return Err(bad("this site has no keeper wallet to pay for free passes"));
    }
    if !can_give(&lv) {
        return Err(bad("this server doesn't hold the price-approval key this plan trusts, so it can't give free passes (see Settings › Server key)"));
    }
    let needs = req.needs.checked().map_err(bad)?;
    if needs.list.iter().any(|n| n.kind == "token" || n.kind == "kcc20") && req.max_claims == 0 {
        return Err(bad("with a token requirement, set the most free passes: tokens can be moved to a new wallet to claim again, so a limit keeps that in check"));
    }
    if req.end_ms <= req.start_ms {
        return Err(bad("the end must come after the start"));
    }
    if req.end_ms - req.start_ms > 366 * 86_400_000 {
        return Err(bad("a promotion can run for a year at most"));
    }
    if req.end_ms <= wall_ms() {
        return Err(bad("the end date has already passed"));
    }
    if req.max_claims > MAX_CAMPAIGN_CLAIMS {
        return Err(bad(format!("{MAX_CAMPAIGN_CLAIMS} claims at most")));
    }
    let old = read_promo(&s.home);
    let id = match &old {
        Some(o) if !req.new_campaign && !o.ended => o.id.clone(),
        _ => format!("c{}", &auth::random_token()[..10]),
    };
    let p = Promo { id, label, level: lv.card.covenant_id.clone(), needs, start_ms: req.start_ms, end_ms: req.end_ms, max_claims: req.max_claims, ended: false };
    save_promo(&s.home, &p).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    warm_history(&p);
    println!("dashboard: promotion \"{}\" set ({})", p.label, p.needs.describe());
    Ok(Json(serde_json::json!({ "ok": true, "promo": p })))
}

/// `POST /v1/creator/promo/end {session}`: ends the campaign now (claims made stay).
pub async fn creator_end(State(s): State<AppState>, Json(req): Json<SessionBody>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let mut p = read_promo(&s.home).ok_or_else(|| bad("there's no promotion"))?;
    p.ended = true;
    save_promo(&s.home, &p).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promo(mode: &str) -> Promo {
        let needs: Needs = serde_json::from_value(serde_json::json!({"match": mode, "list": [{"kind": "nft", "tick": "NACHO", "min": 2}, {"kind": "token", "tick": "NACHO", "min": 1000}]})).unwrap();
        Promo { id: "c1".into(), label: "x".into(), level: "l".into(), needs: needs.checked().unwrap(), start_ms: 100, end_ms: 200, max_claims: 2, ended: false }
    }

    fn checked(nft_ok: bool, token_ok: bool) -> crate::holdings::Checked {
        let row = |kind: &str, ok| crate::holdings::Row { text: String::new(), kind: kind.into(), tick: "NACHO".into(), key: format!("{kind}:NACHO"), need: 1.0, have: 1.0, ok };
        crate::holdings::Checked {
            ok: nft_ok || token_ok,
            rows: vec![row("nft", nft_ok), row("token", token_ok)],
            nft_ids: if nft_ok { vec![("NACHO".into(), vec!["3".into(), "7".into(), "9".into()])] } else { vec![] },
            history_ids: vec![],
        }
    }

    #[test]
    fn campaign_windows_and_limits() {
        let p = promo("all");
        assert_eq!(closed_reason(&p, 0, 99), Some("not_started"));
        assert_eq!(closed_reason(&p, 0, 150), None);
        assert_eq!(closed_reason(&p, 2, 150), Some("all_claimed"));
        assert_eq!(closed_reason(&p, 0, 200), Some("ended"));
        let mut q = p.clone();
        q.ended = true;
        assert_eq!(closed_reason(&q, 0, 150), Some("ended"));
        q.ended = false;
        q.max_claims = 0;
        assert_eq!(closed_reason(&q, 5000, 150), None);
    }

    #[test]
    fn claims_use_the_right_nfts() {
        // all: the first `min` unused ids of each NFT requirement
        assert_eq!(nfts_to_use(&promo("all"), &checked(true, true)), vec!["NACHO:3", "NACHO:7"]);
        // any: a met token requirement uses no NFTs
        assert!(nfts_to_use(&promo("any"), &checked(true, true)).is_empty());
        assert_eq!(nfts_to_use(&promo("any"), &checked(true, false)), vec!["NACHO:3", "NACHO:7"]);
    }
}
