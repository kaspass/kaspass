//! A creator's own look for their pages: colors (with an optional dark version), any Google
//! Font (downloaded once and served from this site, so visitors never contact Google), corner
//! style, logo, profile photo, banner and a "back to my website" link. Plus "Copy from my
//! website", which reads the creator's own site for colors, fonts and a logo as suggestions.
//!
//! Everything a visitor's browser gets from here is built from checked values only: colors are
//! `#rrggbb`, font names are letters, digits and spaces, pictures are real PNG/JPEG/WebP files or
//! SVGs with nothing active in them, served from this site.

use super::*;
use std::net::IpAddr;

const BRAND_FILE: &str = "brand.json";
const BRAND_DIR: &str = "brand";

#[derive(Clone, Serialize, Deserialize, Default, Debug, PartialEq)]
pub(crate) struct Palette {
    pub bg: String,
    pub text: String,
    pub accent: String,
    pub card: String,
}

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
pub(crate) struct Brand {
    /// The colors. With both, visitors get the one matching their device's light/dark setting.
    #[serde(default)]
    pub light: Option<Palette>,
    #[serde(default)]
    pub dark: Option<Palette>,
    #[serde(default)]
    pub head_font: String,
    #[serde(default)]
    pub body_font: String,
    /// "round" (pill buttons, soft cards), "soft" (rounded) or "square".
    #[serde(default)]
    pub corners: String,
    #[serde(default)]
    pub back_url: String,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub photo: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    /// With both palettes, which one a first-time visitor sees: "light" (the default, like most
    /// sites), "dark", or "device" (their device's setting). Visitors can switch with the
    /// sun/moon button either way, and a partner site can pass `?theme=light|dark`.
    #[serde(default)]
    pub start: String,
    /// Bumped on every change, so pages ask for fresh styles and pictures.
    #[serde(default)]
    pub version: i64,
}

