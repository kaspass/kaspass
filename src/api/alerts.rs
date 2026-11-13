//! Alerts for the creator: a direct message (Telegram or Discord, through the site's bot) when
//! someone gets a pass, and optionally on upgrades and renewals. They come from what the chain
//! reader sees, so purchases made anywhere (the site, a QR payment, a gift, a promotion) all
//! count. The creator links their chat app from the dashboard (Settings › Alerts) and chooses
//! which alerts they want (`alerts.json`); `alerts-seen.json` remembers what was already sent.

use super::*;

const SETTINGS_FILE: &str = "alerts.json";
const SEEN_FILE: &str = "alerts-seen.json";
const MAX_SEEN: usize = 5_000;

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct Settings {
    /// New passes: purchases, QR payments, gifts, free trials and promotion claims.
    #[serde(default = "yes")]
    pub new_passes: bool,
    /// Upgrades and renewals, auto-renewals included.
    #[serde(default = "yes")]
    pub renewals: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { new_passes: true, renewals: true }
    }
}

pub fn settings(home: &std::path::Path) -> Settings {
    std::fs::read_to_string(home.join(SETTINGS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn read_seen(home: &std::path::Path) -> Option<Vec<String>> {
    std::fs::read_to_string(home.join(SEEN_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn save_seen(home: &std::path::Path, seen: &[String]) {
    let tmp = home.join("alerts-seen.json.tmp");
    if std::fs::write(&tmp, serde_json::to_string(seen).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, home.join(SEEN_FILE));
    }
}

fn key(level: &str, e: &crate::indexer::Event) -> String {
    format!("{}:{}:{}", &level[..level.len().min(12)], e.txid, e.kind)
}

/// Whether this event is the kind of alert the creator asked for.
fn wanted(st: Settings, e: &crate::indexer::Event) -> bool {
    match e.kind.as_str() {
        "monthly" | "lifetime" | "term" | "trial" | "self_trial" => st.new_passes,
        "renew" | "autorenew" | "upgrade" => st.renewals,
        _ => false,
    }
}

/// "alice.kas", else a short address, else "a new wallet".
async fn who(s: &AppState, owner_hash: &str) -> String {
    let addr = members::address_of(&s.home, owner_hash).or_else(|| {
        s.levels.iter().find_map(|l| l.snap.try_read().ok().and_then(|snap| snap.passes.iter().find(|p| p.owner_hash == owner_hash).and_then(|p| p.owner_address.clone())))
    });
    let Some(a) = addr else { return "a new wallet".into() };
    let mut cands: Vec<String> = s.wallets.group_of(&a).iter().filter_map(|w| crate::names::mainnet_twin(w)).collect();
    cands.dedup();
    let n = tokio::time::timeout(std::time::Duration::from_secs(8), s.names.for_member(&cands)).await.unwrap_or_default();
    n.kas.or(n.k).unwrap_or_else(|| format!("{}…{}", &a[..a.len().min(12)], &a[a.len().saturating_sub(6)..]))
}

fn amount(s: &AppState, e: &crate::indexer::Event, note: Option<&crate::prices::PayNote>) -> String {
    let unit = if s.card.network == "mainnet" { "KAS" } else { "test KAS" };
    let kas = format!("{} {unit}", kas_short(e.creator_sompi));
    match note {
        Some(n) if n.currency != "KAS" && n.amount > 0.0 => format!("{kas} ({})", money_str(&n.currency, n.amount)),
        _ => kas,
    }
}

fn kas_short(sompi: u64) -> String {
    let k = sompi as f64 / crate::SOMPI as f64;
    if k >= 100.0 { format!("{k:.0}") } else { format!("{k:.2}") }
}

fn money_str(currency: &str, amount: f64) -> String {
    match currency {
        "CAD" => format!("CA${amount:.2}"),
        "USD" => format!("US${amount:.2}"),
        "EUR" => format!("€{amount:.2}"),
        c => format!("{amount:.2} {c}"),
    }
}

/// The message for one event.
async fn message(s: &AppState, level_name: &str, e: &crate::indexer::Event, note: Option<&crate::prices::PayNote>) -> Option<String> {
    let w = who(s, &e.owner_hash).await;
    let kind = note.map(|n| n.kind.as_str()).unwrap_or("");
    let amt = amount(s, e, note);
    Some(match (e.kind.as_str(), kind) {
        (_, "promo") => format!("🎁 Promotion claimed: {w} got a free {level_name} pass (the keeper paid; {amt} to your vault)."),
        (_, "gift") => format!("🎁 Gift sent: {w} now has {level_name}."),
        ("trial", _) | ("self_trial", _) => format!("🆓 Free trial: {w} started {level_name}."),
        ("monthly", _) => format!("🎉 New member: {w} bought {level_name} for {amt}."),
        ("lifetime", _) => format!("🎉 New member: {w} bought {level_name} for life, {amt}."),
        ("term", _) => format!("🎉 New member: {w} bought a {level_name} commitment for {amt}."),
        ("autorenew", _) | ("renew", "auto_renewal") => format!("🔁 Auto-renewed: {w}'s {level_name}, {amt}."),
        ("renew", _) => format!("🔁 Renewed: {w} renewed {level_name} for {amt}."),
        ("upgrade", _) => format!("⬆️ Upgrade: {w} moved {level_name} to lifetime for {amt}."),
        _ => return None,
    })
}

async fn dm_creator(s: &AppState, text: &str) {
    let owner = owner_hash_of(&site_owner(s)).map(|h| h.to_string()).unwrap_or_default();
    dm_owner(s, &owner, text).await;
}

/// Watches every level's activity and messages the creator about new events. On the very first
/// run it only notes what's already there, so nobody gets a flood of old sales.
pub async fn run(s: AppState) {
    let mut seen: Vec<String> = read_seen(&s.home).unwrap_or_default();
    let mut primed = read_seen(&s.home).is_some();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(if cfg!(test) { 1 } else { 15 })).await;
        if s.telegram.is_none() && s.discord.is_none() {
            continue;
        }
        let mut fresh: Vec<(String, crate::indexer::Event)> = Vec::new();
        let mut all_synced = true;
        for (i, l) in s.levels.iter().enumerate() {
            if l.card.placeholder {
                continue;
            }
            let snap = l.snap.read().await;
            if !snap.synced {
                all_synced = false;
                continue;
            }
            let name = level_name(&s.home, &s.levels, i);
            for e in snap.events.iter() {
                let k = key(&l.card.covenant_id, e);
                if !seen.contains(&k) {
                    seen.push(k);
                    fresh.push((name.clone(), e.clone()));
                }
            }
        }
        if fresh.is_empty() {
            if !primed && all_synced {
                primed = true;
                save_seen(&s.home, &seen);
            }
            continue;
        }
        if seen.len() > MAX_SEEN {
            let cut = seen.len() - MAX_SEEN;
            seen.drain(..cut);
        }
        save_seen(&s.home, &seen);
        if !primed {
            // First run: what's there already isn't news.
            primed = all_synced;
            continue;
        }
        let st = settings(&s.home);
        let notes = crate::prices::payments_log(&s.home);
        for (name, e) in fresh.iter().filter(|(_, e)| wanted(st, e)).take(20) {
            if let Some(text) = message(&s, name, e, notes.get(&e.txid)).await {
                dm_creator(&s, &text).await;
            }
        }
    }
}

/// `GET /v1/creator/alerts?session=`: the settings, and whether the creator's chat app is linked.
pub async fn creator_get(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &q.session)?;
    let owner = owner_hash_of(&site_owner(&s)).map(|h| h.to_string()).unwrap_or_default();
    let owns = |address: &str| s.wallets.of(address).iter().any(|a| owner_hash_of(a).map(|h| h.to_string() == owner).unwrap_or(false));
    let tg = s.telegram.as_ref().map(|b| b.links().into_iter().any(|l| owns(&l.address) && !l.quiet));
    let dc = s.discord.as_ref().map(|d| d.links().into_iter().any(|l| owns(&l.address)));
    let _ = me;
    Ok(Json(serde_json::json!({
        "settings": settings(&s.home),
        "telegram": s.telegram.as_ref().map(|b| serde_json::json!({ "bot": b.username, "linked": tg == Some(true) })),
        "discord": s.discord.as_ref().map(|_| serde_json::json!({ "linked": dc == Some(true) })),
        "owner": site_owner(&s),
    })))
}

#[derive(Deserialize)]
pub struct SetRequest {
    session: String,
    new_passes: bool,
    renewals: bool,
}

/// `POST /v1/creator/alerts {session, new_passes, renewals}`.
pub async fn creator_set(State(s): State<AppState>, Json(req): Json<SetRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let st = Settings { new_passes: req.new_passes, renewals: req.renewals };
    std::fs::write(s.home.join(SETTINGS_FILE), serde_json::to_string_pretty(&st).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "settings": st })))
}

