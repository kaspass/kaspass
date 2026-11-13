//! Example creators for KasPass's public demo (kaspass.app/demo/<id>): a podcast, an NFT
//! community, a coffee club and a newsletter, all made up. Each is a different look (name, colors,
//! pictures, plan names and descriptions) over the same real Testnet-10 plans, so visitors can see
//! what KasPass would look like for their kind of project and try it with free test KAS.
//!
//! Only on KasPass's own host names. A page asks for an example with the `x-kp-example` header
//! (its scripts add it to every `/v1/` call) or `?ex=` on its pictures and styles; while serving
//! that request, the page readers (names, descriptions, look) answer from the example instead of
//! the creator's files. Nothing is written: the creator's own page and dashboard never change.

use super::*;
use std::sync::Arc;

tokio::task_local! {
    static EXAMPLE: Arc<Example>;
}

/// The example being shown in this request, if any.
pub fn current() -> Option<Arc<Example>> {
    EXAMPLE.try_with(|e| e.clone()).ok()
}

pub struct Example {
    pub id: &'static str,
    pub creator: &'static str,
    /// Plan name, description (HTML) and perks, by level covenant id.
    pub names: HashMap<String, String>,
    pub about: HashMap<String, String>,
    pub perks: HashMap<String, Vec<String>>,
    pub word: (&'static str, &'static str),
    pub page: PageProfile,
    pub brand: brand::Brand,
    pub pictures: [(&'static str, &'static str); 3],
}