pub(crate) fn load(home: &std::path::Path) -> Brand {
    if let Some(e) = super::examples::current() {
        return e.brand.clone();
    }
    std::fs::read_to_string(home.join(BRAND_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(home: &std::path::Path, b: &Brand) -> Result<(), ApiError> {
    // Written to a side file, then swapped in, so a reader never sees half a file.
    let tmp = home.join(format!("{BRAND_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(b).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::rename(&tmp, home.join(BRAND_FILE)).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

fn bad(msg: impl Into<String>) -> ApiError {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Colors

pub(crate) fn hex_ok(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())
}

fn rgb(c: &str) -> (f64, f64, f64) {
    let p = |i: usize| u8::from_str_radix(&c[i..i + 2], 16).unwrap_or(0) as f64;
    (p(1), p(3), p(5))
}

fn hex_of((r, g, b): (f64, f64, f64)) -> String {
    format!("#{:02x}{:02x}{:02x}", r.round().clamp(0.0, 255.0) as u8, g.round().clamp(0.0, 255.0) as u8, b.round().clamp(0.0, 255.0) as u8)
}

/// `a` mixed with `b`: `t` = 0 is all `a`, 1 is all `b`.
fn mix(a: &str, b: &str, t: f64) -> String {
    let (x, y) = (rgb(a), rgb(b));
    hex_of((x.0 + (y.0 - x.0) * t, x.1 + (y.1 - x.1) * t, x.2 + (y.2 - x.2) * t))
}

fn luminance(c: &str) -> f64 {
    let f = |v: f64| {
        let v = v / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = rgb(c);
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// The WCAG contrast ratio of two colors (1 to 21).
pub(crate) fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn check_palette(p: &Palette, which: &str) -> Result<(), ApiError> {
    for (name, c) in [("background", &p.bg), ("text", &p.text), ("buttons and links", &p.accent), ("cards", &p.card)] {
        if !hex_ok(c) {
            return Err(bad(format!("the {which} {name} color must look like #1a2b3c")));
        }
    }
    // Hard floor only: the dashboard warns well before this. Below 3:1 text is unreadable.
    if contrast(&p.text, &p.bg) < 3.0 || contrast(&p.text, &p.card) < 3.0 {
        return Err(bad(format!("the {which} text color is too close to its background to read")));
    }
    Ok(())
}

/// The CSS custom properties for one palette (the page's own variable names).
fn vars(p: &Palette) -> String {
    let ink = if contrast("#ffffff", &p.accent) >= contrast("#000000", &p.accent) { "#ffffff" } else { "#0a0a0a" };
    let dark_bg = luminance(&p.bg) < 0.2;
    format!(
        "--bg: {bg}; --panel: {card}; --line: {line}; --text: {text}; --muted: {muted}; --teal: {acc}; --teal-dim: {dim}; --accent-ink: {ink}; --accent-soft: {soft}; --strip: {strip}; color-scheme: {scheme};",
        bg = p.bg,
        card = p.card,
        line = mix(&p.card, &p.text, 0.16),
        text = p.text,
        muted = mix(&p.text, &p.bg, 0.36),
        acc = p.accent,
        dim = mix(&p.accent, &p.bg, 0.55),
        soft = mix(&p.accent, &p.bg, 0.88),
        strip = mix(&p.bg, &p.text, 0.06),
        scheme = if dark_bg { "dark" } else { "light" },
    )
}

fn radius(corners: &str) -> &'static str {
    match corners {
        "square" => "--r-card: 6px; --r-btn: 6px;",
        "soft" => "--r-card: 14px; --r-btn: 10px;",
        "round" => "--r-card: 20px; --r-btn: 999px;",
        _ => "",
    }
}

fn font_stack(name: &str, fallback: &str) -> String {
    if name.is_empty() { String::new() } else { format!("'{name}', {fallback}") }
}

/// The site's own look as CSS, layered over the page's defaults. Empty when nothing is set.
pub(crate) fn theme_css(b: &Brand) -> String {
    let mut root = String::new();
    let head = font_stack(&b.head_font, "system-ui, sans-serif");
    let body = font_stack(&b.body_font, "system-ui, sans-serif");
    if !head.is_empty() {
        root.push_str(&format!("--font-head: {head}; "));
    }
    if !body.is_empty() {
        root.push_str(&format!("--font-body: {body}; "));
    }
    root.push_str(radius(&b.corners));
    let mut css = String::new();
    match (&b.light, &b.dark) {
        (Some(l), Some(d)) => {
            // The page's start-up script marks <html data-theme="light|dark"> from the visitor's
            // own choice; without one, `start` decides.
            let (first, second, other) = if b.start == "dark" { (d, l, "light") } else { (l, d, "dark") };
            css.push_str(&format!(":root {{ {} {root} }}\n", vars(first)));
            css.push_str(&format!(":root[data-theme=\"{other}\"] {{ {} }}\n", vars(second)));
            if b.start == "device" {
                css.push_str(&format!("@media (prefers-color-scheme: dark) {{ :root:not([data-theme=\"light\"]) {{ {} }} }}\n", vars(d)));
            }
        }
        (Some(p), None) | (None, Some(p)) => css.push_str(&format!(":root {{ {} {root} }}\n", vars(p))),
        (None, None) if !root.is_empty() => css.push_str(&format!(":root {{ {root} }}\n")),
        _ => {}
    }
    css
}

// ---------------------------------------------------------------------------
// Fonts: any Google Font, downloaded once and served from here

pub(crate) fn font_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
}

fn fonts_base() -> String {
    // Tests point this at a stand-in; real sites always use Google's.
    crate::test_switch("KASPASS_FONTS_BASE").unwrap_or_else(|| "https://fonts.googleapis.com".into())
}

fn font_files_ok(url: &str) -> bool {
    url.starts_with("https://fonts.gstatic.com/") || crate::test_switch("KASPASS_FONTS_BASE").is_some_and(|b| url.starts_with(&b))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        // Google sends woff2 files only to browsers it recognises.
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
        .build()
        .unwrap_or_default()
}

/// Reads at most `max` bytes of a response; anything longer is refused.
async fn read_capped(mut r: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    if r.content_length().is_some_and(|n| n as usize > max) {
        return Err("too large".into());
    }
    let mut out = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|e| e.to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > max {
            return Err("too large".into());
        }
    }
    Ok(out)
}

/// Downloads one Google Font family (Latin letters, regular to bold) into this site's brand
/// folder and returns its @font-face rules pointing at the local copies.
async fn fetch_font(home: &std::path::Path, name: &str) -> Result<String, String> {
    let fam = name.replace(' ', "+");
    let base = fonts_base();
    let mut css = None;
    for q in [format!("{fam}:wght@400;500;600;700"), format!("{fam}:wght@400;700"), fam.clone()] {
        let r = http().get(format!("{base}/css2?family={q}&display=swap")).send().await.map_err(|_| "couldn't reach Google Fonts".to_string())?;
        if r.status().is_success() {
            css = Some(String::from_utf8_lossy(&read_capped(r, 400_000).await?).to_string());
            break;
        }
    }
    let css = css.ok_or_else(|| format!("\"{name}\" isn't a Google Font (check the spelling on fonts.google.com)"))?;
    let dir = home.join(BRAND_DIR).join("fonts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut out = String::new();
    let mut total = 0usize;
    // Google labels each block with its script: keep Latin and Latin Extended.
    for block in css.split("/* ").skip(1) {
        let (label, rule) = block.split_once("*/").unwrap_or(("", ""));
        if !matches!(label.trim(), "latin" | "latin-ext") {
            continue;
        }
        let weight = rule.split("font-weight:").nth(1).and_then(|w| w.split(';').next()).map(|w| w.trim().to_string()).unwrap_or("400".into());
        let style = rule.split("font-style:").nth(1).and_then(|w| w.split(';').next()).map(|w| w.trim().to_string()).unwrap_or("normal".into());
        let range = rule.split("unicode-range:").nth(1).and_then(|w| w.split(';').next()).map(|w| w.trim().to_string()).unwrap_or_default();
        let Some(url) = rule.split("url(").nth(1).and_then(|u| u.split(')').next()) else { continue };
        let ok_range = range.chars().all(|c| c.is_ascii_hexdigit() || "U+-, ?".contains(c));
        let ok_weight = weight.chars().all(|c| c.is_ascii_digit() || c == ' ');
        if !font_files_ok(url) || !ok_range || !ok_weight || !matches!(style.as_str(), "normal" | "italic") {
            continue;
        }
        let r = http().get(url).send().await.map_err(|_| "couldn't download the font".to_string())?;
        let bytes = read_capped(r, 600_000).await?;
        if !bytes.starts_with(b"wOF2") {
            return Err("Google sent something that isn't a font file".into());
        }
        total += bytes.len();
        if total > 4_000_000 {
            return Err("that font family is too large".into());
        }
        let id = faster_hex::hex_string(&blake2b_simd::Params::new().hash_length(12).hash(&bytes).as_bytes()[..]);
        std::fs::write(dir.join(format!("{id}.woff2")), &bytes).map_err(|e| e.to_string())?;
        out.push_str(&format!(
            "@font-face {{ font-family: '{name}'; font-style: {style}; font-weight: {weight}; font-display: swap; src: url(/brand/f/{id}.woff2) format('woff2');{} }}\n",
            if range.is_empty() { String::new() } else { format!(" unicode-range: {range};") }
        ));
    }
    if out.is_empty() {
        return Err(format!("\"{name}\" has no Latin letters to use"));
    }
    Ok(out)
}

/// Font files no longer named by the site's fonts are deleted, so changing fonts doesn't pile up.
fn prune_fonts(home: &std::path::Path, css: &str) {
    let Ok(dir) = std::fs::read_dir(home.join(BRAND_DIR).join("fonts")) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".woff2") && !css.contains(&format!("/brand/f/{name}")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn fonts_css_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join(BRAND_DIR).join("fonts.css")
}

// ---------------------------------------------------------------------------
// Pictures

pub(crate) const KINDS: [(&str, usize); 3] = [("logo", 300_000), ("photo", 400_000), ("banner", 1_500_000)];

/// The picture's type from its first bytes, or why it's refused.
pub(crate) fn sniff(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Ok("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("jpg");
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("webp");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "use a PNG, JPEG, WebP or SVG picture".to_string())?;
    let t = text.trim_start_matches('\u{feff}').trim_start();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("<svg") || (lower.starts_with("<?xml") && lower.contains("<svg")) {
        // A lint on top of the real protection (pictures are served with a policy that blocks
        // anything active). Normalise first: whitespace to spaces, and namespace prefixes off
        // tag names (`<x:script` is `<script`).
        let mut norm = String::with_capacity(lower.len());
        let mut chars = lower.chars().peekable();
        while let Some(c) = chars.next() {
            let c = if c.is_whitespace() { ' ' } else { c };
            norm.push(c);
            if c == '<' {
                let mut name = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphanumeric() || n == '_' || n == '-' || n == '.' || n == ':' || n == '/' { name.push(n); chars.next(); } else { break; }
                }
                let slash = name.starts_with('/');
                let bare = name.trim_start_matches('/').rsplit(':').next().unwrap_or("").to_string();
                if slash { norm.push('/'); }
                norm.push_str(&bare);
            }
        }
        let banned = ["<script", "javascript:", "<foreignobject", "<iframe", "<embed", "<object", "<!entity", "<!doctype", "@import", "url(http", "url(//", "<a ", "<a>", "<a/", "<use", "<image", "<feimage", "<animate", "<set ", "<set>", "<set/", "<handler", "<listener", "<link", "<meta", "<style", "&#", "data:"];
        if banned.iter().any(|b| norm.contains(b)) {
            return Err("that SVG has scripts, links or embedded content in it; export a plain SVG or use a PNG".into());
        }
        // Any link must point inside the picture itself (href="#gradient").
        for key in ["href=\"", "href='", "href= \"", "href ="] {
            let mut rest = norm.as_str();
            while let Some(i) = rest.find(key) {
                let after = rest[i + key.len()..].trim_start_matches([' ', '=', '"', '\'']);
                if !after.starts_with('#') {
                    return Err("that SVG links to something outside itself; export a plain SVG or use a PNG".into());
                }
                rest = &rest[i + key.len()..];
            }
        }
        // on…= attributes (onload, onclick, …)
        let bytes = norm.as_bytes();
        for i in 1..bytes.len().saturating_sub(3) {
            if (bytes[i - 1] == b' ' || bytes[i - 1] == b'/' || bytes[i - 1] == b'"' || bytes[i - 1] == b'\'') && bytes[i] == b'o' && bytes[i + 1] == b'n' {
                let rest = &norm[i + 2..];
                let word: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                if !word.is_empty() && rest[word.len()..].trim_start().starts_with('=') {
                    return Err("that SVG has scripts in it; export a plain SVG or use a PNG".into());
                }
            }
        }
        return Ok("svg");
    }
    Err("use a PNG, JPEG, WebP or SVG picture".into())
}

fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn store_picture(home: &std::path::Path, kind: &str, bytes: &[u8]) -> Result<Brand, ApiError> {
    let (_, max) = KINDS.iter().find(|(k, _)| *k == kind).ok_or_else(|| bad("the picture must be a logo, photo or banner"))?;
    if bytes.len() > *max {
        return Err(bad(format!("that picture is too large (the {kind} can be up to {} KB)", max / 1000)));
    }
    if bytes.is_empty() {
        return Err(bad("that file is empty"));
    }
    let ext = sniff(bytes).map_err(bad)?;
    let dir = home.join(BRAND_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for (k, _) in KINDS.iter().filter(|(k, _)| *k == kind) {
        for e in ["png", "jpg", "webp", "svg"] {
            let _ = std::fs::remove_file(dir.join(format!("{k}.{e}")));
        }
    }
    std::fs::write(dir.join(format!("{kind}.{ext}")), bytes).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut b = load(home);
    let name = Some(format!("{kind}.{ext}"));
    match kind {
        "logo" => b.logo = name,
        "photo" => b.photo = name,
        _ => b.banner = name,
    }
    b.version = now();
    save(home, &b)?;
    Ok(b)
}

/// Public: where a page finds the picture (with the version, so a new one shows at once).
pub(crate) fn picture_url(b: &Brand, kind: &str) -> Option<String> {
    let f = match kind {
        "logo" => &b.logo,
        "photo" => &b.photo,
        _ => &b.banner,
    };
    let ex = super::examples::current().map(|e| format!("&ex={}", e.id)).unwrap_or_default();
    f.as_ref().map(|_| format!("/brand/img/{kind}?v={}{ex}", b.version))
}

// ---------------------------------------------------------------------------
// Handlers

pub(super) fn routes(r: Router<AppState>) -> Router<AppState> {
    r.route("/v1/brand", get(public))
        .route("/brand/theme.css", get(theme))
        .route("/brand/fonts.css", get(fonts_css))
        .route("/brand/f/:file", get(font_file))
        .route("/brand/img/:kind", get(picture))
        .route("/v1/creator/brand", post(set_brand))
        .route("/v1/creator/brand/picture", post(set_picture))
        .route("/v1/creator/brand/picture-remove", post(remove_picture))
        .route("/v1/creator/brand/scan", post(scan))
        .route("/v1/creator/brand/logo-from-site", post(logo_from_site))
}

pub(crate) fn public_json(b: &Brand) -> serde_json::Value {
    serde_json::json!({
        "light": b.light, "dark": b.dark, "head_font": b.head_font, "body_font": b.body_font,
        "corners": b.corners, "back_url": b.back_url, "back_host": back_host(&b.back_url),
        "start": if b.start.is_empty() { "light" } else { b.start.as_str() },
        "logo": picture_url(b, "logo"), "photo": picture_url(b, "photo"), "banner": picture_url(b, "banner"),
        "version": b.version,
    })
}

pub(crate) fn back_host(u: &str) -> Option<String> {
    let rest = u.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_string();
    if host.is_empty() { None } else { Some(host.trim_start_matches("www.").to_string()) }
}

async fn public(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(public_json(&load(&s.home)))
}

async fn theme(State(s): State<AppState>) -> axum::response::Response {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], theme_css(&load(&s.home))).into_response()
}

async fn fonts_css(State(s): State<AppState>) -> axum::response::Response {
    let css = std::fs::read_to_string(fonts_css_path(&s.home)).unwrap_or_default();
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], css).into_response()
}

async fn font_file(State(s): State<AppState>, axum::extract::Path(file): axum::extract::Path<String>) -> axum::response::Response {
    let ok = file.len() == 30 && file.ends_with(".woff2") && file[..24].chars().all(|c| c.is_ascii_hexdigit());
    match ok.then(|| std::fs::read(s.home.join(BRAND_DIR).join("fonts").join(&file)).ok()).flatten() {
        Some(b) => ([(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")], b).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn picture(State(s): State<AppState>, axum::extract::Path(kind): axum::extract::Path<String>) -> axum::response::Response {
    if let Some(svg) = super::examples::picture(&kind) {
        return ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public, max-age=300"), (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox")], svg).into_response();
    }
    let b = load(&s.home);
    let f = match kind.as_str() {
        "logo" => b.logo,
        "photo" => b.photo,
        "banner" => b.banner,
        _ => None,
    };
    let Some(f) = f else { return (StatusCode::NOT_FOUND, "not found").into_response() };
    let ext = f.rsplit('.').next().unwrap_or("").to_string();
    match std::fs::read(s.home.join(BRAND_DIR).join(&f)) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, content_type(&ext)),
                (header::CACHE_CONTROL, "public, max-age=300"),
                // Even opened on its own, an SVG here can never run anything.
                (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[derive(Deserialize)]
struct BrandRequest {
    session: String,
    #[serde(default)]
    light: Option<Palette>,
    #[serde(default)]
    dark: Option<Palette>,
    #[serde(default)]
    head_font: String,
    #[serde(default)]
    body_font: String,
    #[serde(default)]
    corners: String,
    #[serde(default)]
    back_url: String,
    #[serde(default)]
    start: String,
    /// Back to the KasPass look (pictures stay until removed).
    #[serde(default)]
    reset: bool,
}

async fn set_brand(State(s): State<AppState>, Json(req): Json<BrandRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let mut b = load(&s.home);
    if req.reset {
        b.light = None;
        b.dark = None;
        b.head_font.clear();
        b.body_font.clear();
        b.corners.clear();
        b.version = now();
        save(&s.home, &b)?;
        let _ = std::fs::remove_file(fonts_css_path(&s.home));
        prune_fonts(&s.home, "");
        return Ok(Json(public_json(&b)));
    }
    if let Some(p) = &req.light {
        check_palette(p, "light")?;
    }
    if let Some(p) = &req.dark {
        check_palette(p, "dark")?;
    }
    let head = req.head_font.trim().to_string();
    let body = req.body_font.trim().to_string();
    for f in [&head, &body] {
        if !f.is_empty() && !font_name_ok(f) {
            return Err(bad("a font name can only have letters, numbers and spaces"));
        }
    }
    if !matches!(req.start.as_str(), "" | "light" | "dark" | "device") {
        return Err(bad("start must be light, dark or device"));
    }
    if !matches!(req.corners.as_str(), "" | "round" | "soft" | "square") {
        return Err(bad("corners must be round, soft or square"));
    }
    let back = req.back_url.trim().to_string();
    if !back.is_empty() && (back.len() > 200 || back_host(&back).is_none() || back.chars().any(|c| c.is_whitespace() || c == '"' || c == '<' || c == '>' || c == '\'')) {
        return Err(bad("the website link must start with https://"));
    }
    // New fonts: download them before saving, so a bad name changes nothing.
    if head != b.head_font || body != b.body_font || !fonts_css_path(&s.home).exists() {
        let mut css = String::new();
        let mut seen = std::collections::HashSet::new();
        for f in [&head, &body] {
            if !f.is_empty() && seen.insert(f.to_lowercase()) {
                css.push_str(&fetch_font(&s.home, f).await.map_err(bad)?);
            }
        }
        std::fs::create_dir_all(s.home.join(BRAND_DIR)).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        std::fs::write(fonts_css_path(&s.home), &css).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        prune_fonts(&s.home, &css);
        // A picture may have changed while the fonts downloaded: start from the file as it is now.
        b = load(&s.home);
    }
    b.light = req.light;
    b.dark = req.dark;
    b.head_font = head;
    b.body_font = body;
    b.corners = req.corners;
    b.start = req.start;
    b.back_url = back;
    b.version = now();
    save(&s.home, &b)?;
    println!("dashboard: the page's look was updated");
    Ok(Json(public_json(&b)))
}

#[derive(Deserialize)]
struct PictureQuery {
    #[serde(default)]
    session: String,
    kind: String,
}

/// The upload's body is the picture, so the session comes in a header (kept out of server logs).
async fn set_picture(State(s): State<AppState>, Query(q): Query<PictureQuery>, headers: HeaderMap, body: axum::body::Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let session = headers.get("x-kaspass-session").and_then(|v| v.to_str().ok()).map(String::from).unwrap_or(q.session);
    creator_of(&s, &session)?;
    let b = store_picture(&s.home, &q.kind, &body)?;
    Ok(Json(public_json(&b)))
}

#[derive(Deserialize)]
struct RemoveRequest {
    session: String,
    kind: String,
}

async fn remove_picture(State(s): State<AppState>, Json(req): Json<RemoveRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let mut b = load(&s.home);
    let slot = match req.kind.as_str() {
        "logo" => &mut b.logo,
        "photo" => &mut b.photo,
        "banner" => &mut b.banner,
        _ => return Err(bad("the picture must be a logo, photo or banner")),
    };
    if let Some(f) = slot.take() {
        let _ = std::fs::remove_file(s.home.join(BRAND_DIR).join(f));
    }
    b.version = now();
    save(&s.home, &b)?;
    Ok(Json(public_json(&b)))
}

// ---------------------------------------------------------------------------
// Copy from my website

/// Addresses a website on the open internet can't have (this machine, private networks, …).
pub(crate) fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_multicast() || v.is_documentation()
                || o[0] == 0 || (o[0] == 100 && (64..128).contains(&o[1])) || (o[0] == 192 && o[1] == 0 && o[2] == 0) || (o[0] == 198 && (18..20).contains(&o[1])) || o[0] >= 240
        }
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return private_ip(IpAddr::V4(m));
            }
            let s = v.segments();
            // IPv4-compatible (::a.b.c.d), unique local, link and site local, documentation,
            // NAT64, 6to4 and Teredo (both can tunnel to IPv4 addresses).
            let v4compat = s[..6].iter().all(|x| *x == 0) && (s[6] != 0 || s[7] > 1);
            v4compat || v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xffc0) == 0xfec0 || s[0] == 0x2001 && s[1] == 0x0db8 || s[0] == 0x64 && s[1] == 0xff9b || s[0] == 0x2002 || s[0] == 0x2001 && s[1] == 0
        }
    }
}

