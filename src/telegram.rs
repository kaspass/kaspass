//! Telegram bot: lets members into private groups and removes them when they no longer qualify.
//!
//! How it works:
//! - A signed-in member clicks "Join the Telegram group" on the website. The server makes a
//!   one-time code for their wallet and sends them to t.me/<bot>?start=<code>.
//! - Telegram delivers "/start <code>" to the bot, which links that Telegram account to the wallet
//!   and replies with a single-use invite for every group the member qualifies for.
//! - Every minute the bot re-checks linked members; anyone who no longer qualifies for a group is
//!   removed from it (not banned) and told how to come back.
//!
//! Each group belongs to one access key: "pass" (an active paid pass) or "tier:<name>" (a holder-club
//! tier). The first group the bot is made admin of belongs to pass holders when there are no club
//! tiers; otherwise an admin sends `/kaspass <tier>` in the group to choose. The bot needs the
//! "Invite users" and "Ban users" admin rights. Its token lives in `telegram.token`.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CODE_TTL: Duration = Duration::from_secs(600);
const INVITE_TTL_S: i64 = 600;
/// How often linked members are re-checked (KASPASS_TELEGRAM_SWEEP_S overrides it, for tests).
pub fn sweep_every() -> Duration {
    Duration::from_secs(crate::test_switch("KASPASS_TELEGRAM_SWEEP_S").and_then(|v| v.parse().ok()).unwrap_or(60))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Link {
    pub user_id: i64,
    pub name: String,
    pub address: String,
    pub linked_ms: i64,
    /// Groups the bot last saw them in.
    #[serde(default)]
    pub in_groups: Vec<i64>,
    /// Older single-group field, kept so old state files still load.
    #[serde(default, skip_serializing)]
    pub in_group: bool,
    /// The member asked for no renewal reminders (/quiet).
    #[serde(default)]
    pub quiet: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Group {
    pub chat_id: i64,
    pub title: String,
    /// "pass", "level:<id>", "tier:<name>", or None until an admin picks one.
    pub key: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct BotState {
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Groups the bot was added to without admin rights (not managed; the creator can remove it).
    #[serde(default)]
    pub unmanaged: Vec<Group>,
    /// Older single-group fields, migrated into `groups` on load.
    #[serde(default, skip_serializing)]
    pub chat_id: Option<i64>,
    #[serde(default, skip_serializing)]
    pub chat_title: Option<String>,
    /// The website's address, learned from members' visits, for links in messages.
    pub site: Option<String>,
    pub links: Vec<Link>,
    pub offset: i64,
}

pub struct Bot {
    token: String,
    api: String,
    /// Replaced after repeated failures, so a dead connection left over from a Telegram outage can't stall the bot.
    http: Mutex<reqwest::Client>,
    pub username: String,
    path: PathBuf,
    pub state: Mutex<BotState>,
    codes: Mutex<HashMap<String, (String, Instant)>>,
    /// What groups can be for besides "pass", as (key, name): paid levels ("level:<id>", "Gold")
    /// then club tiers ("tier:Whale", "Whale"). Kept up to date by the server.
    choices: Mutex<Vec<(String, String)>>,
}

/// "Which access keys does this wallet have right now?" ("pass", "tier:Whale", ...).
/// None means the server can't tell yet (still syncing, or the NFT indexer is down), so nothing changes.
pub type AccessCheck = dyn Fn(&str) -> futures_like::BoxFuture<Option<Vec<String>>> + Send + Sync;

pub mod futures_like {
    pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
}

/// Names for "level:<id>" keys, set by the server (a level's name can change in the dashboard).
static LEVEL_NAMES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn set_level_names(names: Vec<(String, String)>) {
    *LEVEL_NAMES.lock().unwrap_or_else(|e| e.into_inner()) = names;
}

/// How a key reads to people.
pub fn key_label(key: &str) -> String {
    if let Some(t) = key.strip_prefix("tier:") {
        return t.to_string();
    }
    if key == "pass" {
        return "paid members".into();
    }
    if key.starts_with("level:") {
        if let Some((_, n)) = LEVEL_NAMES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| k == key) {
            return format!("{n} members");
        }
        return "a paid level".into();
    }
    key.to_string()
}

impl Bot {
    /// Starts the bot if `telegram.token` is in `dir`. Returns None (bot off) when there's no token.
    pub async fn open(dir: &std::path::Path) -> Result<Option<Bot>> {
        let token = match std::fs::read_to_string(dir.join("telegram.token")) {
            Ok(t) => t.chars().filter(|c| c.is_ascii_graphic()).collect::<String>(),
            Err(_) => return Ok(None),
        };
        if token.is_empty() {
            return Ok(None);
        }
        let api = crate::test_switch("KASPASS_TELEGRAM_API").unwrap_or_else(|| "https://api.telegram.org".into());
        let http = Mutex::new(new_client()?);
        let path = dir.join("telegram.json");
        let mut state: BotState = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if let Some(id) = state.chat_id.take() {
            let title = state.chat_title.take().unwrap_or_else(|| "your group".into());
            if !state.groups.iter().any(|g| g.chat_id == id) {
                state.groups.push(Group { chat_id: id, title, key: Some("pass".into()) });
            }
            for l in state.links.iter_mut().filter(|l| l.in_group) {
                l.in_groups = vec![id];
            }
        }
        let mut bot = Bot { token, api, http, username: String::new(), path, state: Mutex::new(state), codes: Mutex::new(HashMap::new()), choices: Mutex::new(Vec::new()) };
        let me = bot.call("getMe", json!({})).await.map_err(|e| anyhow!("Telegram didn't accept the bot token ({e})"))?;
        bot.username = me["username"].as_str().unwrap_or_default().to_string();
        bot.save();
        Ok(Some(bot))
    }

    pub fn set_choices(&self, choices: Vec<(String, String)>) {
        *self.choices.lock().unwrap_or_else(|e| e.into_inner()) = choices;
    }

    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let url = format!("{}/bot{}/{}", self.api, self.token, method);
        let http = self.http.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let resp: Value = http.post(&url).json(&body).send().await.map_err(|e| anyhow!("{method}: {}", e.without_url()))?.json().await.map_err(|e| anyhow!("{method}: {}", e.without_url()))?;
        if resp["ok"].as_bool() != Some(true) {
            return Err(anyhow!("{method}: {}", resp["description"].as_str().unwrap_or("failed")));
        }
        Ok(resp["result"].clone())
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

    /// A one-time link that opens the bot and links this wallet.
    pub fn link_url(&self, address: &str, site: Option<String>) -> String {
        let code = crate::auth::random_token()[..24].to_string();
        let mut codes = self.codes.lock().unwrap_or_else(|e| e.into_inner());
        codes.retain(|_, (_, t)| t.elapsed() < CODE_TTL);
        codes.insert(code.clone(), (address.to_string(), Instant::now()));
        drop(codes);
        if let Some(site) = site {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.site.as_deref() != Some(site.as_str()) {
                s.site = Some(site);
                drop(s);
                self.save();
            }
        }
        format!("https://t.me/{}?start={}", self.username, code)
    }

    pub fn groups(&self) -> Vec<Group> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).groups.clone()
    }

    /// Whether at least one group is ready for members.
    pub fn has_group(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).groups.iter().any(|g| g.key.is_some())
    }

    /// Groups the bot is in without admin rights.
    pub fn unmanaged(&self) -> Vec<Group> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).unmanaged.clone()
    }

    /// The bot leaves a group it knows about (managed or not).
    pub async fn leave(&self, chat: i64) -> Result<String> {
        let title = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.groups.iter().chain(s.unmanaged.iter()).find(|g| g.chat_id == chat).map(|g| g.title.clone())
        }
        .ok_or_else(|| anyhow::anyhow!("the bot doesn't know that group"))?;
        self.call("leaveChat", json!({ "chat_id": chat })).await?;
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.groups.retain(|g| g.chat_id != chat);
            s.unmanaged.retain(|g| g.chat_id != chat);
        }
        self.save();
        println!("telegram: left group \"{title}\" ({chat}) from the dashboard");
        Ok(title)
    }

    pub fn links(&self) -> Vec<Link> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).links.clone()
    }

    fn site(&self) -> String {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).site.clone().unwrap_or_else(|| "the KasPass website".into())
    }

    /// The website's address, once a member has visited it.
    pub fn site_url(&self) -> Option<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).site.clone()
    }

    /// A direct message to a linked member (for renewal reminders).
    pub async fn notify(&self, user: i64, text: &str) {
        self.say(user, text).await;
    }

    async fn say(&self, chat: i64, text: &str) {
        if let Err(e) = self.call("sendMessage", json!({ "chat_id": chat, "text": text, "disable_web_page_preview": true })).await {
            eprintln!("telegram: {e}");
        }
    }

    /// Long-polls Telegram for messages and group changes. Runs forever.
    pub async fn run(&self, access: &AccessCheck) {
        let mut failures = 0u32;
        loop {
            let offset = self.state.lock().unwrap_or_else(|e| e.into_inner()).offset;
            let body = json!({ "offset": offset, "timeout": 25, "allowed_updates": ["message", "my_chat_member", "chat_member", "chat_join_request"] });
            match self.call("getUpdates", body).await {
                Ok(Value::Array(updates)) => {
                    if failures > 0 {
                        println!("telegram: reconnected");
                        failures = 0;
                    }
                    for u in updates {
                        let id = u["update_id"].as_i64().unwrap_or(0);
                        self.handle(&u, access).await;
                        self.state.lock().unwrap_or_else(|e| e.into_inner()).offset = id + 1;
                    }
                    self.save();
                }
                Ok(_) => {}
                Err(e) => {
                    failures += 1;
                    eprintln!("telegram: {e} (retrying)");
                    // After a few failures in a row, start over with fresh connections.
                    if failures % 3 == 0 {
                        if let Ok(c) = new_client() {
                            *self.http.lock().unwrap_or_else(|e| e.into_inner()) = c;
                            eprintln!("telegram: opened a fresh connection to Telegram");
                        }
                    }
                    // Back off a little more each time (5 s up to 60 s), honoring "retry after N" from Telegram.
                    let asked = e.to_string().rsplit("retry after ").next().and_then(|n| n.trim().parse::<u64>().ok()).unwrap_or(0);
                    let wait = asked.max((5 * failures as u64).min(60));
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
            }
        }
    }

    async fn handle(&self, u: &Value, access: &AccessCheck) {
        if let Some(m) = u.get("my_chat_member") {
            self.on_bot_membership(m).await;
        } else if let Some(m) = u.get("chat_member") {
            self.on_member_change(m);
        } else if let Some(r) = u.get("chat_join_request") {
            self.on_join_request(r, access).await;
        } else if let Some(m) = u.get("message") {
            match m["chat"]["type"].as_str() {
                Some("private") => self.on_private(m, access).await,
                Some("group" | "supergroup") => self.on_group_message(m).await,
                _ => {}
            }
        }
    }

    fn assign_help(&self) -> String {
        let choices = self.choices.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut opts = vec!["/kaspass pass  (any paid member)".to_string()];
        opts.extend(choices.iter().map(|(k, n)| if k.starts_with("level:") { format!("/kaspass {n}  ({n} and higher levels)") } else { format!("/kaspass {n}") }));
        format!("Which members is this group for? An admin of this group, send one of:\n{}", opts.join("\n"))
    }

    /// The bot was added to, promoted in, or removed from a group.
    async fn on_bot_membership(&self, m: &Value) {
        let chat = m["chat"]["id"].as_i64().unwrap_or(0);
        let kind = m["chat"]["type"].as_str().unwrap_or("");
        let title = m["chat"]["title"].as_str().unwrap_or("your group").to_string();
        let status = m["new_chat_member"]["status"].as_str().unwrap_or("");
        if kind != "group" && kind != "supergroup" {
            return;
        }
        let no_tiers = self.choices.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        let reply = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let known = s.groups.iter().position(|g| g.chat_id == chat);
            if status == "administrator" {
                // Made an admin: it's a managed group now.
                s.unmanaged.retain(|g| g.chat_id != chat);
            }
            match status {
                "administrator" => match known {
                    Some(i) => {
                        s.groups[i].title = title.clone();
                        None
                    }
                    None => {
                        let first = s.groups.is_empty();
                        let key = (first && no_tiers).then(|| "pass".to_string());
                        let ready = key.is_some();
                        s.groups.push(Group { chat_id: chat, title: title.clone(), key });
                        println!("telegram: connected to group \"{}\" ({chat})", crate::log_safe(&title));
                        Some(if ready {
                            "KasPass is connected. Members can join from the membership website, and I'll remove anyone whose access ends.".to_string()
                        } else {
                            format!("KasPass is connected. {}", self.assign_help())
                        })
                    }
                },
                "left" | "kicked" => {
                    s.unmanaged.retain(|g| g.chat_id != chat);
                    if let Some(i) = known {
                        s.groups.remove(i);
                        println!("telegram: removed from group \"{title}\"");
                    }
                    None
                }
                // Telegram can report the same "added" twice (for example when a group is created
                // with the bot in it); greet once.
                "member" if known.is_none() && s.unmanaged.iter().any(|g| g.chat_id == chat) => None,
                "member" if known.is_none() => {
                    s.unmanaged.push(Group { chat_id: chat, title: title.clone(), key: None });
                    Some("Thanks for adding me. Make me an admin (with \"Invite users\" and \"Ban users\") and I'll start managing this group for KasPass.".to_string())
                }
                _ => None,
            }
        };
        self.save();
        if let Some(text) = reply {
            self.say(chat, &text).await;
        }
    }

    /// `/kaspass <tier>` in a group: an admin chooses who the group is for.
    async fn on_group_message(&self, m: &Value) {
        let chat = m["chat"]["id"].as_i64().unwrap_or(0);
        let user = m["from"]["id"].as_i64().unwrap_or(0);
        // The group was renamed: use the new name in the bot's messages.
        if let Some(t) = m["new_chat_title"].as_str() {
            self.set_title(chat, t);
            return;
        }
        let text = m["text"].as_str().unwrap_or("").trim();
        let Some(rest) = text.strip_prefix("/kaspass") else { return };
        let rest = rest.split_once(char::is_whitespace).map(|(_, r)| r).unwrap_or(if rest.starts_with('@') { "" } else { rest }).trim();
        if !self.state.lock().unwrap_or_else(|e| e.into_inner()).groups.iter().any(|g| g.chat_id == chat) {
            return;
        }
        let admin = self.call("getChatMember", json!({ "chat_id": chat, "user_id": user })).await.ok().is_some_and(|v| matches!(v["status"].as_str(), Some("administrator" | "creator")));
        if !admin {
            self.say(chat, "Only an admin of this group can choose who it's for.").await;
            return;
        }
        if rest.is_empty() {
            self.say(chat, &self.assign_help()).await;
            return;
        }
        let choices = self.choices.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let key = if rest.eq_ignore_ascii_case("pass") {
            Some("pass".to_string())
        } else {
            // Paid levels come first, so they win over a club tier with the same name.
            choices.iter().find(|(_, n)| n.eq_ignore_ascii_case(rest)).map(|(k, _)| k.clone())
        };
        let Some(key) = key else {
            self.say(chat, &format!("\"{rest}\" isn't one of this site's levels. {}", self.assign_help())).await;
            return;
        };
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(g) = s.groups.iter_mut().find(|g| g.chat_id == chat) {
                g.key = Some(key.clone());
            }
        }
        self.save();
        self.say(chat, &format!("Done: this group is now for {}. Members who qualify can join from the membership website, and I'll remove anyone who stops qualifying.", key_label(&key))).await;
    }

    fn set_title(&self, chat: i64, title: &str) {
        let changed = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match s.groups.iter_mut().find(|g| g.chat_id == chat) {
                Some(g) if g.title != title => {
                    println!("telegram: group \"{}\" is now called \"{}\"", crate::log_safe(&g.title), crate::log_safe(&title));
                    g.title = title.to_string();
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.save();
        }
    }

    /// A join request through one of our invites: approved only for the Telegram account the
    /// invite was made for, while its wallet still qualifies. A forwarded invite gets nobody in.
    async fn on_join_request(&self, r: &Value, access: &AccessCheck) {
        let chat = r["chat"]["id"].as_i64().unwrap_or(0);
        let user = r["from"]["id"].as_i64().unwrap_or(0);
        let Some(name) = r["invite_link"]["name"].as_str().filter(|n| n.starts_with("KasPass ")) else { return };
        let Some(g) = self.groups().into_iter().find(|g| g.chat_id == chat && g.key.is_some()) else { return };
        let meant = name.strip_prefix("KasPass u").and_then(|x| x.parse::<i64>().ok());
        let link = self.state.lock().unwrap_or_else(|e| e.into_inner()).links.iter().find(|l| l.user_id == user).cloned();
        let ok = match (link, meant) {
            (Some(l), Some(m)) if m == user => access(&l.address).await.is_some_and(|keys| keys.iter().any(|k| Some(k.as_str()) == g.key.as_deref())),
            _ => false,
        };
        let body = json!({ "chat_id": chat, "user_id": user });
        if ok {
            if self.call("approveChatJoinRequest", body).await.is_ok() {
                self.mark(user, chat, true);
            }
        } else {
            let _ = self.call("declineChatJoinRequest", body).await;
            println!("telegram: declined a join request to \"{}\" (the invite was made for someone else, or their wallet doesn't qualify)", crate::log_safe(&g.title));
        }
    }

    /// Takes a Telegram account out of every group this bot manages (ban then unban, so they can
    /// come back later through a new invite).
    async fn remove_everywhere(&self, user: i64) {
        for g in self.groups().into_iter().filter(|g| g.key.is_some()) {
            let inside = match self.call("getChatMember", json!({ "chat_id": g.chat_id, "user_id": user })).await {
                Ok(m) => matches!(m["status"].as_str(), Some("member" | "restricted")),
                Err(_) => false,
            };
            if inside && self.call("banChatMember", json!({ "chat_id": g.chat_id, "user_id": user })).await.is_ok() {
                let _ = self.call("unbanChatMember", json!({ "chat_id": g.chat_id, "user_id": user, "only_if_banned": true })).await;
                self.mark(user, g.chat_id, false);
            }
        }
    }

    /// Keeps track of which groups linked members are in.
    fn on_member_change(&self, m: &Value) {
        let chat = m["chat"]["id"].as_i64().unwrap_or(0);
        let user = m["new_chat_member"]["user"]["id"].as_i64().unwrap_or(0);
        let status = m["new_chat_member"]["status"].as_str().unwrap_or("");
        let inside = matches!(status, "member" | "administrator" | "creator" | "restricted");
        if self.state.lock().unwrap_or_else(|e| e.into_inner()).groups.iter().any(|g| g.chat_id == chat) {
            self.mark(user, chat, inside);
        }
    }

    fn mark(&self, user: i64, chat: i64, inside: bool) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for l in s.links.iter_mut().filter(|l| l.user_id == user) {
            let has = l.in_groups.contains(&chat);
            if inside && !has {
                l.in_groups.push(chat);
                changed = true;
            } else if !inside && has {
                l.in_groups.retain(|c| *c != chat);
                changed = true;
            }
        }
        drop(s);
        if changed {
            self.save();
        }
    }

    async fn on_private(&self, m: &Value, access: &AccessCheck) {
        let chat = m["chat"]["id"].as_i64().unwrap_or(0);
        let user = m["from"]["id"].as_i64().unwrap_or(0);
        let name = display_name(&m["from"]);
        let text = m["text"].as_str().unwrap_or("").trim();
        let site = self.site();
        if let Some(code) = text.strip_prefix("/start").map(str::trim).filter(|c| !c.is_empty()) {
            let address = self.codes.lock().unwrap_or_else(|e| e.into_inner()).remove(code).filter(|(_, t)| t.elapsed() < CODE_TTL).map(|(a, _)| a);
            let Some(address) = address else {
                self.say(chat, &format!("That link has expired. Sign in on {site} and click \"Join the Telegram group\" again.")).await;
                return;
            };
            // One Telegram account per wallet: linking a new one unlinks the old one and takes it
            // out of the groups, so a pass can't be shared between several accounts.
            let key = crate::indexer::owner_hash_of(&address).ok();
            let same = |a: &str| key.is_some() && crate::indexer::owner_hash_of(a).ok() == key;
            let replaced: Vec<i64> = {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let old: Vec<i64> = s.links.iter().filter(|l| l.user_id != user && same(&l.address)).map(|l| l.user_id).collect();
                s.links.retain(|l| l.user_id != user && !same(&l.address));
                s.links.push(Link { user_id: user, name: name.clone(), address: address.clone(), linked_ms: now_ms(), in_groups: vec![], in_group: false, quiet: false });
                old
            };
            self.save();
            for old in replaced {
                self.remove_everywhere(old).await;
                self.say(old, &format!("Wallet {} was just linked to another Telegram account, so this one is unlinked and out of the members' groups.", short(&address))).await;
            }
            self.say(chat, &format!("Linked to wallet {}.", short(&address))).await;
            self.invite(chat, user, &address, access).await;
        } else if text.starts_with("/status") || text.starts_with("/join") {
            let link = self.state.lock().unwrap_or_else(|e| e.into_inner()).links.iter().find(|l| l.user_id == user).cloned();
            match link {
                Some(l) => self.invite(chat, user, &l.address, access).await,
                None => self.say(chat, &format!("You're not linked yet. Sign in on {site} and click \"Join the Telegram group\".")).await,
            }
        } else if text.starts_with("/quiet") || text.starts_with("/remind") {
            let on = text.starts_with("/remind");
            let found = {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let mut found = false;
                for l in s.links.iter_mut().filter(|l| l.user_id == user) {
                    l.quiet = !on;
                    found = true;
                }
                found
            };
            self.save();
            self.say(chat, if !found { "You're not linked yet, so there's nothing to change." } else if on { "Renewal reminders are on. I'll message you a little before it's time to renew." } else { "OK, no more renewal reminders. Send /remind to turn them back on." }).await;
        } else if text.starts_with("/unlink") {
            // Out of the groups first: membership comes with the link.
            self.remove_everywhere(user).await;
            self.state.lock().unwrap_or_else(|e| e.into_inner()).links.retain(|l| l.user_id != user);
            self.save();
            self.say(chat, "Unlinked, and out of the members' groups. Link a wallet again any time to come back.").await;
        } else {
            self.say(
                chat,
                &format!("I manage members-only Telegram groups for KasPass.\n\nTo join: sign in on {site} with your wallet and click \"Join the Telegram group\".\n\n/status  check what you qualify for and get invites\n/quiet  stop renewal reminders (/remind turns them back on)\n/unlink  forget your wallet"),
            )
            .await;
        }
    }

    /// Sends single-use invites for every group the wallet qualifies for.
    async fn invite(&self, chat: i64, user: i64, address: &str, access: &AccessCheck) {
        let site = self.site();
        let groups: Vec<Group> = self.groups().into_iter().filter(|g| g.key.is_some()).collect();
        if groups.is_empty() {
            self.say(chat, "There's no members' group here yet. You'll still get renewal reminders (and, for the site's owner, alerts) in this chat.").await;
            return;
        }
        let Some(keys) = access(address).await else {
            self.say(chat, "I can't check your wallet right now (still catching up, or the NFT indexer is busy). Send /status again in a minute.").await;
            return;
        };
        let mut lines = Vec::new();
        for g in &groups {
            let key = g.key.as_deref().unwrap_or_default();
            if !keys.iter().any(|k| k == key) {
                continue;
            }
            if let Ok(m) = self.call("getChatMember", json!({ "chat_id": g.chat_id, "user_id": user })).await {
                if matches!(m["status"].as_str(), Some("member" | "administrator" | "creator")) {
                    self.mark(user, g.chat_id, true);
                    lines.push(format!("{}: you're already in.", g.title));
                    continue;
                }
            }
            // In case they were removed earlier when they stopped qualifying.
            let _ = self.call("unbanChatMember", json!({ "chat_id": g.chat_id, "user_id": user, "only_if_banned": true })).await;
            // A join request link made for this account only: the bot approves the request when it
            // comes from this user, so a forwarded link is useless.
            let body = json!({ "chat_id": g.chat_id, "creates_join_request": true, "expire_date": now_ms() / 1000 + INVITE_TTL_S, "name": format!("KasPass u{user}") });
            match self.call("createChatInviteLink", body).await {
                Ok(l) => lines.push(format!("{}: {}", g.title, l["invite_link"].as_str().unwrap_or_default())),
                Err(e) => {
                    eprintln!("telegram: {e}");
                    lines.push(format!("{}: I couldn't make an invite (the bot needs the \"Invite users\" admin right).", g.title));
                }
            }
        }
        if lines.is_empty() {
            // Say what the wallet does qualify for and what each group needs, so a holder whose
            // tier has no group yet isn't left thinking the check is broken.
            let has: Vec<String> = keys.iter().map(|k| key_label(k)).collect();
            let needs: Vec<String> = groups.iter().map(|g| format!("{} (for {})", g.title, key_label(g.key.as_deref().unwrap_or_default()))).collect();
            let mut msg = if has.is_empty() {
                format!("Wallet {} (with your linked wallets) doesn't qualify for any group right now.", short(address))
            } else {
                format!("Wallet {} (with your linked wallets) qualifies for: {}. None of the Telegram groups are for that yet.", short(address), has.join(", "))
            };
            msg.push_str(&format!("\n\nGroups here: {}.\n\nCheck what you hold on {site}, then send /status.", needs.join(", ")));
            self.say(chat, &msg).await;
        } else {
            self.say(chat, &format!("You qualify for:\n\n{}\n\nEach invite is for you only and works for 10 minutes: tap it, ask to join, and I'll let you in.", lines.join("\n"))).await;
        }
    }

    /// Removes linked members from groups they no longer qualify for. Run every minute.
    pub async fn sweep(&self, access: &AccessCheck) {
        // Pick up renames the bot missed (for example while Telegram was unreachable).
        for g in self.groups() {
            if let Ok(c) = self.call("getChat", json!({ "chat_id": g.chat_id })).await {
                if let Some(t) = c["title"].as_str() {
                    self.set_title(g.chat_id, t);
                }
            }
        }
        let groups: Vec<Group> = self.groups().into_iter().filter(|g| g.key.is_some()).collect();
        if groups.is_empty() {
            return;
        }
        let site = self.site();
        for l in self.links() {
            let Some(keys) = access(&l.address).await else { continue };
            for g in &groups {
                let key = g.key.as_deref().unwrap_or_default();
                let status = match self.call("getChatMember", json!({ "chat_id": g.chat_id, "user_id": l.user_id })).await {
                    Ok(m) => m["status"].as_str().unwrap_or("").to_string(),
                    Err(_) => continue,
                };
                let inside = matches!(status.as_str(), "member" | "restricted");
                if inside && !keys.iter().any(|k| k == key) {
                    // Ban then unban: removes them without blocking a later return.
                    if self.call("banChatMember", json!({ "chat_id": g.chat_id, "user_id": l.user_id })).await.is_ok() {
                        let _ = self.call("unbanChatMember", json!({ "chat_id": g.chat_id, "user_id": l.user_id, "only_if_banned": true })).await;
                        println!("telegram: removed a member from \"{}\" (their wallet no longer qualifies)", crate::log_safe(&g.title));
                        self.mark(l.user_id, g.chat_id, false);
                        let msg = if key.starts_with("tier:") {
                            format!("You no longer qualify for {} with wallet {}, so I've removed you from that group. Once you qualify again (check on {site}), send /status to come back.", g.title, short(&l.address))
                        } else {
                            format!("Your membership has ended, so I've removed you from {}. Renew on {site}, then send /status here to come back.", g.title)
                        };
                        self.say(l.user_id, &msg).await;
                    }
                } else {
                    self.mark(l.user_id, g.chat_id, inside || matches!(status.as_str(), "administrator" | "creator"));
                }
            }
        }
    }
}

fn new_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(40)).connect_timeout(Duration::from_secs(10)).pool_idle_timeout(Duration::from_secs(30)).build()?)
}

fn display_name(u: &Value) -> String {
    if let Some(n) = u["username"].as_str() {
        return format!("@{n}");
    }
    let first = u["first_name"].as_str().unwrap_or("");
    let last = u["last_name"].as_str().unwrap_or("");
    format!("{first} {last}").trim().to_string()
}

pub fn short(a: &str) -> String {
    let tail = a.rsplit(':').next().unwrap_or(a);
    if tail.len() > 12 { format!("{}…{}", &tail[..6], &tail[tail.len() - 4..]) } else { tail.to_string() }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