struct Spec {
    id: &'static str,
    label: &'static str,
    creator: &'static str,
    tagline: &'static str,
    intro: &'static str,
    word: (&'static str, &'static str),
    /// Name, description and perks for the lowest, middle and highest plan.
    plans: [(&'static str, &'static str, &'static [&'static str]); 3],
    light: [&'static str; 4],
    dark: [&'static str; 4],
    corners: &'static str,
    start: &'static str,
    /// Logo, photo and banner, as SVG.
    logo: &'static str,
    photo: &'static str,
    banner: &'static str,
}

const SPECS: &[Spec] = &[
    Spec {
        id: "podcast",
        label: "Podcast",
        creator: "Hollow Pine Radio",
        tagline: "A weekly show about people who build things by hand.",
        intro: "<p>New episodes every Thursday. Members hear them without ads, a day early, and join a live Q&amp;A once a month.</p>",
        word: ("membership", "memberships"),
        plans: [
            ("Listener", "<p>Ad-free episodes, a day before everyone else.</p>", &["Ad-free episodes", "Early access", "Members' chat"]),
            ("Supporter", "<p>Join the monthly live Q&amp;A with the hosts.</p>", &["Monthly live Q&A", "Your name in the show notes"]),
            ("Patron", "<p>The inner circle of the show.</p>", &["Bonus episodes", "A thank-you on air"]),
        ],
        light: ["#f6f1e7", "#1f2a24", "#2f6b4f", "#fffaf0"],
        dark: ["#121a16", "#e9efe9", "#7cc49a", "#1a241f"],
        corners: "round",
        start: "light",
        logo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" rx="112" fill="#2f6b4f"/><path d="M256 92 L380 300 H300 L340 372 H172 L212 300 H132 Z" fill="#f6f1e7"/><rect x="238" y="372" width="36" height="56" rx="8" fill="#f6f1e7"/></svg>"##,
        photo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="#e4d9c3"/><circle cx="256" cy="210" r="96" fill="#2f6b4f"/><rect x="226" y="300" width="60" height="110" rx="30" fill="#2f6b4f"/><rect x="176" y="410" width="160" height="22" rx="11" fill="#1f2a24"/></svg>"##,
        banner: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1600 400"><rect width="1600" height="400" fill="#2f6b4f"/><path d="M0 300 L180 170 L330 290 L520 120 L720 300 L900 160 L1100 300 L1300 140 L1600 300 V400 H0 Z" fill="#245640"/><path d="M0 340 L240 250 L460 340 L700 230 L960 340 L1200 240 L1600 340 V400 H0 Z" fill="#1c4433"/><circle cx="1320" cy="90" r="46" fill="#f6f1e7" opacity=".85"/></svg>"##,
    },
    Spec {
        id: "nft",
        label: "NFT community",
        creator: "Lantern Moths",
        tagline: "An art collective of 2,222 hand-drawn moths on Kaspa.",
        intro: "<p>Members get the collective's drops, votes and private chat. Holders of a Lantern Moth can get perks too: a site can check what a wallet holds and give its holders a discount or a free month.</p>",
        word: ("pass", "passes"),
        plans: [
            ("Wanderer", "<p>Join the chat and see every drop first.</p>", &["Private chat", "Early look at new drops"]),
            ("Keeper", "<p>Help steer the collective.</p>", &["Allowlist spot for each drop", "Vote on new art"]),
            ("Elder", "<p>The collective's inner circle.</p>", &["Studio calls with the artists", "A one-of-one moth drawn for you"]),
        ],
        light: ["#f3f0fa", "#211b33", "#6b4fd8", "#ffffff"],
        dark: ["#0f0b1a", "#ece8f7", "#b7a2ff", "#191329"],
        corners: "soft",
        start: "dark",
        logo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" rx="96" fill="#191329"/><ellipse cx="176" cy="226" rx="104" ry="84" fill="#b7a2ff"/><ellipse cx="336" cy="226" rx="104" ry="84" fill="#b7a2ff"/><ellipse cx="196" cy="340" rx="64" ry="52" fill="#8a72e8"/><ellipse cx="316" cy="340" rx="64" ry="52" fill="#8a72e8"/><rect x="240" y="150" width="32" height="250" rx="16" fill="#ffd36e"/></svg>"##,
        photo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="#2a2140"/><circle cx="256" cy="256" r="150" fill="#ffd36e" opacity=".9"/><ellipse cx="196" cy="246" rx="70" ry="56" fill="#6b4fd8"/><ellipse cx="316" cy="246" rx="70" ry="56" fill="#6b4fd8"/><rect x="242" y="190" width="28" height="140" rx="14" fill="#191329"/></svg>"##,
        banner: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1600 400"><rect width="1600" height="400" fill="#191329"/><circle cx="260" cy="120" r="6" fill="#ece8f7"/><circle cx="520" cy="70" r="4" fill="#ece8f7"/><circle cx="900" cy="140" r="5" fill="#ece8f7"/><circle cx="1180" cy="60" r="4" fill="#ece8f7"/><circle cx="1420" cy="150" r="6" fill="#ece8f7"/><circle cx="800" cy="260" r="110" fill="#ffd36e" opacity=".25"/><circle cx="800" cy="260" r="60" fill="#ffd36e" opacity=".6"/><ellipse cx="720" cy="250" rx="60" ry="46" fill="#b7a2ff"/><ellipse cx="880" cy="250" rx="60" ry="46" fill="#b7a2ff"/><rect x="788" y="200" width="24" height="120" rx="12" fill="#191329"/></svg>"##,
    },
    Spec {
        id: "coffee",
        label: "Coffee club",
        creator: "Driftwood Roasters",
        tagline: "Small-batch coffee from a tiny roastery by the sea.",
        intro: "<p>A fresh bag every month, roasted the week it ships. Members also get first pick of rare lots and a discount in the café.</p>",
        word: ("subscription", "subscriptions"),
        plans: [
            ("Monthly Bag", "<p>One fresh 340 g bag a month.</p>", &["A fresh bag every month", "10% off in the café"]),
            ("Roaster's Pick", "<p>Rare lots and seasonal tastings.</p>", &["First pick of rare lots", "A free tasting each season"]),
            ("Mug Club", "<p>Your own mug hangs on our wall.</p>", &["Your own mug at the café", "A free cup on every visit"]),
        ],
        light: ["#fbf6f0", "#2b1d14", "#a3542a", "#ffffff"],
        dark: ["#17110d", "#f3e9df", "#e19a6b", "#221912"],
        corners: "round",
        start: "light",
        logo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" rx="256" fill="#a3542a"/><ellipse cx="256" cy="256" rx="120" ry="170" fill="#fbf6f0" transform="rotate(-25 256 256)"/><path d="M200 120 C300 200 210 310 312 392" stroke="#a3542a" stroke-width="22" fill="none" stroke-linecap="round"/></svg>"##,
        photo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="#e8d6c4"/><rect x="146" y="190" width="190" height="200" rx="28" fill="#a3542a"/><path d="M336 230 h40 a46 46 0 0 1 0 92 h-40" stroke="#a3542a" stroke-width="26" fill="none"/><path d="M200 150 c0-30 30-30 30-60 M260 150 c0-30 30-30 30-60" stroke="#2b1d14" stroke-width="12" fill="none" stroke-linecap="round"/></svg>"##,
        banner: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1600 400"><rect width="1600" height="400" fill="#e8d6c4"/><path d="M0 260 C300 210 500 320 800 270 C1100 220 1300 320 1600 260 V400 H0 Z" fill="#a3542a"/><path d="M0 310 C320 270 520 360 820 320 C1120 280 1320 360 1600 320 V400 H0 Z" fill="#7a3e1f"/><circle cx="1300" cy="110" r="54" fill="#fbf6f0"/></svg>"##,
    },
    Spec {
        id: "newsletter",
        label: "Newsletter",
        creator: "The Slow Ledger",
        tagline: "One careful essay a week about money, tech and the long run.",
        intro: "<p>Free readers get the weekly essay. Members get the full archive, the monthly deep dive and the readers' forum.</p>",
        word: ("subscription", "subscriptions"),
        plans: [
            ("Reader", "<p>The full archive and the monthly deep dive.</p>", &["Full archive", "Monthly deep dive"]),
            ("Insider", "<p>Talk it over with the author and other readers.</p>", &["Readers' forum", "Quarterly call with the author"]),
            ("Founding Reader", "<p>Support the work at the highest level.</p>", &["Drafts before they're published", "Your name on the founders page"]),
        ],
        light: ["#ffffff", "#16181d", "#1f5fbf", "#f4f6fa"],
        dark: ["#0d1015", "#e8ecf3", "#7fb0ff", "#161b23"],
        corners: "square",
        start: "light",
        logo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="#16181d"/><rect x="120" y="120" width="272" height="34" fill="#ffffff"/><rect x="120" y="190" width="200" height="22" fill="#7fb0ff"/><rect x="120" y="240" width="272" height="22" fill="#ffffff" opacity=".7"/><rect x="120" y="290" width="240" height="22" fill="#ffffff" opacity=".7"/><rect x="120" y="340" width="150" height="22" fill="#ffffff" opacity=".7"/></svg>"##,
        photo: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="#dfe6f2"/><circle cx="256" cy="200" r="86" fill="#1f5fbf"/><path d="M110 430 C130 320 382 320 402 430 Z" fill="#1f5fbf"/></svg>"##,
        banner: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1600 400"><rect width="1600" height="400" fill="#f4f6fa"/><g stroke="#16181d" stroke-opacity=".12" stroke-width="2"><path d="M0 80 H1600 M0 160 H1600 M0 240 H1600 M0 320 H1600"/></g><path d="M120 300 L420 240 L700 270 L980 170 L1260 200 L1500 100" stroke="#1f5fbf" stroke-width="10" fill="none" stroke-linecap="round" stroke-linejoin="round"/><circle cx="1500" cy="100" r="16" fill="#1f5fbf"/></svg>"##,
    },
];

/// The examples a page can switch between: (id, label, creator).
pub fn list() -> Vec<(&'static str, &'static str, &'static str)> {
    SPECS.iter().map(|s| (s.id, s.label, s.creator)).collect()
}

pub fn default_id() -> &'static str {
    SPECS[0].id
}