fn allow_local() -> bool {
    crate::test_switch("KASPASS_SCAN_ALLOW_LOCAL").is_some_and(|v| v == "1")
}

/// Which websites "Copy from my website" may read: the creator's own sites this server lets the
/// page send people back to, when it has any; otherwise any public website.
fn host_allowed(host: &str) -> bool {
    let origins = return_origins();
    if origins.is_empty() {
        return true;
    }
    origins.iter().filter_map(|o| back_host(o).or_else(|| o.strip_prefix("http://").map(|h| h.split('/').next().unwrap_or("").to_string()))).any(|h| {
        let h = h.trim_start_matches("www.");
        let host = host.trim_start_matches("www.");
        host == h || host.ends_with(&format!(".{h}"))
    })
}

/// One careful GET: https only (http too in tests), allowed hosts only, public addresses only
/// (checked, then pinned so the name can't change in between), at most `max` bytes, a short
/// timeout, and at most two redirects on the same host.
async fn safe_get(url: &str, max: usize) -> Result<(String, Vec<u8>, String), String> {
    let mut url = url.to_string();
    for _ in 0..3 {
        let (scheme, rest) = url.split_once("://").ok_or("give the full address, like https://example.com")?;
        if scheme != "https" && !(scheme == "http" && allow_local()) {
            return Err("only https:// websites".into());
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Err("that address isn't allowed".into());
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.contains(']') || authority.starts_with('[') && authority.contains("]:") => (h.trim_matches(['[', ']']).to_string(), p.parse::<u16>().map_err(|_| "bad port")?),
            _ => (authority.trim_matches(['[', ']']).to_string(), if scheme == "https" { 443 } else { 80 }),
        };
        if !allow_local() && port != 443 && port != 80 {
            return Err("only websites on the usual ports".into());
        }
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':') {
            return Err("that address isn't allowed".into());
        }
        if !host_allowed(&host) {
            return Err(format!("this site can only copy from its own website ({})", return_origins().join(", ")));
        }
        let addrs: Vec<std::net::SocketAddr> = tokio::time::timeout(std::time::Duration::from_secs(4), tokio::net::lookup_host((host.as_str(), port)))
            .await
            .map_err(|_| "couldn't find that website".to_string())?
            .map_err(|_| "couldn't find that website".to_string())?
            .collect();
        let addr = addrs.first().copied().ok_or("couldn't find that website")?;
        if !allow_local() && addrs.iter().any(|a| private_ip(a.ip())) {
            return Err("that address isn't on the public internet".into());
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(6))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve(&host, addr)
            .user_agent("KasPass brand reader")
            .build()
            .map_err(|e| e.to_string())?;
        let r = client.get(&url).send().await.map_err(|_| "couldn't load that website".to_string())?;
        if r.status().is_redirection() {
            let loc = r.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let next = if loc.starts_with("https://") || loc.starts_with("http://") { loc } else if loc.starts_with('/') { format!("{scheme}://{authority}{loc}") } else { return Err("that website sent a strange redirect".into()) };
            let nhost = next.split("://").nth(1).and_then(|r| r.split(['/', '?', '#', ':']).next()).unwrap_or("");
            if nhost.trim_start_matches("www.") != host.trim_start_matches("www.") {
                return Err("that website redirects somewhere else".into());
            }
            url = next;
            continue;
        }
        if !r.status().is_success() {
            return Err(format!("that website answered {}", r.status().as_u16()));
        }
        let ty = r.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let body = read_capped(r, max).await.map_err(|_| "that website is too large to read".to_string())?;
        return Ok((url, body, ty));
    }
    Err("too many redirects".into())
}

