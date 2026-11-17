//! KasPass's own website. On KasPass's own host names (kaspass.app by default, or the
//! comma-separated list in `KASPASS_HOME_HOSTS`), `/` is KasPass's home page, `/creators` is
//! "Get your own page" with a contact form, and the membership page moves to `/demo`. Every
//! other site keeps its membership page at `/`.

use super::*;

const HOME_PAGE: &str = include_str!("../../web/home.html");
const CREATORS_PAGE: &str = include_str!("../../web/creators.html");
/// Notes sent through the "Get your own page" form, newest last.
const CONTACT_FILE: &str = "contact-requests.json";
const CONTACT_KEEP: usize = 1000;

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
/// The link preview's address: the host that was asked for (only plain host characters), over
/// https except on this computer, and the page's own path.
pub(crate) fn fill_preview(headers: &HeaderMap, uri: &axum::http::Uri, html: String) -> String {
    let host: String = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(*c)).take(253).collect();
    let local = host.starts_with("127.0.0.1") || host.starts_with("localhost") || host.starts_with("[::1]");
    let origin = if host.is_empty() { String::new() } else { format!("{}://{host}", if local { "http" } else { "https" }) };
    let path: String = uri.path().chars().filter(|c| c.is_ascii_alphanumeric() || "/-_.".contains(*c)).take(200).collect();
    html.replace(super::PAGE_ORIGIN, &origin).replace(super::PAGE_PATH, &path)
}