fn palette(c: [&str; 4]) -> brand::Palette {
    brand::Palette { bg: c[0].into(), text: c[1].into(), accent: c[2].into(), card: c[3].into() }
}

/// The example `id` laid over this site's plans (lowest, middle, highest; extra plans keep
/// their own names).
pub fn build(s: &AppState, id: &str) -> Option<Example> {
    let sp = SPECS.iter().find(|x| x.id == id)?;
    let mut names = HashMap::new();
    let mut about = HashMap::new();
    let mut perks = HashMap::new();
    let plans: Vec<&Level> = s.levels.iter().filter(|l| !l.card.placeholder).collect();
    for (i, l) in plans.iter().enumerate().take(3) {
        let (n, a, p) = sp.plans[i];
        names.insert(l.card.covenant_id.clone(), n.to_string());
        about.insert(l.card.covenant_id.clone(), a.to_string());
        perks.insert(l.card.covenant_id.clone(), p.iter().map(|x| x.to_string()).collect());
    }
    let page = PageProfile { tagline: sp.tagline.into(), intro_html: sp.intro.into(), links: Default::default(), view: "cards".into(), show_count: false, price_display: String::new(), hide_club: true };
    let brand = brand::Brand {
        light: Some(palette(sp.light)),
        dark: Some(palette(sp.dark)),
        head_font: String::new(),
        body_font: String::new(),
        corners: sp.corners.into(),
        back_url: String::new(),
        logo: Some("logo.svg".into()),
        photo: Some("photo.svg".into()),
        banner: Some("banner.svg".into()),
        start: sp.start.into(),
        version: 1,
    };
    Some(Example { id: sp.id, creator: sp.creator, names, about, perks, word: sp.word, page, brand, pictures: [("logo", sp.logo), ("photo", sp.photo), ("banner", sp.banner)] })
}

