//! Holder clubs: access by what a member holds, not only by a paid pass.
//!
//! A club is described by one rules file (`club.json` next to the tier card):
//!
//! ```json
//! {
//!   "club": "Bitcoin the Turtle",
//!   "collection": { "network": "mainnet", "tick": "BITCOIN" },
//!   "ranks_file": "BITCOIN-ranks.json",
//!   "bands": [ { "name": "Gold", "ranks": [5, 21], "credits": 25 } ],
//!   "tiers": [ { "name": "Whale", "rule": { "any_of": [ { "owns_band": "Legacy" }, { "min_credits": 200 } ] } } ]
//! }
//! ```
//!
//! Each NFT's rank comes from the ranks map (token id -> rank). Its rank puts it in a band, the band
//! gives it credits, and a member's credits are the sum over every wallet they've linked. Each tier
//! becomes a Discord role and can have its own Telegram group.
//!
//! Holdings come from a KRC-721 indexer (KaspaCom's API shape). That's the one part a club trusts
//! someone else for; when collections move to KCC covenants this can read the chain directly.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_LINKED: usize = 10;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Collection {
    pub network: String,
    pub tick: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Band {
    pub name: String,
    pub ranks: [u64; 2],
    #[serde(default)]
    pub credits: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TierRule {
    pub name: String,
    pub rule: Value,
    /// Discord role name (defaults to the tier name).
    #[serde(default)]
    pub discord_role: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Rules {
    pub club: String,
    pub collection: Collection,
    /// KRC-721 indexer base URL; defaults per network.
    #[serde(default)]
    pub indexer: Option<String>,
    /// Count NFTs that are listed for sale on a marketplace (default: no, a listed NFT is for sale).
    #[serde(default)]
    pub count_listed: bool,
    #[serde(default)]
    pub ranks: Option<BTreeMap<String, u64>>,
    #[serde(default)]
    pub ranks_file: Option<String>,
    #[serde(default)]
    pub bands: Vec<Band>,
    pub tiers: Vec<TierRule>,
}

/// One parsed rule.
#[derive(Clone, Debug)]
pub enum Rule {
    MinTokens(usize),
    MinCredits(i64),
    OwnsBand(String),
    OwnsToken(HashSet<String>),
    HasPass,
    /// At least `min` NFTs of another collection, or `min` whole KRC-20 tokens.
    Holds(crate::holdings::Need),
    AnyOf(Vec<Rule>),
    AllOf(Vec<Rule>),
}

impl Rule {
    fn parse(v: &Value, bands: &[Band]) -> Result<Rule> {
        let o = v.as_object().ok_or_else(|| anyhow!("a rule must be an object like {{\"min_credits\": 200}}"))?;
        if o.len() != 1 {
            bail!("each rule needs exactly one key (use any_of / all_of to combine): {v}");
        }
        let (k, x) = o.iter().next().unwrap();
        Ok(match k.as_str() {
            "min_tokens" => Rule::MinTokens(x.as_u64().ok_or_else(|| anyhow!("min_tokens must be a whole number"))? as usize),
            "min_credits" => Rule::MinCredits(x.as_i64().ok_or_else(|| anyhow!("min_credits must be a whole number"))?),
            "owns_band" => {
                let b = x.as_str().ok_or_else(|| anyhow!("owns_band must be a band name"))?;
                if !bands.iter().any(|band| band.name == b) {
                    bail!("owns_band \"{b}\" isn't one of the bands");
                }
                Rule::OwnsBand(b.to_string())
            }
            "owns_token" => Rule::OwnsToken(
                x.as_array()
                    .ok_or_else(|| anyhow!("owns_token must be a list of token ids"))?
                    .iter()
                    .map(|t| t.as_u64().map(|n| n.to_string()).or_else(|| t.as_str().map(String::from)).ok_or_else(|| anyhow!("bad token id {t}")))
                    .collect::<Result<_>>()?,
            ),
            "has_pass" => Rule::HasPass,
            "holds_nft" => Rule::Holds(crate::holdings::need_from("nft", x)?),
            "holds_token" => Rule::Holds(crate::holdings::need_from("token", x)?),
            "holds_kcc20" => Rule::Holds(crate::holdings::need_from("kcc20", x)?),
            "any_of" | "all_of" => {
                let list = x.as_array().ok_or_else(|| anyhow!("{k} must be a list of rules"))?.iter().map(|r| Rule::parse(r, bands)).collect::<Result<Vec<_>>>()?;
                if list.is_empty() {
                    bail!("{k} needs at least one rule");
                }
                if k == "any_of" { Rule::AnyOf(list) } else { Rule::AllOf(list) }
            }
            other => bail!("unknown rule \"{other}\" (use min_tokens, min_credits, owns_band, owns_token, has_pass, holds_nft, holds_token, holds_kcc20, any_of, all_of)"),
        })
    }

    fn holds(&self, s: &Standing) -> bool {
        match self {
            Rule::MinTokens(n) => s.tokens >= *n,
            Rule::MinCredits(n) => s.credits >= *n,
            Rule::OwnsBand(b) => s.bands.iter().any(|(name, c)| name == b && *c > 0),
            Rule::OwnsToken(ids) => s.token_ids.iter().any(|t| ids.contains(t)),
            Rule::HasPass => s.has_pass,
            Rule::Holds(n) => s.extra.get(&asset_key(n)).is_some_and(|have| *have + 1e-9 >= n.min),
            Rule::AnyOf(r) => r.iter().any(|x| x.holds(s)),
            Rule::AllOf(r) => r.iter().all(|x| x.holds(s)),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Rule::MinTokens(n) => format!("holds at least {n} NFT{}", if *n == 1 { "" } else { "s" }),
            Rule::MinCredits(n) => format!("{n}+ credits"),
            Rule::OwnsBand(b) => format!("owns a {b}"),
            Rule::OwnsToken(ids) => format!("owns one of {} specific NFTs", ids.len()),
            Rule::HasPass => "has an active KasPass pass".into(),
            Rule::Holds(n) => format!("holds {}", n.describe()),
            Rule::AnyOf(r) => r.iter().map(Rule::describe).collect::<Vec<_>>().join(" OR "),
            Rule::AllOf(r) => r.iter().map(Rule::describe).collect::<Vec<_>>().join(" AND "),
        }
    }
}

fn asset_key(n: &crate::holdings::Need) -> String {
    n.key()
}

impl Rule {
    /// Every other asset this rule looks at (holds_nft, holds_token).
    fn needs(&self, out: &mut Vec<crate::holdings::Need>) {
        match self {
            Rule::Holds(n) => {
                if !out.iter().any(|x| asset_key(x) == asset_key(n)) {
                    out.push(n.clone());
                }
            }
            Rule::AnyOf(r) | Rule::AllOf(r) => r.iter().for_each(|x| x.needs(out)),
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tier {
    pub name: String,
    pub rule: Rule,
    pub discord_role: String,
}

/// What one member holds across their linked wallets, scored against the rules.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Standing {
    pub wallets: Vec<WalletHolding>,
    /// NFTs that count (listed ones are left out unless the club counts them).
    pub tokens: usize,
    pub listed: usize,
    pub credits: i64,
    /// (band name, how many the member holds), in the rules' order.
    pub bands: Vec<(String, usize)>,
    pub tiers: Vec<String>,
    pub has_pass: bool,
    /// Other assets the tiers look at, keyed "nft:TICK", "token:TICK" or "kcc20:<id>": NFTs (not listed for
    /// sale) or whole tokens held across the member's wallets.
    pub extra: BTreeMap<String, f64>,
    #[serde(skip)]
    token_ids: HashSet<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct WalletHolding {
    pub address: String,
    pub tokens: usize,
    pub listed: usize,
}

pub struct Club {
    pub rules: Rules,
    pub tiers: Vec<Tier>,
    ranks: HashMap<String, u64>,
    indexer: String,
}

pub fn default_indexer(network: &str) -> Option<&'static str> {
    match network {
        "mainnet" => Some("https://krc721-indexer.kaspa.com"),
        "testnet-10" => Some("https://dev-krc721-indexer.kaspa.com"),
        _ => None,
    }
}

impl Club {
    /// Checks a rules file (and its ranks) and builds the club. `dir` is where ranks_file lives.
    pub fn from_rules(rules: Rules, ranks_override: Option<BTreeMap<String, u64>>, dir: &Path) -> Result<Club> {
        if rules.club.trim().is_empty() {
            bail!("the club needs a name (\"club\")");
        }
        if rules.collection.tick.trim().is_empty() {
            bail!("collection.tick is missing");
        }
        if rules.tiers.is_empty() {
            bail!("add at least one tier");
        }
        let mut names = HashSet::new();
        for b in &rules.bands {
            if b.ranks[0] == 0 || b.ranks[0] > b.ranks[1] {
                bail!("band \"{}\" has an empty rank range {:?}", b.name, b.ranks);
            }
            if !names.insert(b.name.clone()) {
                bail!("two bands are called \"{}\"", b.name);
            }
        }
        for (i, a) in rules.bands.iter().enumerate() {
            for b in &rules.bands[i + 1..] {
                if a.ranks[0] <= b.ranks[1] && b.ranks[0] <= a.ranks[1] {
                    bail!("bands \"{}\" and \"{}\" overlap", a.name, b.name);
                }
            }
        }
        let mut tier_names = HashSet::new();
        let tiers = rules
            .tiers
            .iter()
            .map(|t| {
                if t.name.trim().is_empty() || !tier_names.insert(t.name.clone()) {
                    bail!("tier names must be present and different (\"{}\")", t.name);
                }
                if t.name.eq_ignore_ascii_case("pass") {
                    bail!("\"pass\" is reserved for paid KasPass passes; pick another tier name");
                }
                let rule = Rule::parse(&t.rule, &rules.bands).map_err(|e| anyhow!("tier \"{}\": {e}", t.name))?;
                Ok(Tier { name: t.name.clone(), rule, discord_role: t.discord_role.clone().unwrap_or_else(|| t.name.clone()) })
            })
            .collect::<Result<Vec<_>>>()?;
        let ranks: HashMap<String, u64> = match (ranks_override, &rules.ranks, &rules.ranks_file) {
            (Some(r), _, _) => r.into_iter().collect(),
            (None, Some(r), _) => r.clone().into_iter().collect(),
            (None, None, Some(f)) => {
                // A plain file name next to club.json, nothing else on the server.
                if f.contains('/') || f.contains('\\') || f.contains("..") || !f.ends_with("-ranks.json") {
                    bail!("ranks_file must be a file name ending in -ranks.json, next to club.json");
                }
                let text = std::fs::read_to_string(dir.join(f)).map_err(|e| anyhow!("can't read ranks file {f}: {e}"))?;
                serde_json::from_str::<BTreeMap<String, u64>>(&text).map_err(|e| anyhow!("ranks file {f} must map token ids to ranks: {e}"))?.into_iter().collect()
            }
            _ => HashMap::new(),
        };
        if !rules.bands.is_empty() && ranks.is_empty() {
            bail!("the bands use ranks, so the club needs a ranks list (token id -> rank)");
        }
        let indexer = match &rules.indexer {
            Some(u) => {
                // An NFT indexer is a public HTTPS service with a name, never this server or its network.
                let host = u.strip_prefix("https://").map(|r| r.split(['/', '?', '#']).next().unwrap_or("")).unwrap_or("");
                let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
                let name = host.split(':').next().unwrap_or("").trim_matches(['[', ']']).to_ascii_lowercase();
                let bad = name.is_empty() || !name.contains('.') || name.parse::<std::net::IpAddr>().is_ok() || name == "localhost" || name.ends_with(".localhost") || name.ends_with(".internal") || name.ends_with(".local") || name.chars().all(|c| c.is_ascii_digit() || c == '.');
                if bad {
                    bail!("\"indexer\" must be a public https:// address with a domain name");
                }
                u.trim_end_matches('/').to_string()
            }
            None => default_indexer(&rules.collection.network)
                .ok_or_else(|| anyhow!("no default NFT indexer for {}; add \"indexer\": \"https://...\" to the rules", rules.collection.network))?
                .to_string(),
        };
        let indexer = crate::test_switch("KASPASS_KRC721_API").map(|u| u.trim_end_matches('/').to_string()).unwrap_or(indexer);
        Ok(Club { rules, tiers, ranks, indexer })
    }

    pub fn load(dir: &Path) -> Result<Option<Club>> {
        let path = dir.join("club.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
        let rules: Rules = serde_json::from_str(&text).map_err(|e| anyhow!("club.json: {e}"))?;
        Club::from_rules(rules, None, dir).map(Some)
    }

    pub fn network(&self) -> &str {
        &self.rules.collection.network
    }

    pub fn band_of(&self, id: &str) -> Option<&Band> {
        let r = *self.ranks.get(id)?;
        self.rules.bands.iter().find(|b| b.ranks[0] <= r && r <= b.ranks[1])
    }

    /// How many ranked NFTs fall in each band (for the dashboard summary).
    pub fn band_sizes(&self) -> Vec<(String, usize)> {
        self.rules
            .bands
            .iter()
            .map(|b| (b.name.clone(), self.ranks.values().filter(|r| b.ranks[0] <= **r && **r <= b.ranks[1]).count()))
            .collect()
    }

    pub fn ranked(&self) -> usize {
        self.ranks.len()
    }

    /// (token id, listed) for every NFT of the collection this wallet holds. None if the indexer failed.
    pub async fn holdings(&self, address: &str) -> Option<Vec<(String, bool)>> {
        let c = &self.rules.collection;
        crate::holdings::nfts_at(&self.indexer, &c.network, &c.tick, address, false).await
    }

    /// Scores a member's wallets. None if any wallet couldn't be read (so nobody loses access to an outage).
    pub async fn standing(&self, wallets: &[String], has_pass: bool) -> Option<Standing> {
        let mut s = Standing { has_pass, ..Default::default() };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for w in wallets {
            if !w.starts_with(if self.network().starts_with("testnet") { "kaspatest:" } else { "kaspa:" }) {
                // A wallet on another network can't hold this collection.
                s.wallets.push(WalletHolding { address: w.clone(), tokens: 0, listed: 0 });
                continue;
            }
            let h = self.holdings(w).await?;
            let listed = h.iter().filter(|(_, l)| *l).count();
            let mut counted = 0;
            for (id, l) in h {
                if l && !self.rules.count_listed {
                    s.listed += 1;
                    continue;
                }
                if !s.token_ids.insert(id.clone()) {
                    continue;
                }
                counted += 1;
                if let Some(b) = self.band_of(&id) {
                    s.credits += b.credits;
                    *counts.entry(b.name.clone()).or_default() += 1;
                }
            }
            s.tokens += counted;
            s.wallets.push(WalletHolding { address: w.clone(), tokens: counted, listed });
        }
        s.bands = self.rules.bands.iter().map(|b| (b.name.clone(), counts.get(&b.name).copied().unwrap_or(0))).collect();
        // Other assets the tiers look at.
        let mut needs = Vec::new();
        self.tiers.iter().for_each(|t| t.rule.needs(&mut needs));
        if !needs.is_empty() {
            let set = crate::holdings::Needs { mode: "any".into(), list: needs };
            let got = crate::holdings::check(self.network(), &set, wallets, &HashSet::new(), false).await?;
            for r in got.rows {
                s.extra.insert(r.key, r.have);
            }
        }
        s.tiers = self.tiers.iter().filter(|t| t.rule.holds(&s)).map(|t| t.name.clone()).collect();
        Some(s)
    }
}

/// Extra wallets members have proven they own, keyed by the wallet they sign in with.
pub struct Wallets {
    path: PathBuf,
    map: Mutex<BTreeMap<String, Vec<String>>>,
}

impl Wallets {
    pub fn open(dir: &Path) -> Wallets {
        let path = dir.join("wallets.json");
        let map = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Wallets { path, map: Mutex::new(map) }
    }

    fn save(&self, map: &BTreeMap<String, Vec<String>>) {
        if let Ok(text) = serde_json::to_string_pretty(map) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// The member's own wallet first, then the linked ones.
    pub fn of(&self, primary: &str) -> Vec<String> {
        let mut v = vec![primary.to_string()];
        v.extend(self.map.lock().unwrap_or_else(|e| e.into_inner()).get(primary).cloned().unwrap_or_default());
        v
    }

    /// Links `extra` to `primary`. A wallet belongs to one member at a time, so it moves if it was
    /// linked elsewhere (its owner just proved control of it).
    /// Every wallet of the member that `address` belongs to, whether it's their sign-in wallet or a linked one.
    pub fn group_of(&self, address: &str) -> Vec<String> {
        let primary = {
            let m = self.map.lock().unwrap_or_else(|e| e.into_inner());
            if m.contains_key(address) { Some(address.to_string()) } else { m.iter().find(|(_, v)| v.iter().any(|a| a == address)).map(|(k, _)| k.clone()) }
        };
        match primary {
            Some(p) => self.of(&p),
            None => vec![address.to_string()],
        }
    }

    pub fn link(&self, primary: &str, extra: &str) -> Result<()> {
        if primary == extra {
            bail!("that's the wallet you signed in with");
        }
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if m.contains_key(extra) {
            bail!("that wallet has its own linked wallets; sign in with it, or unlink them first");
        }
        for v in m.values_mut() {
            v.retain(|a| a != extra);
        }
        let list = m.entry(primary.to_string()).or_default();
        if list.len() >= MAX_LINKED {
            bail!("you can link up to {MAX_LINKED} wallets");
        }
        list.push(extra.to_string());
        m.retain(|_, v| !v.is_empty());
        let snapshot = m.clone();
        drop(m);
        self.save(&snapshot);
        Ok(())
    }

    pub fn unlink(&self, primary: &str, extra: &str) {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = m.get_mut(primary) {
            v.retain(|a| a != extra);
        }
        m.retain(|_, v| !v.is_empty());
        let snapshot = m.clone();
        drop(m);
        self.save(&snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_can_need_other_assets() {
        let r = Rule::parse(&serde_json::json!({ "any_of": [{ "min_tokens": 3 }, { "all_of": [{ "holds_token": { "tick": "nacho", "min": 100000 } }, { "holds_nft": { "tick": "KASPUNKS", "min": 1 } }] }] }), &[]).unwrap();
        let mut needs = Vec::new();
        r.needs(&mut needs);
        assert_eq!(needs.len(), 2);
        assert_eq!(needs[0].tick, "NACHO");
        let mut s = Standing::default();
        assert!(!r.holds(&s));
        s.extra.insert("token:NACHO".into(), 150_000.0);
        assert!(!r.holds(&s));
        s.extra.insert("nft:KASPUNKS".into(), 1.0);
        assert!(r.holds(&s));
        assert!(r.describe().contains("holds at least 100,000 NACHO tokens AND holds at least 1 KASPUNKS NFT"));
        assert!(Rule::parse(&serde_json::json!({ "holds_nft": { "tick": "X", "min": 0.5 } }), &[]).is_err());
    }
}
