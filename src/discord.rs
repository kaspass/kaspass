//! Discord: gives pass holders a "KasPass Member" role in the creator's server and removes it when
//! access ends. Members-only channels are set to be visible to that role only.
//!
//! How it works:
//! - A signed-in member clicks "Connect Discord" on the website. The server sends them to Discord's
//!   own authorization screen (scopes `identify` and `guilds.join`) with a one-time state code.
//! - Discord sends them back to /v1/discord/callback; the server learns their Discord user id, links it
//!   to the wallet and, if the wallet holds an active pass, adds them to the server with the role.
//! - Every minute the bot re-checks linked members: the role is removed when a pass ends (with a DM),
//!   and given back when they renew.
//!
//! Setup lives in `discord.json` next to the tier card: {"bot_token": "...", "client_secret": "..."}.
//! The server, the role and the application id are found (or created) automatically.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const STATE_TTL: Duration = Duration::from_secs(600);
/// A role is only taken away after the wallet has failed every check for this long, so one
/// bad answer from the NFT indexer (or a slow node) never removes it.
const REMOVE_AFTER_MS: i64 = 30 * 60 * 1000;
/// At most one "no longer qualifies" message per member in this long.
const LOST_DM_EVERY_MS: i64 = 24 * 3600 * 1000;
/// The role for paid pass holders.
pub const PASS_ROLE: &str = "Pass Holder";
/// Manage Roles + Create Instant Invite (needed to add members to the server).
const BOT_PERMISSIONS: u64 = 268435456 | 1;

