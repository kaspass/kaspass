//! Who's who in the creator's member list. A pass only names its owner by a hash of their key,
//! so the site remembers the wallet behind each hash whenever it meets one (a sign-in, a purchase
//! or gift made here, a QR order, a promotion claim, a linked wallet), and the creator can keep a
//! private note per member ("Alex from Discord"). Both stay on this server: member-addresses.json
//! and member-notes.json. Notes are never shown to anyone but the site's owner.

use super::*;

const ADDRESSES_FILE: &str = "member-addresses.json";
const NOTES_FILE: &str = "member-notes.json";
const MAX_NOTE: usize = 120;
const MAX_KNOWN: usize = 100_000;

type Known = std::sync::Mutex<Option<(std::path::PathBuf, std::collections::BTreeMap<String, String>)>>;
static KNOWN: Known = std::sync::Mutex::new(None);

fn with_known<T>(home: &std::path::Path, f: impl FnOnce(&mut std::collections::BTreeMap<String, String>) -> T) -> T {
    let mut g = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_none_or(|(p, _)| p != home) {
        let map = std::fs::read_to_string(home.join(ADDRESSES_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        *g = Some((home.to_path_buf(), map));
    }
    f(&mut g.as_mut().unwrap().1)
}

/// Remembers the wallet behind an owner hash (a regular address only; anything else is ignored).
pub fn remember(home: &std::path::Path, address: &str) {
    let a = address.trim();
    let Ok(h) = owner_hash_of(a) else { return };
    let h = h.to_string();
    let changed = with_known(home, |m| {
        // The first wallet met for a hash stays: the same key on the other network (the only
        // other address with the same hash) can't replace it.
        if m.contains_key(&h) || m.len() >= MAX_KNOWN {
            return None;
        }
        m.insert(h, a.to_string());
        Some(m.clone())
    });
    if let Some(m) = changed {
        let _ = crate::prices::write_atomic(&home.join(ADDRESSES_FILE), serde_json::to_string(&m).unwrap_or_default().as_bytes());
    }
}

/// The wallet behind an owner hash, if this site has met it.
pub fn address_of(home: &std::path::Path, owner_hash: &str) -> Option<String> {
    with_known(home, |m| m.get(owner_hash).cloned())
}

/// Everything already on this server that names a member's wallet: linked wallets, QR orders,
/// promotion claims, chat-app links, and the site's owner. Run once at start.
pub fn backfill(s: &AppState) {
    let mut found: Vec<String> = Vec::new();
    found.push(site_owner(s));
    for l in s.levels.iter() {
        found.push(l.card.creator_address.clone());
    }
    if let Ok(t) = std::fs::read_to_string(s.home.join("wallets.json")) {
        if let Ok(m) = serde_json::from_str::<std::collections::BTreeMap<String, Vec<String>>>(&t) {
            for (k, v) in m {
                found.push(k);
                found.extend(v);
            }
        }
    }
    for f in ["payin-orders.json", "payin-history.json"] {
        if let Ok(t) = std::fs::read_to_string(s.home.join(f)) {
            if let Ok(serde_json::Value::Array(a)) = serde_json::from_str::<serde_json::Value>(&t) {
                found.extend(a.iter().filter_map(|o| o["buyer"].as_str().map(String::from)));
            }
        }
    }
    if let Ok(claims) = promo::read_claims(&s.home) {
        found.extend(claims.into_iter().map(|c| c.wallet));
    }
    if let Some(b) = &s.telegram {
        found.extend(b.links().into_iter().map(|l| l.address));
    }
    if let Some(d) = &s.discord {
        found.extend(d.links().into_iter().map(|l| l.address));
    }
    for a in found {
        remember(&s.home, &a);
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Note {
    pub note: String,
    pub at_ms: i64,
}

pub fn notes(home: &std::path::Path) -> std::collections::BTreeMap<String, Note> {
    std::fs::read_to_string(home.join(NOTES_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The member list's extra columns for each pass: the owner's wallet when known, who paid (the
/// keeper for a promotion), and the creator's note.
pub fn enrich(s: &AppState, passes: &[FoundPass]) -> Vec<serde_json::Value> {
    let notes = notes(&s.home);
    let keeper = keeper_of(s);
    passes
        .iter()
        .map(|p| {
            let mut v = serde_json::to_value(p).unwrap_or_default();
            if p.owner_address.is_none() {
                if let Some(a) = address_of(&s.home, &p.owner_hash) {
                    v["owner_address"] = serde_json::json!(a);
                }
            }
            if let Some(b) = &p.bought_by {
                if Some(b) == keeper.as_ref() {
                    v["bought_by_keeper"] = serde_json::json!(true);
                }
            }
            if let Some(n) = notes.get(&p.owner_hash) {
                v["note"] = serde_json::json!(n.note);
            }
            v
        })
        .collect()
}

#[derive(Deserialize)]
pub struct NoteRequest {
    session: String,
    owner_hash: String,
    note: String,
}

/// `POST /v1/creator/member-note {session, owner_hash, note}`: the creator's private note on a
/// member (empty removes it). It follows the member's key, so it shows on all their passes.
pub async fn creator_note(State(s): State<AppState>, Json(req): Json<NoteRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let h = req.owner_hash.trim().to_lowercase();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad("that isn't a member of this site"));
    }
    let mut known = false;
    for l in s.levels.iter() {
        if l.snap.read().await.passes.iter().any(|p| p.owner_hash == h) {
            known = true;
            break;
        }
    }
    if !known {
        return Err(bad("that isn't a member of this site"));
    }
    let note: String = req.note.chars().filter(|c| !c.is_control()).take(MAX_NOTE).collect::<String>().trim().to_string();
    let mut all = notes(&s.home);
    if note.is_empty() {
        all.remove(&h);
    } else {
        all.insert(h, Note { note: note.clone(), at_ms: wall_ms() });
    }
    let tmp = s.home.join("member-notes.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, s.home.join(NOTES_FILE))).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "note": note })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_wallets_by_owner_hash() {
        let dir = std::env::temp_dir().join(format!("kp-members-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = "kaspatest:qq789twmfl0sntu57ry56llf9gux5lnse79pmpv3vwrtkff4c7cmzy2gvg8je";
        remember(&dir, a);
        remember(&dir, "not an address");
        let h = owner_hash_of(a).unwrap().to_string();
        assert_eq!(address_of(&dir, &h).as_deref(), Some(a));
        assert!(std::fs::read_to_string(dir.join(ADDRESSES_FILE)).unwrap().contains(a));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