fn join_url(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.starts_with("https://") || href.starts_with("http://") {
        return Some(href.to_string());
    }
    if href.starts_with("//") {
        return Some(format!("https:{href}"));
    }
    let (scheme, rest) = base.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if href.starts_with('/') {
        return Some(format!("{scheme}://{authority}{href}"));
    }
    let path = rest[authority.len()..].split(['?', '#']).next().unwrap_or("/");
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    Some(format!("{scheme}://{authority}{dir}/{href}"))
}

/// The value of an attribute in one tag (`<link rel="icon" href="…">`).
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    for q in ['"', '\''] {
        let key = format!("{name}={q}");
        if let Some(i) = lower.find(&format!(" {key}")).or_else(|| lower.find(&format!("\t{key}"))).or_else(|| lower.find(&format!("\n{key}"))) {
            let start = i + 1 + key.len();
            let end = tag[start..].find(q)? + start;
            return Some(tag[start..end].to_string());
        }
    }
    None
}

fn tags<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{name}");
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find(&open) {
        let start = i + p;
        let Some(end) = lower[start..].find('>') else { break };
        out.push(&html[start..start + end + 1]);
        i = start + end + 1;
    }
    out
}

/// A CSS color (hex, rgb(), hsl()) as `#rrggbb`, if it's one we can read.
pub(crate) fn parse_color(v: &str) -> Option<String> {
    let v = v.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
    if let Some(h) = v.strip_prefix('#') {
        let h: String = h.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        return match h.len() {
            3 | 4 => Some(format!("#{}", h.chars().take(3).flat_map(|c| [c, c]).collect::<String>())),
            6 | 8 => Some(format!("#{}", &h[..6])),
            _ => None,
        };
    }
    let nums = |s: &str| -> Vec<f64> { s.split(|c: char| c == ',' || c == ' ' || c == '/').filter(|x| !x.is_empty()).map(|x| x.trim_end_matches('%').trim_end_matches("deg").parse::<f64>().unwrap_or(f64::NAN)).collect() };
    if let Some(inner) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")).and_then(|r| r.split(')').next()) {
        let n = nums(inner);
        if n.len() >= 3 && n[..3].iter().all(|x| x.is_finite()) {
            return Some(hex_of((n[0], n[1], n[2])));
        }
    }
    if let Some(inner) = v.strip_prefix("hsl(").or_else(|| v.strip_prefix("hsla(")).and_then(|r| r.split(')').next()) {
        let n = nums(inner);
        if n.len() >= 3 && n[..3].iter().all(|x| x.is_finite()) {
            let (h, s, l) = (n[0].rem_euclid(360.0) / 360.0, n[1] / 100.0, n[2] / 100.0);
            let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
            let p = 2.0 * l - q;
            let f = |t: f64| {
                let t = t.rem_euclid(1.0);
                if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p }
            };
            return Some(hex_of((f(h + 1.0 / 3.0) * 255.0, f(h) * 255.0, f(h - 1.0 / 3.0) * 255.0)));
        }
    }
    match v.as_str() {
        "white" => Some("#ffffff".into()),
        "black" => Some("#000000".into()),
        _ => None,
    }
}