#[derive(Deserialize)]
struct Secrets {
    bot_token: String,
    client_secret: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Link {
    pub user_id: String,
    pub name: String,
    pub address: String,
    pub linked_ms: i64,
    /// Access keys whose role they had at the last check.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Older single-role field, kept so old state files still load.
    #[serde(default, skip_serializing)]
    pub has_role: bool,
    /// Access key -> when the wallet first stopped qualifying for it (ms), while it still has the role.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub missing_since: BTreeMap<String, i64>,
    /// When it was last told that a role was removed (ms).
    #[serde(default)]
    pub lost_dm_ms: i64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct DiscordState {
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    /// Older single-role field, migrated into `roles` (as "pass").
    #[serde(default, skip_serializing)]
    pub role_id: Option<String>,
    /// Access key ("pass", "tier:Whale") -> Discord role id.
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    pub site: Option<String>,
    pub links: Vec<Link>,
}

pub struct Discord {
    secrets: Secrets,
    api: String,
    http: reqwest::Client,
    pub app_id: String,
    pub bot_name: String,
    path: PathBuf,
    pub state: Mutex<DiscordState>,
    pending: Mutex<HashMap<String, (String, String, Instant)>>,
    /// Access key -> role name for every role the server manages (kept up to date by the server).
    wanted: Mutex<Vec<(String, String)>>,
}

pub type AccessCheck = crate::telegram::AccessCheck;

impl Discord {
    /// Starts Discord support if `discord.json` is in `dir`; None (off) without it.
    pub async fn open(dir: &std::path::Path) -> Result<Option<Discord>> {
        let Ok(text) = std::fs::read_to_string(dir.join("discord.json")) else { return Ok(None) };
        let clean: String = text.chars().filter(|c| !c.is_control() || *c == '\n').collect();
        let mut secrets: Secrets = serde_json::from_str(&clean).map_err(|e| anyhow!("discord.json: {e}"))?;
        secrets.bot_token.retain(|c| c.is_ascii_graphic());
        secrets.client_secret.retain(|c| c.is_ascii_graphic());
        let api = crate::test_switch("KASPASS_DISCORD_API").unwrap_or_else(|| "https://discord.com/api/v10".into());
        let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?;
        let path = dir.join("discord-state.json");
        let mut state: DiscordState = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if let Some(r) = state.role_id.take() {
            state.roles.entry("pass".into()).or_insert(r);
            for l in state.links.iter_mut().filter(|l| l.has_role) {
                l.keys = vec!["pass".into()];
            }
        }
        let mut d = Discord { secrets, api, http, app_id: String::new(), bot_name: String::new(), path, state: Mutex::new(state), pending: Mutex::new(HashMap::new()), wanted: Mutex::new(vec![("pass".into(), PASS_ROLE.into())]) };
        let app = d.bot("GET", "/oauth2/applications/@me", None).await.map_err(|e| anyhow!("Discord didn't accept the bot token ({e})"))?;
        d.app_id = app["id"].as_str().unwrap_or_default().to_string();
        let me = d.bot("GET", "/users/@me", None).await?;
        d.bot_name = me["username"].as_str().unwrap_or("KasPass").to_string();
        Ok(Some(d))
    }

    /// The link that adds the bot to the creator's server with the permissions it needs.
    pub fn invite_bot_url(&self) -> String {
        format!("https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions={}", self.app_id, BOT_PERMISSIONS)
    }

    async fn bot(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{}", self.api, path);
        let m = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut req = self.http.request(m, &url).header("Authorization", format!("Bot {}", self.secrets.bot_token));
        if let Some(b) = body {
            req = req.json(&b);
        } else if method != "GET" {
            req = req.header("Content-Length", "0");
        }
        let resp = req.send().await.map_err(|e| anyhow!("{path}: {}", e.without_url()))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{path}: {} {}", status.as_u16(), short_err(&text));
        }
        Ok(if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::Null) })
    }

    fn save(&self) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(text) = serde_json::to_string_pretty(&*s) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Sets the roles the server manages: access key -> role name ("pass" -> "Pass Holder", tiers...).
    pub fn set_wanted(&self, wanted: Vec<(String, String)>) {
        *self.wanted.lock().unwrap_or_else(|e| e.into_inner()) = wanted;
    }

    /// Finds the server the bot was added to, and makes sure every managed role exists with the
    /// right name (creating or renaming roles as needed).
    pub async fn ensure_setup(&self) -> Result<bool> {
        let guild = self.state.lock().unwrap_or_else(|e| e.into_inner()).guild_id.clone();
        let (gid, gname) = match guild {
            // Already set up: re-read the server's name so a rename shows up on the website.
            Some(g) => {
                let name = self.bot("GET", &format!("/guilds/{g}"), None).await.ok().and_then(|v| v["name"].as_str().map(String::from));
                let old = self.state.lock().unwrap_or_else(|e| e.into_inner()).guild_name.clone();
                (g, name.filter(|n| Some(n) != old.as_ref()))
            }
            None => {
                let guilds = self.bot("GET", "/users/@me/guilds", None).await?;
                let Some(g) = guilds.as_array().and_then(|a| a.first()).cloned() else { return Ok(false) };
                (g["id"].as_str().unwrap_or_default().to_string(), Some(g["name"].as_str().unwrap_or("your server").to_string()))
            }
        };
        let wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let known = self.state.lock().unwrap_or_else(|e| e.into_inner()).roles.clone();
        let need_list = wanted.iter().any(|(k, _)| !known.contains_key(k)) || gname.is_some();
        let existing: Vec<Value> = if need_list || !wanted.is_empty() { self.bot("GET", &format!("/guilds/{gid}/roles"), None).await?.as_array().cloned().unwrap_or_default() } else { vec![] };
        let mut roles = known.clone();
        for (key, name) in &wanted {
            let current = roles.get(key).and_then(|id| existing.iter().find(|r| r["id"].as_str() == Some(id.as_str())));
            match current {
                Some(r) if r["name"].as_str() == Some(name.as_str()) => {}
                Some(r) => {
                    let id = r["id"].as_str().unwrap_or_default().to_string();
                    if self.bot("PATCH", &format!("/guilds/{gid}/roles/{id}"), Some(json!({ "name": name }))).await.is_ok() {
                        println!("discord: renamed role to \"{}\"", crate::log_safe(&name));
                    }
                }
                None => {
                    let by_name = existing.iter().find(|r| r["name"].as_str() == Some(name.as_str())).and_then(|r| r["id"].as_str()).map(String::from);
                    let id = match by_name {
                        Some(id) => id,
                        None => {
                            let r = self.bot("POST", &format!("/guilds/{gid}/roles"), Some(json!({ "name": name, "mentionable": false, "hoist": true }))).await?;
                            println!("discord: created role \"{}\"", crate::log_safe(&name));
                            r["id"].as_str().unwrap_or_default().to_string()
                        }
                    };
                    roles.insert(key.clone(), id);
                }
            }
        }
        // Roles for tiers that no longer exist stay in the server but are no longer managed.
        roles.retain(|k, _| wanted.iter().any(|(w, _)| w == k));
        let changed = roles != known || gname.is_some();
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(n) = gname {
                println!("discord: managing roles in server \"{}\"", crate::log_safe(&n));
                s.guild_id = Some(gid);
                s.guild_name = Some(n);
            }
            s.roles = roles;
        }
        if changed {
            self.save();
        }
        Ok(true)
    }

    /// (server name, ready): ready once the bot is in a server and its roles exist.
    pub fn status(&self) -> (Option<String>, bool) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (s.guild_name.clone(), s.guild_id.is_some() && !s.roles.is_empty())
    }

    /// Managed roles as (access key, role name).
    pub fn managed(&self) -> Vec<(String, String)> {
        self.wanted.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The website's address, once a member has connected from it.
    pub fn site_url(&self) -> Option<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).site.clone()
    }

    pub fn links(&self) -> Vec<Link> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).links.clone()
    }

    /// Discord's authorization screen for this wallet. `site` is "https://host/".
    pub fn authorize_url(&self, address: &str, site: &str) -> String {
        let code = crate::auth::random_token()[..24].to_string();
        let redirect = format!("{site}v1/discord/callback");
        {
            let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            p.retain(|_, (_, _, t)| t.elapsed() < STATE_TTL);
            p.insert(code.clone(), (address.to_string(), redirect.clone(), Instant::now()));
        }
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // The address members are pointed back to: a real name wins over a raw IP address.
            let raw = |u: &str| raw_host(u);
            if s.site.as_deref() != Some(site) && (s.site.is_none() || !raw(site) || s.site.as_deref().is_some_and(raw)) {
                s.site = Some(site.to_string());
            }
        }
        self.save();
        format!(
            "https://discord.com/oauth2/authorize?client_id={}&response_type=code&scope=identify%20guilds.join&prompt=none&redirect_uri={}&state={}",
            self.app_id,
            urlencode(&redirect),
            code
        )
    }

    /// Finishes the link after Discord sends the member back. Returns a sentence for the page.
    pub async fn complete(&self, code: &str, state: &str, access: &AccessCheck) -> Result<String> {
        let (address, redirect) = {
            let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            match p.remove(state) {
                Some((a, r, t)) if t.elapsed() < STATE_TTL => (a, r),
                _ => bail!("This link has expired. Go back to the site and click \"Connect Discord\" again."),
            }
        };
        let form = [("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect.as_str()), ("client_id", self.app_id.as_str()), ("client_secret", self.secrets.client_secret.as_str())];
        let tok: Value = self.http.post(format!("{}/oauth2/token", self.api)).form(&form).send().await.map_err(|e| anyhow!("token: {}", e.without_url()))?.json().await?;
        let access_token = tok["access_token"].as_str().ok_or_else(|| anyhow!("Discord didn't confirm the sign-in ({})", tok["error_description"].as_str().or(tok["error"].as_str()).unwrap_or("no token")))?.to_string();
        let me: Value = self.http.get(format!("{}/users/@me", self.api)).bearer_auth(&access_token).send().await?.json().await?;
        let user_id = me["id"].as_str().ok_or_else(|| anyhow!("Discord didn't say who you are"))?.to_string();
        let name = me["global_name"].as_str().or(me["username"].as_str()).unwrap_or("member").to_string();
        // One Discord account per wallet: a new one replaces the old, whose roles come off now,
        // so a pass can't be shared between several accounts.
        let key = crate::indexer::owner_hash_of(&address).ok();
        let same = |a: &str| key.is_some() && crate::indexer::owner_hash_of(a).ok() == key;
        let replaced: Vec<String> = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let old: Vec<String> = s.links.iter().filter(|l| l.user_id != user_id && same(&l.address)).map(|l| l.user_id.clone()).collect();
            s.links.retain(|l| l.user_id != user_id && !same(&l.address));
            s.links.push(Link { user_id: user_id.clone(), name: name.clone(), address: address.clone(), linked_ms: now_ms(), keys: vec![], has_role: false, missing_since: BTreeMap::new(), lost_dm_ms: 0 });
            old
        };
        self.save();
        let (gid, managed): (Option<String>, Vec<String>) = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.guild_id.clone(), s.roles.values().cloned().collect())
        };
        if let Some(gid) = gid {
            for old in &replaced {
                for rid in &managed {
                    let _ = self.bot("DELETE", &format!("/guilds/{gid}/members/{old}/roles/{rid}"), None).await;
                }
            }
        }
        if !self.ensure_setup().await? {
            return Ok(format!("Linked {name} to wallet {}. The creator hasn't added the bot to their server yet, so check back soon.", short(&address)));
        }
        let gid = self.state.lock().unwrap_or_else(|e| e.into_inner()).guild_id.clone().unwrap_or_default();
        match access(&address).await {
            None => Ok(format!("Linked {name} to wallet {}. The site can't check your wallet right now; your roles will be added within a few minutes.", short(&address))),
            Some(keys) => {
                let roles = self.role_ids(&keys);
                if roles.is_empty() {
                    return Ok(format!("Linked {name} to wallet {}, but it doesn't qualify for any role right now. Once it does (buy a pass, or hold the right NFTs), the role is added automatically within a minute.", short(&address)));
                }
                // Adds them to the server with their roles, or just gives the roles if they're already there.
                self.bot("PUT", &format!("/guilds/{gid}/members/{user_id}"), Some(json!({ "access_token": access_token, "roles": roles }))).await?;
                for r in &roles {
                    self.bot("PUT", &format!("/guilds/{gid}/members/{user_id}/roles/{r}"), None).await?;
                }
                self.mark(&user_id, keys.clone());
                let guild = self.status().0.unwrap_or_else(|| "the server".into());
                let names = self.role_names(&keys).join(", ");
                Ok(format!("You're in! {name} now has: {names} in {guild}. Open Discord to see the members-only channels."))
            }
        }
    }

    fn role_ids(&self, keys: &[String]) -> Vec<String> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        keys.iter().filter_map(|k| s.roles.get(k).cloned()).collect()
    }

    fn role_names(&self, keys: &[String]) -> Vec<String> {
        let w = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
        keys.iter().filter_map(|k| w.iter().find(|(x, _)| x == k).map(|(_, n)| n.clone())).collect()
    }

    fn mark(&self, user: &str, keys: Vec<String>) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for l in s.links.iter_mut().filter(|l| l.user_id == user) {
            if l.keys != keys {
                l.keys = keys.clone();
                changed = true;
            }
        }
        drop(s);
        if changed {
            self.save();
        }
    }

    pub async fn dm(&self, user: &str, text: &str) {
        if let Ok(ch) = self.bot("POST", "/users/@me/channels", Some(json!({ "recipient_id": user }))).await {
            if let Some(id) = ch["id"].as_str() {
                let _ = self.bot("POST", &format!("/channels/{id}/messages"), Some(json!({ "content": text }))).await;
            }
        }
    }

    /// Gives or removes each managed role to match what every linked member qualifies for. Run every minute.
    pub async fn sweep(&self, access: &AccessCheck) {
        match self.ensure_setup().await {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                eprintln!("discord: {e}");
                return;
            }
        }
        let (gid, roles, site) = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.guild_id.clone().unwrap_or_default(), s.roles.clone(), s.site.clone().unwrap_or_else(|| "the KasPass website".into()))
        };
        let names: HashMap<String, String> = self.managed().into_iter().collect();
        for l in self.links() {
            let Some(keys) = access(&l.address).await else { continue };
            // Check the member's real roles so changes made by hand are noticed too.
            let member = match self.bot("GET", &format!("/guilds/{gid}/members/{}", l.user_id), None).await {
                Ok(m) => m,
                Err(_) => {
                    self.mark(&l.user_id, vec![]);
                    continue; // not in the server
                }
            };
            let has: Vec<String> = member["roles"].as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let mut lost = Vec::new();
            let now = now_ms();
            let mut missing = l.missing_since.clone();
            for (key, rid) in &roles {
                let want = keys.iter().any(|k| k == key);
                let got = has.iter().any(|r| r == rid);
                let name = names.get(key).cloned().unwrap_or_else(|| key.clone());
                if want {
                    missing.remove(key);
                }
                if want && !got {
                    if self.bot("PUT", &format!("/guilds/{gid}/members/{}/roles/{rid}", l.user_id), None).await.is_ok() {
                        println!("discord: gave a member the {name} role");
                    }
                } else if !want && got {
                    // Only after it has failed every check for a while.
                    let since = *missing.entry(key.clone()).or_insert(now);
                    if now - since < REMOVE_AFTER_MS {
                        continue;
                    }
                    if self.bot("DELETE", &format!("/guilds/{gid}/members/{}/roles/{rid}", l.user_id), None).await.is_ok() {
                        println!("discord: removed the {name} role from a member (their wallet hasn't qualified for {} min)", (now - since) / 60_000);
                        missing.remove(key);
                        lost.push(name);
                    }
                } else if !want {
                    missing.remove(key);
                }
            }
            let tell = !lost.is_empty() && now - l.lost_dm_ms >= LOST_DM_EVERY_MS;
            {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                for x in s.links.iter_mut().filter(|x| x.user_id == l.user_id) {
                    x.missing_since = missing.clone();
                    if tell {
                        x.lost_dm_ms = now;
                    }
                }
            }
            self.save();
            self.mark(&l.user_id, keys.iter().filter(|k| roles.contains_key(*k)).cloned().collect());
            if tell {
                self.dm(&l.user_id, &format!("Your wallet {} no longer qualifies for: {}. The role was removed and comes back automatically once you qualify again (check on {site}).", short(&l.address), lost.join(", "))).await;
            }
        }
    }
}

fn short_err(text: &str) -> String {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v["message"].as_str().map(String::from)).unwrap_or_else(|| text.chars().take(120).collect())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn short(a: &str) -> String {
    crate::telegram::short(a)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// True for an address that is only an IP (or an IP wrapped in sslip.io / nip.io), not a real site name.
fn raw_host(u: &str) -> bool {
    let h = u.trim_start_matches("https://").trim_start_matches("http://");
    let h = h.split(['/', ':']).next().unwrap_or("");
    h.ends_with(".sslip.io") || h.ends_with(".nip.io") || h.parse::<std::net::IpAddr>().is_ok() || h.starts_with('[')
}

#[cfg(test)]
mod tests {
    #[test]
    fn raw_hosts() {
        assert!(super::raw_host("https://10-0-0-1.sslip.io/"));
        assert!(super::raw_host("http://10.0.0.1:8787/"));
        assert!(!super::raw_host("https://kaspass.app/"));
        assert!(!super::raw_host("https://pass.kaspa.vision/"));
        assert!(!super::raw_host("https://1kas.example/"));
    }
}