/// The example's picture (an SVG), for `/brand/img/<kind>`.
pub fn picture(kind: &str) -> Option<&'static str> {
    current().and_then(|e| e.pictures.iter().find(|(k, _)| *k == kind).map(|(_, svg)| *svg))
}

/// Middleware: serves the request "as" an example when it asks for one (KasPass's own site
/// only; never the creator's own calls).
pub async fn layer(State(s): State<AppState>, req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let path = req.uri().path();
    let wanted = if path.starts_with("/v1/creator") || path.starts_with("/dashboard") || !home::is_home(req.headers()) {
        None
    } else if path.starts_with("/v1/") {
        req.headers().get("x-kp-example").and_then(|v| v.to_str().ok()).map(String::from)
    } else if path.starts_with("/brand/") {
        req.uri().query().into_iter().flat_map(|q| q.split('&')).find_map(|kv| kv.strip_prefix("ex=")).map(String::from)
    } else {
        None
    };
    match wanted.and_then(|id| build(&s, &id)) {
        Some(e) => EXAMPLE.scope(Arc::new(e), next.run(req)).await,
        None => next.run(req).await,
    }
}

/// `/demo/<id>`: the membership page as one of the example creators.
pub async fn page(State(s): State<AppState>, headers: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>) -> axum::response::Response {
    if !home::is_home(&headers) {
        return axum::response::Redirect::to("/").into_response();
    }
    let Some(e) = build(&s, &id) else { return axum::response::Redirect::to(&format!("/demo/{}", default_id())).into_response() };
    let html = EXAMPLE.scope(Arc::new(e), async { fill_site(&s) }).await;
    let tabs: String = list()
        .iter()
        .map(|(x, label, _)| format!(r#"<a href="/demo/{x}"{}>{label}</a>"#, if *x == id { r#" aria-current="page""# } else { "" }))
        .collect();
    let bar = format!(r#"<nav class="exbar" aria-label="Example creators"><span>Example creators, made up for this demo on Kaspa's test network (memberships cost free test KAS). Just looking? You can browse without a wallet.</span><span class="exlinks">{tabs}</span><a class="exabout" href="/">About KasPass</a></nav>"#);
    let html = html
        .replace(r#"href="/""#, &format!(r#"href="/demo/{id}""#))
        .replacen("<head>", &format!(r#"<head><meta name="kp-example" content="{id}">"#), 1)
        .replacen("/brand/fonts.css?v=", &format!("/brand/fonts.css?ex={id}&v="), 1)
        .replacen("/brand/theme.css?v=", &format!("/brand/theme.css?ex={id}&v="), 1)
        .replacen("<body>", &format!("<body>{bar}"), 1);
    ([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response()
}