#[derive(Default, Debug)]
struct Found {
    light: HashMap<&'static str, String>,
    dark: HashMap<&'static str, String>,
    light_sure: HashMap<&'static str, u8>,
    dark_sure: HashMap<&'static str, u8>,
    fonts: Vec<String>,
    /// From the site's buttons: "round" (pills), "soft" or "square".
    corners: Option<&'static str>,
}

/// Which palette slot a CSS custom property's name suggests, and how sure that is (3 = sure).
/// Names are split into words, so `--acc` and `--accent` count but `--acc-text`, `--bg2` and
/// `--green` don't.
fn slot_of(var: &str) -> Option<(&'static str, u8)> {
    let v = var.trim_start_matches("--").to_ascii_lowercase();
    let words: Vec<&str> = v.split(['-', '_']).filter(|w| !w.is_empty()).collect();
    let any = |w: &[&str]| words.iter().any(|x| w.contains(x));
    let none_of = |w: &[&str]| !words.iter().any(|x| w.contains(x));
    let side = ["muted", "soft", "dim", "hover", "active", "light", "lighter", "dark", "darker", "subtle", "secondary", "tertiary", "faint", "weak", "strong", "contrast", "inverse", "alt", "2", "3", "4"];
    let calm = none_of(&side);
    if calm && none_of(&["card", "btn", "button", "accent", "acc", "primary", "brand", "text", "ink", "fg"]) {
        if any(&["bg", "background"]) { return Some(("bg", 3)); }
        if any(&["canvas", "page"]) { return Some(("bg", 2)); }
    }
    if calm && none_of(&["text", "ink", "fg", "foreground", "border", "line"]) {
        if any(&["card", "surface", "panel"]) { return Some(("card", 3)); }
        if any(&["paper", "elevated"]) { return Some(("card", 2)); }
    }
    if calm && none_of(&["accent", "acc", "primary", "brand", "link", "btn", "button", "bg", "background", "card", "surface"]) {
        if any(&["text", "ink", "fg", "foreground"]) { return Some(("text", 3)); }
        if v == "color" { return Some(("text", 1)); }
    }
    if calm && none_of(&["fg", "text", "ink", "foreground", "bg", "background", "border", "line", "glow", "ring", "shadow"]) {
        if any(&["accent", "acc", "primary", "brand"]) { return Some(("accent", 3)); }
        if any(&["cta"]) { return Some(("accent", 2)); }
        if any(&["link"]) { return Some(("accent", 1)); }
    }
    None
}

