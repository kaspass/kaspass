//! A plan's badges on its card (update 98). Until the creator edits them, a plan shows the automatic
//! ones: "Save N% vs monthly" and "Early price until …" (and, from update 97, one line of their own
//! text on top). Once edited, it shows exactly the creator's list: up to three badges, each their
//! own text, the saving against a plan they pick, or the early price, in one of six colours that
//! read in both the day and night looks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The most badges one plan shows.
pub(crate) const MAX_BADGES: usize = 3;
/// The longest badge text, in characters.
pub(crate) const BADGE_MAX: usize = 24;
/// The colours a badge can have (`web/index.html` draws each, in both looks).
pub(crate) const TONES: [&str; 6] = ["accent", "gold", "green", "red", "purple", "grey"];
/// Smallest saving the automatic "Save N%" badge shows (as before update 98).
const AUTO_MIN_PCT: i64 = 5;

/// One badge as the creator sets it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub(crate) struct Spec {
    /// "text", "save" or "early".
    pub kind: String,
    /// The words, for "text".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The plan to compare with, for "save" (empty: the shortest plan in the same currency).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vs: String,
    /// One of `TONES`.
    #[serde(default)]
    pub tone: String,
}

/// What `level-badges.json` holds for a plan.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub(crate) enum Saved {
    /// Update 97: one line of the creator's text, on top of the automatic badges.
    Text(String),
    /// Update 98: the creator's own list, in place of the automatic badges.
    Custom { list: Vec<Spec> },
}

pub(crate) fn load(home: &std::path::Path) -> HashMap<String, Saved> {
    if super::examples::current().is_some() {
        return HashMap::new();
    }
    std::fs::read_to_string(home.join("level-badges.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub(crate) fn store(home: &std::path::Path, all: &HashMap<String, Saved>) -> std::io::Result<()> {
    std::fs::write(home.join("level-badges.json"), serde_json::to_string_pretty(all).unwrap_or_default())
}

/// The badge text as saved: no control characters, spaces squeezed, at most BADGE_MAX characters.
pub(crate) fn clean_text(t: &str) -> String {
    let one: String = t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(BADGE_MAX).collect::<String>().trim().to_string()
}

/// The creator's list as saved: known kinds only, text badges with words, a "vs" plan that is one of
/// this site's plans (else the shortest), a known colour (else the accent), at most MAX_BADGES.
pub(crate) fn clean(list: Vec<Spec>, plans: &[String]) -> Vec<Spec> {
    list.into_iter()
        .filter_map(|b| {
            let tone = if TONES.contains(&b.tone.as_str()) { b.tone } else { "accent".into() };
            match b.kind.as_str() {
                "text" => {
                    let text = clean_text(&b.text);
                    (!text.is_empty()).then(|| Spec { kind: "text".into(), text, vs: String::new(), tone })
                }
                "save" => Some(Spec { kind: "save".into(), text: String::new(), vs: if plans.contains(&b.vs) { b.vs } else { String::new() }, tone }),
                "early" => Some(Spec { kind: "early".into(), text: String::new(), vs: String::new(), tone }),
                _ => None,
            }
        })
        .take(MAX_BADGES)
        .collect()
}

/// One plan's prices, for working out its badges.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub id: String,
    pub period_ms: u64,
    pub currency: Option<String>,
    pub amount: Option<f64>,
    /// The early price's end, while one runs.
    pub early_until_ms: Option<u64>,
}

/// "monthly", "yearly", "weekly" or "daily" for a period (the membership page's rule).
fn unit_adj(period_ms: u64) -> Option<&'static str> {
    let d = period_ms as f64 / 86_400_000.0;
    if (360.0..=366.0).contains(&d) {
        Some("yearly")
    } else if (28.0..=31.0).contains(&d) {
        Some("monthly")
    } else if d == 7.0 {
        Some("weekly")
    } else if d == 1.0 {
        Some("daily")
    } else {
        None
    }
}