#[derive(Deserialize)]
pub struct TestRequest {
    session: String,
}

/// `POST /v1/creator/alerts/test {session}`: sends a test message.
pub async fn creator_test(State(s): State<AppState>, Json(req): Json<TestRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    if s.telegram.is_none() && s.discord.is_none() {
        return Err(bad("this site has no Telegram or Discord bot yet"));
    }
    dm_creator(&s, "🔔 Test alert from your KasPass site. You'll get messages like this when someone gets a pass.").await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str) -> crate::indexer::Event {
        crate::indexer::Event { txid: "t".into(), kind: kind.into(), owner_hash: "o".into(), amount_sompi: 0, fee_sompi: 0, creator_sompi: 0, voucher: false, time_ms: 0 }
    }

    #[test]
    fn chooses_alerts() {
        let both = Settings::default();
        let new_only = Settings { new_passes: true, renewals: false };
        assert!(wanted(both, &ev("monthly")) && wanted(both, &ev("renew")) && wanted(both, &ev("autorenew")) && wanted(both, &ev("upgrade")));
        assert!(wanted(new_only, &ev("trial")) && !wanted(new_only, &ev("renew")));
        assert!(!wanted(both, &ev("transfer")));
        assert_eq!(money_str("CAD", 10.0), "CA$10.00");
        assert_eq!(kas_short(23_313_649_800), "233");
        assert_eq!(kas_short(150_000_000), "1.50");
    }
}