/// Keeps the surest guess for each slot.
fn offer(target: &mut HashMap<&'static str, String>, sure: &mut HashMap<&'static str, u8>, slot: &'static str, p: u8, c: String) {
    if sure.get(slot).is_none_or(|old| p > *old) {
        sure.insert(slot, p);
        target.insert(slot, c);
    }
}

/// Reads colors and fonts out of a stylesheet.
fn read_css(css: &str, f: &mut Found) {
    // Bounded work whatever the stylesheet: at most 1 MB read, and at most 64 dark-mode blocks
    // matched (each match is one forward scan).
    let css = &css[..css.floor_char_boundary(1_000_000)];
    let mut dark_scans = 0;
    // Rough rule split: selector { body }. @media dark blocks mark their rules as dark.
    let lower = css.to_ascii_lowercase();
    let mut i = 0;
    let mut dark_until = 0usize;
    while i < lower.len() {
        let Some(open) = lower[i..].find('{') else { break };
        let open = i + open;
        let sel = lower[i..open].trim().to_string();
        let sel = sel.rsplit(['}', ';']).next().unwrap_or("").trim().to_string();
        if sel.starts_with("@media") {
            if (sel.contains("prefers-color-scheme: dark") || sel.contains("prefers-color-scheme:dark")) && open >= dark_until && dark_scans < 64 {
                dark_scans += 1;
                // Find the matching close of this block.
                let mut depth = 0;
                for (k, ch) in lower[open..].char_indices() {
                    if ch == '{' { depth += 1 } else if ch == '}' { depth -= 1; if depth == 0 { dark_until = open + k; break; } }
                }
            }
            i = open + 1;
            continue;
        }
        let Some(close) = lower[open..].find('}') else { break };
        let close = open + close;
        let body = &css[open + 1..close];
        let darkish = sel.contains("dark") || sel.contains("night");
        let dark = open < dark_until || darkish && (sel.contains("[data-theme") || sel.contains(".dark") || sel.contains(".night") || sel.contains("[data-mode") || sel.contains("[data-color") || sel.contains("[data-scheme"));
        let root_like = sel.contains(":root") || sel == "html" || sel == "body" || sel.contains("[data-theme") || sel.starts_with(".dark") || sel.starts_with(".light") || sel.starts_with("html[");
        let (target, sure) = if dark { (&mut f.dark, &mut f.dark_sure) } else { (&mut f.light, &mut f.light_sure) };
        // Buttons' corners: pills, soft or square.
        if f.corners.is_none() && (sel.contains("btn") || sel.contains("button") || sel.contains(".cta")) {
            if let Some(r) = body.to_ascii_lowercase().split("border-radius:").nth(1).and_then(|x| x.split([';', '}']).next()) {
                let r = r.trim();
                let px: f64 = r.trim_end_matches("px").trim_end_matches("rem").trim().parse().unwrap_or(-1.0);
                let px = if r.ends_with("rem") { px * 16.0 } else { px };
                f.corners = if r.ends_with('%') || px >= 30.0 { Some("round") } else if (0.0..=6.0).contains(&px) { Some("square") } else if px > 0.0 { Some("soft") } else { None };
            }
        }
        for decl in body.split(';') {
            let Some((k, v)) = decl.split_once(':') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k.starts_with("--") && root_like {
                if let (Some((slot, p)), Some(c)) = (slot_of(k), parse_color(v)) {
                    offer(target, sure, slot, p, c);
                }
                if k.contains("font") && !k.contains("size") && !k.contains("weight") {
                    if let Some(name) = first_font(v) {
                        if !f.fonts.contains(&name) {
                            f.fonts.push(name);
                        }
                    }
                }
            } else if sel == "body" || sel == "html" || sel == ":root" || sel == "html,body" || sel == "html, body" {
                match k {
                    "background" | "background-color" => {
                        if let Some(c) = parse_color(v.split_whitespace().next().unwrap_or("")) {
                            offer(target, sure, "bg", 1, c);
                        }
                    }
                    "color" => {
                        if let Some(c) = parse_color(v) {
                            offer(target, sure, "text", 1, c);
                        }
                    }
                    "font-family" | "font" => {
                        if let Some(name) = first_font(v) {
                            if !f.fonts.contains(&name) {
                                f.fonts.push(name);
                            }
                        }
                    }
                    _ => {}
                }
            } else if (sel.starts_with("h1") || sel.contains(" h1") || sel.starts_with("h1,")) && k == "font-family" {
                if let Some(name) = first_font(v) {
                    if !f.fonts.contains(&name) {
                        f.fonts.insert(0, name);
                    }
                }
            }
        }
        i = close + 1;
    }
}

