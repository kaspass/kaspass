//! KasPass's own website. On KasPass's own host names (kaspass.app by default, or the
//! comma-separated list in `KASPASS_HOME_HOSTS`), `/` is KasPass's home page, `/creators` is
//! "Get your own page" with a contact form, and the membership page moves to `/demo`. Every
//! other site keeps its membership page at `/`.

use super::*;

const HOME_PAGE: &str = include_str!("../../web/home.html");
const CREATORS_PAGE: &str = include_str!("../../web/creators.html");
/// Notes sent through the "Get your own page" form, newest last.
const CONTACT_FILE: &str = "contact-requests.json";
const CONTACT_KEEP: usize = 300;

/// Is this request for KasPass's own website?
pub fn is_home(headers: &HeaderMap) -> bool {
    let hosts = std::env::var("KASPASS_HOME_HOSTS").unwrap_or_else(|_| "kaspass.app,www.kaspass.app".into());
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else { return false };
    // The host without a port ("[::1]:8787" keeps its brackets).
    let name = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or_default().to_string() };
    hosts.split(',').map(|h| h.trim()).any(|h| !h.is_empty() && h.eq_ignore_ascii_case(&name))
}

/// A page only people with its private link can open ("page-private.json": `{ "key": ... }`).
/// Visiting with `?k=<key>` sets a cookie, so the page's own links keep working afterwards.
const PRIVATE_FILE: &str = "page-private.json";
const VIEW_COOKIE: &str = "kp_view";

#[derive(Serialize, Deserialize, Clone)]
pub struct Private {
    pub key: String,
    pub since_ms: i64,
}

pub fn private(home: &std::path::Path) -> Option<Private> {
    std::fs::read_to_string(home.join(PRIVATE_FILE)).ok().and_then(|t| serde_json::from_str::<Private>(&t).ok()).filter(|p| p.key.len() >= 16)
}

/// None: show the page (with a cookie to set, if any). Some: the "not public" answer instead.
fn gate(s: &AppState, headers: &HeaderMap, uri: &axum::http::Uri) -> Result<Option<String>, axum::response::Response> {
    let Some(p) = private(&s.home) else { return Ok(None) };
    let eq = |a: &str| a.len() == p.key.len() && a.bytes().zip(p.key.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0;
    let from_query = uri.query().into_iter().flat_map(|q| q.split('&')).filter_map(|kv| kv.strip_prefix("k=")).any(eq);
    if from_query {
        return Ok(Some(format!("{VIEW_COOKIE}={}; Path=/; Max-Age=7776000; HttpOnly; Secure; SameSite=Lax", p.key)));
    }
    let from_cookie = headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).filter_map(|c| c.trim().strip_prefix(&format!("{VIEW_COOKIE}="))).any(eq);
    if from_cookie {
        return Ok(None);
    }
    Err((
        StatusCode::NOT_FOUND,
        [(header::CACHE_CONTROL, "no-store"), (header::HeaderName::from_static("x-robots-tag"), "noindex")],
        Html("<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><meta name=robots content=noindex><title>Not public</title><body style=\"font-family:system-ui,sans-serif;background:#0b1111;color:#e8f1f0;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px;text-align:center\"><div><h1 style=\"font-size:22px\">This page isn't public</h1><p style=\"color:#8aa09d\">Only people with its private link can open it.</p></div></body>"),
    )
        .into_response())
}