/// The shortest plan in `p`'s currency, the automatic badge's base.
fn shortest<'a>(p: &Plan, all: &'a [Plan]) -> Option<&'a Plan> {
    all.iter().filter(|x| x.currency.is_some() && x.currency == p.currency && x.period_ms > 0 && x.amount.is_some()).min_by_key(|x| x.period_ms)
}

/// "Save 17% vs monthly": `p` paid once against `base` paid for the same time, when `base` is a
/// shorter plan in the same currency and the saving is at least `min_pct`.
fn save_words(p: &Plan, base: &Plan, min_pct: i64) -> Option<String> {
    let (Some(pa), Some(ba)) = (p.amount, base.amount) else { return None };
    if p.currency.is_none() || p.currency != base.currency || base.period_ms == 0 || base.period_ms >= p.period_ms || ba <= 0.0 {
        return None;
    }
    let n = (p.period_ms as f64 / base.period_ms as f64).round();
    if n < 2.0 {
        return None;
    }
    let pct = ((1.0 - pa / (n * ba)) * 100.0).round() as i64;
    let unit = unit_adj(base.period_ms)?;
    (pct >= min_pct).then(|| format!("Save {pct}% vs {unit}"))
}

/// A plan's badges, top first, for `/v1/levels`: `{kind, text, tone}`, plus `until_ms` on the early
/// price so a page can write the date its own way. `tone` is empty on the automatic ones (each page
/// draws those as it always has). Also says whether these are the creator's own list.
pub(crate) fn resolve(saved: Option<&Saved>, p: &Plan, all: &[Plan], day: impl Fn(u64) -> String) -> (Vec<serde_json::Value>, bool) {
    let early = |tone: &str| p.early_until_ms.map(|u| serde_json::json!({ "kind": "early", "text": format!("Early price until {}", day(u.saturating_sub(1))), "tone": tone, "until_ms": u }));
    match saved {
        Some(Saved::Custom { list }) => {
            let out = list
                .iter()
                .filter_map(|b| match b.kind.as_str() {
                    "text" => Some(serde_json::json!({ "kind": "text", "text": b.text, "tone": b.tone })),
                    "save" => {
                        let base = all.iter().find(|x| x.id == b.vs && !b.vs.is_empty()).or_else(|| shortest(p, all))?;
                        save_words(p, base, 1).map(|t| serde_json::json!({ "kind": "save", "text": t, "tone": b.tone }))
                    }
                    "early" => early(&b.tone),
                    _ => None,
                })
                .collect();
            (out, true)
        }
        other => {
            let mut out = Vec::new();
            if let Some(Saved::Text(t)) = other {
                if !t.is_empty() {
                    out.push(serde_json::json!({ "kind": "text", "text": t, "tone": "accent" }));
                }
            }
            if let Some(t) = shortest(p, all).and_then(|b| save_words(p, b, AUTO_MIN_PCT)) {
                out.push(serde_json::json!({ "kind": "save", "text": t, "tone": "" }));
            }
            out.extend(early(""));
            (out, false)
        }
    }
}