/// The first real font in a font-family list (skipping generic and system fonts and variables).
fn first_font(v: &str) -> Option<String> {
    let generic = ["serif", "sans-serif", "monospace", "system-ui", "ui-sans-serif", "ui-serif", "ui-monospace", "-apple-system", "blinkmacsystemfont", "segoe ui", "roboto", "helvetica neue", "helvetica", "arial", "inherit", "initial", "apple color emoji", "segoe ui emoji", "noto color emoji", "cursive", "fantasy"];
    for part in v.split(',') {
        let n = part.trim().trim_matches(['"', '\'']).trim();
        // "font:" shorthand: the family is after the size.
        let n = n.rsplit(' ').next().filter(|_| n.contains("px ") || n.contains("rem ") || n.contains("em ")).unwrap_or(n);
        let n = n.trim_matches(['"', '\'']);
        if n.is_empty() || n.starts_with("var(") || generic.contains(&n.to_ascii_lowercase().as_str()) {
            continue;
        }
        if font_name_ok(n) {
            return Some(n.to_string());
        }
    }
    None
}

fn google_families(href: &str) -> Vec<String> {
    href.split(['?', '&'])
        .filter_map(|p| p.strip_prefix("family="))
        .filter_map(|f| {
            let name = f.split(':').next()?.replace('+', " ").replace("%20", " ");
            font_name_ok(&name).then_some(name)
        })
        .collect()
}

#[derive(Deserialize)]
struct ScanRequest {
    session: String,
    url: String,
}

async fn scan(State(s): State<AppState>, Json(req): Json<ScanRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let mut url = req.url.trim().to_string();
    if !url.contains("://") {
        url = format!("https://{url}");
    }
    let (final_url, html, _ty) = safe_get(&url, 2_000_000).await.map_err(bad)?;
    let html = String::from_utf8_lossy(&html).to_string();
    let mut f = Found::default();
    let mut gfonts: Vec<String> = Vec::new();
    let mut logo: Option<String> = None;
    let mut theme_color: Option<String> = None;
    for m in tags(&html, "meta") {
        if attr(m, "name").is_some_and(|n| n.eq_ignore_ascii_case("theme-color")) {
            if let Some(c) = attr(m, "content").and_then(|c| parse_color(&c)) {
                let dark = attr(m, "media").is_some_and(|m| m.contains("dark"));
                if dark { f.dark.entry("bg").or_insert(c); } else { theme_color.get_or_insert(c); }
            }
        }
        if logo.is_none() && attr(m, "property").is_some_and(|p| p == "og:logo") {
            logo = attr(m, "content");
        }
    }
    let mut sheets = Vec::new();
    let mut icons: Vec<(i32, String)> = Vec::new();
    for l in tags(&html, "link") {
        let rel = attr(l, "rel").unwrap_or_default().to_ascii_lowercase();
        let Some(href) = attr(l, "href") else { continue };
        if href.contains("fonts.googleapis.com") {
            gfonts.extend(google_families(&href.replace("&amp;", "&")));
        } else if rel.contains("stylesheet") {
            sheets.push(href);
        } else if rel.contains("apple-touch-icon") {
            icons.push((3, href));
        } else if rel.contains("icon") {
            icons.push((if href.ends_with(".svg") { 2 } else { 1 }, href));
        }
    }
    icons.sort_by_key(|(p, _)| -p);
    // Inline styles first, then up to three of the site's own stylesheets.
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<style") {
        let start = i + p;
        let Some(gt) = lower[start..].find('>') else { break };
        let Some(end) = lower[start + gt..].find("</style>").map(|e| e + gt) else { break };
        let css = &html[start + gt + 1..start + end];
        for imp in css.split("@import").skip(1) {
            if let Some(u) = imp.split(['"', '\'', '(', ')']).find(|x| x.contains("fonts.googleapis.com")) {
                gfonts.extend(google_families(u));
            }
        }
        read_css(css, &mut f);
        i = start + end + 8;
    }
    for href in sheets.iter().take(3) {
        let Some(u) = join_url(&final_url, href) else { continue };
        if let Ok((_, css, _)) = safe_get(&u, 1_000_000).await {
            let css = String::from_utf8_lossy(&css).to_string();
            for imp in css.split("@import").skip(1).take(5) {
                if let Some(u) = imp.split(['"', '\'', '(', ')']).find(|x| x.contains("fonts.googleapis.com")) {
                    gfonts.extend(google_families(u));
                }
            }
            read_css(&css, &mut f);
        }
    }
    if let Some(tc) = theme_color {
        // theme-color is usually the background or the brand color.
        if f.light.get("bg").is_none_or(|b| b == &tc) || luminance(&tc) > 0.85 || luminance(&tc) < 0.02 { f.light.entry("bg").or_insert(tc); } else { f.light.entry("accent").or_insert(tc); }
    }
    // Fonts: headings first (from h1 rules or the first Google family), then text.
    let mut fonts: Vec<String> = Vec::new();
    for n in f.fonts.iter().chain(gfonts.iter()) {
        if !fonts.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            fonts.push(n.clone());
        }
    }
    let pal = |m: &HashMap<&'static str, String>, fallback: Option<&Palette>| -> Option<Palette> {
        let bg = m.get("bg").cloned().or(fallback.map(|p| p.bg.clone()))?;
        let text = m.get("text").cloned().unwrap_or_else(|| if luminance(&bg) < 0.2 { "#f2f2f2".into() } else { "#111111".into() });
        let accent = m.get("accent").cloned().or(fallback.map(|p| p.accent.clone())).unwrap_or_else(|| text.clone());
        let card = m.get("card").cloned().unwrap_or_else(|| if luminance(&bg) < 0.2 { mix(&bg, "#ffffff", 0.05) } else if luminance(&bg) > 0.9 { mix(&bg, "#000000", 0.03) } else { "#ffffff".into() });
        Some(Palette { bg, text, accent, card })
    };
    let light = pal(&f.light, None);
    let dark = pal(&f.dark, light.as_ref()).filter(|d| Some(&d.bg) != light.as_ref().map(|l| &l.bg));
    // Light and dark the wrong way round (a dark site that also has a light mode): swap.
    let (light, dark) = match (&light, &dark) {
        (Some(l), Some(d)) if luminance(&l.bg) < luminance(&d.bg) => (dark.clone(), light.clone()),
        _ => (light, dark),
    };
    let logo_url = logo.or_else(|| icons.first().map(|(_, h)| h.clone())).and_then(|h| join_url(&final_url, &h));
    let head_font = fonts.first().cloned().unwrap_or_default();
    let body_font = fonts.get(1).cloned().unwrap_or_else(|| head_font.clone());
    Ok(Json(serde_json::json!({
        "url": final_url,
        "light": light,
        "dark": dark,
        "head_font": head_font,
        "body_font": body_font,
        "fonts": fonts,
        "logo_url": logo_url,
        "corners": f.corners,
        "back_url": back_host(&final_url).map(|h| format!("https://{h}")),
    })))
}

