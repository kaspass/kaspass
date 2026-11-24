//! The KasPass guide: one long, plain page that explains everything, for people and for AI
//! assistants. Written once in Markdown (`web/guide.md`), served three ways:
//! - `/guide`: the page, rendered to HTML on the server, so it reads without scripts;
//! - `/guide.md`: the same text as Markdown, for AI assistants and anyone who wants plain text;
//! - `/llms.txt`: a short pointer to both, where AI tools look first.

use axum::http::{HeaderMap, header};
use axum::response::{Html, IntoResponse};
use std::sync::OnceLock;

pub(crate) const GUIDE_MD: &str = include_str!("../../web/guide.md");
const GUIDE_SHELL: &str = include_str!("../../web/guide.html");

/// The guide as HTML (rendered once). Headings keep the ids written in the Markdown (`{#id}`),
/// so links like `/guide#refunds` stay stable; tables scroll sideways on a phone.
pub(crate) fn guide_html() -> &'static str {
    static HTML: OnceLock<String> = OnceLock::new();
    HTML.get_or_init(|| {
        use pulldown_cmark::{Options, Parser, html};
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
        let mut body = String::new();
        html::push_html(&mut body, Parser::new_ext(GUIDE_MD, opts));
        let body = body.replace("<table>", r#"<div class="tablewrap"><table>"#).replace("</table>", "</table></div>");
        GUIDE_SHELL.replacen("<!--GUIDE-->", &body, 1)
    })
}

pub(crate) async fn guide_page(axum::extract::State(s): axum::extract::State<super::AppState>, headers: HeaderMap) -> impl IntoResponse {
    let html = if super::home::is_home(&headers) {
        // On KasPass's own site, KasPass's own look (as the help page does).
        guide_html().replacen("<link rel=\"stylesheet\" href=\"/brand/fonts.css\">\n<link rel=\"stylesheet\" href=\"/brand/theme.css\">", super::HELP_KASPASS_LOOK, 1)
    } else {
        super::creator_look(&s, guide_html())
    };
    ([(header::CACHE_CONTROL, "public, max-age=300"), (header::VARY, "Host")], Html(html))
}

pub(crate) async fn guide_markdown() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=300")], GUIDE_MD)
}

pub(crate) async fn llms_txt() -> impl IntoResponse {
    const TXT: &str = "# KasPass\n\n> Memberships on the Kaspa blockchain. This site runs KasPass: members hold their membership in their own wallet, payments go straight to the creator, and the contracts enforce the rules.\n\n## Docs\n\n- [The KasPass guide](/guide.md): everything in one plain-text page: the creator dashboard tab by tab, joining and managing a membership, how the contracts work, what is fixed and what can change, money and refunds, promotions, Telegram, Discord, linking from other sites, and common messages. Each section has a stable anchor, for example /guide#refunds.\n- [The same guide as a web page](/guide)\n- [Quick help for members](/help)\n";
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=300")], TXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading_ids() -> Vec<String> {
        GUIDE_MD.lines().filter(|l| l.starts_with('#')).filter_map(|l| l.split("{#").nth(1).map(|r| r.trim_end_matches('}').trim().to_string())).collect()
    }

    #[test]
    fn guide_headings_have_unique_ids_and_links_resolve() {
        let ids = heading_ids();
        let mut seen = std::collections::HashSet::new();
        for id in &ids { assert!(seen.insert(id.clone()), "duplicate heading id {id}"); }
        for h in GUIDE_MD.lines().filter(|l| l.starts_with("## ") || l.starts_with("### ")) {
            assert!(h.contains("{#"), "heading without a stable id: {h}");
        }
        // Every in-page link points at a heading that exists.
        for part in GUIDE_MD.split("](#").skip(1) {
            let id = part.split(')').next().unwrap();
            assert!(seen.contains(id), "link to missing section #{id}");
        }
        let html = guide_html();
        assert!(html.contains(r#"<h3 id="refunds">"#) && html.contains(r#"<h2 id="money">"#) && html.contains("<div class=\"tablewrap\"><table>"));
        assert!(!html.contains("<!--GUIDE-->") && !html.contains("{#"));
    }

    /// The rules-file section's example is a real, working rules file (the same as the one in
    /// `examples/`), and it names every rule the server accepts.
    #[test]
    fn guide_holder_club_example_is_valid() {
        let sec = GUIDE_MD.split("{#holder-club-rules}").nth(1).expect("the holder club rules section");
        let json = sec.split("```json").nth(1).and_then(|x| x.split("```").next()).expect("an example");
        let rules: crate::clubs::Rules = serde_json::from_str(json).expect("the guide's example parses");
        crate::clubs::Club::from_rules(rules.clone(), Some([("1".to_string(), 3u64), ("2".to_string(), 900)].into_iter().collect()), std::path::Path::new(".")).expect("the guide's example is accepted");
        let file: serde_json::Value = serde_json::from_str(include_str!("../../examples/club-rules-example.json")).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(json).unwrap(), file, "the guide's example and examples/club-rules-example.json differ");
        for r in ["min_tokens", "min_credits", "owns_band", "owns_token", "has_pass", "holds_nft", "holds_token", "holds_kcc20", "any_of", "all_of"] {
            assert!(sec.contains(&format!("`{r}`")), "the guide doesn't explain the {r} rule");
        }
        // The dashboard's "Download an example" file is the same club.
        let dash = include_str!("../../web/dashboard.js");
        assert!(dash.contains(r#"club: "Bitcoin the Turtle""#));
        assert!(include_str!("../../web/dashboard.html").contains(r#"href="/guide#holder-club-rules""#));
    }

    /// Every panel in the dashboard is explained in the guide, so a new panel can't ship
    /// without its help. (Fable reviews the guide's accuracy now and then; this only checks
    /// that nothing is missing.)
    #[test]
    fn guide_mentions_every_dashboard_panel() {
        let dash = include_str!("../../web/dashboard.html");
        let guide = GUIDE_MD.to_lowercase();
        let mut missing = vec![];
        for part in dash.split("<h2").skip(1) {
            let inner = part.split_once('>').map(|x| x.1).unwrap_or("");
            let title = inner.split("</h2>").next().unwrap_or("");
            let mut text = String::new();
            let mut tag = false;
            for c in title.chars() { match c { '<' => tag = true, '>' => tag = false, _ if !tag => text.push(c), _ => {} } }
            let text = text.trim().replace("&amp;", "&");
            if text.is_empty() { continue; }
            let key = text.split(" (").next().unwrap().to_lowercase();
            if !guide.contains(&key) { missing.push(text); }
        }
        assert!(missing.is_empty(), "the guide doesn't mention these dashboard panels: {missing:?}");
    }
}
