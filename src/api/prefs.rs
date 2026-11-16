//! The creator's own dashboard settings, so they follow the creator from one device to another.
//! Today that is the dashboard theme (Settings › Dashboard theme): which theme, its colours, and
//! the glow and pattern switches. Only the signed-in creator reads or writes it; nothing here is
//! public and nothing needs a wallet approval. Members never see it: their page has its own look
//! (`brand.rs`).
//!
//! Stored as `dash-prefs.json` in the site folder, with the owner's address, so a new owner after
//! a handover starts from the default theme rather than the old owner's.

use super::*;
use super::brand::{Palette, contrast, hex_ok};

const FILE: &str = "dash-prefs.json";
/// A theme is a few hundred bytes. Anything bigger isn't one.
const MAX_BYTES: usize = 4096;

#[derive(Serialize, Deserialize, Clone)]
struct Chart {
    light: Vec<String>,
    dark: Vec<String>,
}

/// A `kaspass_theme` 1 object: the same format as theme files and the member page's colours.
/// The dashboard keeps only the colours (fonts and corners stay KasPass's own).
#[derive(Serialize, Deserialize, Clone)]
struct Theme {
    kaspass_theme: u32,
    #[serde(default)]
    name: String,
    light: Palette,
    dark: Palette,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chart: Option<Chart>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Look {
    /// A preset's id ("kaspass", "gold", …), "page" for a copy of the page's colours, or "custom".
    id: String,
    theme: Theme,
    #[serde(default = "yes")]
    glow: bool,
    #[serde(default = "yes")]
    pattern: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    owner: String,
    /// [saved_at_ms, look]: the browser keeps the same pair, and the newer one wins.
    look: Option<(i64, Look)>,
}

fn load(home: &std::path::Path) -> Stored {
    std::fs::read_to_string(home.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn bad(msg: &str) -> ApiError {
    (StatusCode::BAD_REQUEST, msg.to_string())
}

fn check(l: &Look) -> Result<(), ApiError> {
    if l.id.is_empty() || l.id.len() > 40 || !l.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(bad("the theme's id must be short, in lowercase letters, numbers and dashes"));
    }
    let t = &l.theme;
    if t.kaspass_theme != 1 {
        return Err(bad("this server only knows kaspass_theme 1"));
    }
    if t.name.chars().count() > 40 || t.name.chars().any(|c| c.is_control()) {
        return Err(bad("the theme's name must be at most 40 characters"));
    }
    for p in [&t.light, &t.dark] {
        for c in [&p.bg, &p.text, &p.accent, &p.card] {
            if !hex_ok(c) {
                return Err(bad("every colour must look like #1a2b3c"));
            }
        }
        // The same floor as the page's own colours: below 3 to 1, text can't be read.
        if contrast(&p.text, &p.bg) < 3.0 || contrast(&p.text, &p.card) < 3.0 {
            return Err(bad("the text colour is too close to its background to read"));
        }
    }
    if let Some(ch) = &t.chart {
        if ch.light.len() != 4 || ch.dark.len() != 4 || !ch.light.iter().chain(ch.dark.iter()).all(|c| hex_ok(c)) {
            return Err(bad("chart colours must be four colour codes for day and four for night"));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct GetQuery {
    #[serde(default)]
    session: String,
}

async fn get_prefs(State(s): State<AppState>, Query(q): Query<GetQuery>) -> Result<axum::response::Response, ApiError> {
    let owner = creator_of(&s, &q.session)?;
    let st = load(&s.home);
    let look = if st.owner == owner { st.look } else { None };
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "look": look }))).into_response())
}

#[derive(Deserialize)]
struct SetRequest {
    session: String,
    look: (i64, Look),
}

async fn set_prefs(State(s): State<AppState>, Json(req): Json<SetRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let owner = creator_of(&s, &req.session)?;
    // The browser sends its own time, but the server's clock stamps the save: a device whose clock
    // runs behind can't lose a newer choice, and two quick picks keep their order.
    let (_, look) = req.look;
    check(&look)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let old = load(&s.home);
    let last = if old.owner == owner { old.look.map(|(t, _)| t).unwrap_or(0) } else { 0 };
    let at = now.max(last + 1);
    let st = Stored { owner, look: Some((at, look)) };
    let text = serde_json::to_string(&st).unwrap_or_default();
    if text.len() > MAX_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "that theme is too big".into()));
    }
    let tmp = s.home.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, s.home.join(FILE))).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "saved_at": at })))
}

pub(super) fn routes(r: Router<AppState>) -> Router<AppState> {
    r.route("/v1/creator/prefs", get(get_prefs).post(set_prefs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(text: &str, bg: &str) -> Look {
        let p = Palette { bg: bg.into(), text: text.into(), accent: "#49d9c8".into(), card: bg.into() };
        Look { id: "custom".into(), theme: Theme { kaspass_theme: 1, name: "Mine".into(), light: p.clone(), dark: p, chart: None }, glow: true, pattern: true }
    }

    #[test]
    fn readable_themes_pass_and_unreadable_ones_dont() {
        assert!(check(&look("#f2f5f4", "#0b1615")).is_ok());
        assert!(check(&look("#222222", "#111111")).is_err());
        let mut l = look("#f2f5f4", "#0b1615");
        l.theme.light.accent = "teal".into();
        assert!(check(&l).is_err());
        let mut l = look("#f2f5f4", "#0b1615");
        l.id = "Not ok!".into();
        assert!(check(&l).is_err());
        let mut l = look("#f2f5f4", "#0b1615");
        l.theme.chart = Some(Chart { light: vec!["#000000".into(); 3], dark: vec!["#000000".into(); 4] });
        assert!(check(&l).is_err());
    }
}