#[derive(Deserialize)]
struct LogoRequest {
    session: String,
    url: String,
}

async fn logo_from_site(State(s): State<AppState>, Json(req): Json<LogoRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let (_, bytes, _) = safe_get(req.url.trim(), 300_000).await.map_err(bad)?;
    let b = store_picture(&s.home, "logo", &bytes)?;
    Ok(Json(public_json(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_checked() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0]).unwrap(), "png");
        assert_eq!(sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"4\"/></svg>").unwrap(), "svg");
        for bad in [
            "<svg><script>alert(1)</script></svg>",
            "<svg onload=\"alert(1)\"></svg>",
            "<svg><g onclick = 'x()'></g></svg>",
            "<svg><foreignObject></foreignObject></svg>",
            "<svg><a href=\"javascript:alert(1)\">x</a></svg>",
            "<svg><image href=\"https://evil/x.png\"/></svg>",
            "<svg><use href=\"#a\"/></svg>",
            "<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY x \"y\">]><svg></svg>",
            "<html><body>hi</body></html>",
            "<svg><x:script xmlns:x=\"http://www.w3.org/2000/svg\">alert(1)</x:script></svg>",
            "<svg><a\nhref=\"&#106;avascript:alert(1)\">x</a></svg>",
            "<svg><set\tattributeName=\"x\"/></svg>",
            "<svg><feImage href=\"//evil/x\"/></svg>",
            "<svg><g><path href=\"//evil.com\"/></g></svg>",
        ] {
            assert!(sniff(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(sniff(b"<svg><defs><linearGradient id=\"g\"/></defs><rect fill=\"url(#g)\"/><path href=\"#g\"/></svg>").is_ok());
        // A word starting with "on" that isn't an attribute is fine.
        assert!(sniff(b"<svg><title>one</title><path d=\"M0 0\" class=\"online\"/></svg>").is_ok());
    }

    #[test]
    fn colors_and_contrast() {
        assert_eq!(parse_color("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_color("rgb(19, 127, 109)").as_deref(), Some("#137f6d"));
        assert_eq!(parse_color("hsl(0, 100%, 50%)").as_deref(), Some("#ff0000"));
        assert_eq!(parse_color("var(--x)"), None);
        assert!(contrast("#ffffff", "#000000") > 20.9);
        assert!(check_palette(&Palette { bg: "#ffffff".into(), text: "#fefefe".into(), accent: "#137f6d".into(), card: "#ffffff".into() }, "light").is_err());
        assert!(check_palette(&Palette { bg: "#f4f9f7".into(), text: "#0b2420".into(), accent: "#137f6d".into(), card: "#ffffff".into() }, "light").is_ok());
        assert!(check_palette(&Palette { bg: "red".into(), text: "#0b2420".into(), accent: "#137f6d".into(), card: "#ffffff".into() }, "light").is_err());
    }

    #[test]
    fn names_and_addresses() {
        assert!(font_name_ok("Space Grotesk"));
        assert!(!font_name_ok("x'; } body { display:none"));
        assert!(!font_name_ok(""));
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "::169.254.169.254", "2002:a9fe:a9fe::1", "2001:0:4136:e378::1", "fec0::1"] {
            assert!(private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "2606:4700::1111"] {
            assert!(!private_ip(ip.parse().unwrap()), "{ip}");
        }
        assert_eq!(back_host("https://www.kaspa.vision/path").as_deref(), Some("kaspa.vision"));
        assert_eq!(back_host("http://kaspa.vision"), None);
    }

    #[test]
    fn hostile_css_is_cheap() {
        // Thousands of unclosed dark-mode blocks: bounded work, no panic.
        let css = "@media (prefers-color-scheme:dark){".repeat(30_000);
        let t = std::time::Instant::now();
        let mut f = Found::default();
        read_css(&css, &mut f);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn reads_a_day_night_site() {
        let mut f = Found::default();
        read_css("body{margin:0;background:var(--bg)} a{color:#0F7564} html,html[data-theme=\"day\"]{--bg:#F4F9F7;--bg2:#E9F2EF;--soft:#E0ECE8;--ink:#0B2420;--ink2:#2F4D48;--acc:#137F6D;--accT:#0F7564;--accInk:#FFFFFF;--green:#2E8B57;--glow:rgba(19,127,109,0.14)} html[data-theme=\"night\"]{--bg:#081013;--bg2:#0d181b;--ink:#E6F2EF;--acc:#49EACB;--accInk:#05211C;--green:#3fbf7f} .kv-btn{padding:10px;border-radius:999px}", &mut f);
        assert_eq!(f.light.get("accent").map(String::as_str), Some("#137f6d"));
        assert_eq!(f.light.get("bg").map(String::as_str), Some("#f4f9f7"));
        assert_eq!(f.light.get("text").map(String::as_str), Some("#0b2420"));
        assert_eq!(f.dark.get("bg").map(String::as_str), Some("#081013"));
        assert_eq!(f.dark.get("accent").map(String::as_str), Some("#49eacb"));
        assert_eq!(f.dark.get("text").map(String::as_str), Some("#e6f2ef"));
        assert_eq!(f.corners, Some("round"));
    }

    #[test]
    fn reads_a_site() {
        let mut f = Found::default();
        read_css(":root{--bg:#f4f9f7;--ink:#0b2420;--accent:#137f6d;--surface:#fff;--font-display:'Space Grotesk',sans-serif} body{font-family:Inter,system-ui} @media (prefers-color-scheme: dark){:root{--bg:#081013;--ink:#e6f2ef;--accent:#49eacb}} h1{font-family:'Space Grotesk'}", &mut f);
        assert_eq!(f.light.get("bg").map(String::as_str), Some("#f4f9f7"));
        assert_eq!(f.light.get("text").map(String::as_str), Some("#0b2420"));
        assert_eq!(f.light.get("accent").map(String::as_str), Some("#137f6d"));
        assert_eq!(f.light.get("card").map(String::as_str), Some("#ffffff"));
        assert_eq!(f.dark.get("bg").map(String::as_str), Some("#081013"));
        assert_eq!(f.dark.get("accent").map(String::as_str), Some("#49eacb"));
        assert_eq!(f.fonts.first().map(String::as_str), Some("Space Grotesk"));
        assert!(f.fonts.iter().any(|x| x == "Inter"));
    }
}