/// The single line of text update 97 introduced (`badge` in `/v1/levels`): the first text badge.
pub(crate) fn first_text(saved: Option<&Saved>) -> String {
    match saved {
        Some(Saved::Text(t)) => t.clone(),
        Some(Saved::Custom { list }) => list.iter().find(|b| b.kind == "text").map(|b| b.text.clone()).unwrap_or_default(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(id: &str, days: u64, amount: f64, early: Option<u64>) -> Plan {
        Plan { id: id.into(), period_ms: days * 86_400_000, currency: Some("CAD".into()), amount: Some(amount), early_until_ms: early }
    }
    fn day(ms: u64) -> String {
        format!("day {ms}")
    }

    #[test]
    fn automatic_until_edited() {
        let all = [plan("m", 30, 10.0, None), plan("y", 365, 100.0, None), plan("t", 365, 200.0, Some(5))];
        let (b, custom) = resolve(None, &all[1], &all, day);
        assert!(!custom);
        assert_eq!(b, vec![serde_json::json!({ "kind": "save", "text": "Save 17% vs monthly", "tone": "" })]);
        let (b, _) = resolve(None, &all[2], &all, day);
        assert_eq!(b, vec![serde_json::json!({ "kind": "early", "text": "Early price until day 4", "tone": "", "until_ms": 5 })]);
        // Update 97's single line stays on top of the automatic ones.
        let (b, custom) = resolve(Some(&Saved::Text("Most popular".into())), &all[1], &all, day);
        assert!(!custom);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0]["text"], "Most popular");
    }

    #[test]
    fn the_creators_list_replaces_them() {
        let all = [plan("m", 30, 10.0, None), plan("y", 365, 100.0, None), plan("t", 365, 200.0, Some(5))];
        let list = vec![
            Spec { kind: "text".into(), text: "Best value".into(), vs: String::new(), tone: "gold".into() },
            Spec { kind: "save".into(), text: String::new(), vs: "m".into(), tone: "green".into() },
        ];
        let (b, custom) = resolve(Some(&Saved::Custom { list: list.clone() }), &all[1], &all, day);
        assert!(custom);
        assert_eq!(b, vec![serde_json::json!({ "kind": "text", "text": "Best value", "tone": "gold" }), serde_json::json!({ "kind": "save", "text": "Save 17% vs monthly", "tone": "green" })]);
        // An empty list hides every badge, the automatic ones included.
        let (b, custom) = resolve(Some(&Saved::Custom { list: vec![] }), &all[2], &all, day);
        assert!(custom && b.is_empty());
        // A saving that isn't one, or no early price running, shows nothing.
        let early = vec![Spec { kind: "early".into(), tone: "red".into(), ..Default::default() }, Spec { kind: "save".into(), vs: "t".into(), tone: "green".into(), ..Default::default() }];
        let (b, _) = resolve(Some(&Saved::Custom { list: early }), &all[1], &all, day);
        assert!(b.is_empty());
    }

    #[test]
    fn saved_lists_are_cleaned() {
        let plans = vec!["m".to_string(), "y".to_string()];
        let raw = vec![
            Spec { kind: "text".into(), text: "  Most   popular\n".into(), tone: "neon".into(), ..Default::default() },
            Spec { kind: "text".into(), text: "   ".into(), tone: "gold".into(), ..Default::default() },
            Spec { kind: "save".into(), vs: "nope".into(), tone: "green".into(), text: "ignored".into() },
            Spec { kind: "script".into(), text: "x".into(), ..Default::default() },
            Spec { kind: "early".into(), tone: "red".into(), ..Default::default() },
            Spec { kind: "save".into(), vs: "m".into(), tone: "grey".into(), ..Default::default() },
        ];
        let c = clean(raw, &plans);
        assert_eq!(c.len(), MAX_BADGES);
        assert_eq!(c[0], Spec { kind: "text".into(), text: "Most popular".into(), vs: String::new(), tone: "accent".into() });
        assert_eq!(c[1], Spec { kind: "save".into(), text: String::new(), vs: String::new(), tone: "green".into() });
        assert_eq!(c[2].kind, "early");
        assert_eq!(clean_text(&"x".repeat(40)).chars().count(), BADGE_MAX);
    }

    #[test]
    fn files_from_update_97_still_read() {
        let old: HashMap<String, Saved> = serde_json::from_str(r#"{"abc":"Most popular"}"#).unwrap();
        assert_eq!(old["abc"], Saved::Text("Most popular".into()));
        let new: HashMap<String, Saved> = serde_json::from_str(r#"{"abc":{"list":[{"kind":"early","tone":"red"}]}}"#).unwrap();
        assert!(matches!(&new["abc"], Saved::Custom { list } if list.len() == 1));
        assert_eq!(first_text(Some(&new["abc"])), "");
    }
}