/// The membership page, behind the private-link gate when the creator turned it on.
fn membership_page(s: &AppState, headers: &HeaderMap, uri: &axum::http::Uri, html: String) -> axum::response::Response {
    match gate(s, headers, uri) {
        Err(r) => r,
        Ok(cookie) => {
            let mut r = ([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response();
            if private(&s.home).is_some() {
                r.headers_mut().insert(header::HeaderName::from_static("x-robots-tag"), header::HeaderValue::from_static("noindex"));
            }
            if let Some(c) = cookie.and_then(|c| header::HeaderValue::from_str(&c).ok()) {
                r.headers_mut().insert(header::SET_COOKIE, c);
            }
            r
        }
    }
}

/// KasPass's home page. Its demo links and section (marked `<!--demo-->...<!--/demo-->`) would be
/// left out while the demo is private, but KasPass's own site always has the example creators
/// (kaspass.app/demo/<id>) to show instead, so they stay.
fn home_page(s: &AppState) -> String {
    if private(&s.home).is_none() || !examples::list().is_empty() {
        return HOME_PAGE.to_string();
    }
    let mut out = String::with_capacity(HOME_PAGE.len());
    let mut rest = HOME_PAGE;
    while let Some(i) = rest.find("<!--demo-->") {
        out.push_str(&rest[..i]);
        match rest[i..].find("<!--/demo-->") {
            Some(j) => rest = &rest[i + j + "<!--/demo-->".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    // With the demo gone, "Get your own page" becomes the main button.
    out.replacen(r#"<a class="btn ghost" href="/creators">"#, r#"<a class="btn" href="/creators">"#, 1)
}

/// On KasPass's own site the membership page lives at /demo: its links to "/" follow it there.
pub fn to_demo(html: &str) -> String {
    html.replace(r#"href="/""#, r#"href="/demo""#)
}

/// `/`: KasPass's home page on its own host names, the membership page everywhere else.
pub async fn root(State(s): State<AppState>, headers: HeaderMap, uri: axum::http::Uri) -> axum::response::Response {
    if is_home(&headers) {
        // Older links to the membership page (`/?level=...`, `?return=...`) keep working.
        if let Some(q) = uri.query().filter(|q| !q.is_empty()) {
            return axum::response::Redirect::to(&format!("/demo?{q}")).into_response();
        }
        return ([(header::CACHE_CONTROL, "public, max-age=60")], Html(home_page(&s))).into_response();
    }
    membership_page(&s, &headers, &uri, fill_site(&s))
}

/// `/demo`: the membership page on KasPass's own site (elsewhere, back to `/`).
pub async fn demo(State(s): State<AppState>, headers: HeaderMap, uri: axum::http::Uri) -> axum::response::Response {
    if !is_home(&headers) {
        return axum::response::Redirect::to("/").into_response();
    }
    // A private page: visitors without its link see the example creators instead.
    if private(&s.home).is_some() && gate(&s, &headers, &uri).is_err() {
        return axum::response::Redirect::to(&format!("/demo/{}", examples::default_id())).into_response();
    }
    membership_page(&s, &headers, &uri, to_demo(&fill_site(&s)))
}

/// `GET /v1/creator/visibility?session=`: whether the page is private, and its private link key.
pub async fn visibility_get(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    let p = private(&s.home);
    Ok(Json(serde_json::json!({ "private": p.is_some(), "key": p.as_ref().map(|p| p.key.clone()), "since_ms": p.map(|p| p.since_ms) })))
}

#[derive(Deserialize)]
pub struct VisibilityRequest {
    session: String,
    private: bool,
    /// Make a new private link (the old one stops working).
    #[serde(default)]
    new_link: bool,
}

/// `POST /v1/creator/visibility {session, private, new_link}`.
pub async fn visibility_set(State(s): State<AppState>, Json(req): Json<VisibilityRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let path = s.home.join(PRIVATE_FILE);
    if !req.private {
        let _ = std::fs::remove_file(&path);
        println!("dashboard: the page is public again");
        return Ok(Json(serde_json::json!({ "private": false })));
    }
    let p = match private(&s.home) {
        Some(p) if !req.new_link => p,
        _ => Private { key: auth::random_token()[..24].to_string(), since_ms: wall_ms() },
    };
    std::fs::write(&path, serde_json::to_string_pretty(&p).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    println!("dashboard: the page is private (link only)");
    Ok(Json(serde_json::json!({ "private": true, "key": p.key, "since_ms": p.since_ms })))
}

/// `/creators`: "Get your own page", on KasPass's own site only.
pub async fn creators(headers: HeaderMap) -> axum::response::Response {
    if !is_home(&headers) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    ([(header::CACHE_CONTROL, "public, max-age=300")], Html(CREATORS_PAGE)).into_response()
}

#[derive(Serialize, Deserialize, Clone)]
struct ContactNote {
    id: String,
    at_ms: i64,
    name: String,
    reach: String,
    message: String,
}

fn notes(home: &std::path::Path) -> Vec<ContactNote> {
    std::fs::read_to_string(home.join(CONTACT_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_notes(home: &std::path::Path, all: &[ContactNote]) -> std::io::Result<()> {
    let tmp = home.join(format!("{CONTACT_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(all).unwrap_or_default())?;
    std::fs::rename(tmp, home.join(CONTACT_FILE))
}

/// Recent notes, for a simple limit on how many can arrive (whoever sends them).
static RECENT: std::sync::LazyLock<Mutex<Vec<(i64, String)>>> = std::sync::LazyLock::new(|| Mutex::new(Vec::new()));
static LAST_DM: Mutex<i64> = Mutex::new(0);
const PER_VISITOR_HOUR: usize = 3;
const PER_HOUR: usize = 20;

#[derive(Deserialize)]
pub struct ContactRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    reach: String,
    #[serde(default)]
    message: String,
    /// A box people never see: anything in it means a bot filled in the form.
    #[serde(default)]
    website: String,
}

/// Plain text only, without control characters, at most `max` characters.
fn clean(t: &str, max: usize, lines: bool) -> String {
    t.chars().filter(|c| !c.is_control() || (lines && *c == '\n')).take(max).collect::<String>().trim().to_string()
}

/// `POST /v1/contact`: a note from the "Get your own page" form, kept for the site's owner.
pub async fn contact(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<ContactRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    if !is_home(&headers) {
        return Err((StatusCode::NOT_FOUND, "Not found".into()));
    }
    // A bot that filled in the hidden box gets the same answer, and nothing is kept.
    if !req.website.trim().is_empty() {
        return Ok(Json(serde_json::json!({ "ok": true })));
    }
    let (name, reach, message) = (clean(&req.name, 80, false), clean(&req.reach, 120, false), clean(&req.message, 2000, true));
    if name.is_empty() || reach.is_empty() {
        return Err(bad("please add your name and how to reach you"));
    }
    if message.chars().count() < 10 {
        return Err(bad("please say a little about what you'd like to sell"));
    }
    let now = wall_ms();
    let ip = super::client_ip(&headers).unwrap_or_default();
    {
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        recent.retain(|(t, _)| now - t < 3_600_000);
        // A few notes an hour from one visitor, and a cap for everyone together.
        if !ip.is_empty() && recent.iter().filter(|(_, i)| *i == ip).count() >= PER_VISITOR_HOUR {
            return Err((StatusCode::TOO_MANY_REQUESTS, "thanks, your notes arrived; please wait a while before sending another".into()));
        }
        if recent.len() >= PER_HOUR {
            return Err((StatusCode::TOO_MANY_REQUESTS, "lots of notes are arriving right now; please try again in a while".into()));
        }
        recent.push((now, ip));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let note = ContactNote { id: format!("{now:x}{nanos:08x}"), at_ms: now, name: name.clone(), reach: reach.clone(), message };
    let mut all = notes(&s.home);
    all.push(note);
    if all.len() > CONTACT_KEEP {
        let extra = all.len() - CONTACT_KEEP;
        all.drain(..extra);
    }
    save_notes(&s.home, &all).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("couldn't save the note: {e}")))?;
    println!("website: a note from the \"Get your own page\" form");
    // At most one message to the owner every 10 minutes, however many notes arrive.
    let tell = {
        let mut last = LAST_DM.lock().unwrap_or_else(|e| e.into_inner());
        let go = now - *last >= 600_000;
        if go {
            *last = now;
        }
        go
    };
    if tell {
        if let Some(owner) = s.shop.as_ref().and_then(|sh| sh.record()).and_then(|r| owner_hash_of(&r.owner).ok()) {
            dm_owner(&s, &owner.to_string(), "📬 A new note arrived through your KasPass website. Read it in the dashboard, Overview.").await;
        }
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct NotesQuery {
    session: String,
}

/// `GET /v1/creator/contact`: the notes, newest first (the site's creator only).
pub async fn creator_notes(State(s): State<AppState>, Query(q): Query<NotesQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    let mut all = notes(&s.home);
    all.reverse();
    Ok(Json(serde_json::json!({ "notes": all })))
}

#[derive(Deserialize)]
pub struct NoteDelete {
    session: String,
    id: String,
}

/// `POST /v1/creator/contact/delete`: removes one note.
pub async fn creator_note_delete(State(s): State<AppState>, Json(req): Json<NoteDelete>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let mut all = notes(&s.home);
    let before = all.len();
    all.retain(|n| n.id != req.id);
    if all.len() == before {
        return Err(bad("that note isn't here any more"));
    }
    save_notes(&s.home, &all).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true, "left": all.len() })))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn h(host: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::HOST, host.parse().unwrap());
        m
    }
    #[test]
    fn home_hosts() {
        assert!(is_home(&h("kaspass.app")));
        assert!(is_home(&h("KasPass.app:443")));
        assert!(is_home(&h("www.kaspass.app")));
        assert!(!is_home(&h("pass.kaspa.vision")));
        assert!(!is_home(&h("kaspass.app.evil.example")));
        assert!(!is_home(&h("evilkaspass.app")));
    }
    #[test]
    fn demo_links() {
        assert_eq!(to_demo(r#"<a href="/">x</a><a href="/help">y</a>"#), r#"<a href="/demo">x</a><a href="/help">y</a>"#);
        assert_eq!(clean("a\u{0}b\nc", 10, false), "abc");
        assert_eq!(clean("a\nb", 10, true), "a\nb");
    }
}