fn membership_page(s: &AppState, headers: &HeaderMap, uri: &axum::http::Uri, html: String) -> axum::response::Response {
    match gate(s, headers, uri) {
        Err(r) => r,
        Ok(cookie) => {
            let html = fill_preview(headers, uri, html);
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
    out
}

/// How people can reach KasPass's own site owner, from `contact.json` next to the data
/// (`{"telegram": "handle", "email": "hello@example.com"}`, either or both). Kept out of the code
/// so nothing personal sits in the repository.
const CONTACT_SETTINGS: &str = "contact.json";

fn contact_html(home: &std::path::Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(CONTACT_SETTINGS)).ok()?).ok()?;
    let ok = |t: &str, extra: &str| !t.is_empty() && t.len() <= 80 && t.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c) || extra.contains(c));
    let tg = v["telegram"].as_str().map(|t| t.trim().trim_start_matches('@').to_string()).filter(|t| ok(t, ""));
    let mail = v["email"].as_str().map(|t| t.trim().to_string()).filter(|t| ok(t, "@+") && t.contains('@'));
    let parts: Vec<String> = [
        tg.map(|t| format!(r#"<a href="https://t.me/{t}" rel="noopener">@{t} on Telegram</a>"#)),
        mail.map(|m| format!(r#"<a href="mailto:{m}">{m}</a>"#)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" or "))
}

/// Keeps `<!--contact-->…<!--/contact-->` (with `{contact}` filled in) when there's a contact,
/// and `<!--nocontact-->…<!--/nocontact-->` when there isn't; drops the other.
pub fn fill_contact(home: &std::path::Path, html: &str) -> String {
    let c = contact_html(home);
    let (keep, drop) = if c.is_some() { ("contact", "nocontact") } else { ("nocontact", "contact") };
    let html = strip_blocks(html, drop);
    html.replace(&format!("<!--{keep}-->"), "").replace(&format!("<!--/{keep}-->"), "").replace("{contact}", c.as_deref().unwrap_or(""))
}

fn strip_blocks(html: &str, name: &str) -> String {
    let (open, close) = (format!("<!--{name}-->"), format!("<!--/{name}-->"));
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(&open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(&close) {
            Some(j) => rest = &rest[i + j + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// " (about $0.25 today)" for a KAS amount, from the last rate read (empty before there is one).
fn usd_hint(kas: f64) -> String {
    crate::prices::last().filter(|r| r.kas_usd > 0.0).map(|r| format!(" (about US${:.2} today)", kas * r.kas_usd)).unwrap_or_default()
}

/// On KasPass's own site the membership page lives at /demo: its links to "/" follow it there.
pub fn to_demo(html: &str) -> String {
    html.replace(r#"href="/""#, r#"href="/demo""#)
}

/// `/`: KasPass's home page on its own host names, the membership page everywhere else.
pub async fn root(State(s): State<AppState>, headers: HeaderMap, uri: axum::http::Uri) -> axum::response::Response {
    if is_home(&headers) {
        // Older links to the membership page (`/?level=...`, `?return=...`) keep working; other
        // queries (`?utm_source=`, `?fbclid=`) just show the home page.
        const PAGE_KEYS: [&str; 7] = ["level=", "return=", "invite=", "do=", "theme=", "style=", "k="];
        if let Some(q) = uri.query().filter(|q| q.split('&').any(|kv| PAGE_KEYS.iter().any(|k| kv.starts_with(k)))) {
            return axum::response::Redirect::to(&format!("/demo?{q}")).into_response();
        }
        let html = fill_contact(&s.home, &home_page(&s)).replace("{kas1}", &usd_hint(1.0)).replace("{kas25}", &usd_hint(25.0)).replace("{kas200}", &usd_hint(200.0));
        return ([(header::CACHE_CONTROL, "public, max-age=60")], Html(html)).into_response();
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
pub async fn creators(State(s): State<AppState>, headers: HeaderMap) -> axum::response::Response {
    if !is_home(&headers) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let page = fill_contact(&s.home, CREATORS_PAGE);
    // The form carries the moment the page was made, signed by this server: a note must come at
    // least a few seconds later (people take that long to type; scripts posting straight away don't).
    let page = page.replacen(r#"<input id="cT" name="t" type="hidden" value="">"#, &format!(r#"<input id="cT" name="t" type="hidden" value="{}">"#, form_stamp(wall_ms())), 1);
    ([(header::CACHE_CONTROL, "no-cache")], Html(page)).into_response()
}

const PRIVACY_PAGE: &str = include_str!("../../web/privacy.html");
const TERMS_PAGE: &str = include_str!("../../web/terms.html");

/// `/privacy` and `/terms`, on KasPass's own site only.
pub async fn privacy(State(s): State<AppState>, headers: HeaderMap) -> axum::response::Response {
    legal(&s, &headers, PRIVACY_PAGE)
}

pub async fn terms(State(s): State<AppState>, headers: HeaderMap) -> axum::response::Response {
    legal(&s, &headers, TERMS_PAGE)
}

fn legal(s: &AppState, headers: &HeaderMap, page: &str) -> axum::response::Response {
    if !is_home(headers) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    ([(header::CACHE_CONTROL, "public, max-age=300")], Html(fill_contact(&s.home, page))).into_response()
}

/// `/robots.txt`: KasPass's own site lists its pages; creators' sites keep the dashboard and API out.
pub async fn robots(headers: HeaderMap) -> axum::response::Response {
    let mut t = String::from("User-agent: *\nDisallow: /dashboard\nDisallow: /club\nDisallow: /v1/\n");
    if is_home(&headers) {
        t.push_str("Disallow: /demo\nSitemap: https://kaspass.app/sitemap.xml\n");
    }
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=3600")], t).into_response()
}

/// `/sitemap.xml`, on KasPass's own site only.
pub async fn sitemap(headers: HeaderMap) -> axum::response::Response {
    if !is_home(&headers) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let urls = ["", "creators", "help", "privacy", "terms"].iter().map(|p| format!("  <url><loc>https://kaspass.app/{p}</loc></url>\n")).collect::<String>();
    let x = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{urls}</urlset>\n");
    ([(header::CONTENT_TYPE, "application/xml; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=3600")], x).into_response()
}

/// A secret made at start, for signing the form's time stamp.
static FORM_KEY: std::sync::LazyLock<[u8; 32]> = std::sync::LazyLock::new(secp256k1::rand::random);

fn form_stamp(ms: i64) -> String {
    let mac = blake2b_simd::Params::new().hash_length(16).key(&*FORM_KEY).hash(ms.to_string().as_bytes());
    format!("{ms}.{}", mac.to_hex())
}

/// How old a valid stamp is (ms), or None when it isn't one of ours.
fn stamp_age(t: &str) -> Option<i64> {
    let (ms, _) = t.split_once('.')?;
    let ms: i64 = ms.parse().ok()?;
    (form_stamp(ms) == t).then(|| wall_ms() - ms)
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
/// (when, visitor, text, looked like junk), for the last day.
static RECENT: std::sync::LazyLock<Mutex<Vec<(i64, String, String, bool)>>> = std::sync::LazyLock::new(|| Mutex::new(Vec::new()));
static LAST_DM: Mutex<i64> = Mutex::new(0);
/// Notes kept per visitor in a day, and site-wide in an hour (a safety net only).
const PER_VISITOR_DAY: usize = 3;
const PER_HOUR: usize = 60;
/// Notes that look like junk (no words, only a link, or the same text again): a small site-wide allowance.
const JUNK_PER_HOUR: usize = 5;
/// Where to write instead, shown when the form can't take a note.
const OTHER_WAY: &str = "You can also reach us on Telegram or by email (see the bottom of this page).";

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
    /// The page's signed time stamp.
    #[serde(default)]
    t: String,
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
    match stamp_age(&req.t) {
        None => return Err(bad(format!("this page is out of date. Please reload it and send your note again. {OTHER_WAY}"))),
        Some(age) if age < 4_000 => return Err(bad("that was very quick. Please check your note and press Send again.")),
        Some(age) if age > 2 * 86_400_000 => return Err(bad("this page has been open a long time. Please reload it and send your note again.")),
        Some(_) => {}
    }
    let now = wall_ms();
    let ip = super::client_ip(&headers).unwrap_or_default();
    let junk = !message.chars().any(char::is_alphabetic) || (message.starts_with("http") && !message.trim().contains(' '));
    {
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        recent.retain(|r| now - r.0 < 86_400_000);
        let again = recent.iter().any(|r| r.2 == message);
        let junk = junk || again;
        let last_hour = recent.iter().filter(|r| now - r.0 < 3_600_000);
        if !ip.is_empty() && recent.iter().filter(|r| r.1 == ip).count() >= PER_VISITOR_DAY {
            return Err((StatusCode::TOO_MANY_REQUESTS, "thanks, your notes arrived. Please wait until tomorrow before sending another.".into()));
        }
        if junk && last_hour.clone().filter(|r| r.3).count() >= JUNK_PER_HOUR {
            return Err((StatusCode::TOO_MANY_REQUESTS, format!("we couldn't take that note right now. Please add a few words about what you'd like to sell. {OTHER_WAY}")));
        }
        if last_hour.count() >= PER_HOUR {
            return Err((StatusCode::TOO_MANY_REQUESTS, format!("lots of notes are arriving right now; please try again in a while. {OTHER_WAY}")));
        }
        recent.push((now, ip, message.clone(), junk));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let note = ContactNote { id: format!("{now:x}{nanos:08x}"), at_ms: now, name: name.clone(), reach: reach.clone(), message };
    let mut all = notes(&s.home);
    // Never push out a note the owner hasn't dealt with: when the inbox is full, refuse new ones.
    if all.len() >= CONTACT_KEEP {
        return Err((StatusCode::SERVICE_UNAVAILABLE, format!("our inbox is full right now. {OTHER_WAY}")));
    }
    all.push(note);
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
