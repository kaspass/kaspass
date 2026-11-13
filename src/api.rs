//! Small read-only web API over the indexer, for apps and websites.
//!
//!   GET /v1/health                  status and counts
//!   GET /v1/tier                    the tier card
//!   GET /v1/passes                  every live, verified pass
//!   GET /v1/check?address=kaspa...  does this address hold an ACTIVE pass?
//!   GET /v1/nonce                   one-time sign-in challenge
//!   POST /v1/login                  {address, message, signature} -> verified membership + session
//!   GET /v1/members?session=...     members-only content, only for an active pass
//!   GET /v1/offer                   current prices (when this server can sell)
//!   POST /v1/buy/prepare            {address, kind} -> a purchase for the buyer's wallet to sign
//!   POST /v1/buy/submit             {quote, psktTransactionJson} -> checked and broadcast
//!   GET /                           the KasPass website

use std::sync::Arc;
mod payin;
mod brand;
mod home;
mod promo;
mod trial;
mod members;
mod alerts;
mod examples;

use anyhow::Result;
use std::sync::Mutex;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::auth::{self, Sessions};
use crate::shop::{Quote, Shop};
use std::collections::HashMap;

use crate::Chain;
use crate::indexer::{Event, FoundLane, FoundPass, Indexer, TierCard, owner_hash_of};

#[derive(Default, Clone, Serialize)]
struct Snapshot {
    passes: Vec<FoundPass>,
    /// Live sales lanes found on-chain.
    lanes: Vec<FoundLane>,
    /// Every payment and trial, for the creator dashboard.
    events: Vec<Event>,
    scanned_txs: usize,
    updated_ms: i64,
    synced: bool,
}

/// One paid level on this site: its own membership (tier card), what the chain says about it,
/// and its shop. Levels are listed lowest first; a higher level includes the perks of lower ones.
#[derive(Clone)]
struct Level {
    card: TierCard,
    snap: Arc<RwLock<Snapshot>>,
    shop: Option<Arc<Shop>>,
}

/// `?level=<covenant id, or its first 12 characters>` on any endpoint; none means the lowest level.
#[derive(Deserialize, Default)]
struct LevelQ {
    #[serde(default)]
    level: Option<String>,
}

impl LevelQ {
    fn key(&self) -> Option<&str> {
        self.level.as_deref().map(str::trim).filter(|k| !k.is_empty())
    }
}

/// The short id used in access keys and URLs: the covenant id's first 12 characters.
fn short_id(covenant_id: &str) -> &str {
    &covenant_id[..covenant_id.len().min(12)]
}

#[derive(Clone)]
struct AppState {
    /// Every level on sale, lowest first. `card`, `snap` and `shop` below are the selected one.
    levels: Arc<Vec<Level>>,
    card: TierCard,
    snap: Arc<RwLock<Snapshot>>,
    sessions: Arc<Mutex<Sessions>>,
    members_html: Arc<String>,
    chain: Arc<tokio::sync::Mutex<Chain>>,
    shop: Option<Arc<Shop>>,
    /// Prepared transactions waiting for a signature, with the level (covenant id) they're for.
    quotes: Arc<Mutex<HashMap<String, (String, Quote)>>>,
    /// Wallets that just bought a pass, so a second tab or a double click can't buy another
    /// before the indexer has seen the first.
    recent_buys: Arc<Mutex<HashMap<String, std::time::Instant>>>,
    telegram: Option<Arc<crate::telegram::Bot>>,
    discord: Option<Arc<crate::discord::Discord>>,
    club: Arc<std::sync::RwLock<Option<Arc<crate::clubs::Club>>>>,
    wallets: Arc<crate::clubs::Wallets>,
    home: std::path::PathBuf,
    access: Arc<crate::telegram::AccessCheck>,
    names: Arc<crate::names::Resolver>,
}

impl AppState {
    /// This state, with one level selected (the lowest when `key` is None).
    fn at(&self, key: Option<&str>) -> Result<AppState, ApiError> {
        let lv = match key {
            None => self.levels.first(),
            Some(k) => self.levels.iter().find(|l| l.card.covenant_id == k || (k.len() >= 12 && l.card.covenant_id.starts_with(k))),
        }
        .ok_or_else(|| (StatusCode::NOT_FOUND, "that level isn't on this site".to_string()))?;
        let mut s = self.clone();
        s.card = lv.card.clone();
        s.snap = lv.snap.clone();
        s.shop = lv.shop.clone();
        Ok(s)
    }

    fn lv(&self, q: &LevelQ) -> Result<AppState, ApiError> {
        self.at(q.key())
    }

    /// The selected level's rank (0 = lowest).
    fn rank(&self) -> usize {
        self.levels.iter().position(|l| l.card.covenant_id == self.card.covenant_id).unwrap_or(0)
    }

    /// The selected level's name: the creator's name for it, else "Level N" when there are several.
    fn level_name(&self) -> String {
        level_name(&self.home, &self.levels, self.rank())
    }
}

fn level_name(home: &std::path::Path, levels: &[Level], i: usize) -> String {
    levels.get(i).and_then(|l| tier_name(home, &l.card.covenant_id)).map(|n| fill_tax_year(&n)).unwrap_or_else(|| if levels.len() > 1 { format!("Level {}", i + 1) } else { "This membership".into() })
}

/// The highest level (rank) on which any of these owners holds an active pass.
async fn top_rank(levels: &[Level], owners: &[String]) -> Option<Option<usize>> {
    let mut top = None;
    for (i, l) in levels.iter().enumerate() {
        let s = l.snap.read().await;
        if !s.synced {
            return None;
        }
        if s.passes.iter().any(|p| p.active && owners.contains(&p.owner_hash)) {
            top = Some(i);
        }
    }
    Some(top)
}

const RECENT_BUY: std::time::Duration = std::time::Duration::from_secs(120);

const QUOTE_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// At most this many prepared transactions wait for a signature; the oldest go first.
const MAX_QUOTES: usize = 2_000;

fn store_quote(s: &AppState, quote: Quote) -> String {
    let id = auth::random_token();
    let mut quotes = s.quotes.lock().unwrap_or_else(|e| e.into_inner());
    quotes.retain(|_, (_, q)| q.created.elapsed() < QUOTE_TTL);
    while quotes.len() >= MAX_QUOTES {
        let Some(k) = quotes.iter().min_by_key(|(_, (_, q))| q.created).map(|(k, _)| k.clone()) else { break };
        quotes.remove(&k);
    }
    quotes.insert(id.clone(), (s.card.covenant_id.clone(), quote));
    id
}

/// Runs a background job forever: if it crashes or stops, it's logged and started again, so one
/// unexpected error can't silently switch off the keeper, reminders or a bot.
fn supervise<F, Fut>(name: &'static str, make: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            match tokio::spawn(make()).await {
                Err(e) if e.is_panic() => {
                    let p = e.into_panic();
                    let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                    eprintln!("{name}: crashed ({msg}); starting it again in 10 s");
                }
                _ => eprintln!("{name}: stopped; starting it again in 10 s"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
}

/// Wall-clock milliseconds of the indexer's last progress (a page read or a refresh).
static LAST_PROGRESS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn progress_now() {
    LAST_PROGRESS.store(wall_ms(), std::sync::atomic::Ordering::Relaxed);
}

/// If the site hasn't read the chain for KASPASS_WATCHDOG_S (default 10 minutes), the connection
/// is stuck: under systemd the site exits so it's started fresh; run by hand, it says so.
fn watchdog() {
    let limit_ms = std::env::var("KASPASS_WATCHDOG_S").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(600) * 1000;
    progress_now();
    tokio::spawn(async move {
        let mut warned = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs((limit_ms / 20_000).clamp(1, 30) as u64)).await;
            let idle = wall_ms() - LAST_PROGRESS.load(std::sync::atomic::Ordering::Relaxed);
            if idle < limit_ms {
                warned = false;
                continue;
            }
            if std::env::var("INVOCATION_ID").is_ok() {
                eprintln!("watchdog: no successful chain read for {} s; restarting the site", idle / 1000);
                std::process::exit(1);
            } else if !warned {
                eprintln!("watchdog: no successful chain read for {} s; restart the site if it doesn't recover", idle / 1000);
                warned = true;
            }
        }
    });
}

const SITE: &str = include_str!("../web/index.html");

/// Shown to signed-in members when the creator hasn't written their own members.html.
pub const DEFAULT_MEMBERS_HTML: &str = "<p>Welcome in. Your membership checked out on-chain.</p>\
<p class=\"muted small\">Creators: put your own members-only content in a file called <b>members.html</b> next to tier-card.json, then restart <code>serve</code>. It is only ever sent to visitors who proved they hold an active pass.</p>";

#[derive(Deserialize)]
struct CheckQuery {
    address: String,
}

/// One level to serve: its indexer, its shop, and where to save scan progress.
pub struct LevelSetup {
    pub ix: Indexer,
    pub shop: Option<Shop>,
    pub cache: Option<std::path::PathBuf>,
}

/// A second, independent node that must also see a pass's coin before the pass counts, so one
/// node that lies (or is broken) can't grant passes.
#[derive(Default)]
pub struct SecondNode {
    pub chain: Option<Chain>,
    /// The network, the `--cross-check` setting and the main `--node`, for reconnecting.
    pub net: String,
    pub url: Option<String>,
    /// The main node's address, which the second node must differ from.
    pub main_url: String,
}

/// Second-node check status for /v1/health: "on", "off", or "waiting" (no second node answering).
static CROSS_CHECK: std::sync::Mutex<&'static str> = std::sync::Mutex::new("off");

pub async fn serve(setups: Vec<LevelSetup>, chain: Chain, port: u16, members_html: String, telegram: Option<crate::telegram::Bot>, discord: Option<crate::discord::Discord>, club: Option<crate::clubs::Club>, home: std::path::PathBuf, second: SecondNode) -> Result<()> {
    auth::self_test()?;
    anyhow::ensure!(!setups.is_empty(), "no membership to serve");
    let chain = Arc::new(tokio::sync::Mutex::new(chain));
    if setups.iter().any(|l| l.shop.is_some()) {
        println!("Website sales are ON. Prices and the sales lane come from the chain; no private files are used.");
    }
    let mut levels = Vec::new();
    let mut scanners = Vec::new();
    for l in setups {
        let snap = Arc::new(RwLock::new(Snapshot::default()));
        levels.push(Level { card: l.ix.card.clone(), snap: snap.clone(), shop: l.shop.map(Arc::new) });
        scanners.push((l.ix, l.cache, snap));
    }
    if levels.len() > 1 {
        println!("{} levels on sale (lowest first): {}", levels.len(), levels.iter().map(|l| short_id(&l.card.covenant_id).to_string()).collect::<Vec<_>>().join(", "));
    }
    let levels = Arc::new(levels);
    let club = Arc::new(std::sync::RwLock::new(club.map(Arc::new)));
    let wallets = Arc::new(crate::clubs::Wallets::open(&home));
    let access: Arc<crate::telegram::AccessCheck> = {
        let (levels, club, wallets) = (levels.clone(), club.clone(), wallets.clone());
        Arc::new(move |address: &str| {
            let (levels, club, wallets) = (levels.clone(), club.clone(), wallets.clone());
            let address = address.to_string();
            Box::pin(async move { access_keys(&levels, &club, &wallets, &address).await })
        })
    };
    let first = levels[0].clone();
    let state = AppState {
        levels: levels.clone(),
        card: first.card.clone(),
        snap: first.snap.clone(),
        sessions: Arc::new(Mutex::new({
            let mut sessions = Sessions::default();
            let n = auth::load_handoff(&home, &mut sessions);
            if n > 0 {
                println!("Kept {n} sign-in{} across the restart.", if n == 1 { "" } else { "s" });
            }
            sessions
        })),
        members_html: Arc::new(members_html),
        chain: chain.clone(),
        shop: first.shop.clone(),
        quotes: Arc::new(Mutex::new(HashMap::new())),
        recent_buys: Arc::new(Mutex::new(HashMap::new())),
        telegram: telegram.map(Arc::new),
        discord: discord.map(Arc::new),
        club,
        wallets,
        home,
        access: access.clone(),
        names: Arc::new(crate::names::Resolver::default()),
    };
    sync_bots(&state);
    crate::bridge::set_dir(&state.home);
    if let Some(p) = promo::read_promo(&state.home) {
        promo::warm_history(&p);
    }
    members::backfill(&state);
    tokio::spawn(alerts::run(state.clone()));
    tokio::spawn(record_watch(state.clone()));
    if let Some(d) = state.discord.clone() {
        let access = access.clone();
        supervise("discord", move || {
            let (d, access) = (d.clone(), access.clone());
            async move {
                loop {
                    d.sweep(&*access).await;
                    tokio::time::sleep(crate::telegram::sweep_every()).await;
                }
            }
        });
    }
    if let Some(bot) = state.telegram.clone() {
        let (b1, a1) = (bot.clone(), access.clone());
        supervise("telegram", move || {
            let (b, a) = (b1.clone(), a1.clone());
            async move { b.run(&*a).await }
        });
        let access = access.clone();
        supervise("telegram sweep", move || {
            let (bot, access) = (bot.clone(), access.clone());
            async move {
                loop {
                    tokio::time::sleep(crate::telegram::sweep_every()).await;
                    bot.sweep(&*access).await;
                }
            }
        });
    }
    for l in levels.iter() {
        let st = state.at(Some(&l.card.covenant_id)).map_err(|e| anyhow::anyhow!(e.1))?;
        let tag = if levels.len() > 1 { format!(" {}", short_id(&l.card.covenant_id)) } else { String::new() };
        let name: &'static str = Box::leak(format!("reminders{tag}").into_boxed_str());
        let st2 = st.clone();
        supervise(name, move || remind_loop(st2.clone()));
        if is_v5(&st.card) && st.shop.is_some() {
            let name: &'static str = Box::leak(format!("price-alert{tag}").into_boxed_str());
            let st3 = st.clone();
            supervise(name, move || price_alert_loop(st3.clone()));
        }
        if is_v5(&st.card) && st.shop.is_some() {
            let name: &'static str = Box::leak(format!("pay-in{tag}").into_boxed_str());
            let st4 = st.clone();
            supervise(name, move || payin::watch_loop(st4.clone()));
        }
        if st.card.settings.v4.is_some() && st.shop.as_ref().is_some_and(|s| s.keeper.is_some()) {
            let name: &'static str = Box::leak(format!("keeper{tag}").into_boxed_str());
            supervise(name, move || keeper_loop(st.clone()));
        }
    }
    watchdog();

    // Background: follow the chain and refresh verified passes, for every level. Each level keeps
    // its own cursor; pages read from the node are shared, so extra levels cost little.
    let mut cursors = Vec::new();
    for (ix, cache, _) in scanners.iter_mut() {
        let start: kaspa_consensus_core::Hash = ix.card.start_hash.parse().map_err(|_| anyhow::anyhow!("bad start hash in tier card"))?;
        let resumed = cache.as_deref().and_then(|p| ix.load_cache(p));
        cursors.push(Some(resumed.unwrap_or(start)));
    }
    if cursors.len() == 1 && scanners[0].1.as_deref().is_some_and(|p| p.exists()) {
        println!("Continuing from the last scan (saved progress found).");
    } else {
        println!("Reading the chain; the site's member checks start once that's done.");
    }
    let cross = second.chain.is_some() || (second.url.as_deref() != Some("off") && !second.net.is_empty());
    *CROSS_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = if second.chain.is_some() { "on" } else if cross { "waiting" } else { "off" };
    let mut second = second;
    // Passes the second node has confirmed (by coin), per level, kept while it is briefly
    // unreachable. Saved to disk, so a restart doesn't hide every pass until it answers again.
    let confirmed_file = state.home.join("second-node-confirmed.json");
    let mut confirmed: HashMap<String, std::collections::HashSet<(String, u32)>> = std::fs::read_to_string(&confirmed_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if cross && !confirmed.is_empty() {
        println!("second-node pass check: {} confirmed pass(es) remembered from before the restart", confirmed.values().map(|v| v.len()).sum::<usize>());
    }
    // Try the second node right away at startup (then at most once a minute).
    let mut last_try = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(61)).unwrap_or_else(std::time::Instant::now);
    tokio::spawn(async move {
        let mut pages = PageCache::default();
        loop {
            for ((ix, cache, snap), cursor) in scanners.iter_mut().zip(cursors.iter_mut()) {
                let res: Result<()> = async {
                    // One page at a time, so purchases never wait behind a long catch-up.
                    loop {
                        let done = pages.step(ix, &chain, cursor).await?;
                        progress_now();
                        if done {
                            break;
                        }
                    }
                    let chain = chain.lock().await;
                    let now = chain.now_ms().await?;
                    let mut passes = ix.verified(&chain, now).await?;
                    let lanes = ix.verified_lanes(&chain).await?;
                    drop(chain);
                    if cross {
                        if second.chain.is_none() && last_try.elapsed() > std::time::Duration::from_secs(60) {
                            last_try = std::time::Instant::now();
                            second.chain = crate::connect_second(second.url.clone(), &second.net, &second.main_url).await.map_err(|e| eprintln!("second-node pass check: {e:#}")).ok();
                        }
                        let seen = match &second.chain {
                            Some(c) => ix.confirmed_on(c, &passes).await.map_err(|e| eprintln!("second-node pass check: {e:#} (reconnecting)")).ok(),
                            None => None,
                        };
                        match seen {
                            Some(ok) => {
                                *CROSS_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = "on";
                                let before = passes.len();
                                passes.retain(|p| ok.contains(&(p.txid.clone(), p.index)));
                                if passes.len() < before {
                                    eprintln!("second-node pass check: {} pass(es) not seen on the second node yet; holding them back", before - passes.len());
                                }
                                let mine = confirmed.entry(ix.card.covenant_id.clone()).or_default();
                                let before_set = mine.clone();
                                mine.retain(|k| passes.iter().any(|p| p.txid == k.0 && p.index == k.1));
                                mine.extend(ok.into_iter().filter(|k| passes.iter().any(|p| p.txid == k.0 && p.index == k.1)));
                                if *mine != before_set {
                                    if let Ok(t) = serde_json::to_string(&confirmed) {
                                        let tmp = confirmed_file.with_extension("json.tmp");
                                        if std::fs::write(&tmp, t).is_ok() {
                                            let _ = std::fs::rename(&tmp, &confirmed_file);
                                        }
                                    }
                                }
                            }
                            None => {
                                *CROSS_CHECK.lock().unwrap_or_else(|e| e.into_inner()) = "waiting";
                                second.chain = None;
                                // Only passes it confirmed earlier still count until it answers again.
                                let mine = confirmed.get(&ix.card.covenant_id);
                                passes.retain(|p| mine.is_some_and(|m| m.contains(&(p.txid.clone(), p.index))));
                            }
                        }
                    }
                    let mut s = snap.write().await;
                    s.passes = passes;
                    s.lanes = lanes;
                    s.events = ix.events().to_vec();
                    s.scanned_txs = ix.scanned_txs;
                    s.updated_ms = now;
                    s.synced = true;
                    drop(s);
                    progress_now();
                    if let (Some(p), Some(c)) = (cache.as_deref(), *cursor) {
                        ix.save_cache(p, c)?;
                    }
                    Ok(())
                }
                .await;
                if let Err(e) = res {
                    eprintln!("indexer {}: {e} (retrying)", short_id(&ix.card.covenant_id));
                }
            }
            pages.clear();
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    let app = Router::new()
        .route("/", get(home::root))
        .route("/demo", get(home::demo))
        .route("/demo/:id", get(examples::page))
        .route("/creators", get(home::creators))
        .route("/v1/contact", post(home::contact))
        .route("/v1/creator/contact", get(home::creator_notes))
        .route("/v1/creator/contact/delete", post(home::creator_note_delete))
        .route("/assets/*name", get(asset))
        .route("/v1/nonce", get(nonce))
        .route("/v1/login", post(login))
        .route("/v1/members", get(members))
        .route("/v1/offer", get(offer))
        .route("/v1/buy/prepare", post(buy_prepare))
        .route("/v1/buy/submit", post(buy_submit))
        .route("/v1/pass/prepare", post(pass_prepare))
        .route("/v1/submit", post(buy_submit))
        .route("/dashboard", get(dashboard))
        .route("/club", get(club_page))
        .route("/help", get(help_page))
        .route("/v1/creator", get(creator_overview))
        .route("/v1/creator/prepare", post(creator_prepare))
        .route("/v1/launch/prepare", post(launch_prepare))
        .route("/v1/health", get(health))
        .route("/v1/tier", get(tier))
        .route("/v1/passes", get(passes))
        .route("/v1/check", get(check))
        .route("/v1/member", get(member))
        .route("/v1/telegram", get(telegram_info))
        .route("/v1/telegram/link", post(telegram_link))
        .route("/v1/creator/telegram", get(creator_telegram))
        .route("/v1/discord", get(discord_info))
        .route("/v1/discord/link", post(discord_link))
        .route("/v1/discord/callback", get(discord_callback))
        .route("/v1/creator/discord", get(creator_discord))
        .route("/v1/club", get(club_info))
        .route("/v1/club/check", get(club_check))
        .route("/v1/club/me", get(club_me))
        .route("/v1/wallets/link", post(wallet_link))
        .route("/v1/wallets/unlink", post(wallet_unlink))
        .route("/v1/creator/club", get(creator_club).post(creator_club_upload))
        .route("/v1/creator/sale", post(creator_sale))
        .route("/v1/creator/names", get(creator_names))
        .route("/v1/holder", get(holder_info))
        .route("/v1/creator/cards", get(creator_cards))
        .route("/v1/creator/name", post(creator_name))
        .route("/v1/creator/display-name", post(creator_display_name))
        .route("/v1/creator/pass-word", post(creator_pass_word))
        .route("/v1/names", get(names_info))
        .route("/v1/creator/telegram/leave", post(creator_telegram_leave))
        .route("/v1/creator/switch", post(creator_switch))
        .route("/v1/creator/discount", post(creator_discount))
        .route("/v1/levels", get(levels_info))
        .route("/v1/addresses", get(addresses))
        .route("/v1/promo", get(promo::info))
        .route("/v1/promo/check", post(promo::check_me))
        .route("/v1/promo/claim", post(promo::claim))
        .route("/v1/creator/promo", get(promo::creator_get).post(promo::creator_set))
        .route("/v1/creator/promo/end", post(promo::creator_end))
        .route("/v1/creator/member-note", post(members::creator_note))
        .route("/v1/creator/alerts", get(alerts::creator_get).post(alerts::creator_set))
        .route("/v1/creator/alerts/test", post(alerts::creator_test))
        .route("/v1/creator/visibility", get(home::visibility_get).post(home::visibility_set))
        .route("/v1/quote", get(quote))
        .route("/v1/payin/order", get(payin::status).post(payin::create))
        .route("/v1/logout", post(logout))
        .route("/v1/trial", get(trial::info))
        .route("/v1/trial/prepare", post(trial::prepare))
        .route("/v1/creator/perks", post(creator_perks))
        .route("/v1/creator/site-names", post(creator_site_names))
        .route("/v1/page", get(page_info))
        .route("/v1/creator/page", post(creator_page))
        .route("/v1/creator/about", post(creator_about))
        .route("/v1/creator/levels", post(creator_levels))
        .route("/v1/creator/record", get(creator_record).post(creator_record_prepare))
        .route("/v1/creator/withdraw", post(creator_withdraw_prepare))
        .route("/v1/creator/payments.csv", get(creator_payments_csv))
        .route("/v1/creator/server-key", get(server_key_status).post(server_key_new))
        .route("/v1/creator/server-key/prepare", post(server_key_prepare))
        .route("/v1/creator/level-price", post(creator_level_price))
        .route("/v1/handover/prepare", post(handover_prepare));
    // The simulator's stand-in phone wallet exists only in the simulator.
    let app = if crate::test_switch("KASPASS_SIM_SERVE").is_some() { app.route("/v1/sim/pay", post(payin::sim_pay)) } else { app };
    let app = brand::routes(app)
        // Pages send the session in a header (never in a link); handlers still read `session`.
        .layer(axum::middleware::from_fn(session_header))
        // Any website may read the public API (GET); nothing cross-site may write.
        .layer(tower_http::cors::CorsLayer::new().allow_origin(tower_http::cors::Any).allow_methods([axum::http::Method::GET]))
        .layer(axum::middleware::from_fn_with_state(state.clone(), examples::layer))
        .layer(axum::middleware::from_fn(guard))
        .with_state(state);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => anyhow::bail!(
            "port {port} is already in use (another KasPass site may still be running). Stop it with Ctrl+C, or pick another port: serve --port {}",
            port + 1
        ),
        Err(e) => return Err(e.into()),
    };
    println!("KasPass website on http://{addr}/   (API under /v1, Ctrl+C to stop)");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Pages of blocks read from the node in this pass, shared by every level's indexer.
#[derive(Default)]
struct PageCache {
    pages: HashMap<Option<kaspa_consensus_core::Hash>, Arc<(Vec<(kaspa_consensus_core::tx::Transaction, i64)>, Option<kaspa_consensus_core::Hash>, bool)>>,
}

impl PageCache {
    /// Like `scan_step`, but a page another level already read in this pass isn't fetched again.
    async fn step(&mut self, ix: &mut Indexer, chain: &tokio::sync::Mutex<Chain>, cursor: &mut Option<kaspa_consensus_core::Hash>) -> Result<bool> {
        let page = match self.pages.get(cursor) {
            Some(p) => p.clone(),
            None => {
                let p = Arc::new(chain.lock().await.blocks_page(*cursor).await?);
                // Only the last few pages are kept: enough for levels that are in step.
                if self.pages.len() >= 64 {
                    self.pages.clear();
                }
                self.pages.insert(*cursor, p.clone());
                p
            }
        };
        for (tx, t) in &page.0 {
            ix.process_at(tx, *t);
        }
        let advanced = page.1 != *cursor;
        *cursor = page.1;
        Ok(page.2 || !advanced)
    }

    fn clear(&mut self) {
        self.pages.clear();
    }
}

async fn health(State(s): State<AppState>) -> Json<serde_json::Value> {
    let mut synced = true;
    for l in s.levels.iter() {
        synced &= l.snap.read().await.synced;
    }
    let snap = s.snap.read().await;
    Json(serde_json::json!({
        "ok": true,
        "synced": synced,
        "levels": s.levels.len(),
        "covenant_id": s.card.covenant_id,
        "scanned_txs": snap.scanned_txs,
        "live_passes": snap.passes.len(),
        "updated_ms": snap.updated_ms,
        // How long ago the site last read the chain successfully (the watchdog restarts it after 10 minutes).
        "last_read_s_ago": (wall_ms() - LAST_PROGRESS.load(std::sync::atomic::Ordering::Relaxed)).max(0) / 1000,
        "network": s.card.network,
        "second_node_check": *CROSS_CHECK.lock().unwrap_or_else(|e| e.into_inner()),
        "empty": s.card.placeholder,
    }))
}

async fn tier(State(s): State<AppState>, Query(lq): Query<LevelQ>) -> Result<Json<TierCard>, ApiError> {
    let s = s.lv(&lq)?;
    let mut card = s.card.clone();
    // After a handover the dashboard belongs to the record's new owner.
    card.creator_address = site_owner(&s);
    Ok(Json(card))
}

// ---------------------------------------------------------------------------
// v5: owner record, payout vault, handover

#[derive(Deserialize)]
struct RecordRequest {
    session: String,
    #[serde(default)]
    recovery: String,
    #[serde(default)]
    new_owner: String,
    #[serde(default)]
    new_recovery: String,
}

#[derive(Deserialize)]
struct LevelPriceRequest {
    session: String,
    currency: String,
    amount: f64,
    #[serde(default)]
    founding_amount: Option<f64>,
    #[serde(default)]
    founding_until_ms: Option<i64>,
    /// Lifetime and commitment-term prices (only where the plan was launched with them).
    #[serde(default)]
    lifetime_amount: Option<f64>,
    #[serde(default)]
    term_amount: Option<f64>,
}

/// v5: the creator changes a level's price in its currency (no transaction: the chain only holds
/// the KAS ceiling). Passes renew at the new price from their next renewal.
async fn creator_level_price(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<LevelPriceRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    creator_of(&s, &req.session)?;
    if !is_v5(&s.card) {
        return Err(bad("this membership's price is in KAS; change it with the KAS prices"));
    }
    if !crate::prices::PLAN_CURRENCIES.contains(&req.currency.as_str()) {
        return Err(bad(format!("currency must be one of {}", crate::prices::PLAN_CURRENCIES.join(", "))));
    }
    if !(0.5..=1e7).contains(&req.amount) {
        return Err(bad("the price must be between 0.50 and 10,000,000"));
    }
    let lane_now = current_lane(&s).await?.lane;
    let old = crate::prices::level_prices(&s.home).get(&s.card.covenant_id).cloned();
    // A plan priced in KAS keeps its currency (its on-chain prices are the real prices).
    if let Some(o) = &old {
        if (o.currency == "KAS") != (req.currency == "KAS") {
            return Err(bad("a plan priced in KAS stays in KAS, and a plan priced in a currency stays in a currency; launch a new level to switch"));
        }
    }
    for (what, amount, offered) in [("lifetime", req.lifetime_amount, lane_now.life > 0), ("commitment", req.term_amount, lane_now.term_price > 0)] {
        if let Some(a) = amount {
            if !offered {
                return Err(bad(format!("this plan was launched without a {what} price; launch a new level to offer one")));
            }
            if !(0.5..=1e8).contains(&a) {
                return Err(bad(format!("the {what} price must be between 0.50 and 100,000,000")));
            }
        }
    }
    let founding = match (req.founding_amount, req.founding_until_ms) {
        (Some(a), Some(u)) if a > 0.0 => {
            if a >= req.amount {
                return Err(bad("the founding price must be lower than the regular price"));
            }
            Some(crate::prices::Founding { amount: a, until_ms: u })
        }
        _ => None,
    };
    let mut all = crate::prices::level_prices(&s.home);
    let lifetime = req.lifetime_amount.or(old.as_ref().and_then(|o| o.lifetime));
    let term = req.term_amount.or(old.as_ref().and_then(|o| o.term));
    all.insert(s.card.covenant_id.clone(), crate::prices::LevelPrice { currency: req.currency.clone(), amount: req.amount, founding, lifetime, term });
    crate::prices::save_level_prices(&s.home, &all).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Above the ceiling, buyers pay the ceiling: say so.
    let ceiling = current_lane(&s).await.map(|l| l.lane.price as f64 / crate::SOMPI as f64).unwrap_or(0.0);
    let warning = crate::prices::fresh().and_then(|r| r.kas(&req.currency, req.amount)).filter(|k| *k > ceiling).map(|k| {
        format!("At today's rate that's {k:.2} KAS, above this plan's ceiling of {ceiling:.2} KAS, so buyers would pay {ceiling:.2} KAS. Launch a new level with a higher ceiling if you need this price.")
    });
    Ok(Json(serde_json::json!({ "saved": true, "warning": warning })))
}

/// The site's owner record and payout vault, for its owner (or the recovery wallet).
async fn creator_record(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&q.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let shop = shop_of(&s)?;
    let rec = shop.record();
    let is_recovery = rec.as_ref().is_some_and(|r| r.recovery == me);
    if me != site_owner(&s) && !is_recovery {
        return Err((StatusCode::FORBIDDEN, "this is for the site's owner".into()));
    }
    let Some(rec) = rec else {
        return Ok(Json(serde_json::json!({ "record": null, "role": "owner" })));
    };
    let vault = rec.vault_address().map_err(bad)?;
    let addr: kaspa_addresses::Address = vault.as_str().try_into().map_err(|_| bad("bad vault address"))?;
    let coins = s.chain.lock().await.utxos_at(&kaspa_txscript::pay_to_address_script(&addr)).await.map_err(bad)?;
    let balance: u64 = coins.iter().map(|u| u.entry.amount).sum();
    Ok(Json(serde_json::json!({ "record": rec, "vault_address": vault, "vault_balance_sompi": balance, "vault_payments": coins.len(), "role": if is_recovery { "recovery" } else { "owner" } })))
}

async fn creator_record_prepare(State(s): State<AppState>, Json(req): Json<RecordRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &req.session)?;
    let shop = shop_of(&s)?;
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        shop.prepare_record(&chain, &me, &req.recovery).await.map_err(bad)?
    };
    out["quote"] = serde_json::json!(store_quote(&s, quote));
    Ok(Json(out))
}

async fn creator_withdraw_prepare(State(s): State<AppState>, Json(req): Json<RecordRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &req.session)?;
    let shop = shop_of(&s)?;
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        shop.prepare_vault_withdraw(&chain, &me).await.map_err(bad)?
    };
    out["quote"] = serde_json::json!(store_quote(&s, quote));
    Ok(Json(out))
}

/// v5 key swap status: the key plans and passes should trust, and how many still trust an older one.
async fn server_key_status(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    Ok(Json(key_status(&s).await))
}

async fn key_status(s: &AppState) -> serde_json::Value {
    let (mut lanes, mut passes, mut v5) = (0, 0, false);
    let mut key = None;
    for l in s.levels.iter() {
        let Some(shop) = &l.shop else { continue };
        if !shop.v4().is_some_and(|v| v.record_id.is_some()) {
            continue;
        }
        v5 = true;
        key = shop.v5_key().map(|w| w.id.to_string());
        let snap = l.snap.read().await;
        let (a, b) = shop.rekey_left(&snap.lanes, &snap.passes);
        lanes += a;
        passes += b;
    }
    let replaced = s.shop.as_ref().is_some_and(|sh| !sh.voucher_ring.read().unwrap_or_else(|e| e.into_inner()).is_empty());
    serde_json::json!({
        "v5": v5,
        "alerts": price_alerts(&s.home),
        "key": key.map(|k| k[..12].to_string()),
        "replaced": replaced,
        "lanes_left": lanes,
        "passes_left": passes,
    })
}

/// v5: makes a new server (voucher) key. New plans trust it at once; existing plans and passes
/// move over with "Move to the new key" batches, each approved by the owner.
async fn server_key_new(State(s): State<AppState>, Json(req): Json<RecordRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &req.session)?;
    let shop = shop_of(&s)?;
    if shop.record().map(|r| r.owner) != Some(me) {
        return Err((StatusCode::FORBIDDEN, "only the owner record's current owner can replace the server key".into()));
    }
    let st = key_status(&s).await;
    if st["lanes_left"].as_u64().unwrap_or(0) + st["passes_left"].as_u64().unwrap_or(0) > 0 {
        return Err((StatusCode::CONFLICT, "a key swap is already under way: move the remaining plans and passes first".into()));
    }
    let w = shop.new_v5_key().map_err(bad)?;
    eprintln!("Server key replaced: new voucher key {}… (plans and passes move over as the owner approves).", &w.id.to_string()[..12]);
    Ok(Json(key_status(&s).await))
}

/// v5: one approval's worth of plans and passes moved to the newest server key.
async fn server_key_prepare(State(s): State<AppState>, Json(req): Json<RecordRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &req.session)?;
    let shop = shop_of(&s)?;
    let mut levels = Vec::new();
    for l in s.levels.iter() {
        if let Some(sh) = &l.shop {
            let snap = l.snap.read().await;
            levels.push((sh.clone(), snap.lanes.clone(), snap.passes.clone()));
        }
    }
    let refs: Vec<(&crate::shop::Shop, Vec<FoundLane>, Vec<FoundPass>)> = levels.iter().map(|(sh, l, p)| (sh.as_ref(), l.clone(), p.clone())).collect();
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        shop.prepare_rekey(&chain, &me, &refs).await.map_err(bad)?
    };
    out["quote"] = serde_json::json!(store_quote(&s, quote));
    Ok(Json(out))
}

/// The recovery wallet hands the site to a new owner (signed in as the recovery wallet).
async fn handover_prepare(State(s): State<AppState>, Json(req): Json<RecordRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&req.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let shop = shop_of(&s)?;
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        shop.prepare_handover(&chain, &me, &req.new_owner, &req.new_recovery).await.map_err(bad)?
    };
    out["quote"] = serde_json::json!(store_quote(&s, quote));
    Ok(Json(out))
}

async fn passes(State(s): State<AppState>, Query(lq): Query<LevelQ>) -> Result<Json<Vec<FoundPass>>, ApiError> {
    Ok(Json(s.lv(&lq)?.snap.read().await.passes.clone()))
}

/// Public: does this address hold an active pass? With `?level=`, on that level; otherwise on
/// any level (each pass then says which level it's on, and `levels` lists them all).
async fn check(State(s): State<AppState>, Query(q): Query<CheckQuery>, Query(lq): Query<LevelQ>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let owner = owner_hash_of(&q.address).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?.to_string();
    let picked: Vec<usize> = match lq.key() {
        Some(_) => vec![s.lv(&lq)?.rank()],
        None => (0..s.levels.len()).collect(),
    };
    let mut passes = Vec::new();
    let mut levels = Vec::new();
    let mut now_ms = 0;
    let ar = access_ranks(&s.home, &s.levels);
    for i in picked {
        let l = &s.levels[i];
        let snap = l.snap.read().await;
        if !snap.synced {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "indexer is still catching up".into()));
        }
        now_ms = now_ms.max(snap.updated_ms);
        let name = level_name(&s.home, &s.levels, i);
        let mine: Vec<&FoundPass> = snap.passes.iter().filter(|p| p.owner_hash == owner).collect();
        levels.push(serde_json::json!({ "level": l.card.covenant_id, "name": name, "rank": i, "access_rank": ar[i], "active": mine.iter().any(|p| p.active) }));
        for p in mine {
            let mut v = serde_json::to_value(p).unwrap_or_default();
            v["level"] = serde_json::json!(l.card.covenant_id);
            v["level_name"] = serde_json::json!(name);
            v["rank"] = serde_json::json!(i);
            v["access_rank"] = serde_json::json!(ar[i]);
            v["period_ms"] = serde_json::json!(l.card.settings.period_ms);
            // auto_renew: "on" (prepaid balance), "off", "waiting_for_price" (v5, no fresh price) or
            // "not_enough_balance" (due, but the balance is less than the renewal's price; then
            // `renewal_due_sompi` is that price). A pass that ran out while waiting keeps
            // lapsed_reason "no_price" or "not_enough_balance" until it's renewed.
            let short = short_of_balance(&p.txid, p.index).filter(|due| *due > p.balance_sompi);
            if let Some(due) = short {
                v["renewal_due_sompi"] = serde_json::json!(due);
            }
            v["auto_renew"] = serde_json::json!(if waiting_for_price(&p.txid, p.index) {
                "waiting_for_price"
            } else if short.is_some() && p.balance_sompi > 0 {
                "not_enough_balance"
            } else if p.balance_sompi > 0 && !p.lifetime {
                "on"
            } else {
                "off"
            });
            if !p.active && lapsed_no_price(&s.home, &p.txid, p.index) {
                v["lapsed_reason"] = serde_json::json!("no_price");
            } else if !p.active && lapsed_in(&s.home, LAPSED_LOW_FILE, &p.txid, p.index) {
                v["lapsed_reason"] = serde_json::json!("not_enough_balance");
            }
            passes.push(v);
        }
    }
    let active = levels.iter().any(|l| l["active"] == serde_json::json!(true));
    Ok(Json(serde_json::json!({ "address": q.address, "active": active, "passes": passes, "levels": levels, "now_ms": now_ms })))
}

/// Plans that give the same access share an `access_rank` (0 = lowest). A plan listed in
/// levels.json's `same_access` gives the same access as the plan right above it, e.g. a monthly
/// plan under the yearly one. With nothing listed, `access_rank` is the same as `rank`.
fn access_ranks(home: &std::path::Path, levels: &[Level]) -> Vec<usize> {
    let same = read_same_access(home);
    let mut out = Vec::with_capacity(levels.len());
    let mut g = 0;
    for (i, l) in levels.iter().enumerate() {
        out.push(g);
        if !(i + 1 < levels.len() && same.contains(&l.card.covenant_id)) {
            g += 1;
        }
    }
    out
}

fn read_same_access(home: &std::path::Path) -> std::collections::HashSet<String> {
    let v: serde_json::Value = std::fs::read_to_string(home.join(LEVELS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    v["same_access"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// The payout vault a level's payments go to (v5), if it has one.
fn vault_of(s: &AppState) -> Option<String> {
    s.shop.as_ref()?.record()?.vault_address().ok()
}

/// Public, read-only: a wallet's memberships for a partner's "Membership" page. Everything in
/// `/v1/check`, plus per pass when it renews, what a renewal costs, how many are prepaid, and the
/// wallet's past payments (times, kinds, KAS and txids, all on chain; no currency amounts).
async fn member(State(s): State<AppState>, headers: HeaderMap, Query(q): Query<CheckQuery>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let owner = owner_hash_of(&q.address).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?.to_string();
    let Json(mut out) = check(State(s.clone()), Query(CheckQuery { address: q.address.clone() }), Query(LevelQ { level: None })).await?;
    let prices = crate::prices::level_prices(&s.home);
    let rates = crate::prices::fresh();
    let notes = crate::prices::payments_log(&s.home);
    if let Some(passes) = out["passes"].as_array_mut() {
        for v in passes.iter_mut() {
            let cov = v["level"].as_str().unwrap_or_default().to_string();
            let Some(l) = s.levels.iter().find(|l| l.card.covenant_id == cov) else { continue };
            let st = &l.card.settings;
            let lifetime = v["lifetime"].as_bool().unwrap_or(false);
            let expiry = v["expiry_ms"].as_i64().unwrap_or(0);
            let rate = v["rate_sompi"].as_u64().unwrap_or(0);
            let balance = v["balance_sompi"].as_u64().unwrap_or(0);
            if lifetime {
                v["renewal"] = serde_json::Value::Null;
                v["periods_prepaid"] = serde_json::Value::Null;
                continue;
            }
            v["renew_opens_ms"] = serde_json::json!(expiry - st.max_ahead_ms);
            v["renews_from_ms"] = match &st.v4 {
                Some(v4) => serde_json::json!(expiry - v4.auto_window_ms),
                None => serde_json::Value::Null,
            };
            // What a renewal charges today: the plan's price at today's rate, never above the
            // pass's KAS ceiling; a plan priced in KAS charges the pass's rate.
            let (currency, amount, kas_sompi) = match prices.get(&cov) {
                Some(p) => {
                    let k = rates.as_ref().and_then(|r| r.kas(&p.currency, p.amount)).map(|k| ((k * crate::SOMPI as f64).round() as u64).min(if rate > 0 { rate } else { u64::MAX }));
                    (p.currency.clone(), p.amount, k)
                }
                None => ("KAS".to_string(), rate as f64 / crate::SOMPI as f64, Some(rate)),
            };
            v["renewal"] = serde_json::json!({
                "currency": currency,
                "amount": amount,
                "kas_today": kas_sompi.map(|k| (k as f64 / crate::SOMPI as f64 * 100.0).round() / 100.0),
            });
            v["periods_prepaid"] = match kas_sompi {
                Some(k) if k > 0 => serde_json::json!(balance / k),
                _ => serde_json::Value::Null,
            };
        }
    }
    // Past payments into each plan by this wallet, oldest first.
    let mut payments: Vec<(i64, serde_json::Value)> = Vec::new();
    let promos: HashMap<String, promo::Claim> = promo::read_claims(&s.home).unwrap_or_default().into_iter().map(|c| (c.txid.clone(), c)).collect();
    for (i, l) in s.levels.iter().enumerate() {
        let name = level_name(&s.home, &s.levels, i);
        let snap = l.snap.read().await;
        for e in snap.events.iter().filter(|e| e.owner_hash == owner && e.amount_sompi > 0) {
            let base = match e.kind.as_str() {
                "monthly" => "purchase",
                "renew" => "renewal",
                "autorenew" => "auto_renewal",
                "lifetime" => "lifetime",
                "upgrade" => "upgrade",
                _ => continue,
            };
            let kind = notes.get(&e.txid).map(|n| n.kind.clone()).unwrap_or_else(|| base.to_string());
            payments.push((e.time_ms, serde_json::json!({
                "time_ms": if e.time_ms > 0 { serde_json::json!(e.time_ms) } else { serde_json::Value::Null },
                "kind": kind,
                "kas": e.amount_sompi as f64 / crate::SOMPI as f64,
                "txid": e.txid,
                "level": l.card.covenant_id,
                "level_name": name,
                // A promotion's free pass: which campaign, and its line.
                "promo": promos.get(&e.txid).map(|c| serde_json::json!({ "campaign": c.campaign, "label": c.label })),
            })));
        }
    }
    payments.sort_by_key(|(t, _)| *t);
    out["payments"] = serde_json::json!(payments.into_iter().map(|(_, v)| v).collect::<Vec<_>>());
    // QR payments: each one-time pay-in address this wallet was given, and what became of it.
    let me = signed_in_as(&s, &headers, q.address.trim()).is_ok();
    out["payin_orders"] = serde_json::json!(payin::for_buyer(&s, q.address.trim(), me));
    Ok(Json(out))
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn fill_site(s: &AppState) -> String {
    let first = s.levels.first().map(|l| &l.card).unwrap_or(&s.card);
    let w = pass_word(&s.home, &first.covenant_id);
    let cap = |t: &str| { let mut c = t.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() };
    let a = if w.one.starts_with(['a', 'e', 'i', 'o', 'u']) { format!("an {}", w.one) } else { format!("a {}", w.one) };
    let club = site_names(&s.home).club_name;
    let creator = creator_display(&s.home, &first.creator_address);
    // The heading: the club's name, else the creator's chosen name. Without either, the script
    // fills in the creator's .k/.kas name, and the heading stays empty until then.
    let (heading, title) = if !club.is_empty() {
        (club.clone(), club.clone())
    } else if let Some(n) = &creator {
        (n.clone(), format!("{n} · {}", cap(&w.one)))
    } else {
        (String::new(), cap(&w.one))
    };
    let b = brand::load(&s.home);
    let mut html = SITE.to_string();
    // The creator's look: name and logo in the header, the way back to their website, banner,
    // photo, and fresh styles whenever the look changes.
    let shown = if heading.is_empty() { String::new() } else { heading.clone() };
    html = html.replacen(r#"<span id="brandName"></span>"#, &format!(r#"<span id="brandName">{}</span>"#, esc(&shown)), 1);
    if let Some(u) = brand::picture_url(&b, "logo") {
        html = html.replacen(r#"<img id="brandLogo" class="hidden" alt="">"#, &format!(r#"<img id="brandLogo" src="{u}" alt="">"#), 1);
        html = html.replacen("<link rel=\"stylesheet\" href=\"/brand/fonts.css", &format!("<link rel=\"icon\" href=\"{u}\">\n<link rel=\"stylesheet\" href=\"/brand/fonts.css"), 1);
    }
    let back = if !b.back_url.is_empty() { Some((b.back_url.clone(), brand::back_host(&b.back_url).unwrap_or_default())) } else {
        return_origins().into_iter().next().map(|o| { let h = o.split("://").nth(1).unwrap_or(&o).trim_end_matches('/').trim_start_matches("www.").to_string(); (o, h) })
    };
    if let Some((url, host)) = back.filter(|(_, h)| !h.is_empty()) {
        for id in ["brandBack", "menuBack"] {
            let arrow = if id == "brandBack" { "← " } else { "" };
            html = html.replacen(&format!(r##"<a href="#" id="{id}" class="hidden">{arrow}Back to <span class="bh"></span></a>"##), &format!(r#"<a href="{}" id="{id}">{arrow}Back to <span class="bh">{}</span></a>"#, esc(&url), esc(&host)), 1);
        }
    }
    if b.light.is_some() && b.dark.is_some() {
        let start = if b.start.is_empty() { "light" } else { b.start.as_str() };
        html = html.replacen(r#"<button id="themeToggle" class="themetoggle hidden""#, &format!(r#"<button id="themeToggle" class="themetoggle" data-start="{start}""#), 1);
    }
    if let Some(u) = brand::picture_url(&b, "banner") {
        html = html.replacen(r#"<div class="brandbanner hidden" id="brandBanner"><img id="brandBannerImg" alt=""></div>"#, &format!(r#"<div class="brandbanner" id="brandBanner"><img id="brandBannerImg" src="{u}" alt=""></div>"#), 1);
    }
    if let Some(u) = brand::picture_url(&b, "photo") {
        html = html.replacen(r#"<img id="brandPhoto" class="avatar hidden" alt="">"#, &format!(r#"<img id="brandPhoto" class="avatar" src="{u}" alt="">"#), 1);
    }
    html = html.replacen("/brand/fonts.css?v=0", &format!("/brand/fonts.css?v={}", b.version), 1).replacen("/brand/theme.css?v=0", &format!("/brand/theme.css?v={}", b.version), 1);
    html.replacen("<title>Membership</title>", &format!("<title>{}</title>", esc(&title)), 1)
        .replacen(r#"<h1 class="page" id="pageTitle">Membership</h1>"#, &format!(r#"<h1 class="page" id="pageTitle">{}</h1>"#, esc(&heading)), 1)
        .replacen(r#"aria-current="page">Membership</a>"#, &format!(r#"aria-current="page">{}</a>"#, esc(&cap(&w.one))), 1)
        .replace(r#"data-pw="">membership<"#, &format!(r#"data-pw="">{}<"#, esc(&w.one)))
        .replace(r#"data-pw="C">Membership<"#, &format!(r#"data-pw="C">{}<"#, esc(&cap(&w.one))))
        .replace(r#"data-pw="a">a membership<"#, &format!(r#"data-pw="a">{}<"#, esc(&a)))
        .replace(r#"data-pw="s">memberships<"#, &format!(r#"data-pw="s">{}<"#, esc(&w.many)))
}

/// Scripts, styles and fonts for the pages, all built into the program.
const ASSETS: &[(&str, &str, &[u8])] = &[
    ("index.js", "text/javascript; charset=utf-8", include_bytes!("../web/index.js")),
    // QR codes for paying from any wallet: qrcode-generator 1.4.4 (Kazuhiko Arase, MIT).
    ("qrcode.js", "text/javascript; charset=utf-8", include_bytes!("../web/qrcode.js")),
    ("theme-boot.js", "text/javascript; charset=utf-8", include_bytes!("../web/theme-boot.js")),
    // Kaspire mobile over WalletConnect: @walletconnect/sign-client 2.21.8 (WalletConnect, Apache-2.0; the last release under that license),
    // bundled with esbuild. Loaded only when someone presses "Use Kaspire mobile".
    ("walletconnect.js", "text/javascript; charset=utf-8", include_bytes!("../web/walletconnect.js")),
    ("dashboard.js", "text/javascript; charset=utf-8", include_bytes!("../web/dashboard.js")),
    ("club.js", "text/javascript; charset=utf-8", include_bytes!("../web/club.js")),
    ("help.js", "text/javascript; charset=utf-8", include_bytes!("../web/help.js")),
    ("fonts.css", "text/css; charset=utf-8", include_bytes!("../web/fonts.css")),
    // KasPass's own website (kaspass.app).
    // Kaspire mobile (WalletConnect) for the creator dashboard.
    ("kaspire-mobile.js", "text/javascript; charset=utf-8", include_bytes!("../web/kaspire-mobile.js")),
    ("home.css", "text/css; charset=utf-8", include_bytes!("../web/home.css")),
    ("home.js", "text/javascript; charset=utf-8", include_bytes!("../web/home.js")),
    ("favicon.svg", "image/svg+xml", include_bytes!("../web/favicon.svg")),
    ("fonts/inter-latin-400-normal.woff2", "font/woff2", include_bytes!("../web/fonts/inter-latin-400-normal.woff2")),
    ("fonts/inter-latin-500-normal.woff2", "font/woff2", include_bytes!("../web/fonts/inter-latin-500-normal.woff2")),
    ("fonts/inter-latin-600-normal.woff2", "font/woff2", include_bytes!("../web/fonts/inter-latin-600-normal.woff2")),
    ("fonts/inter-latin-700-normal.woff2", "font/woff2", include_bytes!("../web/fonts/inter-latin-700-normal.woff2")),
    ("fonts/montserrat-latin-600-normal.woff2", "font/woff2", include_bytes!("../web/fonts/montserrat-latin-600-normal.woff2")),
    ("fonts/montserrat-latin-800-normal.woff2", "font/woff2", include_bytes!("../web/fonts/montserrat-latin-800-normal.woff2")),
];

async fn asset(axum::extract::Path(name): axum::extract::Path<String>) -> axum::response::Response {
    match ASSETS.iter().find(|(n, _, _)| *n == name) {
        Some((n, ty, body)) => {
            // Fonts never change; scripts and styles follow the program, so they're re-checked.
            let cache = if n.starts_with("fonts/") { "public, max-age=31536000, immutable" } else { "no-cache" };
            ([(header::CONTENT_TYPE, *ty), (header::CACHE_CONTROL, cache)], *body).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

// ---------------------------------------------------------------------------
// Security: headers, same-site writes, rate limits

/// Only this site's own files may run or load; nothing from other sites, and no inline scripts.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self' wss://relay.walletconnect.org https://verify.walletconnect.org https://verify.walletconnect.com; frame-src https://verify.walletconnect.org https://verify.walletconnect.com; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

static CSP_MODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `on` (default), `report` (browser only warns in its console), or `off`.
pub fn set_csp_mode(mode: &str) -> anyhow::Result<()> {
    anyhow::ensure!(matches!(mode, "on" | "report" | "off"), "--csp must be on, report or off");
    let _ = CSP_MODE.set(mode.to_string());
    Ok(())
}

static CREATOR_ONLY_LAUNCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Simulator only: allow /v1/sim/pay (a stand-in phone wallet for tests).
pub fn set_sim_pay() {
    payin::SIM_PAY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// With --creator-only-launch, only the site's creator wallet can prepare a new level.
pub fn set_creator_only_launch(on: bool) {
    CREATOR_ONLY_LAUNCH.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The visitor's address as Caddy saw it. Caddy replaces any X-Forwarded-For a visitor sends with
/// the real one, so it can be trusted here; requests without it come from the server itself.
pub(crate) fn client_ip(h: &HeaderMap) -> Option<String> {
    let v = h.get("x-forwarded-for")?.to_str().ok()?;
    let first = v.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }
    // One IPv6 home or server gets a whole /64, so that's what counts as one visitor.
    match first.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(a)) if a.to_ipv4_mapped().is_none() => {
            let s = a.segments();
            Some(format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3]))
        }
        _ => Some(first.to_string()),
    }
}

/// Is this POST from one of this site's own pages? Browsers always send Origin on a POST, so a
/// write from another website is refused. Tools without an Origin (curl, the CLI) are allowed;
/// they can't carry a visitor's sign-in anyway.
fn same_site(h: &HeaderMap) -> bool {
    let Some(origin) = h.get(header::ORIGIN) else { return true };
    let Ok(origin) = origin.to_str() else { return false };
    let host = h.get("x-forwarded-host").or_else(|| h.get(header::HOST)).and_then(|v| v.to_str().ok()).unwrap_or("");
    let bare = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://"));
    matches!(bare, Some(b) if !host.is_empty() && b.eq_ignore_ascii_case(host))
}

/// Per-visitor token buckets: (tokens left, last refill).
#[derive(Default)]
struct Buckets {
    reads: HashMap<String, (f64, std::time::Instant)>,
    writes: HashMap<String, (f64, std::time::Instant)>,
}

static BUCKETS: std::sync::LazyLock<Mutex<Buckets>> = std::sync::LazyLock::new(|| Mutex::new(Buckets::default()));

/// Reads: bursts of 240, then 4 a second. Writes (sign-ins, purchases, dashboard saves): bursts of
/// 30, then one every 2 seconds. Ordinary use never comes close; a script hammering the site does.
fn allow(ip: &str, write: bool) -> bool {
    let (cap, per_s) = if write { (30.0, 0.5) } else { (240.0, 4.0) };
    let now = std::time::Instant::now();
    let mut b = BUCKETS.lock().unwrap_or_else(|e| e.into_inner());
    let map = if write { &mut b.writes } else { &mut b.reads };
    if map.len() > 20_000 {
        // Forget visitors whose bucket has refilled anyway.
        map.retain(|_, (t, at)| *t + now.duration_since(*at).as_secs_f64() * per_s < cap);
    }
    let e = map.entry(ip.to_string()).or_insert((cap, now));
    e.0 = (e.0 + now.duration_since(e.1).as_secs_f64() * per_s).min(cap);
    e.1 = now;
    if e.0 >= 1.0 {
        e.0 -= 1.0;
        true
    } else {
        false
    }
}

async fn guard(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::Method;
    let write = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) || req.uri().path() == "/v1/nonce";
    if write && *req.method() != Method::OPTIONS && !same_site(req.headers()) {
        return (StatusCode::FORBIDDEN, "requests that change something must come from this site's own pages").into_response();
    }
    if let Some(ip) = client_ip(req.headers()) {
        if !allow(&ip, write) {
            return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "10")], "too many requests; please wait a few seconds").into_response();
        }
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    match CSP_MODE.get().map(String::as_str).unwrap_or("on") {
        // A stricter policy a handler set itself (creator pictures) stays.
        "on" if !h.contains_key(header::CONTENT_SECURITY_POLICY) => {
            h.insert(header::CONTENT_SECURITY_POLICY, axum::http::HeaderValue::from_static(CSP));
        }
        "report" => {
            h.insert(header::CONTENT_SECURITY_POLICY_REPORT_ONLY, axum::http::HeaderValue::from_static(CSP));
        }
        _ => {}
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, axum::http::HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, axum::http::HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, axum::http::HeaderValue::from_static("no-referrer"));
    res
}

async fn nonce(State(s): State<AppState>) -> Json<serde_json::Value> {
    let n = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).new_nonce();
    Json(serde_json::json!({ "nonce": n }))
}

#[derive(Deserialize)]
struct LoginRequest {
    address: String,
    message: String,
    signature: String,
}

#[derive(Serialize)]
struct LoginResponse {
    address: String,
    active: bool,
    passes: Vec<FoundPass>,
    session: String,
    now_ms: i64,
}

type ApiError = (StatusCode, String);

fn bad(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::BAD_REQUEST, e.to_string())
}

fn expected_prefix(network: &str) -> kaspa_addresses::Prefix {
    if network.starts_with("testnet") { kaspa_addresses::Prefix::Testnet } else { kaspa_addresses::Prefix::Mainnet }
}

/// Wallet sign-in: proves the visitor controls the address, then reports its passes.
async fn login(State(s): State<AppState>, Query(lq): Query<LevelQ>, headers: HeaderMap, Json(req): Json<LoginRequest>) -> Result<Json<LoginResponse>, ApiError> {
    let s = s.lv(&lq)?;
    let addr: kaspa_addresses::Address = req.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    // Paid passes live on the tier's network; a holder club can be on another (mainnet NFTs).
    // Signing in is a signed message, never a transaction, so either network is safe.
    let club_net = club_of(&s).map(|c| c.rules.collection.network.clone());
    let ok_net = addr.prefix == expected_prefix(&s.card.network) || club_net.as_deref().is_some_and(|n| addr.prefix == expected_prefix(n));
    if !ok_net {
        return Err(bad(format!("this site lives on {}; switch your wallet to that network", club_net.unwrap_or_else(|| s.card.network.clone()))));
    }
    if addr.version != kaspa_addresses::Version::PubKey {
        return Err(bad("only regular (P2PK) addresses can sign in"));
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_string();
    let sig = auth::decode_signature(&req.signature).map_err(bad)?;
    // Check the signature first, so a bad one can't burn somebody else's nonce.
    auth::verify_kip5(&req.message, &sig, &addr.payload).map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;
    auth::check_message(&req.message, &req.address, &host, &mut s.sessions.lock().unwrap_or_else(|e| e.into_inner())).map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    let (active, passes) = passes_for(&s, &req.address).await?;
    let now_ms = s.snap.read().await.updated_ms;
    let session = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).open(&req.address);
    members::remember(&s.home, &req.address);
    Ok(Json(LoginResponse { address: req.address, active, passes, session, now_ms }))
}

async fn passes_for(s: &AppState, address: &str) -> Result<(bool, Vec<FoundPass>), ApiError> {
    let owner = owner_hash_of(address).map_err(bad)?.to_string();
    let snap = s.snap.read().await;
    if !snap.synced {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "indexer is still catching up, try again in a moment".into()));
    }
    let mine: Vec<FoundPass> = snap.passes.iter().filter(|p| p.owner_hash == owner).cloned().collect();
    Ok((mine.iter().any(|p| p.active), mine))
}

#[derive(Deserialize)]
struct MembersQuery {
    session: String,
}

/// Members-only content. Re-checks the pass every time, so an expired pass loses access.
async fn members(State(s): State<AppState>, Query(q): Query<MembersQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let address = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&q.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let owner = owner_hash_of(&address).map_err(bad)?.to_string();
    let top = top_rank(&s.levels, &[owner]).await.ok_or((StatusCode::SERVICE_UNAVAILABLE, "indexer is still catching up, try again in a moment".to_string()))?;
    if top.is_none() {
        return Err((StatusCode::FORBIDDEN, "no active pass for this address".into()));
    }
    if let Some(e) = examples::current() {
        return Ok(Json(serde_json::json!({ "address": address, "html": format!("<p>Welcome in. This is where {}'s members-only content would go: anything the creator writes for members, shown only to wallets holding an active pass.</p><p class=\"muted small\">An example creator, made up for the KasPass demo.</p>", esc(e.creator)) })));
    }
    Ok(Json(serde_json::json!({ "address": address, "html": s.members_html.as_str() })))
}

fn shop_of(s: &AppState) -> Result<Arc<Shop>, ApiError> {
    s.shop.clone().ok_or((StatusCode::NOT_FOUND, "this website isn't selling passes right now".to_string()))
}

/// The lane to sell from: the first with passes left (or any, to show "sold out").
async fn current_lane(s: &AppState) -> Result<FoundLane, ApiError> {
    let snap = s.snap.read().await;
    if !snap.synced {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "still reading the chain, try again in a moment".into()));
    }
    snap.lanes
        .iter()
        .find(|l| l.lane.remaining != 0)
        .or(snap.lanes.first())
        .cloned()
        .ok_or((StatusCode::NOT_FOUND, "this membership isn't on sale right now (no sales lane found on-chain)".to_string()))
}

async fn offer(State(s): State<AppState>, Query(lq): Query<LevelQ>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(offer_for(&s.lv(&lq)?).await?))
}

async fn offer_for(s: &AppState) -> Result<serde_json::Value, ApiError> {
    let s = s.clone();
    if s.card.placeholder {
        return Err((StatusCode::NOT_FOUND, "nothing is on sale here yet".to_string()));
    }
    let lane = current_lane(&s).await?;
    let mut v = shop_of(&s)?.offer(&lane).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(view) = read_sale(&s.home, &s.card.covenant_id).and_then(|sale| sale_view(&sale, &lane.lane)) {
        if view["ended"] != serde_json::json!(true) {
            v["sale"] = view;
        }
    }
    if v["v4"].is_object() {
        v["v4"]["holder_bps"] = serde_json::json!(holder_setting(&s));
    }
    // v5: the price in its currency and what it is in KAS right now (no holder discounts).
    if is_v5(&s.card) {
        v["payin"] = serde_json::json!(payin::enabled(&s));
        v["ceiling_sompi"] = v["monthly_sompi"].clone();
        match v5_price_json(&s, lane.lane.price) {
            Some((price, founding)) => {
                let max_ppm = s.card.settings.v4.as_ref().map(|x| x.max_discount_bps * 100).unwrap_or(0);
                let today = |ceiling: u64, kas: Option<f64>| kas.map(|k| crate::discounted_ppm(ceiling, crate::prices::ppm_for(ceiling, k, max_ppm)));
                if let Some(m) = today(lane.lane.price, price["kas_today"].as_f64()) {
                    v["monthly_sompi"] = serde_json::json!(m);
                } else {
                    v["price_unavailable"] = serde_json::json!(true);
                }
                // Today's lifetime and term prices in KAS (0 where not offered, as before).
                v["lifetime_ceiling_sompi"] = serde_json::json!(lane.lane.life);
                v["lifetime_sompi"] = serde_json::json!(if lane.lane.life > 0 { today(lane.lane.life, price["lifetime"]["kas_today"].as_f64()).unwrap_or(0) } else { 0 });
                if lane.lane.term_price > 0 {
                    v["v4"]["term_ceiling_sompi"] = serde_json::json!(lane.lane.term_price);
                    v["v4"]["term_sompi"] = serde_json::json!(today(lane.lane.term_price, price["term"]["kas_today"].as_f64()).unwrap_or(0));
                }
                v["price"] = price;
                if !founding.is_null() {
                    v["founding"] = founding;
                }
            }
            None => v["price_unavailable"] = serde_json::json!(true),
        }
    }
    if let Some(n) = tier_name(&s.home, &s.card.covenant_id) {
        v["name"] = serde_json::json!(fill_tax_year(&n));
    }
    if let Some(n) = creator_display(&s.home, &s.card.creator_address) {
        v["creator_name"] = serde_json::json!(n);
    }
    v["pass_word"] = serde_json::json!(pass_word(&s.home, &s.card.covenant_id));
    let sn = site_names(&s.home);
    if !sn.club_name.is_empty() {
        v["club_name"] = serde_json::json!(sn.club_name);
    }
    v["level_word"] = serde_json::json!(level_word(&s.home));
    v["level"] = serde_json::json!({
        "covenant_id": s.card.covenant_id,
        "id": short_id(&s.card.covenant_id),
        "rank": s.rank(),
        "count": s.levels.len(),
        "name": s.level_name(),
        "perks": level_perks(&s.home).get(&s.card.covenant_id).map(|v| v.iter().map(|x| fill_tax_year(x)).collect::<Vec<_>>()).unwrap_or_default(),
    });
    Ok(v)
}

/// The name a creator chose for themselves ("Bitcoin the Turtle"), by creator address. It's
/// self-chosen, so pages always show it next to the address (and any .kas/.k names) that prove who it is.
fn creator_displays(home: &std::path::Path) -> HashMap<String, String> {
    std::fs::read_to_string(home.join("creator-names.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn creator_display(home: &std::path::Path, address: &str) -> Option<String> {
    if let Some(e) = examples::current() {
        return Some(e.creator.to_string());
    }
    creator_displays(home).remove(address).filter(|n| !n.is_empty())
}

async fn creator_display_name(State(s): State<AppState>, Json(req): Json<NameRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &req.session)?;
    let name: String = req.name.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string();
    let mut all = creator_displays(&s.home);
    if name.is_empty() {
        all.remove(&me);
    } else {
        all.insert(me.clone(), name.clone());
    }
    std::fs::write(s.home.join("creator-names.json"), serde_json::to_string_pretty(&all).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    println!("dashboard: creator display name set to \"{name}\"");
    Ok(Json(serde_json::json!({ "name": name })))
}

// ---------------------------------------------------------------------------
// The creator's word for what members hold ("membership", "pass", "subscription"), by
// covenant. Pages and bot messages use it instead of a fixed word. Default: "membership".

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct PassWord {
    pub one: String,
    pub many: String,
}

impl Default for PassWord {
    fn default() -> Self {
        PassWord { one: "membership".into(), many: "memberships".into() }
    }
}

fn pass_words(home: &std::path::Path) -> HashMap<String, PassWord> {
    std::fs::read_to_string(home.join("pass-words.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn pass_word(home: &std::path::Path, covenant_id: &str) -> PassWord {
    if let Some(e) = examples::current() {
        return PassWord { one: e.word.0.into(), many: e.word.1.into() };
    }
    pass_words(home).remove(covenant_id).unwrap_or_default()
}

/// Cleans a word the creator typed: letters, spaces, hyphens and apostrophes, at most 24
/// characters, lower case (it sits in the middle of sentences). Empty means "use the default".
fn clean_word(w: &str) -> Result<String, String> {
    let w = w.trim().to_lowercase();
    if w.chars().count() > 24 {
        return Err("keep it to 24 characters or fewer".into());
    }
    if !w.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '-' || c == '\'') {
        return Err("use letters only (spaces and hyphens are fine)".into());
    }
    Ok(w.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// English plural, good enough for the words creators pick ("pass" → "passes", "membership" → "memberships").
fn plural(one: &str) -> String {
    if one.ends_with('s') || one.ends_with('x') || one.ends_with("sh") || one.ends_with("ch") {
        format!("{one}es")
    } else if one.ends_with('y') && !one.ends_with("ay") && !one.ends_with("ey") && !one.ends_with("oy") {
        format!("{}ies", &one[..one.len() - 1])
    } else {
        format!("{one}s")
    }
}

#[derive(Deserialize)]
struct PassWordRequest {
    session: String,
    one: String,
    #[serde(default)]
    many: String,
}

async fn creator_pass_word(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<PassWordRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    creator_of(&s, &req.session)?;
    let one = clean_word(&req.one).map_err(bad)?;
    let mut many = clean_word(&req.many).map_err(bad)?;
    let mut all = pass_words(&s.home);
    if !one.is_empty() && many.is_empty() {
        many = plural(&one);
    }
    // One word for the whole site, so every level reads the same.
    for l in s.levels.iter() {
        if one.is_empty() {
            all.remove(&l.card.covenant_id);
        } else {
            all.insert(l.card.covenant_id.clone(), PassWord { one: one.clone(), many: many.clone() });
        }
    }
    std::fs::write(s.home.join("pass-words.json"), serde_json::to_string_pretty(&all).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let w = pass_word(&s.home, &s.card.covenant_id);
    println!("dashboard: members now hold a \"{}\"", w.one);
    Ok(Json(serde_json::json!({ "pass_word": w })))
}

/// How bot messages name what a member holds: "Your Bitcoin the Turtle membership", or
/// "Your membership" when the creator hasn't chosen a display name.
fn your_pass(s: &AppState) -> String {
    let w = pass_word(&s.home, &s.card.covenant_id).one;
    let level = if s.levels.len() > 1 { format!(" ({})", s.level_name()) } else { String::new() };
    match creator_display(&s.home, &s.card.creator_address) {
        Some(n) => format!("Your {n} {w}{level}"),
        None => format!("Your {w}{level}"),
    }
}

// ---------------------------------------------------------------------------
// Levels: what's on sale, each level's perks, and the creator's list (levels.json).

fn level_perks(home: &std::path::Path) -> HashMap<String, Vec<String>> {
    if let Some(e) = examples::current() {
        return e.perks.clone();
    }
    std::fs::read_to_string(home.join("level-perks.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

// Partner sites the membership page may link back to after someone pays (`serve --allow-return`).
static RETURN_ORIGINS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Each must be a bare https origin like `https://example.com` (a trailing slash is fine).
pub fn set_return_origins(list: &[String]) -> anyhow::Result<()> {
    let mut out = Vec::new();
    for o in list {
        let o = o.trim().trim_end_matches('/').to_lowercase();
        let host = o.strip_prefix("https://").ok_or_else(|| anyhow::anyhow!("--allow-return {o}: must start with https://"))?;
        anyhow::ensure!(!host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':'), "--allow-return {o}: give just the site, like https://example.com");
        out.push(o);
    }
    if !out.is_empty() {
        println!("Membership page may send people back to: {}", out.join(", "));
    }
    let _ = RETURN_ORIGINS.set(out);
    Ok(())
}

fn return_origins() -> Vec<String> {
    RETURN_ORIGINS.get().cloned().unwrap_or_default()
}

/// Public: every level on sale, lowest first, with its offer (prices, options) and perks.
/// The server's keeper wallet (it pays the network fee for auto-renewals), if this site has one.
fn keeper_of(s: &AppState) -> Option<String> {
    s.shop.as_ref().and_then(|sh| sh.keeper.as_ref()).map(|w| w.addr.to_string())
}

/// `GET /v1/addresses`: every fixed address this site's memberships use, in one place: each
/// level's id (the covenant id on its sales lanes and passes), the vault, the KasPass fee address
/// (when there's a fee) and the keeper.
async fn addresses(State(s): State<AppState>) -> Json<serde_json::Value> {
    let mut levels = Vec::new();
    for l in s.levels.iter().filter(|l| !l.card.placeholder) {
        let Ok(ls) = s.at(Some(&l.card.covenant_id)) else { continue };
        let fee = l.card.settings.fee.as_ref().filter(|f| f.bps > 0);
        levels.push(serde_json::json!({
            "level": l.card.covenant_id,
            "vault": vault_of(&ls),
            "fee_address": fee.map(|f| f.address.clone()),
            "fee_bps": fee.map(|f| f.bps).unwrap_or(0),
            "bond_sompi": l.card.settings.bond,
        }));
    }
    let vault = levels.iter().find_map(|l| l["vault"].as_str().map(str::to_string));
    Json(serde_json::json!({ "network": s.card.network, "vault": vault, "keeper": keeper_of(&s), "levels": levels }))
}

async fn levels_info(State(s): State<AppState>) -> Json<serde_json::Value> {
    let perks = level_perks(&s.home);
    let about = level_about(&s.home);
    let ar = access_ranks(&s.home, &s.levels);
    let mut out = Vec::new();
    for (i, l) in s.levels.iter().enumerate() {
        if l.card.placeholder {
            continue;
        }
        let Ok(ls) = s.at(Some(&l.card.covenant_id)) else { continue };
        let offer = offer_for(&ls).await.ok();
        out.push(serde_json::json!({
            "level": l.card.covenant_id,
            "id": short_id(&l.card.covenant_id),
            "rank": i,
            "access_rank": ar[i],
            "vault": vault_of(&ls),
            "keeper": keeper_of(&ls),
            "name": level_name(&s.home, &s.levels, i),
            "named": tier_name(&s.home, &l.card.covenant_id).is_some(),
            "perks": perks.get(&l.card.covenant_id).map(|v| v.iter().map(|x| fill_tax_year(x)).collect::<Vec<_>>()).unwrap_or_default(),
            "about_html": about.get(&l.card.covenant_id).map(|h| fill_tax_year(h)).unwrap_or_default(),
            "period_ms": l.card.settings.period_ms,
            "price": offer.as_ref().and_then(|o| o.get("price")).cloned(),
            "founding": offer.as_ref().and_then(|o| o.get("founding")).cloned(),
            "offer": offer,
        }));
    }
    let sn = site_names(&s.home);
    let no_fee = s.shop.as_ref().is_some_and(|sh| sh.no_fee);
    let fx = crate::prices::last().map(|r| serde_json::json!({ "kas_usd": r.kas_usd, "usd_cad": r.usd_cad, "eur_usd": r.eur_usd, "at_ms": r.at_ms, "source": "kraken" }));
    let display = match page_profile(&s.home).price_display.as_str() {
        "viewer" => "viewer",
        _ => "plan",
    };
    Json(serde_json::json!({ "levels": out, "empty": s.card.placeholder, "network": s.card.network, "launch_no_fee": no_fee, "fx": fx, "price_display": display, "v5": s.shop.as_ref().is_some_and(|sh| sh.record().is_some()), "keeper": keeper_of(&s), "launch_open": !CREATOR_ONLY_LAUNCH.load(std::sync::atomic::Ordering::Relaxed) && s.shop.as_ref().is_none_or(|sh| sh.record().is_none()), "includes_lower": true, "club_name": sn.club_name, "level_word": level_word(&s.home), "return_origins": return_origins() }))
}

#[derive(Deserialize)]
struct PerksRequest {
    session: String,
    perks: Vec<String>,
    #[serde(default)]
    covenant_id: Option<String>,
}

/// The creator lists what a level includes (shown on its card): up to 8 short lines.
async fn creator_perks(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<PerksRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let me = creator_of(&s, &req.session)?;
    let cov = target_covenant(&s, &me, req.covenant_id.as_deref())?;
    let list: Vec<String> = req.perks.iter().map(|p| p.chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_string()).filter(|p| !p.is_empty()).take(8).collect();
    let mut all = level_perks(&s.home);
    if list.is_empty() {
        all.remove(&cov);
    } else {
        all.insert(cov, list.clone());
    }
    std::fs::write(s.home.join("level-perks.json"), serde_json::to_string_pretty(&all).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "perks": list })))
}

#[derive(Deserialize)]
struct LevelsRequest {
    session: String,
    /// Tier card files on this site, lowest level first.
    files: Vec<String>,
    /// Covenant ids of plans that give the same access as the plan right above them.
    #[serde(default)]
    same_access: Vec<String>,
}

/// The creator chooses which memberships are on sale, and in what order (lowest first). Saved in
/// levels.json; the site restarts itself to load them (as switching does).
async fn creator_levels(State(s): State<AppState>, Json(req): Json<LevelsRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let me = creator_of(&s, &req.session)?;
    if req.files.is_empty() {
        return Err(bad("choose at least one membership to sell"));
    }
    if req.files.len() > 5 {
        return Err(bad("five levels at most"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut ids: Vec<String> = Vec::new();
    for f in &req.files {
        if !safe_card_name(f) {
            return Err(bad("that isn't a membership on this site"));
        }
        let text = std::fs::read_to_string(s.home.join(f)).map_err(|_| bad(format!("{f} isn't on this site")))?;
        let card: TierCard = serde_json::from_str(&text).map_err(|e| bad(format!("{f} can't be read: {e}")))?;
        if !is_v5(&card) {
            return Err(bad(format!("{f} uses a retired contract version")));
        }
        if card.creator_address != me {
            return Err((StatusCode::FORBIDDEN, "every level must be a membership you created".to_string()));
        }
        if card.network != s.card.network {
            return Err(bad("every level must be on the same network"));
        }
        if !seen.insert(card.covenant_id.clone()) {
            return Err(bad("the same membership is listed twice"));
        }
        crate::indexer::Indexer::new(card.clone()).map_err(|e| bad(format!("{f} doesn't check out: {e}")))?;
        ids.push(card.covenant_id.clone());
    }
    for id in &req.same_access {
        match ids.iter().position(|x| x == id) {
            Some(i) if i + 1 < ids.len() => {}
            Some(_) => return Err(bad("the top level has no level above it to share access with")),
            None => return Err(bad("\"same access\" names a membership that isn't in the list")),
        }
    }
    let same: Vec<&String> = ids.iter().filter(|id| req.same_access.contains(id)).collect();
    std::fs::write(s.home.join(LEVELS_FILE), serde_json::to_string_pretty(&serde_json::json!({ "levels": req.files, "same_access": same })).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let restarts = std::env::var("INVOCATION_ID").is_ok();
    println!("dashboard: levels on sale set to {} ({})", req.files.join(", "), if restarts { "restarting" } else { "restart the site to load them" });
    if restarts {
        let (sessions, home) = (s.sessions.clone(), s.home.clone());
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            auth::save_handoff(&home, &sessions.lock().unwrap_or_else(|e| e.into_inner()));
            std::process::exit(0);
        });
    }
    Ok(Json(serde_json::json!({ "ok": true, "levels": req.files.len(), "restarting": restarts })))
}

// ---------------------------------------------------------------------------
// The creator's name for a tier ("Supporter", "Bronze"), shown instead of "This tier".

fn tier_names(home: &std::path::Path) -> HashMap<String, String> {
    if let Some(e) = examples::current() {
        return e.names.clone();
    }
    std::fs::read_to_string(home.join("tier-names.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn tier_name(home: &std::path::Path, covenant_id: &str) -> Option<String> {
    tier_names(home).remove(covenant_id).filter(|n| !n.is_empty())
}

/// The tax year a payment made now pays for: from November 1 (Pacific time) it's this year,
/// before that it's last year. So the 2026 tax year is sold from Nov 1, 2026 to Oct 31, 2027.
fn tax_year_at(ms: i64) -> i64 {
    // Midnight Nov 1 in Pacific time is always 07:00 UTC (daylight time ends later in November).
    let d = utc_iso(ms - 7 * 3_600_000);
    let year: i64 = d.get(0..4).and_then(|y| y.parse().ok()).unwrap_or(1970);
    let month: u32 = d.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
    if month >= 11 { year } else { year - 1 }
}

/// A plan's name or perks as members see them: `{tax_year}` becomes the tax year on sale now
/// (and `{next_tax_year}` the one after), so a name like "KasTax {tax_year}" moves on by itself
/// every November 1. The creator's dashboard keeps showing the text as written.
#[cfg(test)]
#[test]
fn tax_year_moves_on_nov_1_pacific() {
    // 2026-11-01T06:59:59Z is still Oct 31 in Pacific time; 07:00:00Z is Nov 1.
    assert_eq!(tax_year_at(1_793_516_399_000), 2025);
    assert_eq!(tax_year_at(1_793_516_400_000), 2026);
    assert_eq!(tax_year_at(1_798_761_600_000), 2026); // 2027-01-01
    assert_eq!(tax_year_at(1_825_002_000_000), 2026); // 2027-10-31T17:00Z
    assert_eq!(tax_year_at(1_825_052_400_000), 2027); // 2027-11-01T07:00Z
}

fn fill_tax_year(text: &str) -> String {
    if !text.contains('{') {
        return text.to_string();
    }
    let y = tax_year_at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0));
    text.replace("{tax_year}", &y.to_string()).replace("{next_tax_year}", &(y + 1).to_string())
}

#[derive(Deserialize)]
struct NameRequest {
    session: String,
    name: String,
    /// A membership on this site that isn't on sale yet (one just created), instead of the selected level.
    #[serde(default)]
    covenant_id: Option<String>,
}

/// Which membership a name or perks change is for: the selected level, or one of the creator's
/// own memberships on this site named by covenant id (so a level can be named as it's created).
fn target_covenant(s: &AppState, me: &str, covenant_id: Option<&str>) -> Result<String, ApiError> {
    let Some(c) = covenant_id.map(str::trim).filter(|c| !c.is_empty()) else { return Ok(s.card.covenant_id.clone()) };
    if s.levels.iter().any(|l| l.card.covenant_id == c) || cards_here(&s.home).iter().any(|(_, card, _)| card.covenant_id == c && card.creator_address == me) {
        Ok(c.to_string())
    } else {
        Err((StatusCode::FORBIDDEN, "that isn't one of your memberships on this site".into()))
    }
}

// The club's own name ("Executive Club") and what it calls its levels ("tier", "plan").
#[derive(Serialize, Deserialize, Clone, Default)]
struct SiteNames {
    #[serde(default)]
    club_name: String,
    #[serde(default)]
    level_word: Option<PassWord>,
}

fn site_names(home: &std::path::Path) -> SiteNames {
    if examples::current().is_some() {
        return SiteNames { club_name: String::new(), level_word: Some(PassWord { one: "tier".into(), many: "tiers".into() }) };
    }
    std::fs::read_to_string(home.join("site-names.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn level_word(home: &std::path::Path) -> PassWord {
    site_names(home).level_word.unwrap_or(PassWord { one: "plan".into(), many: "plans".into() })
}

#[derive(Deserialize)]
struct SiteNamesRequest {
    session: String,
    #[serde(default)]
    club_name: String,
    #[serde(default)]
    level_word: String,
}

async fn creator_site_names(State(s): State<AppState>, Json(req): Json<SiteNamesRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let club: String = req.club_name.chars().filter(|c| !c.is_control()).take(60).collect::<String>().trim().to_string();
    let w = clean_word(&req.level_word).map_err(bad)?;
    let names = SiteNames { club_name: club, level_word: (!w.is_empty()).then(|| PassWord { many: plural(&w), one: w }) };
    std::fs::write(s.home.join("site-names.json"), serde_json::to_string_pretty(&names).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "club_name": names.club_name, "level_word": level_word(&s.home) })))
}

// ---------------------------------------------------------------------------
// The membership page's profile (a tagline, an intro, links) and a description for each level.
// Formatted text is kept to a short allowlist (paragraphs, bold, italic, headings, lists, quotes,
// links) and cleaned here, so nothing a creator pastes can run on members' pages.

const PAGE_FILE: &str = "page.json";
const ABOUT_FILE: &str = "level-about.json";
const LINK_KINDS: [&str; 7] = ["website", "x", "tiktok", "youtube", "instagram", "telegram", "discord"];

pub fn clean_html(html: &str) -> String {
    let tags: std::collections::HashSet<&str> = ["p", "br", "b", "strong", "i", "em", "u", "s", "h3", "h4", "ul", "ol", "li", "blockquote", "a"].into_iter().collect();
    let out = ammonia::Builder::empty()
        .tags(tags)
        .add_tag_attributes("a", ["href"])
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(Some("noopener noreferrer nofollow"))
        .set_tag_attribute_value("a", "target", "_blank")
        .clean(html)
        .to_string();
    // An editor left empty still sends "<p><br></p>".
    if out.replace("<p>", "").replace("</p>", "").replace("<br>", "").trim().is_empty() { String::new() } else { out }
}

fn clean_text(t: &str, max: usize) -> String {
    t.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// Links must be plain https (or http) addresses.
fn clean_link(u: &str) -> Result<String, String> {
    let u = u.trim();
    if u.is_empty() {
        return Ok(String::new());
    }
    // "javascript:…", "data:…" and other schemes are refused, not turned into https.
    if let Some((scheme, _)) = u.split_once(':') {
        if !u.contains("://") && !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-') && !scheme.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("\"{u}\" isn't a web address"));
        }
    }
    let u = if u.contains("://") { u.to_string() } else { format!("https://{u}") };
    if u.len() > 200 || !(u.starts_with("https://") || u.starts_with("http://")) || u.chars().any(|c| c.is_whitespace() || c == '"' || c == '<' || c == '>') {
        return Err(format!("\"{u}\" isn't a web address"));
    }
    Ok(u)
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct PageProfile {
    #[serde(default)]
    tagline: String,
    #[serde(default)]
    intro_html: String,
    #[serde(default)]
    links: std::collections::BTreeMap<String, String>,
    /// How the levels show first: "cards" or "compare".
    #[serde(default)]
    view: String,
    #[serde(default)]
    show_count: bool,
    /// v5 prices: "plan" (always the plan's currency) or "viewer" (an approximate price in
    /// each visitor's currency).
    #[serde(default)]
    price_display: String,
    /// Leave the holder club (its link, section and sign-up) off the membership page.
    #[serde(default)]
    hide_club: bool,
}

fn page_profile(home: &std::path::Path) -> PageProfile {
    if let Some(e) = examples::current() {
        return e.page.clone();
    }
    std::fs::read_to_string(home.join(PAGE_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn level_about(home: &std::path::Path) -> HashMap<String, String> {
    if let Some(e) = examples::current() {
        return e.about.clone();
    }
    std::fs::read_to_string(home.join(ABOUT_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Public: the page's profile and each level's description.
async fn page_info(State(s): State<AppState>) -> Json<serde_json::Value> {
    let p = page_profile(&s.home);
    let about = level_about(&s.home);
    let levels: serde_json::Map<String, serde_json::Value> = s.levels.iter().filter_map(|l| about.get(&l.card.covenant_id).map(|h| (l.card.covenant_id.clone(), serde_json::json!(fill_tax_year(h))))).collect();
    // As written, for the creator's editor (so `{tax_year}` survives an edit).
    let raw: serde_json::Map<String, serde_json::Value> = s.levels.iter().filter_map(|l| about.get(&l.card.covenant_id).map(|h| (l.card.covenant_id.clone(), serde_json::json!(h)))).collect();
    let mut members = serde_json::Value::Null;
    if p.show_count {
        // Each member once, whatever level they're on.
        let mut owners = std::collections::HashSet::new();
        for l in s.levels.iter() {
            owners.extend(l.snap.read().await.passes.iter().filter(|x| x.active).map(|x| x.owner_hash.clone()));
        }
        members = serde_json::json!(owners.len());
    }
    Json(serde_json::json!({
        "tagline": p.tagline, "intro_html": p.intro_html, "links": p.links,
        "view": if p.view == "compare" { "compare" } else { "cards" },
        "show_count": p.show_count, "price_display": if p.price_display == "viewer" { "viewer" } else { "plan" }, "hide_club": p.hide_club, "members": members, "about": levels, "about_raw": raw,
    }))
}

#[derive(Deserialize)]
struct PageRequest {
    session: String,
    #[serde(default)]
    tagline: String,
    #[serde(default)]
    intro_html: String,
    #[serde(default)]
    links: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    view: String,
    #[serde(default)]
    show_count: bool,
    #[serde(default)]
    price_display: String,
    #[serde(default)]
    hide_club: bool,
}

async fn creator_page(State(s): State<AppState>, Json(req): Json<PageRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    if req.intro_html.len() > 60_000 {
        return Err(bad("the intro is too long"));
    }
    let intro = clean_html(&req.intro_html);
    if intro.len() > 20_000 {
        return Err(bad("the intro is too long (about 3,000 words at most)"));
    }
    let mut links = std::collections::BTreeMap::new();
    for (k, v) in &req.links {
        if !LINK_KINDS.contains(&k.as_str()) {
            continue;
        }
        let u = clean_link(v).map_err(bad)?;
        if !u.is_empty() {
            links.insert(k.clone(), u);
        }
    }
    let p = PageProfile { tagline: clean_text(&req.tagline, 140), intro_html: intro, links, view: if req.view == "compare" { "compare".into() } else { "cards".into() }, show_count: req.show_count, price_display: if req.price_display == "viewer" { "viewer".into() } else { "plan".into() }, hide_club: req.hide_club };
    std::fs::write(s.home.join(PAGE_FILE), serde_json::to_string_pretty(&p).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "tagline": p.tagline, "intro_html": p.intro_html, "links": p.links, "view": p.view, "show_count": p.show_count, "price_display": p.price_display, "hide_club": p.hide_club })))
}

#[derive(Deserialize)]
struct AboutRequest {
    session: String,
    #[serde(default)]
    html: String,
    #[serde(default)]
    covenant_id: Option<String>,
}

async fn creator_about(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<AboutRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let me = creator_of(&s, &req.session)?;
    let cov = target_covenant(&s, &me, req.covenant_id.as_deref())?;
    if req.html.len() > 60_000 {
        return Err(bad("the description is too long"));
    }
    let html = clean_html(&req.html);
    if html.len() > 20_000 {
        return Err(bad("the description is too long (about 3,000 words at most)"));
    }
    let mut all = level_about(&s.home);
    if html.is_empty() {
        all.remove(&cov);
    } else {
        all.insert(cov, html.clone());
    }
    std::fs::write(s.home.join(ABOUT_FILE), serde_json::to_string_pretty(&all).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "html": html })))
}

async fn creator_name(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<NameRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let me = creator_of(&s, &req.session)?;
    let cov = target_covenant(&s, &me, req.covenant_id.as_deref())?;
    let name: String = req.name.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string();
    let mut all = tier_names(&s.home);
    if name.is_empty() {
        all.remove(&cov);
    } else {
        all.insert(cov.clone(), name.clone());
    }
    std::fs::write(s.home.join("tier-names.json"), serde_json::to_string_pretty(&all).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    println!("dashboard: tier named \"{name}\"");
    Ok(Json(serde_json::json!({ "name": name })))
}

// ---------------------------------------------------------------------------
// v4: holder discounts and auto-renew. The creator picks one holder discount (up to the
// tier's on-chain cap); this server's voucher key approves it only for members who qualify
// for a holder-club tier right now, so it's checked again at every renewal.

#[derive(Serialize, Deserialize, Default)]
struct DiscountSetting {
    holder_bps: u64,
}

/// The creator's holder discount, capped by the tier (0 when this server can't approve discounts).
fn holder_setting(s: &AppState) -> u64 {
    let Some(shop) = &s.shop else { return 0 };
    if !shop.can_discount() {
        return 0;
    }
    let cap = s.card.settings.v4.as_ref().map(|v| v.max_discount_bps).unwrap_or(0);
    let d: DiscountSetting = std::fs::read_to_string(s.home.join("discount.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    d.holder_bps.min(cap)
}

// ---------------------------------------------------------------------------
// v5 prices: today's price in the plan's currency, as a signed discount off the KAS ceiling

/// Passes whose auto-renewal is waiting for a fresh price ("txid:index").
static WAITING_FOR_PRICE: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

pub fn waiting_for_price(txid: &str, index: u32) -> bool {
    WAITING_FOR_PRICE.lock().unwrap_or_else(|e| e.into_inner()).contains(&format!("{txid}:{index}"))
}

/// Passes due to auto-renew whose prepaid balance is less than the renewal's price
/// ("txid:index" -> the price in sompi): nothing is charged, and the member is told once.
static LOW_BALANCE: std::sync::LazyLock<Mutex<HashMap<String, u64>>> = std::sync::LazyLock::new(Default::default);

/// The renewal price a pass's balance doesn't cover, if it's waiting for a top-up.
pub fn short_of_balance(txid: &str, index: u32) -> Option<u64> {
    LOW_BALANCE.lock().unwrap_or_else(|e| e.into_inner()).get(&format!("{txid}:{index}")).copied()
}

/// Passes that lapsed because no price was available ("txid:index"). A renewal moves the pass
/// to a new coin, so the marker goes away by itself.
const LAPSED_FILE: &str = "lapsed-no-price.json";

/// Passes that lapsed because the prepaid balance didn't cover the renewal ("txid:index").
const LAPSED_LOW_FILE: &str = "lapsed-low-balance.json";

fn lapsed_no_price(home: &std::path::Path, txid: &str, index: u32) -> bool {
    lapsed_in(home, LAPSED_FILE, txid, index)
}

fn lapsed_in(home: &std::path::Path, file: &str, txid: &str, index: u32) -> bool {
    let all: Vec<String> = std::fs::read_to_string(home.join(file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.contains(&format!("{txid}:{index}"))
}

fn mark_lapsed_no_price(home: &std::path::Path, txid: &str, index: u32) {
    mark_lapsed_in(home, LAPSED_FILE, txid, index)
}

fn mark_lapsed_in(home: &std::path::Path, file: &str, txid: &str, index: u32) {
    let mut all: Vec<String> = std::fs::read_to_string(home.join(file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let k = format!("{txid}:{index}");
    if !all.contains(&k) {
        all.push(k);
        // Old markers stop mattering once their passes are long gone.
        if all.len() > 10_000 {
            all.drain(..all.len() - 10_000);
        }
        let _ = std::fs::write(home.join(file), serde_json::to_string(&all).unwrap_or_default());
    }
}

#[derive(Clone, Copy)]
enum V5For<'a> {
    /// A purchase by this address (founding and upgrade credit may apply).
    Buy(&'a str),
    /// A lifetime or commitment-term purchase by this address (upgrade credit may apply).
    BuyKind(&'a str, crate::Sale),
    /// A renewal of this pass (the plan's regular price in its currency, less any holder
    /// discount), paid by this address when known.
    Renew(&'a FoundPass, Option<&'a str>),
}

#[derive(Debug)]
struct V5Quote {
    bps: u64,
    /// What a renewal costs today (the plan's regular price, no founding or credit), in millionths off the ceiling.
    renew_ppm: u64,
    /// What this costs today, in the plan's currency, after founding and credit.
    amount: f64,
    currency: String,
    kas_today: f64,
    founding: bool,
    credit: f64,
    /// Upgrade: the lower plan the credit was worked out from (level id, its pass's end).
    from: Option<(String, i64)>,
    /// The NFT holder discount included (basis points).
    holder_bps: u64,
}

fn is_v5(card: &TierCard) -> bool {
    card.settings.v4.as_ref().is_some_and(|v| v.record_id.is_some())
}

/// Today's price for a v5 level (None on older levels). Errors when there's no fresh rate,
/// so nothing is ever charged at the ceiling by accident.
async fn v5_quote(s: &AppState, what: V5For<'_>) -> Result<Option<V5Quote>, ApiError> {
    if !is_v5(&s.card) {
        return Ok(None);
    }
    let unavailable = || (StatusCode::SERVICE_UNAVAILABLE, "price unavailable, try again shortly".to_string());
    let all = crate::prices::level_prices(&s.home);
    let price = all.get(&s.card.covenant_id).cloned().ok_or_else(|| (StatusCode::CONFLICT, "this plan has no price set yet".to_string()))?;
    // A plan priced in KAS needs no exchange rate (an upgrade credit from a plan priced in a
    // currency still does).
    let rates = match crate::prices::fresh() {
        Some(r) => r,
        None if price.currency == "KAS" => crate::prices::Rates::kas_only(),
        None => return Err(unavailable()),
    };
    // v5 discounts are in millionths of the ceiling.
    let max_ppm = s.card.settings.v4.as_ref().map(|v| v.max_discount_bps * 100).unwrap_or(0);
    let plan_kas = |p: &crate::prices::LevelPrice, amount: f64| rates.kas(&p.currency, amount).ok_or_else(unavailable);
    let (what, kind) = match what {
        V5For::BuyKind(b, k) => (V5For::Buy(b), k),
        w => (w, crate::Sale::Monthly),
    };
    let (ceiling, renew_ceiling, amount, founding, credit_kas, from) = match what {
        V5For::Renew(p, _) => (p.rate_sompi, p.rate_sompi, price.amount, false, 0.0, None),
        V5For::BuyKind(..) => unreachable!(),
        V5For::Buy(buyer) => {
            let lane = current_lane(s).await?;
            let owner = owner_hash_of(buyer).map(|h| h.to_string()).unwrap_or_default();
            // Founding: the first purchase of this level, before the deadline.
            let held_here = s.snap.read().await.passes.iter().any(|p| p.owner_hash == owner);
            let founding = price.founding.as_ref().filter(|f| wall_ms() < f.until_ms && !held_here && kind == crate::Sale::Monthly);
            let not_offered = || (StatusCode::BAD_REQUEST, "this plan doesn't offer that".to_string());
            let (kind_ceiling, kind_amount) = match kind {
                crate::Sale::Lifetime => (lane.lane.life, price.lifetime.ok_or_else(not_offered)?),
                crate::Sale::Term => (lane.lane.term_price, price.term.ok_or_else(not_offered)?),
                _ => (lane.lane.price, price.amount),
            };
            if kind_ceiling == 0 {
                return Err(not_offered());
            }
            let amount = founding.map(|f| f.amount).unwrap_or(kind_amount);
            // Upgrade credit: the unused part of an active pass on a lower level, valued at what
            // was paid for it (in its currency), never below zero.
            let mut credit_kas: f64 = 0.0;
            let mut from: Option<(String, i64)> = None;
            let rank = s.levels.iter().position(|l| l.card.covenant_id == s.card.covenant_id).unwrap_or(0);
            let paid = crate::prices::paid_log(&s.home);
            for l in s.levels.iter().take(rank) {
                let Some(lp) = all.get(&l.card.covenant_id) else { continue };
                let snap = l.snap.read().await;
                // Free trials earn no credit (nothing was paid).
                for p in snap.passes.iter().filter(|p| p.owner_hash == owner && p.active && !p.lifetime && p.paid_sompi > 0) {
                    let period = l.card.settings.period_ms.max(1) as f64;
                    let unused = ((p.expiry_ms - snap.updated_ms) as f64 / period).clamp(0.0, 1.0);
                    let (cur, amt) = paid.get(&format!("{}:{}:{}", l.card.covenant_id, p.owner_hash, p.paid_sompi)).cloned().unwrap_or((lp.currency.clone(), lp.amount));
                    // At most one period at that level's regular price: a commitment term's logged
                    // amount covers many periods, so it can't stand for one.
                    let one = match (rates.kas(&cur, amt), rates.kas(&lp.currency, lp.amount)) {
                        (Some(a), Some(b)) => Some(a.min(b) * unused),
                        _ => None,
                    };
                    if let Some(k) = one.filter(|k| k.is_finite()) {
                        if k > credit_kas {
                            credit_kas = k;
                            from = Some((l.card.covenant_id.clone(), p.expiry_ms));
                        }
                    }
                }
            }
            (kind_ceiling, lane.lane.price, amount, founding.is_some(), credit_kas, from)
        }
    };
    // NFT holders' discount (the creator's holder-club setting), checked at every purchase and renewal.
    // (`what` is a Buy here for lifetime and commitment purchases too: they get the discount.)
    let holder = match what {
        V5For::Renew(p, payer) => holder_bps_for_pass(s, p, payer).await,
        V5For::Buy(b) | V5For::BuyKind(b, _) => holder_bps(s, b).await,
    }
    .min(10_000);
    // A sale takes its percent off new purchases (after the holder discount), never renewals.
    let sale = match what {
        V5For::Renew(..) => 0,
        _ => active_sale(s).map(|(_, p)| p).unwrap_or(0),
    };
    // Together, discounts never take more than 95% off.
    let renew_keep = (1.0 - holder as f64 / 10_000.0).max(0.05);
    let keep = (renew_keep * (1.0 - sale as f64 / 100.0)).max(0.05);
    let full_kas = plan_kas(&price, amount)?;
    let kas_today = (full_kas - credit_kas).max(0.0) * keep;
    let credit = if full_kas > 0.0 { amount * credit_kas.min(full_kas) / full_kas } else { 0.0 };
    let bps = crate::prices::ppm_for(ceiling, kas_today, max_ppm);
    // Renewals (and prepaid balances) are measured against the monthly ceiling, whatever was bought.
    // (Renewals never get the sale, so prepaid balances are sized without it.)
    let renew_ppm = crate::prices::ppm_for(renew_ceiling, plan_kas(&price, price.amount)? * renew_keep, max_ppm);
    let from = from.filter(|_| credit > 0.0);
    Ok(Some(V5Quote { bps, renew_ppm, amount: (amount - credit).max(0.0) * keep, currency: price.currency, kas_today, founding, credit, from, holder_bps: holder }))
}

#[derive(Deserialize)]
struct PaymentsQuery {
    session: String,
    /// Only payments made in this calendar year (UTC); all of them when left out.
    #[serde(default)]
    year: Option<i32>,
}

/// "2026-11-02T07:00:29Z" for a time in ms since 1970 (UTC).
fn utc_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// Owner only: every payment on this site's plans priced in a currency, as CSV, for taxes.
/// One row per payment: txid, time_utc, plan_level_id, plan_name, kind, kas_paid, cad_quoted,
/// credit_cad. No member addresses. `cad_quoted` and `credit_cad` are in the plan's currency
/// (CAD for Kaspa Vision) as KasPass quoted them; blank when this server never recorded a quote.
async fn creator_payments_csv(State(s): State<AppState>, Query(q): Query<PaymentsQuery>) -> Result<axum::response::Response, ApiError> {
    creator_of(&s, &q.session)?;
    let notes = crate::prices::payments_log(&s.home);
    let paid = crate::prices::paid_log(&s.home);
    let prices = crate::prices::level_prices(&s.home);
    let mut rows: Vec<(i64, String)> = Vec::new();
    for (i, l) in s.levels.iter().enumerate() {
        if !is_v5(&l.card) {
            continue;
        }
        let name = level_name(&s.home, &s.levels, i);
        let cov = &l.card.covenant_id;
        let snap = l.snap.read().await;
        for e in snap.events.iter().filter(|e| e.amount_sompi > 0) {
            let base_kind = match e.kind.as_str() {
                "monthly" => "purchase",
                "renew" => "renewal",
                "autorenew" => "auto_renewal",
                _ => continue,
            };
            if let (Some(y), true) = (q.year, e.time_ms > 0) {
                if !utc_iso(e.time_ms).starts_with(&format!("{y:04}-")) {
                    continue;
                }
            }
            let (kind, cad, credit) = match notes.get(&e.txid) {
                Some(n) => (n.kind.clone(), Some(n.amount), Some(n.credit)),
                None => {
                    // Older payments: the quote logged by owner and amount paid, if any.
                    let cad = paid.get(&format!("{cov}:{}:{}", e.owner_hash, e.amount_sompi)).map(|(_, a)| *a);
                    let base = prices.get(cov).map(|p| p.founding.as_ref().filter(|f| e.time_ms == 0 || e.time_ms < f.until_ms).map(|f| f.amount).unwrap_or(p.amount));
                    match (base_kind, cad, base) {
                        ("purchase", Some(c), Some(b)) if b - c > 0.5 => ("upgrade".to_string(), Some(c), Some(b - c)),
                        (_, c, _) => (base_kind.to_string(), c, c.map(|_| 0.0)),
                    }
                }
            };
            let money = |x: Option<f64>| x.map(|v| format!("{v:.2}")).unwrap_or_default();
            let line = [
                e.txid.clone(),
                if e.time_ms > 0 { utc_iso(e.time_ms) } else { String::new() },
                cov.clone(),
                csv_field(&name),
                kind,
                format!("{:.8}", e.amount_sompi as f64 / crate::SOMPI as f64),
                money(cad),
                money(credit),
            ]
            .join(",");
            rows.push((e.time_ms, line));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut body = String::from("txid,time_utc,plan_level_id,plan_name,kind,kas_paid,cad_quoted,credit_cad\n");
    for (_, l) in rows {
        body.push_str(&l);
        body.push('\n');
    }
    let file = match q.year { Some(y) => format!("kaspass-payments-{y}.csv"), None => "kaspass-payments.csv".into() };
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file}\"")),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response())
}

/// What this wallet would pay for a level right now (read-only: no transaction is built).
/// `action`: "buy", "upgrade" (a purchase with a credit for a lower plan), "renew" (its own pass,
/// renewal open or ended) or "own" (it holds this level, or a higher one that includes it).
/// What a quote is for: a monthly purchase (default), a lifetime pass or a commitment term.
#[derive(Deserialize)]
struct KindQ {
    #[serde(default)]
    kind: Option<String>,
}

async fn quote(State(s): State<AppState>, Query(q): Query<CheckQuery>, Query(lq): Query<LevelQ>, Query(kq): Query<KindQ>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    if !is_v5(&s.card) {
        return Err(bad("quotes are for plans priced in a currency"));
    }
    let owner = owner_hash_of(q.address.trim()).map_err(|e| bad(e.to_string()))?.to_string();
    let period = s.card.settings.period_ms;
    let (mine, now) = {
        let snap = s.snap.read().await;
        if !snap.synced {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "still reading the chain, try again in a moment".into()));
        }
        let mine = snap.passes.iter().filter(|p| p.owner_hash == owner).max_by_key(|p| p.expiry_ms).cloned();
        (mine, snap.updated_ms)
    };
    let rank = s.rank();
    let mut higher: Option<String> = None;
    for l in s.levels.iter().skip(rank + 1) {
        if l.snap.read().await.passes.iter().any(|p| p.owner_hash == owner && p.active) {
            higher = Some(l.card.covenant_id.clone());
        }
    }
    let price = crate::prices::level_prices(&s.home).get(&s.card.covenant_id).cloned().ok_or_else(|| (StatusCode::CONFLICT, "this plan has no price set yet".to_string()))?;
    let rates = crate::prices::fresh().or_else(|| (price.currency == "KAS").then(crate::prices::Rates::kas_only));
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let kas_next = rates.and_then(|r| r.kas(&price.currency, price.amount)).map(r2);
    let kind = match kq.kind.as_deref() {
        None | Some("monthly") => crate::Sale::Monthly,
        Some("lifetime") => crate::Sale::Lifetime,
        Some("term") => crate::Sale::Term,
        Some(_) => return Err(bad("kind must be monthly, term or lifetime")),
    };
    let mut out = serde_json::json!({
        "level": s.card.covenant_id,
        "currency": price.currency,
        "regular": price.amount,
        "quoted_at_ms": wall_ms(),
        "from_level": null,
        "from_until_ms": null,
    });
    let renew_open = |p: &FoundPass| !p.active || p.expiry_ms - now <= s.card.settings.max_ahead_ms;
    let v5 = match (&mine, &higher) {
        // A higher plan includes this one: nothing to buy or renew here.
        (_, Some(h)) => {
            out["action"] = serde_json::json!("own");
            out["included_in"] = serde_json::json!(h);
            if let Some(p) = &mine {
                out["until_ms"] = serde_json::json!(p.expiry_ms);
            }
            Ok(None)
        }
        (Some(p), _) if renew_open(p) => {
            out["action"] = serde_json::json!("renew");
            out["until_ms"] = serde_json::json!(p.expiry_ms.max(now) + period);
            v5_quote(&s, V5For::Renew(p, Some(q.address.trim()))).await
        }
        (Some(p), _) => {
            out["action"] = serde_json::json!("own");
            out["until_ms"] = serde_json::json!(p.expiry_ms);
            out["renew_opens_ms"] = serde_json::json!(p.expiry_ms - s.card.settings.max_ahead_ms);
            Ok(None)
        }
        (None, None) => {
            out["kind"] = serde_json::json!(match kind { crate::Sale::Lifetime => "lifetime", crate::Sale::Term => "term", _ => "monthly" });
            match kind {
                crate::Sale::Lifetime => {
                    out["until_ms"] = serde_json::Value::Null;
                    v5_quote(&s, V5For::BuyKind(q.address.trim(), kind)).await
                }
                crate::Sale::Term => {
                    let lane = current_lane(&s).await?;
                    out["until_ms"] = serde_json::json!(now + lane.lane.term_ms);
                    out["term_periods"] = serde_json::json!(lane.lane.term_ms / period.max(1));
                    v5_quote(&s, V5For::BuyKind(q.address.trim(), kind)).await
                }
                _ => {
                    out["until_ms"] = serde_json::json!(now + period);
                    v5_quote(&s, V5For::Buy(q.address.trim())).await
                }
            }
        }
    };
    match v5 {
        Ok(Some(v)) => {
            if out.get("action").is_none() {
                out["action"] = serde_json::json!(if v.credit > 0.0 { "upgrade" } else { "buy" });
            }
            out["amount"] = serde_json::json!(r2(v.amount));
            out["founding"] = serde_json::json!(v.founding);
            out["credit"] = serde_json::json!(r2(v.credit));
            out["kas_today"] = serde_json::json!(r2(v.kas_today));
            out["kas_next"] = serde_json::json!(kas_next);
            if v.holder_bps > 0 {
                out["holder_pct"] = serde_json::json!(v.holder_bps as f64 / 100.0);
            }
            if let Some((lvl, until)) = v.from {
                out["from_level"] = serde_json::json!(lvl);
                out["from_until_ms"] = serde_json::json!(until);
            }
        }
        Ok(None) => {
            out["kas_next"] = serde_json::json!(kas_next);
        }
        Err((StatusCode::SERVICE_UNAVAILABLE, _)) => {
            if out.get("action").is_none() {
                out["action"] = serde_json::json!("buy");
            }
            out["price_unavailable"] = serde_json::json!(true);
        }
        Err(e) => return Err(e),
    }
    Ok(Json(out))
}

/// A v5 level's public price: `price` {currency, amount, kas_today, kas_ceiling, quoted_at_ms,
/// source} and `founding` {amount, kas_today, until_ms} (null when there's none or it's over).
/// `kas_today` is missing when there's no fresh rate.
fn v5_price_json(s: &AppState, ceiling_sompi: u64) -> Option<(serde_json::Value, serde_json::Value)> {
    let p = crate::prices::level_prices(&s.home).get(&s.card.covenant_id).cloned()?;
    let rates = crate::prices::fresh().or_else(|| (p.currency == "KAS").then(crate::prices::Rates::kas_only));
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let kas = |amount: f64| rates.and_then(|r| r.kas(&p.currency, amount)).map(r2);
    let mut price = serde_json::json!({
        "currency": p.currency,
        "amount": p.amount,
        "kas_ceiling": r2(ceiling_sompi as f64 / crate::SOMPI as f64),
        "source": "kraken",
    });
    if let (Some(k), Some(r)) = (kas(p.amount), rates) {
        price["kas_today"] = serde_json::json!(k);
        price["quoted_at_ms"] = serde_json::json!(r.at_ms);
    }
    // Lifetime and commitment-term prices, where the plan offers them.
    for (key, amount) in [("lifetime", p.lifetime), ("term", p.term)] {
        if let Some(a) = amount {
            let mut j = serde_json::json!({ "amount": a });
            if let Some(k) = kas(a) {
                j["kas_today"] = serde_json::json!(k);
            }
            price[key] = j;
        }
    }
    let founding = match &p.founding {
        Some(f) if wall_ms() < f.until_ms => {
            let mut j = serde_json::json!({ "amount": f.amount, "until_ms": f.until_ms });
            if let Some(k) = kas(f.amount) {
                j["kas_today"] = serde_json::json!(k);
            }
            j
        }
        _ => serde_json::Value::Null,
    };
    Some((price, founding))
}

/// The discount `address` gets right now: the holder discount if any of its wallets qualify
/// for a holder-club tier, else 0.
async fn holder_bps(s: &AppState, address: &str) -> u64 {
    let want = holder_setting(s);
    if want == 0 {
        return 0;
    }
    let Some(club) = club_of(s) else { return 0 };
    // The member's linked wallets, plus the mainnet address of each one's own key (signing in
    // proves control of that key, so its NFTs are theirs too).
    let mut all = s.wallets.of(address);
    for w in all.clone() {
        if let Some(m) = crate::names::mainnet_twin(&w) {
            if !all.contains(&m) {
                all.push(m);
            }
        }
    }
    let owners: Vec<String> = all.iter().filter_map(|a| owner_hash_of(a).ok().map(|h| h.to_string())).collect();
    let has_pass = top_rank(&s.levels, &owners).await.flatten().is_some();
    match club.standing(&all, has_pass).await {
        Some(st) if !st.tiers.is_empty() => want,
        _ => 0,
    }
}

/// The discount a pass's renewals get: from its owner's standing (when the owner is known).
async fn holder_bps_for_pass(s: &AppState, p: &FoundPass, payer: Option<&str>) -> u64 {
    let owner = match payer {
        Some(a) if owner_hash_of(a).map(|h| h.to_string() == p.owner_hash).unwrap_or(false) => Some(a.to_string()),
        _ => p.owner_address.clone(),
    };
    match owner {
        Some(a) => holder_bps(s, &a).await,
        None => 0,
    }
}

/// Public: the holder price this address would get (anyone can look up holdings, like an explorer).
async fn holder_info(State(s): State<AppState>, headers: HeaderMap, Query(lq): Query<LevelQ>, Query(q): Query<AddressQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let _: kaspa_addresses::Address = q.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    if holder_setting(&s) > 0 {
        lookup_allowed(&s, &headers, &q.address)?;
    }
    let setting = holder_setting(&s);
    let bps = if setting > 0 { holder_bps(&s, &q.address).await } else { 0 };
    Ok(Json(serde_json::json!({ "enabled": setting > 0, "holder_bps": setting, "bps": bps, "qualifies": bps > 0 })))
}

#[derive(Deserialize)]
struct DiscountRequest {
    session: String,
    holder_pct: f64,
}

/// The creator sets the holder discount (0 turns it off). Capped by the tier's on-chain maximum.
async fn creator_discount(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<DiscountRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    creator_of(&s, &req.session)?;
    let cap = s.card.settings.v4.as_ref().map(|v| v.max_discount_bps).unwrap_or(0);
    if !s.shop.as_ref().is_some_and(|sh| sh.can_discount()) {
        return Err(bad("this membership can't give holder discounts (it needs a v4 membership launched with a maximum discount)"));
    }
    if !(0.0..=100.0).contains(&req.holder_pct) {
        return Err(bad("the discount must be between 0% and 100%"));
    }
    let bps = (req.holder_pct * 100.0).round() as u64;
    if bps > cap {
        return Err(bad(format!("this membership's contracts allow at most {}% off", cap as f64 / 100.0)));
    }
    std::fs::write(s.home.join("discount.json"), serde_json::to_string(&DiscountSetting { holder_bps: bps }).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    println!("discount: holder discount set to {}%", bps as f64 / 100.0);
    Ok(Json(serde_json::json!({ "holder_bps": bps })))
}

// ---------------------------------------------------------------------------
// Switching the live tier from the dashboard. Tier cards launched from this site are saved
// next to it; the creator of the tier that's live now can make any of them the live one.
// The choice is saved in active-card.txt and the site restarts itself to load it.

/// The level files saved in the dashboard (levels.json), lowest first. None if there's no list.
pub fn read_levels(home: &std::path::Path) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.join(LEVELS_FILE)).ok()?).ok()?;
    let files: Vec<String> = v["levels"].as_array()?.iter().filter_map(|f| f.as_str()).filter(|f| safe_card_name(f)).map(String::from).collect();
    (!files.is_empty()).then_some(files)
}

pub const LEVELS_FILE: &str = "levels.json";

/// A tier card file this site wrote (no paths, nothing else).
pub fn safe_card_name(name: &str) -> bool {
    name.starts_with("tier-card") && name.ends_with(".json") && name.len() <= 80 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') && !name.contains("..")
}

fn cards_here(home: &std::path::Path) -> Vec<(String, TierCard, i64)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(home) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !safe_card_name(&name) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let Ok(card) = serde_json::from_str::<TierCard>(&text) else { continue };
            // Memberships on retired contract versions (v2, v3, v4) stay on disk but aren't offered.
            if !is_v5(&card) {
                continue;
            }
            let when = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
            out.push((name, card, when));
        }
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

async fn creator_cards(State(s): State<AppState>, Query(lq): Query<LevelQ>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let me = creator_of(&s, &q.session)?;
    let list: Vec<serde_json::Value> = cards_here(&s.home)
        .into_iter()
        .filter(|(_, c, _)| c.network == s.card.network)
        .map(|(file, c, when)| {
            let t = &c.settings;
            let lane = c.known_lane.as_ref().map(|k| k.lane);
            serde_json::json!({
                "file": file,
                "name": tier_name(&s.home, &c.covenant_id),
                "covenant_id": c.covenant_id,
                "live": c.covenant_id == s.card.covenant_id,
                "on_sale": s.levels.iter().any(|l| l.card.covenant_id == c.covenant_id),
                "rank": s.levels.iter().position(|l| l.card.covenant_id == c.covenant_id),
                "yours": c.creator_address == me,
                "creator": c.creator_address,
                "version": 5,
                "period_ms": t.period_ms,
                "launch_price_sompi": lane.map(|l| l.price),
                "saved_ms": when,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "cards": list, "levels": s.levels.len(), "restarts_itself": std::env::var("INVOCATION_ID").is_ok() })))
}

#[derive(Deserialize)]
struct SwitchRequest {
    session: String,
    file: String,
}

async fn creator_switch(State(s): State<AppState>, Json(req): Json<SwitchRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    if !safe_card_name(&req.file) {
        return Err(bad("that isn't a membership on this site"));
    }
    let text = std::fs::read_to_string(s.home.join(&req.file)).map_err(|_| bad("that membership isn't on this site"))?;
    let card: TierCard = serde_json::from_str(&text).map_err(|e| bad(format!("that membership's card can't be read: {e}")))?;
    if !is_v5(&card) {
        return Err(bad("that membership uses a retired contract version"));
    }
    if card.network != s.card.network {
        return Err(bad("that membership is on a different network"));
    }
    if card.covenant_id == s.card.covenant_id {
        return Err(bad("that membership is already the live one"));
    }
    crate::indexer::Indexer::new(card.clone()).map_err(|e| bad(format!("that membership's card doesn't check out: {e}")))?;
    std::fs::write(s.home.join(crate::ACTIVE_CARD), &req.file).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Switching means selling only this one, so a saved list of levels no longer applies.
    let _ = std::fs::remove_file(s.home.join(LEVELS_FILE));
    // A sale belongs to the tier it was started on.
    let _ = std::fs::remove_file(s.home.join("sale.json"));
    let restarts = std::env::var("INVOCATION_ID").is_ok();
    println!("dashboard: live tier switched to {} ({}); {}", req.file, &card.covenant_id[..12], if restarts { "restarting" } else { "restart the site to load it" });
    if restarts {
        // systemd starts the site again within seconds, now serving the chosen tier.
        let (sessions, home) = (s.sessions.clone(), s.home.clone());
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            auth::save_handoff(&home, &sessions.lock().unwrap_or_else(|e| e.into_inner()));
            std::process::exit(0);
        });
    }
    Ok(Json(serde_json::json!({ "ok": true, "covenant_id": card.covenant_id, "restarting": restarts })))
}

// ---------------------------------------------------------------------------
// Names: send a pass to alice.kas or alice.k, and show members' and creators' names.

/// A recipient typed as an address or a name. Returns the address on this tier's network and,
/// for a name, a line saying exactly where it points (shown before anything is signed).
async fn resolve_recipient(s: &AppState, input: &str) -> Result<(String, Option<String>), ApiError> {
    let input = input.trim();
    if kaspa_addresses::Address::try_from(input).is_ok() {
        return Ok((input.to_string(), None));
    }
    let (name, owner) = s.names.address_of(input).await.map_err(bad)?;
    let want = expected_prefix(&s.card.network);
    if owner.starts_with(if want == kaspa_addresses::Prefix::Mainnet { "kaspa:" } else { "kaspatest:" }) {
        return Ok((owner.clone(), Some(format!("{name} is {owner}"))));
    }
    // Names live on mainnet. On testnet, use the owner's own linked testnet wallet if they linked
    // one here; otherwise the testnet address of the same key.
    if want == kaspa_addresses::Prefix::Testnet {
        if let Some(t) = s.wallets.group_of(&owner).into_iter().find(|a| a.starts_with("kaspatest:")) {
            return Ok((t.clone(), Some(format!("{name} belongs to {owner}, whose linked testnet wallet is {t}"))));
        }
    }
    let twin = crate::names::twin_on(&owner, want).ok_or_else(|| bad("that name's owner address can't be used here"))?;
    Ok((twin.clone(), Some(format!("{name} belongs to {owner}; on this network that key is {twin}"))))
}

/// The .kas and .k names for an address, from the member's linked mainnet wallets or the
/// mainnet address of the same key.
async fn names_for(s: &AppState, address: &str) -> crate::names::Names {
    let mut cands: Vec<String> = Vec::new();
    for w in s.wallets.group_of(address) {
        if let Some(m) = crate::names::mainnet_twin(&w) {
            if !cands.contains(&m) {
                cands.push(m);
            }
        }
    }
    cands.sort_by_key(|c| crate::names::mainnet_twin(address).as_deref() == Some(c.as_str()));
    s.names.for_member(&cands).await
}

/// Public: names are public records, like the address itself.
async fn names_info(State(s): State<AppState>, headers: HeaderMap, Query(q): Query<AddressQuery>) -> Result<Json<crate::names::Names>, ApiError> {
    let _: kaspa_addresses::Address = q.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    lookup_allowed(&s, &headers, &q.address)?;
    Ok(Json(names_for(&s, &q.address).await))
}

#[derive(Deserialize)]
struct LeaveRequest {
    session: String,
    chat_id: i64,
}

/// The creator removes the bot from a Telegram group (for example one it was added to by mistake).
async fn creator_telegram_leave(State(s): State<AppState>, Json(req): Json<LeaveRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let bot = bot_of(&s)?;
    let title = bot.leave(req.chat_id).await.map_err(bad)?;
    Ok(Json(serde_json::json!({ "left": title })))
}

/// Sends a direct message on Telegram and Discord to whoever linked a wallet that owns `owner_hash`.
async fn dm_owner(s: &AppState, owner_hash: &str, text: &str) {
    let owns = |address: &str| s.wallets.of(address).iter().any(|a| owner_hash_of(a).map(|h| h.to_string() == owner_hash).unwrap_or(false));
    if let Some(bot) = &s.telegram {
        for l in bot.links().into_iter().filter(|l| !l.quiet && owns(&l.address)) {
            bot.notify(l.user_id, text).await;
        }
    }
    if let Some(d) = &s.discord {
        for l in d.links().into_iter().filter(|l| owns(&l.address)) {
            d.dm(&l.user_id, text).await;
        }
    }
}

/// v5 alerts (`price-alerts.json`): payments that used a price approval this server never made.
const ALERTS_FILE: &str = "price-alerts.json";
/// The payments the alert has already checked, one file per level (each level runs its own
/// watcher; one shared file let them overwrite each other, so a restart re-checked old payments).
fn alerts_seen_file(s: &AppState) -> std::path::PathBuf {
    s.home.join(format!("price-alerts-seen-{}.json", short_id(&s.card.covenant_id)))
}

fn price_alerts(home: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(home.join(ALERTS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// v5: watches this level's payments. One that carried a price approval (a voucher signature)
/// this server didn't sign means its key is in someone else's hands: tell the owner at once.
async fn price_alert_loop(s: AppState) {
    // The payments already checked, kept on disk so whatever lands while the server is down is
    // still checked when it comes back.
    let file = alerts_seen_file(&s);
    let mut seen: Option<std::collections::HashSet<String>> = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok());
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let snap = s.snap.read().await;
        if !snap.synced {
            continue;
        }
        // The very first start: everything already on chain is history, not news.
        let first = seen.is_none();
        let known = seen.get_or_insert_with(|| snap.events.iter().map(|e| e.txid.clone()).collect());
        let fresh: Vec<Event> = snap.events.iter().filter(|e| !known.contains(&e.txid)).cloned().collect();
        drop(snap);
        let save = first || !fresh.is_empty();
        let too_old = wall_ms() - crate::shop::SIGNED_KEEP_MS + 24 * 3600 * 1000;
        for e in fresh {
            known.insert(e.txid.clone());
            if !e.voucher || (e.time_ms > 0 && e.time_ms < too_old) || crate::shop::was_signed(&s.home, &e.txid) {
                continue;
            }
            let what = match e.kind.as_str() {
                "monthly" | "lifetime" | "term" => "A purchase",
                "self_trial" => "A free trial",
                _ => "A renewal",
            };
            let text = format!(
                "⚠️ KasPass alert: {what} on {} paid {} KAS with a price approval this server never made (tx {}…). Someone may have this server's key. Replace it now: dashboard, Payouts, Server key. Payments still go only to your vault.",
                s.level_name(),
                kas4(e.amount_sompi),
                &e.txid[..12]
            );
            eprintln!("price alert: {} on {} paid {} KAS with an approval this server didn't sign (tx {})", e.kind, short_id(&s.card.covenant_id), kas4(e.amount_sompi), e.txid);
            let mut all = price_alerts(&s.home);
            all.push(serde_json::json!({ "txid": e.txid, "level": s.card.covenant_id, "kind": e.kind, "amount_sompi": e.amount_sompi, "at_ms": wall_ms() }));
            let _ = std::fs::write(s.home.join(ALERTS_FILE), serde_json::to_string_pretty(&all).unwrap_or_default());
            if let Some(owner) = s.shop.as_ref().and_then(|sh| sh.record()).and_then(|r| owner_hash_of(&r.owner).ok()) {
                dm_owner(&s, &owner.to_string(), &text).await;
            }
        }
        if save {
            let _ = crate::prices::write_atomic(&file, serde_json::to_string(&*known).unwrap_or_default().as_bytes());
        }
    }
}

/// The auto-renew keeper: renews v4 passes from their own prepaid balance once they're inside
/// the tier's auto-renew window. The contract only lets the payment go to the creator (and the
/// fee), so the keeper can't take anything; it just pays the network fee.
async fn keeper_loop(s: AppState) {
    let every = crate::test_switch("KASPASS_KEEPER_EVERY_S").and_then(|v| v.parse().ok()).unwrap_or(20);
    let mut tried: HashMap<String, std::time::Instant> = HashMap::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(every)).await;
        keeper_once(&s, &mut tried).await;
    }
}

async fn keeper_once(s: &AppState, tried: &mut HashMap<String, std::time::Instant>) {
    let (Some(shop), Some(v4)) = (s.shop.clone(), s.card.settings.v4.clone()) else { return };
    let (passes, now) = {
        let snap = s.snap.read().await;
        if !snap.synced {
            return;
        }
        (snap.passes.clone(), snap.updated_ms)
    };
    tried.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(300));
    let period = s.card.settings.period_ms;
    for p in passes {
        // Inside the window, and not lapsed for more than a period (then the member decides).
        if p.lifetime || p.balance_sompi == 0 || p.expiry_ms - now > v4.auto_window_ms || now - p.expiry_ms > period {
            continue;
        }
        // A self-serve trial is charged only at its very end: the member can cancel until then.
        if v4.self_trial && p.paid_sompi == 0 && (p.expiry_ms - now > trial::charge_window_ms() || trial::cancelled(&s.home, &s.card.covenant_id, &p.owner_hash)) {
            continue;
        }
        let key = format!("{}:{}", p.txid, p.index);
        if tried.contains_key(&key) {
            continue;
        }
        // v5: today's price, signed by this server; no fresh price = wait and try again later.
        let mut note = None;
        let bps = match v5_quote(s, V5For::Renew(&p, None)).await {
            Ok(Some(q)) => {
                WAITING_FOR_PRICE.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                note = Some(crate::prices::PayNote { currency: q.currency.clone(), amount: q.amount, credit: 0.0, kind: "auto_renewal".into() });
                q.bps
            }
            Ok(None) => holder_bps_for_pass(s, &p, None).await,
            Err(e) => {
                let first = WAITING_FOR_PRICE.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone());
                if first {
                    eprintln!("keeper: an auto-renewal is waiting for a price ({})", e.1);
                    // Tell the member once (Telegram or Discord, if they linked a wallet).
                    let text = format!("{} is due to renew, but no KAS price is available right now, so nothing has been charged. It tries again every few minutes. If it hasn't renewed by the time it ends, you can renew by hand.", your_pass(s));
                    dm_owner(s, &p.owner_hash, &text).await;
                }
                // Ran out while waiting: remember why, for the member's page.
                if p.expiry_ms <= now {
                    mark_lapsed_no_price(&s.home, &p.txid, p.index);
                }
                continue;
            }
        };
        let due = crate::discounted_ppm(p.rate_sompi, bps);
        if p.balance_sompi < due {
            // Not enough prepaid for this renewal: nothing is charged (no partial renewals). Tell
            // the member once; a top-up before the end date lets the next round renew it.
            let first = LOW_BALANCE.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), due).is_none();
            if first {
                let text = format!(
                    "{} is due to renew ({}), but its prepaid balance ({} KAS) is less than today's renewal price ({} KAS), so nothing has been charged. Top up at least {} KAS at {} and it renews by itself. Otherwise it simply ends, and your balance stays yours to take back.",
                    your_pass(s),
                    if p.expiry_ms > now { format!("it ends in {}", human_span(p.expiry_ms - now)) } else { "it has just ended".to_string() },
                    kas4(p.balance_sompi),
                    kas4(due),
                    kas4(due - p.balance_sompi),
                    site_url(s)
                );
                dm_owner(s, &p.owner_hash, &text).await;
            }
            if p.expiry_ms <= now {
                mark_lapsed_in(&s.home, LAPSED_LOW_FILE, &p.txid, p.index);
            }
            continue;
        }
        LOW_BALANCE.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        tried.insert(key, std::time::Instant::now());
        let res = {
            let mut chain = s.chain.lock().await;
            shop.autorenew(&mut chain, &p, bps).await
        };
        match res {
            Ok((txid, paid, left)) => {
                if let Some(n) = note.take() {
                    crate::prices::log_payment(&s.home, &txid, n);
                }
                let covers = left / p.rate_sompi.max(1);
                println!("keeper: auto-renewed a pass for {} KAS{} (balance left {} KAS), tx {txid}", kas4(paid), format!(", today's price ({:.4}% off the ceiling)", bps as f64 / 10_000.0), kas4(left));
                let text = format!(
                    "{} renewed automatically: {} KAS from your prepaid balance{}. Balance left: {} KAS ({} more renewal{}). You can top up or cancel any time at {}.",
                    your_pass(s),
                    kas4(paid),
                    " (today's price)",
                    kas4(left),
                    covers,
                    if covers == 1 { "" } else { "s" },
                    site_url(s)
                );
                dm_owner(s, &p.owner_hash, &text).await;
            }
            Err(e) => eprintln!("keeper: couldn't auto-renew a pass: {e:#}"),
        }
    }
}

fn kas4(sompi: u64) -> f64 {
    (sompi as f64 / crate::SOMPI as f64 * 10_000.0).round() / 10_000.0
}

fn site_url(s: &AppState) -> String {
    s.telegram.as_ref().and_then(|b| b.site_url()).or_else(|| s.discord.as_ref().and_then(|d| d.site_url())).unwrap_or_else(|| "the membership website".into())
}

// ---------------------------------------------------------------------------
// Sales: the creator lowers the on-chain price and the site shows the regular
// price struck through. The record is only shown while the chain's price is
// actually below the regular price, so a stale record can't mislead anyone.

#[derive(Serialize, Deserialize, Clone)]
struct SaleInfo {
    regular_monthly_sompi: u64,
    regular_lifetime_sompi: u64,
    #[serde(default)]
    label: String,
    #[serde(default)]
    ends_ms: Option<i64>,
    started_ms: i64,
    /// The tier the sale was started on; a record from another tier is never shown.
    #[serde(default)]
    covenant_id: Option<String>,
    /// Percent off new purchases (monthly, commitment and lifetime), applied to today's price by
    /// this server. Renewals stay at the regular price. The sale ends by itself at `ends_ms`.
    #[serde(default)]
    pct: u64,
}

/// The sale running on this level right now: its percent off, if any.
fn active_sale(s: &AppState) -> Option<(SaleInfo, u64)> {
    let sale = read_sale(&s.home, &s.card.covenant_id)?;
    let pct = sale.pct.min(90);
    (pct > 0 && !sale.ends_ms.is_some_and(|e| wall_ms() > e)).then_some((sale, pct))
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn read_sale(home: &std::path::Path, covenant_id: &str) -> Option<SaleInfo> {
    let sale: SaleInfo = std::fs::read_to_string(home.join("sale.json")).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    (sale.covenant_id.as_deref() == Some(covenant_id)).then_some(sale)
}

fn pct_off(now: u64, regular: u64) -> Option<u64> {
    (regular > 0 && now < regular).then(|| ((regular - now) as f64 * 100.0 / regular as f64).round() as u64)
}

fn sale_view(sale: &SaleInfo, lane: &crate::Lane) -> Option<serde_json::Value> {
    if sale.pct > 0 {
        let pct = sale.pct.min(90);
        return Some(serde_json::json!({
            "label": sale.label,
            "ends_ms": sale.ends_ms,
            "ended": sale.ends_ms.is_some_and(|e| wall_ms() > e),
            "pct": pct,
            "monthly_pct": pct,
            "lifetime_pct": if lane.life > 0 { Some(pct) } else { None },
            "term_pct": if lane.term_price > 0 { Some(pct) } else { None },
        }));
    }
    let m = pct_off(lane.price, sale.regular_monthly_sompi);
    let l = if lane.life > 0 { pct_off(lane.life, sale.regular_lifetime_sompi) } else { None };
    if m.is_none() && l.is_none() {
        return None;
    }
    Some(serde_json::json!({
        "label": sale.label,
        "ends_ms": sale.ends_ms,
        "ended": sale.ends_ms.is_some_and(|e| wall_ms() > e),
        "regular_monthly_sompi": sale.regular_monthly_sompi,
        "regular_lifetime_sompi": sale.regular_lifetime_sompi,
        "monthly_pct": m,
        "lifetime_pct": l,
    }))
}

#[derive(Deserialize)]
struct SaleRequest {
    session: String,
    action: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    ends_ms: Option<i64>,
    #[serde(default)]
    pct: Option<u64>,
}

/// Records (or clears) the regular prices behind a sale. The price change itself is signed by
/// the creator in Kaspire, like any other price change.
async fn creator_sale(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<SaleRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    creator_of(&s, &req.session)?;
    let path = s.home.join("sale.json");
    match req.action.as_str() {
        "start" => {
            // A percent off new purchases, applied by this server to today's price: no transaction.
            let pct = req.pct.ok_or_else(|| bad("choose how much off (1% to 90%)"))?;
            if !(1..=90).contains(&pct) {
                return Err(bad("a sale is 1% to 90% off"));
            }
            if req.ends_ms.is_some_and(|e| e <= wall_ms()) {
                return Err(bad("the end date is in the past"));
            }
            let label: String = req.label.unwrap_or_default().chars().filter(|c| !c.is_control()).take(40).collect();
            let sale = SaleInfo { regular_monthly_sompi: 0, regular_lifetime_sompi: 0, label: label.trim().to_string(), ends_ms: req.ends_ms, started_ms: wall_ms(), covenant_id: Some(s.card.covenant_id.clone()), pct };
            std::fs::write(&path, serde_json::to_string_pretty(&sale).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            println!("sale: started \"{}\" ({pct}% off)", sale.label);
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        "end" => {
            let _ = std::fs::remove_file(&path);
            println!("sale: ended");
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        _ => Err(bad("action must be start or end")),
    }
}

/// .kas and .k names for the tier's members, for display in the dashboard. A member's names
/// come from their linked mainnet wallets, or from the mainnet address of the same key.
async fn creator_names(State(s): State<AppState>, Query(lq): Query<LevelQ>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    creator_of(&s, &q.session)?;
    let owners: Vec<String> = {
        let snap = s.snap.read().await;
        let mut v: Vec<String> = snap.passes.iter().filter_map(|p| p.owner_address.clone().or_else(|| members::address_of(&s.home, &p.owner_hash))).collect();
        v.extend(snap.passes.iter().filter_map(|p| p.bought_by.clone()).filter(|a| owner_hash_of(a).is_ok()));
        v.sort();
        v.dedup();
        v
    };
    let mut set = tokio::task::JoinSet::new();
    for owner in owners.into_iter().take(200) {
        let (names, wallets) = (s.names.clone(), s.wallets.clone());
        set.spawn(async move {
            let mut cands: Vec<String> = Vec::new();
            for w in wallets.group_of(&owner) {
                if let Some(m) = crate::names::mainnet_twin(&w) {
                    if !cands.contains(&m) {
                        cands.push(m);
                    }
                }
            }
            // A mainnet wallet the member linked comes before the same-key twin.
            cands.sort_by_key(|c| crate::names::mainnet_twin(&owner).as_deref() == Some(c.as_str()));
            (owner, names.for_member(&cands).await)
        });
    }
    let mut out = serde_json::Map::new();
    while let Some(Ok((owner, n))) = set.join_next().await {
        if n.kas.is_some() || n.k.is_some() {
            out.insert(owner, serde_json::to_value(n).unwrap_or_default());
        }
    }
    Ok(Json(serde_json::Value::Object(out)))
}

// ---------------------------------------------------------------------------
// Renewal reminders: a direct message on Telegram and Discord a little before a
// monthly pass ends, once per pass period.

/// How long before the end to remind: KASPASS_REMIND_BEFORE_MS, else a third of the period, at most 3 days.
fn remind_window(period_ms: i64) -> i64 {
    crate::test_switch("KASPASS_REMIND_BEFORE_MS").and_then(|v| v.parse().ok()).unwrap_or((period_ms / 3).min(3 * 24 * 3600 * 1000))
}

fn human_span(ms: i64) -> String {
    let m = (ms / 60_000).max(1);
    let (d, h) = (m / 1440, (m % 1440) / 60);
    if d > 0 {
        format!("{d} day{}{}", if d == 1 { "" } else { "s" }, if h > 0 { format!(" {h} hour{}", if h == 1 { "" } else { "s" }) } else { String::new() })
    } else if h > 0 {
        format!("{h} hour{}", if h == 1 { "" } else { "s" })
    } else {
        format!("{m} minute{}", if m == 1 { "" } else { "s" })
    }
}

async fn remind_loop(s: AppState) {
    let every = crate::test_switch("KASPASS_REMIND_EVERY_S").and_then(|v| v.parse().ok()).unwrap_or(60);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(every)).await;
        remind_once(&s).await;
    }
}

async fn remind_once(s: &AppState) {
    if s.telegram.is_none() && s.discord.is_none() {
        return;
    }
    let (passes, now) = {
        let snap = s.snap.read().await;
        if !snap.synced {
            return;
        }
        (snap.passes.clone(), snap.updated_ms)
    };
    let window = remind_window(s.card.settings.period_ms);
    let due: Vec<FoundPass> = passes.into_iter().filter(|p| !p.lifetime && p.active && p.expiry_ms > now && p.expiry_ms - now <= window).collect();
    if due.is_empty() {
        return;
    }
    let path = s.home.join("reminders.json");
    let mut sent: HashMap<String, i64> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let before = sent.len();
    sent.retain(|_, exp| *exp > now - 7 * 24 * 3600 * 1000);
    let mut changed = sent.len() != before;
    let site = s.telegram.as_ref().and_then(|b| b.site_url()).or_else(|| s.discord.as_ref().and_then(|d| d.site_url())).unwrap_or_else(|| "the membership website".into());
    let owners_of = |address: &str| -> Vec<String> { s.wallets.of(address).iter().filter_map(|a| owner_hash_of(a).ok().map(|h| h.to_string())).collect() };
    let auto = s.card.settings.v4.is_some() && s.shop.as_ref().is_some_and(|sh| sh.keeper.is_some());
    let yours = your_pass(s);
    let text = |p: &FoundPass| {
        if auto && p.balance_sompi >= p.rate_sompi {
            format!(
                "{yours} renews automatically in about {} from your prepaid balance ({} KAS left). Nothing to do; to cancel, take your balance back at {site}.",
                human_span(p.expiry_ms - now),
                kas4(p.balance_sompi)
            )
        } else {
            format!(
                "{yours} ends in {}. Renew at {site} to keep your access, at your locked rate of {} KAS.",
                human_span(p.expiry_ms - now),
                kas4(p.rate_sompi)
            )
        }
    };
    if let Some(bot) = &s.telegram {
        for l in bot.links().into_iter().filter(|l| !l.quiet) {
            let owners = owners_of(&l.address);
            for p in due.iter().filter(|p| owners.contains(&p.owner_hash)) {
                let key = format!("tg:{}:{}:{}", l.user_id, p.owner_hash, p.expiry_ms);
                if sent.contains_key(&key) {
                    continue;
                }
                bot.notify(l.user_id, &format!("{}\n\n(Send /quiet to stop these reminders.)", text(p))).await;
                println!("reminder: telegram {} (pass ends in {})", crate::log_safe(&l.name), human_span(p.expiry_ms - now));
                sent.insert(key, p.expiry_ms);
                changed = true;
            }
        }
    }
    if let Some(d) = &s.discord {
        for l in d.links() {
            let owners = owners_of(&l.address);
            for p in due.iter().filter(|p| owners.contains(&p.owner_hash)) {
                let key = format!("dc:{}:{}:{}", l.user_id, p.owner_hash, p.expiry_ms);
                if sent.contains_key(&key) {
                    continue;
                }
                d.dm(&l.user_id, &text(p)).await;
                println!("reminder: discord {} (pass ends in {})", crate::log_safe(&l.name), human_span(p.expiry_ms - now));
                sent.insert(key, p.expiry_ms);
                changed = true;
            }
        }
    }
    if changed {
        let _ = std::fs::write(&path, serde_json::to_string(&sent).unwrap_or_default());
    }
}

#[derive(Deserialize)]
struct PrepareRequest {
    address: String,
    kind: String,
    /// v4: renewals to prepay into the pass for auto-renew.
    #[serde(default)]
    prepaid_periods: u64,
    /// The cushion on each prepaid renewal in case KAS falls: 0, 10 (default) or 25 percent.
    #[serde(default)]
    cushion_pct: Option<u64>,
}

/// A cushion the member picked, or the default; anything else is refused.
fn cushion(pct: Option<u64>) -> Result<u64, ApiError> {
    let c = pct.unwrap_or(crate::shop::DEFAULT_CUSHION);
    if crate::shop::CUSHIONS.contains(&c) { Ok(c) } else { Err(bad("the cushion can be 0%, 10% or 25%")) }
}

/// One pass per wallet from the website: refuse a purchase if this wallet already holds an
/// active pass, or bought one moments ago. (Gifts and extra passes stay possible from the
/// creator's tool with --force.)
async fn no_duplicate(s: &AppState, address: &str) -> Result<(), ApiError> {
    let conflict = |m: &str| (StatusCode::CONFLICT, m.to_string());
    let (active, _) = passes_for(s, address).await?;
    if active {
        return Err(conflict(if s.levels.len() > 1 { "this wallet already has this level, so there's nothing to buy. Sign in again to see it" } else { "this wallet is already a member, so there's nothing to buy. Sign in again to see it" }));
    }
    let mut recent = s.recent_buys.lock().unwrap_or_else(|e| e.into_inner());
    recent.retain(|_, t| t.elapsed() < RECENT_BUY);
    if recent.contains_key(&format!("{}:{address}", s.card.covenant_id)) {
        return Err(conflict("this wallet just bought this level; give it a moment to show up"));
    }
    Ok(())
}

async fn buy_prepare(State(s): State<AppState>, headers: HeaderMap, Query(lq): Query<LevelQ>, Json(req): Json<PrepareRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    signed_in_as(&s, &headers, &req.address)?;
    let shop = shop_of(&s)?;
    let addr: kaspa_addresses::Address = req.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    if addr.prefix != expected_prefix(&s.card.network) {
        return Err(bad(format!("this membership lives on {}; switch your wallet to that network", s.card.network)));
    }
    let kind = match req.kind.as_str() {
        "monthly" => crate::Sale::Monthly,
        "lifetime" => crate::Sale::Lifetime,
        "term" => crate::Sale::Term,
        _ => return Err(bad("kind must be monthly, term or lifetime")),
    };
    no_duplicate(&s, &req.address).await?;
    // Checked before taking the chain lock (it may ask the NFT indexer).
    let v5 = v5_quote(&s, if kind == crate::Sale::Monthly { V5For::Buy(&req.address) } else { V5For::BuyKind(&req.address, kind) }).await?;
    let bps = match &v5 {
        Some(q) => q.bps,
        None if s.card.settings.v4.is_some() => holder_bps(&s, &req.address).await,
        None => 0,
    };
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        let lane = current_lane(&s).await?;
        if lane.lane.remaining == 0 {
            return Err((StatusCode::CONFLICT, "sold out: every pass for this membership has been sold".into()));
        }
        let r = shop.prepare_v4(&chain, &req.address, kind, &lane, req.prepaid_periods, cushion(req.cushion_pct)?, bps, v5.as_ref().map(|q| q.renew_ppm)).await.map_err(bad)?;
        if let Some(q) = &v5 {
            let note = if q.credit > 0.0 { "upgrade" } else { "purchase" };
            crate::prices::later_payment(&r.0.txid(), crate::prices::PayNote { currency: q.currency.clone(), amount: q.amount, credit: q.credit, kind: note.into() });
            // What this pass was paid in the plan's currency, to value a later upgrade credit.
            let ceiling = match kind { crate::Sale::Lifetime => lane.lane.life, crate::Sale::Term => lane.lane.term_price, _ => lane.lane.price };
            let due = crate::discounted_ppm(ceiling, q.bps);
            crate::prices::later_paid(&r.0.txid(), &s.card.covenant_id, &owner_hash_of(&req.address).map(|h| h.to_string()).unwrap_or_default(), due as i64, &q.currency, q.amount);
        }
        r
    };
    if let Some(q) = &v5 {
        out["price"] = serde_json::json!({ "currency": q.currency, "amount": (q.amount * 100.0).round() / 100.0, "kas_today": (q.kas_today * 100.0).round() / 100.0, "founding": q.founding, "credit": (q.credit * 100.0).round() / 100.0 });
    }
    let id = store_quote(&s, quote);
    out["quote"] = serde_json::json!(id);
    Ok(Json(out))
}

#[derive(Deserialize)]
struct PassRequest {
    /// Who pays (the signed-in wallet). Anyone may renew or upgrade any pass; only the owner can close it.
    address: String,
    txid: String,
    index: u32,
    action: String,
    /// Recipient, for transfers.
    #[serde(default)]
    to: Option<String>,
    /// v4 top-up: renewals to add to the balance (or an amount in KAS).
    #[serde(default)]
    periods: Option<u64>,
    #[serde(default)]
    amount_kas: Option<f64>,
    /// Top-up by renewals: the cushion in case KAS falls (0, 10 or 25 percent).
    #[serde(default)]
    cushion_pct: Option<u64>,
}

async fn pass_prepare(State(s): State<AppState>, headers: HeaderMap, Query(lq): Query<LevelQ>, Json(req): Json<PassRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    signed_in_as(&s, &headers, &req.address)?;
    let shop = shop_of(&s)?;
    let addr: kaspa_addresses::Address = req.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    if addr.prefix != expected_prefix(&s.card.network) {
        return Err(bad(format!("this membership lives on {}; switch your wallet to that network", s.card.network)));
    }
    let action = match req.action.as_str() {
        "renew" => Some(crate::PassAction::Renew),
        "upgrade" => Some(crate::PassAction::Upgrade),
        "close" | "transfer" | "deposit" | "withdraw" => None,
        _ => return Err(bad("action must be renew, upgrade, close, transfer, deposit or withdraw")),
    };
    let mut req = req;
    let mut named: Option<String> = None;
    if req.action == "transfer" {
        let (to, label) = resolve_recipient(&s, req.to.as_deref().unwrap_or_default()).await?;
        // One pass per wallet applies to the recipient too.
        if passes_for(&s, &to).await.map(|(active, _)| active).unwrap_or(false) {
            return Err((StatusCode::CONFLICT, "that wallet already has an active pass for this membership".into()));
        }
        req.to = Some(to);
        named = label;
    }
    // Only passes the indexer has verified on-chain can be renewed or upgraded here.
    let pass = s
        .snap
        .read()
        .await
        .passes
        .iter()
        .find(|p| p.txid == req.txid && p.index == req.index)
        .cloned()
        .ok_or_else(|| bad("that pass isn't live any more; refresh the page"))?;
    // v5 upgrades pay the lifetime price locked into the pass, with no discount voucher. On a plan
    // priced in a currency that price is the KAS ceiling, not today's price, so refuse it there.
    if is_v5(&s.card) && action == Some(crate::PassAction::Upgrade) {
        if let Some(p) = crate::prices::level_prices(&s.home).get(&s.card.covenant_id) {
            if p.currency != "KAS" {
                return Err(bad(format!("upgrading to lifetime isn't offered on plans priced in {}", p.currency)));
            }
        }
    }
    let v5 = v5_quote(&s, V5For::Renew(&pass, Some(req.address.as_str()))).await?;
    let bps = match &v5 {
        Some(q) if action == Some(crate::PassAction::Renew) => q.bps,
        Some(_) => 0,
        None if action == Some(crate::PassAction::Renew) && s.card.settings.v4.is_some() => holder_bps_for_pass(&s, &pass, Some(&req.address)).await,
        None => 0,
    };
    let add = match (req.periods, req.amount_kas) {
        // v5: renewals at today's price (plus the member's cushion for price moves); older plans at the locked rate.
        (Some(n), _) if (1..=36).contains(&n) => match &v5 {
            Some(q) => n * shop.prepaid_with(crate::discounted_ppm(pass.rate_sompi, crate::prices::ppm_for(pass.rate_sompi, q.kas_today, s.card.settings.v4.as_ref().map(|v| v.max_discount_bps * 100).unwrap_or(0))), cushion(req.cushion_pct)?),
            None => n * pass.rate_sompi,
        },
        (_, Some(k)) if k > 0.0 && k <= 1e9 => (k * crate::SOMPI as f64).round() as u64,
        _ => 0,
    };
    if req.action == "deposit" && add == 0 {
        return Err(bad("choose how much to add: 1 to 36 renewals, or an amount in KAS"));
    }
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        match action {
            Some(a) => shop.prepare_pass_v4(&chain, &req.address, &pass, a, bps).await,
            None if req.action == "transfer" => shop.prepare_transfer(&chain, &req.address, &pass, req.to.as_deref().unwrap_or_default()).await,
            None if req.action == "deposit" => shop.prepare_deposit(&chain, &req.address, &pass, add).await,
            None if req.action == "withdraw" => shop.prepare_withdraw(&chain, &req.address, &pass).await,
            None => shop.prepare_close(&chain, &req.address, &pass).await,
        }
        .map_err(bad)?
    };
    // A self-serve trial: "Cancel my trial" is remembered here (only for the pass's owner), so the keeper leaves the pass
    // alone even if taking the money back doesn't go through; adding money again undoes it.
    let is_owner = owner_hash_of(&req.address).is_ok_and(|h| h.to_string() == pass.owner_hash);
    if is_owner && s.card.settings.v4.as_ref().is_some_and(|v| v.self_trial) && pass.paid_sompi == 0 && !pass.lifetime {
        match req.action.as_str() {
            "withdraw" => trial::set_cancelled(&s.home, &s.card.covenant_id, &pass.owner_hash, true),
            "deposit" => trial::set_cancelled(&s.home, &s.card.covenant_id, &pass.owner_hash, false),
            _ => {}
        }
    }
    if let (Some(q), Some(crate::PassAction::Renew)) = (&v5, action) {
        let due = crate::discounted_ppm(pass.rate_sompi, q.bps);
        crate::prices::later_paid(&quote.txid(), &s.card.covenant_id, &pass.owner_hash, (pass.paid_sompi + due) as i64, &q.currency, q.amount);
        crate::prices::later_payment(&quote.txid(), crate::prices::PayNote { currency: q.currency.clone(), amount: q.amount, credit: 0.0, kind: "renewal".into() });
    }
    if let Some(n) = named {
        out["summary"]["text"] = serde_json::json!(format!("{n}. {}", out["summary"]["text"].as_str().unwrap_or_default()));
        out["summary"]["name"] = serde_json::json!(n);
    }
    let id = store_quote(&s, quote);
    out["quote"] = serde_json::json!(id);
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitRequest {
    quote: String,
    pskt_transaction_json: String,
}

/// Answers already given by /v1/submit, by quote: a phone browser waking up can send the same
/// signed transaction twice (the first attempt's answer lost on a dead connection). The repeat
/// gets the same answer instead of "expired".
static SUBMITTED: std::sync::Mutex<Vec<(String, std::time::Instant, serde_json::Value)>> = std::sync::Mutex::new(Vec::new());

async fn buy_submit(State(s): State<AppState>, Json(req): Json<SubmitRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    {
        let mut done = SUBMITTED.lock().unwrap_or_else(|e| e.into_inner());
        done.retain(|(_, t, _)| t.elapsed() < QUOTE_TTL);
        if let Some((_, _, out)) = done.iter().find(|(q, _, _)| *q == req.quote) {
            return Ok(Json(out.clone()));
        }
    }
    let (level, quote) = s
        .quotes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&req.quote)
        .filter(|(_, q)| q.created.elapsed() < QUOTE_TTL)
        .ok_or_else(|| bad("this purchase expired; please start again"))?;
    let s = s.at(Some(&level))?;
    let shop = shop_of(&s)?;
    let buyer = quote.buyer.clone();
    let owner = quote.new_pass_owner.clone();
    if let Some(o) = &owner {
        // Checked again here: two tabs could each have prepared a purchase.
        no_duplicate(&s, o).await?;
    }
    let qid = quote.txid();
    trial::reserve(&s, &qid).await?;
    let mut chain = s.chain.lock().await;
    let done = match shop.finish(&mut chain, quote, &req.pskt_transaction_json).await {
        Ok(d) => d,
        Err(e) => {
            trial::release(&s.home, &qid);
            return Err(bad(e));
        }
    };
    drop(chain);
    let (txid, new_lane) = (done.txid.clone(), done.new_lane);
    crate::prices::commit(&s.home, &txid);
    trial::commit(&s.home, &txid);
    // Sell from the new lane right away, without waiting for the next scan.
    if let Some((lane, (old_txid, old_index))) = new_lane {
        let mut snap = s.snap.write().await;
        snap.lanes.retain(|l| !(l.txid == old_txid && l.index == old_index));
        snap.lanes.insert(0, lane);
    }
    if let Some(o) = &owner {
        s.recent_buys.lock().unwrap_or_else(|e| e.into_inner()).insert(format!("{}:{o}", s.card.covenant_id), std::time::Instant::now());
        members::remember(&s.home, o);
    }
    members::remember(&s.home, &buyer);
    println!("website: tx {txid}");
    let mut out = serde_json::json!({ "txid": txid, "address": buyer });
    if let Some((card, file)) = done.card {
        println!("new tier launched: covenant {} (card saved as {file})", card.covenant_id);
        out["card"] = serde_json::to_value(&card).unwrap_or_default();
        out["card_file"] = serde_json::json!(file);
    }
    {
        let mut done = SUBMITTED.lock().unwrap_or_else(|e| e.into_inner());
        done.push((req.quote.clone(), std::time::Instant::now(), out.clone()));
        if done.len() > 500 {
            done.remove(0);
        }
    }
    Ok(Json(out))
}

// ---- Creator dashboard -----------------------------------------------------

#[derive(Deserialize)]
struct SessionQuery {
    session: String,
}

/// `POST /v1/logout` (session header): ends the session on the server, not just in the browser.
async fn logout(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Json<serde_json::Value> {
    let closed = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).close(&q.session);
    Json(serde_json::json!({ "ok": true, "closed": closed }))
}

/// The signed-in wallet, if it's this tier's creator.
/// Who runs this site: the owner record's current owner (v5), else the membership's creator.
fn site_owner(s: &AppState) -> String {
    s.shop.as_ref().and_then(|sh| sh.record()).map(|r| r.owner).unwrap_or_else(|| s.card.creator_address.clone())
}

/// `x-kaspass-session: <token>` works wherever `?session=` does, so tokens stay out of links,
/// browser history and access logs.
async fn session_header(mut req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let tok = req.headers().get("x-kaspass-session").and_then(|v| v.to_str().ok()).filter(|t| !t.is_empty() && t.len() <= 128 && t.bytes().all(|b| b.is_ascii_alphanumeric())).map(String::from);
    if let Some(tok) = tok {
        let uri = req.uri().clone();
        let has = uri.query().is_some_and(|q| q.split('&').any(|kv| kv.starts_with("session=")));
        if !has {
            let pq = match uri.query() {
                Some(q) if !q.is_empty() => format!("{}?{q}&session={tok}", uri.path()),
                _ => format!("{}?session={tok}", uri.path()),
            };
            if let Ok(pq) = pq.parse::<axum::http::uri::PathAndQuery>() {
                let mut parts = uri.into_parts();
                parts.path_and_query = Some(pq);
                if let Ok(u) = axum::http::Uri::from_parts(parts) {
                    *req.uri_mut() = u;
                }
            }
        }
    }
    next.run(req).await
}

/// Public lookups that make the server ask outside indexers (NFTs, tokens, names) about an
/// address: the signed-in wallet and its linked wallets always; any other address at most
/// `PUBLIC_LOOKUPS_PER_MIN` new ones a minute site-wide (repeats within 10 minutes are free), so
/// nobody can get this server throttled by the indexers everyone's holder checks depend on.
const PUBLIC_LOOKUPS_PER_MIN: usize = 30;
static PUBLIC_LOOKUPS: std::sync::LazyLock<std::sync::Mutex<(Vec<i64>, HashMap<String, i64>)>> = std::sync::LazyLock::new(|| std::sync::Mutex::new((Vec::new(), HashMap::new())));

fn lookup_allowed(s: &AppState, headers: &HeaderMap, address: &str) -> Result<(), ApiError> {
    let token = headers.get("x-kaspass-session").and_then(|v| v.to_str().ok()).unwrap_or_default();
    if let Some(me) = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(token) {
        if s.wallets.of(&me).iter().any(|w| w == address) || me == address {
            return Ok(());
        }
    }
    let now = wall_ms();
    let mut g = PUBLIC_LOOKUPS.lock().unwrap_or_else(|e| e.into_inner());
    let (times, seen) = &mut *g;
    if seen.get(address).is_some_and(|t| now - t < 600_000) {
        return Ok(());
    }
    times.retain(|t| now - t < 60_000);
    let cap = if crate::test_switch("KASPASS_SIM_SERVE").is_some() { 10_000 } else { PUBLIC_LOOKUPS_PER_MIN };
    if times.len() >= cap {
        return Err((StatusCode::TOO_MANY_REQUESTS, "lots of wallet checks right now; please try again in a minute".into()));
    }
    times.push(now);
    if seen.len() > 10_000 {
        seen.retain(|_, t| now - *t < 600_000);
    }
    seen.insert(address.to_string(), now);
    Ok(())
}

/// The paying wallet must be the signed-in one (session in the `x-kaspass-session` header), so
/// nobody can make the server build, price and log transactions for any address on chain.
fn signed_in_as(s: &AppState, headers: &HeaderMap, address: &str) -> Result<(), ApiError> {
    let token = headers.get("x-kaspass-session").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let who = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(token).ok_or((StatusCode::UNAUTHORIZED, "please sign in with Kaspire first".to_string()))?;
    let same = matches!((owner_hash_of(&who), owner_hash_of(address.trim())), (Ok(a), Ok(b)) if a == b);
    if !same {
        return Err((StatusCode::FORBIDDEN, "sign in with the wallet that pays".into()));
    }
    Ok(())
}

/// Set when the owner record is no longer on chain with the wallets record.json names (handed
/// over with another tool, for example after a leaked seed): the old owner must not keep the
/// dashboard, so creator actions stop until record.json is brought up to date.
static RECORD_MOVED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Checks every 5 minutes that the owner record is where record.json says. Two misses in a row
/// (with the node answering) lock the dashboard; finding it again unlocks it.
async fn record_watch(s: AppState) {
    let mut misses = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        let Some(r) = s.shop.as_ref().and_then(|sh| sh.record()) else { continue };
        let got = {
            let chain = s.chain.lock().await;
            crate::find_record(&chain, &r).await
        };
        match got {
            Ok(_) => {
                misses = 0;
                if RECORD_MOVED.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    println!("dashboard: the owner record is back where record.json says; creator actions are on again");
                }
            }
            Err(e) if e.downcast_ref::<crate::RecordMissing>().is_some() => {
                misses += 1;
                if misses >= 2 && !RECORD_MOVED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("dashboard: the owner record isn't on chain with the wallets in record.json (handed over elsewhere?); creator actions are locked");
                }
            }
            Err(_) => {}
        }
    }
}

fn creator_of(s: &AppState, session: &str) -> Result<String, ApiError> {
    let address = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    if RECORD_MOVED.load(std::sync::atomic::Ordering::Relaxed) {
        return Err((StatusCode::FORBIDDEN, "this site's owner record has moved to other wallets on chain, so the dashboard is locked until the server is updated".into()));
    }
    if address != site_owner(s) {
        return Err((StatusCode::FORBIDDEN, "this dashboard is for the membership's creator".into()));
    }
    Ok(address)
}

/// Everything the creator's dashboard shows, from the chain.
async fn creator_overview(State(s): State<AppState>, Query(lq): Query<LevelQ>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let address = creator_of(&s, &q.session)?;
    // The keeper's balance first: never wait for the chain while holding the snapshot (the
    // indexer takes them in the other order).
    let keeper_bal: Option<u64> = match s.shop.as_ref().and_then(|sh| sh.keeper.clone()) {
        Some(k) if s.card.settings.v4.is_some() => {
            let chain = s.chain.lock().await;
            Some(chain.utxos_at(&k.spk).await.map(|u| u.iter().filter(|u| u.entry.covenant_id.is_none()).map(|u| u.entry.amount).sum()).unwrap_or(0))
        }
        _ => None,
    };
    let snap = s.snap.read().await;
    if !snap.synced {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "still reading the chain, try again in a moment".into()));
    }
    let paid: u64 = snap.events.iter().map(|e| e.amount_sompi).sum();
    let fees: u64 = snap.events.iter().map(|e| e.fee_sompi).sum();
    let earned: u64 = snap.events.iter().map(|e| e.creator_sompi).sum();
    let count = |k: &str| snap.events.iter().filter(|e| e.kind == k).count();
    let sale = snap.lanes.iter().find(|l| l.lane.remaining != 0).or(snap.lanes.first()).and_then(|l| read_sale(&s.home, &s.card.covenant_id).and_then(|sale| sale_view(&sale, &l.lane)));
    let v4 = match (&s.card.settings.v4, &s.shop) {
        (Some(v), Some(shop)) => {
            let keeper = shop.keeper.as_ref().map(|k| serde_json::json!({ "address": k.addr.to_string(), "balance_sompi": keeper_bal.unwrap_or(0) }));
            let auto_on = snap.passes.iter().filter(|p| !p.lifetime && p.balance_sompi >= p.rate_sompi).count();
            serde_json::json!({
                "max_discount_bps": v.max_discount_bps,
                "auto_window_ms": v.auto_window_ms,
                "discounts": shop.can_discount(),
                "holder_bps": holder_setting(&s),
                "keeper": keeper,
                "auto_renewing": auto_on,
                "prepaid_sompi": snap.passes.iter().map(|p| p.balance_sompi).sum::<u64>(),
            })
        }
        _ => serde_json::Value::Null,
    };
    Ok(Json(serde_json::json!({
        "v4": v4,
        "name": tier_name(&s.home, &s.card.covenant_id),
        "creator_name": creator_display(&s.home, &s.card.creator_address),
        "pass_word": pass_word(&s.home, &s.card.covenant_id),
        "club_name": site_names(&s.home).club_name,
        "level_word": level_word(&s.home),
        "level": { "covenant_id": s.card.covenant_id, "rank": s.rank(), "name": s.level_name(), "perks": level_perks(&s.home).get(&s.card.covenant_id).cloned().unwrap_or_default() },
        "levels": s.levels.iter().enumerate().map(|(i, l)| serde_json::json!({ "covenant_id": l.card.covenant_id, "id": short_id(&l.card.covenant_id), "rank": i, "name": level_name(&s.home, &s.levels, i) })).collect::<Vec<_>>(),
        "creator": address,
        "card": s.card,
        "sale": sale,
        "lanes": snap.lanes,
        "passes": members::enrich(&s, &snap.passes),
        "events": snap.events,
        "now_ms": snap.updated_ms,
        "totals": {
            "paid_sompi": paid,
            "earned_sompi": earned,
            "fees_sompi": fees,
            "active": snap.passes.iter().filter(|p| p.active).count(),
            "live": snap.passes.len(),
            "sales": count("monthly") + count("lifetime") + count("term"),
            "terms": count("term"),
            "renewals": count("renew") + count("autorenew"),
            "autorenewals": count("autorenew"),
            "upgrades": count("upgrade"),
            "transfers": count("transfer"),
            "trials": count("trial") + count("self_trial"),
        },
    })))
}

#[derive(Deserialize)]
struct CreatorRequest {
    session: String,
    action: String,
    /// Recipient, for "trial" and "gift".
    #[serde(default)]
    to: Option<String>,
    /// "monthly" or "lifetime", for "gift".
    #[serde(default)]
    kind: Option<String>,
    /// New prices in KAS, for "prices".
    #[serde(default)]
    price_kas: Option<f64>,
    #[serde(default)]
    lifetime_kas: Option<f64>,
    #[serde(default)]
    credit: Option<bool>,
    /// v4 commitment term: price in KAS (0 = not offered) and length in periods.
    #[serde(default)]
    term_kas: Option<f64>,
    #[serde(default)]
    term_periods: Option<i64>,
}

async fn creator_prepare(State(s): State<AppState>, Query(lq): Query<LevelQ>, Json(req): Json<CreatorRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    let shop = shop_of(&s)?;
    let creator = creator_of(&s, &req.session)?;
    let lane = current_lane(&s).await?;
    let mut named: Option<String> = None;
    let (quote, mut out) = match req.action.as_str() {
        "prices" => {
            let k = |x: Option<f64>| -> Result<u64, ApiError> {
                let x = x.ok_or_else(|| bad("missing price"))?;
                if !(0.0..=1e9).contains(&x) {
                    return Err(bad("prices must be between 0 and 1,000,000,000 KAS"));
                }
                Ok((x * crate::SOMPI as f64).round() as u64)
            };
            let price = k(req.price_kas)?;
            // v5: only the monthly KAS ceiling changes, and never below today's price (buyers would
            // then pay less than the plan's price). Lifetime, commitment and credit stay as launched.
            let v5_keep = if is_v5(&s.card) {
                let lp = crate::prices::level_prices(&s.home).get(&s.card.covenant_id).cloned();
                let today = lp.and_then(|p| crate::prices::fresh().or_else(|| (p.currency == "KAS").then(crate::prices::Rates::kas_only)).and_then(|r| r.kas(&p.currency, p.amount)));
                let today = today.ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "price unavailable, try again shortly".to_string()))?;
                if (price as f64) < today * crate::SOMPI as f64 {
                    return Err(bad(&format!("the ceiling must be at least today's price ({today:.2} KAS)")));
                }
                Some((lane.lane.life, lane.lane.credit, lane.lane.term_price, lane.lane.term_ms))
            } else {
                None
            };
            let (life, credit) = match v5_keep {
                Some((l, c, _, _)) => (l, c),
                None => (k(req.lifetime_kas)?, req.credit.unwrap_or(false)),
            };
            let term_kas = if v5_keep.is_some() { None } else { req.term_kas };
            let (term_price, term_ms) = match term_kas {
                Some(t) => {
                    let tp = k(Some(t))?;
                    let n = req.term_periods.unwrap_or(0);
                    if tp > 0 && !(1..=120).contains(&n) {
                        return Err(bad("a commitment term runs 1 to 120 periods"));
                    }
                    (tp, if tp > 0 { n * s.card.settings.period_ms } else { 0 })
                }
                None => (lane.lane.term_price, lane.lane.term_ms),
            };
            let chain = s.chain.lock().await;
            shop.prepare_prices_v4(&chain, &creator, &lane, price, life, credit, term_price, term_ms).await.map_err(bad)?
        }
        "trial" | "gift" => {
            let (to, label) = resolve_recipient(&s, req.to.as_deref().unwrap_or_default()).await?;
            named = label;
            if lane.lane.remaining == 0 {
                return Err((StatusCode::CONFLICT, "sold out: every pass for this membership has been sold".into()));
            }
            no_duplicate(&s, to.trim()).await?;
            let kind = match (req.action.as_str(), req.kind.as_deref()) {
                ("trial", _) => crate::Sale::Trial,
                (_, Some("lifetime")) => crate::Sale::Lifetime,
                _ => crate::Sale::Monthly,
            };
            if kind == crate::Sale::Trial {
                // A free trial, through the owner record.
                let chain = s.chain.lock().await;
                shop.prepare_trial_v5(&chain, &creator, to.trim(), &lane).await.map_err(bad)?
            } else {
                // A gift is paid at today's price (with the recipient's founding price or upgrade
                // credit), never at the KAS ceiling.
                let q = v5_quote(&s, if kind == crate::Sale::Monthly { V5For::Buy(to.trim()) } else { V5For::BuyKind(to.trim(), kind) }).await?.ok_or_else(|| bad("no price for this plan"))?;
                let chain = s.chain.lock().await;
                let r = shop.prepare_gift_v5(&chain, &creator, to.trim(), &lane, q.bps, kind).await.map_err(bad)?;
                // Paid by the owner from their own wallet into their own vault.
                crate::prices::later_payment(&r.0.txid(), crate::prices::PayNote { currency: q.currency.clone(), amount: q.amount, credit: q.credit, kind: "gift".into() });
                r
            }
        }
        _ => return Err(bad("action must be prices, trial or gift")),
    };
    if let Some(n) = named {
        out["summary"]["text"] = serde_json::json!(format!("{n}. {}", out["summary"]["text"].as_str().unwrap_or_default()));
        out["summary"]["name"] = serde_json::json!(n);
    }
    let id = store_quote(&s, quote);
    out["quote"] = serde_json::json!(id);
    Ok(Json(out))
}

#[derive(Deserialize)]
struct LaunchRequest {
    session: String,
    #[serde(flatten)]
    form: crate::shop::LaunchForm,
}

/// Anyone signed in can launch their own tier; they become its creator.
async fn launch_prepare(State(s): State<AppState>, Json(req): Json<LaunchRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let shop = shop_of(&s)?;
    let address = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&req.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    if CREATOR_ONLY_LAUNCH.load(std::sync::atomic::Ordering::Relaxed) && address != s.card.creator_address {
        return Err((StatusCode::FORBIDDEN, "only this site's creator can launch levels here".to_string()));
    }
    let (quote, mut out) = {
        let chain = s.chain.lock().await;
        shop.prepare_launch(&chain, &address, &req.form).await.map_err(bad)?
    };
    let id = store_quote(&s, quote);
    out["quote"] = serde_json::json!(id);
    Ok(Json(out))
}

const DASHBOARD: &str = include_str!("../web/dashboard.html");

async fn dashboard(headers: HeaderMap) -> impl IntoResponse {
    // On KasPass's own site, the membership page is at /demo.
    ([(header::CACHE_CONTROL, "no-store")], Html(if home::is_home(&headers) { home::to_demo(DASHBOARD) } else { DASHBOARD.to_string() }))
}

// ---------------------------------------------------------------------------
// Telegram

fn bot_of(s: &AppState) -> Result<Arc<crate::telegram::Bot>, ApiError> {
    s.telegram.clone().ok_or((StatusCode::NOT_FOUND, "this site has no Telegram group".to_string()))
}

/// Whether the site offers Telegram groups (the members area shows a button if so).
async fn telegram_info(State(s): State<AppState>) -> Json<serde_json::Value> {
    match &s.telegram {
        Some(b) => Json(serde_json::json!({ "enabled": true, "bot": b.username, "group_connected": b.has_group() })),
        None => Json(serde_json::json!({ "enabled": false })),
    }
}

#[derive(Deserialize)]
struct LinkRequest {
    session: String,
}

/// A one-time t.me link that connects the signed-in wallet to the member's Telegram account.
async fn telegram_link(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<LinkRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let bot = bot_of(&s)?;
    let address = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&req.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    Ok(Json(serde_json::json!({ "url": bot.link_url(&address, site_of(&headers)) })))
}

/// The creator's view: the bot, its groups and who each is for, and every linked member.
async fn creator_telegram(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    let Some(bot) = s.telegram.clone() else {
        return Ok(Json(serde_json::json!({ "enabled": false })));
    };
    let groups: Vec<serde_json::Value> = bot
        .groups()
        .into_iter()
        .map(|g| serde_json::json!({ "title": g.title, "for": g.key.as_deref().map(crate::telegram::key_label), "assigned": g.key.is_some() }))
        .collect();
    let titles: HashMap<i64, String> = bot.groups().into_iter().map(|g| (g.chat_id, g.title)).collect();
    let mut links = Vec::new();
    for l in bot.links() {
        let keys = (s.access)(&l.address).await;
        links.push(serde_json::json!({
            "name": l.name, "address": l.address,
            "qualifies": keys.map(|k| k.iter().map(|x| crate::telegram::key_label(x)).collect::<Vec<_>>()),
            "in": l.in_groups.iter().filter_map(|c| titles.get(c).cloned()).collect::<Vec<_>>(),
        }));
    }
    let unmanaged: Vec<serde_json::Value> = bot.unmanaged().into_iter().map(|g| serde_json::json!({ "title": g.title, "chat_id": g.chat_id })).collect();
    Ok(Json(serde_json::json!({ "enabled": true, "bot": bot.username, "group_connected": bot.has_group(), "groups": groups, "unmanaged": unmanaged, "links": links })))
}

/// The site's own address as the visitor sees it ("https://host/").
fn site_of(headers: &HeaderMap) -> Option<String> {
    headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(|h| {
        let local = h.starts_with("127.0.0.1") || h.starts_with("localhost");
        format!("{}://{h}/", if local { "http" } else { "https" })
    })
}

// ---------------------------------------------------------------------------
// Discord

fn discord_of(s: &AppState) -> Result<Arc<crate::discord::Discord>, ApiError> {
    s.discord.clone().ok_or((StatusCode::NOT_FOUND, "this site has no Discord server".to_string()))
}

async fn discord_info(State(s): State<AppState>) -> Json<serde_json::Value> {
    match &s.discord {
        Some(d) => {
            let (guild, ready) = d.status();
            Json(serde_json::json!({ "enabled": true, "ready": ready, "server": guild }))
        }
        None => Json(serde_json::json!({ "enabled": false })),
    }
}

async fn discord_link(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<LinkRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let d = discord_of(&s)?;
    let address = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&req.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let site = site_of(&headers).ok_or_else(|| bad("missing Host header"))?;
    Ok(Json(serde_json::json!({ "url": d.authorize_url(&address, &site) })))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Where Discord sends the member back after they approve.
async fn discord_callback(State(s): State<AppState>, Query(q): Query<CallbackQuery>) -> Html<String> {
    let (title, text) = match (&s.discord, &q.code, &q.state) {
        (None, _, _) => ("Discord isn't set up", "This site doesn't have a Discord server connected.".to_string()),
        (_, _, _) if q.error.is_some() => ("Not connected", "You cancelled the Discord sign-in, so nothing was linked. You can try again from the members area.".to_string()),
        (Some(d), Some(code), Some(state)) => {
            let access = s.access.clone();
            match d.complete(code, state, &*access).await {
                Ok(t) => ("Discord connected", t),
                Err(e) => ("Something went wrong", e.to_string()),
            }
        }
        _ => ("Something went wrong", "Discord didn't send the expected details back. Try again from the members area.".to_string()),
    };
    let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    // Holder-club members came from the club page; everyone else from the storefront.
    let back = match club_of(&s) {
        Some(c) => ("/club", c.rules.club.clone()),
        None => ("/", "the membership page".to_string()),
    };
    Html(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>KasPass</title>\
<body style='background:#0b0f0f;color:#e6f2ef;font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0;padding:16px'>\
<div style='max-width:460px;background:#111817;border:1px solid #1f2b29;border-radius:14px;padding:24px'><h2 style='margin-top:0;color:#49d6c4'>{}</h2><p style='line-height:1.5'>{}</p>\
<p><a href='{}' style='color:#49d6c4'>Back to {}</a>{}</p></div></body>",
        esc(title),
        esc(&text),
        back.0,
        esc(&back.1),
        if back.0 == "/club" { " · <a href='/' style='color:#49d6c4'>Membership page</a>" } else { "" }
    ))
}

async fn creator_discord(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    let Some(d) = s.discord.clone() else {
        return Ok(Json(serde_json::json!({ "enabled": false })));
    };
    let (guild, ready) = d.status();
    let names: HashMap<String, String> = d.managed().into_iter().collect();
    let mut links = Vec::new();
    for l in d.links() {
        let keys = (s.access)(&l.address).await;
        links.push(serde_json::json!({
            "name": l.name, "address": l.address,
            "qualifies": keys.map(|k| k.iter().filter_map(|x| names.get(x).cloned()).collect::<Vec<_>>()),
            "roles": l.keys.iter().filter_map(|x| names.get(x).cloned()).collect::<Vec<_>>(),
        }));
    }
    Ok(Json(serde_json::json!({
        "enabled": true, "bot": d.bot_name, "invite_bot_url": d.invite_bot_url(), "ready": ready, "server": guild,
        "roles": d.managed().into_iter().map(|(_, n)| n).collect::<Vec<_>>(), "links": links,
    })))
}

// ---------------------------------------------------------------------------
// Holder clubs and linked wallets

/// Every access key a member has right now: "pass" for an active paid pass on any of their wallets,
/// and "tier:<name>" for each holder-club tier they qualify for. None if it can't be decided yet.
async fn access_keys(
    levels: &Arc<Vec<Level>>,
    club: &Arc<std::sync::RwLock<Option<Arc<crate::clubs::Club>>>>,
    wallets: &Arc<crate::clubs::Wallets>,
    address: &str,
) -> Option<Vec<String>> {
    let all = wallets.of(address);
    let owners: Vec<String> = all.iter().filter_map(|a| owner_hash_of(a).ok().map(|h| h.to_string())).collect();
    let top = top_rank(levels, &owners).await?;
    let has_pass = top.is_some();
    let mut keys = Vec::new();
    if let Some(top) = top {
        keys.push("pass".to_string());
        // A level includes the perks of every level below it.
        keys.extend(levels[..=top].iter().map(|l| format!("level:{}", short_id(&l.card.covenant_id))));
    }
    let club = club.read().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(c) = club {
        let st = c.standing(&all, has_pass).await?;
        keys.extend(st.tiers.into_iter().map(|t| format!("tier:{t}")));
    }
    Some(keys)
}

/// The paid levels groups and roles can be for, as (access key, name): only when there are several.
fn level_choices(s: &AppState) -> Vec<(String, String)> {
    if s.levels.len() < 2 {
        return Vec::new();
    }
    (0..s.levels.len()).map(|i| (format!("level:{}", short_id(&s.levels[i].card.covenant_id)), level_name(&s.home, &s.levels, i))).collect()
}

/// Tells the bots which levels and club tiers exist (Telegram groups can be assigned to them,
/// Discord makes their roles).
fn sync_bots(s: &AppState) {
    let tiers: Vec<(String, String)> = s.club.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.tiers.iter().map(|t| (t.name.clone(), t.discord_role.clone())).collect()).unwrap_or_default();
    let levels = level_choices(s);
    crate::telegram::set_level_names(levels.clone());
    if let Some(b) = &s.telegram {
        let mut choices = levels.clone();
        choices.extend(tiers.iter().map(|(n, _)| (format!("tier:{n}"), n.clone())));
        b.set_choices(choices);
    }
    if let Some(d) = &s.discord {
        let mut wanted = vec![("pass".to_string(), crate::discord::PASS_ROLE.to_string())];
        wanted.extend(levels);
        wanted.extend(tiers.into_iter().map(|(n, r)| (format!("tier:{n}"), r)));
        d.set_wanted(wanted);
    }
}

fn club_of(s: &AppState) -> Option<Arc<crate::clubs::Club>> {
    s.club.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn club_summary(c: &crate::clubs::Club) -> serde_json::Value {
    let sizes: HashMap<String, usize> = c.band_sizes().into_iter().collect();
    serde_json::json!({
        "enabled": true,
        "club": c.rules.club,
        "collection": c.rules.collection,
        "count_listed": c.rules.count_listed,
        "ranked": c.ranked(),
        "bands": c.rules.bands.iter().map(|b| serde_json::json!({ "name": b.name, "ranks": b.ranks, "credits": b.credits, "nfts": sizes.get(&b.name).copied().unwrap_or(0) })).collect::<Vec<_>>(),
        "tiers": c.tiers.iter().map(|t| serde_json::json!({ "name": t.name, "rule": t.rule.describe(), "discord_role": t.discord_role })).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize, Default)]
struct ClubFor {
    #[serde(default, rename = "for")]
    for_: String,
}

async fn club_info(State(s): State<AppState>, Query(f): Query<ClubFor>) -> Json<serde_json::Value> {
    // Example creators on the demo have no holder club of their own; and the creator can leave
    // the club off their membership page (the club page itself still works).
    if examples::current().is_some() || (f.for_ == "page" && page_profile(&s.home).hide_club) {
        return Json(serde_json::json!({ "enabled": false }));
    }
    match club_of(&s) {
        Some(c) => {
            let mut v = club_summary(&c);
            // Where each tier gets you: Telegram groups (by name) and the Discord server.
            if let Some(b) = &s.telegram {
                v["telegram"] = serde_json::json!(b.groups().into_iter().filter_map(|g| g.key.map(|k| serde_json::json!({ "title": g.title, "key": k }))).collect::<Vec<_>>());
            }
            if let Some(d) = &s.discord {
                let (guild, ready) = d.status();
                v["discord"] = serde_json::json!({ "server": guild, "ready": ready });
            }
            Json(v)
        }
        None => Json(serde_json::json!({ "enabled": false })),
    }
}

const CLUB_PAGE: &str = include_str!("../web/club.html");
const HELP_PAGE: &str = include_str!("../web/help.html");

/// How to get Kaspire, sign in, buy, auto-renew, renew, upgrade and close (served as /help,
/// so partner sites can link to steps that stay in step with the buy page).
async fn help_page(headers: HeaderMap) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "public, max-age=300"), (header::VARY, "Host")], Html(if home::is_home(&headers) { home::to_demo(HELP_PAGE) } else { HELP_PAGE.to_string() }))
}

/// The holder club's own page: sign in, see your standing, join the groups. No paid passes.
async fn club_page(headers: HeaderMap) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Html(if home::is_home(&headers) { home::to_demo(CLUB_PAGE) } else { CLUB_PAGE.to_string() }))
}

#[derive(Deserialize)]
struct AddressQuery {
    address: String,
}

async fn standing_for(s: &AppState, wallets: &[String]) -> Result<serde_json::Value, ApiError> {
    let c = club_of(s).ok_or((StatusCode::NOT_FOUND, "this site has no holder club".to_string()))?;
    let owners: Vec<String> = wallets.iter().filter_map(|a| owner_hash_of(a).ok().map(|h| h.to_string())).collect();
    let has_pass = top_rank(&s.levels, &owners).await.flatten().is_some();
    let st = c.standing(wallets, has_pass).await.ok_or((StatusCode::SERVICE_UNAVAILABLE, "the NFT indexer didn't answer; try again in a minute".to_string()))?;
    Ok(serde_json::to_value(&st).unwrap_or_default())
}

/// Preview: what a wallet holds and which tiers it qualifies for (public, like any explorer).
async fn club_check(State(s): State<AppState>, headers: HeaderMap, Query(q): Query<AddressQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let _: kaspa_addresses::Address = q.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    lookup_allowed(&s, &headers, &q.address)?;
    Ok(Json(standing_for(&s, &[q.address.clone()]).await?))
}

/// The signed-in member's wallets and standing.
async fn club_me(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let address = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&q.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let wallets = s.wallets.of(&address);
    let standing = if club_of(&s).is_some() { Some(standing_for(&s, &wallets).await?) } else { None };
    let keys = (s.access)(&address).await.unwrap_or_default();
    Ok(Json(serde_json::json!({ "address": address, "wallets": wallets, "standing": standing, "access": keys.iter().map(|k| crate::telegram::key_label(k)).collect::<Vec<_>>(), "has_access": !keys.is_empty() })))
}

#[derive(Deserialize)]
struct WalletLinkRequest {
    session: String,
    address: String,
    message: String,
    signature: String,
}

/// Links another wallet: the member signs a message with that wallet naming the wallet they're signed in with.
async fn wallet_link(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<WalletLinkRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let primary = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&req.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    let addr: kaspa_addresses::Address = req.address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    if addr.version != kaspa_addresses::Version::PubKey {
        return Err(bad("only regular (P2PK) addresses can be linked"));
    }
    let link_to = req.message.lines().find_map(|l| l.strip_prefix("Link to: ")).map(str::trim);
    if link_to != Some(primary.as_str()) {
        return Err(bad("this message links to a different wallet"));
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_string();
    let sig = auth::decode_signature(&req.signature).map_err(bad)?;
    auth::verify_kip5(&req.message, &sig, &addr.payload).map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;
    auth::check_message(&req.message, &req.address, &host, &mut s.sessions.lock().unwrap_or_else(|e| e.into_inner())).map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;
    s.wallets.link(&primary, &req.address).map_err(bad)?;
    members::remember(&s.home, &req.address);
    Ok(Json(serde_json::json!({ "wallets": s.wallets.of(&primary) })))
}

#[derive(Deserialize)]
struct WalletUnlinkRequest {
    session: String,
    address: String,
}

async fn wallet_unlink(State(s): State<AppState>, Json(req): Json<WalletUnlinkRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let primary = s.sessions.lock().unwrap_or_else(|e| e.into_inner()).address_for(&req.session).ok_or((StatusCode::UNAUTHORIZED, "please sign in again".to_string()))?;
    s.wallets.unlink(&primary, &req.address);
    Ok(Json(serde_json::json!({ "wallets": s.wallets.of(&primary) })))
}

async fn creator_club(State(s): State<AppState>, Query(q): Query<SessionQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &q.session)?;
    Ok(Json(club_of(&s).map(|c| club_summary(&c)).unwrap_or_else(|| serde_json::json!({ "enabled": false }))))
}

#[derive(Deserialize)]
struct ClubUpload {
    session: String,
    rules: serde_json::Value,
    #[serde(default)]
    ranks: Option<serde_json::Value>,
}

/// The creator uploads the club's rules (and optionally the ranks list); it's checked, saved and applied.
async fn creator_club_upload(State(s): State<AppState>, Json(req): Json<ClubUpload>) -> Result<Json<serde_json::Value>, ApiError> {
    creator_of(&s, &req.session)?;
    let mut rules: crate::clubs::Rules = serde_json::from_value(req.rules).map_err(|e| bad(format!("rules: {e}")))?;
    let ranks = match req.ranks {
        Some(v) => Some(parse_ranks(v).map_err(bad)?),
        None => None,
    };
    if let Some(r) = &ranks {
        let file = format!("{}-ranks.json", rules.collection.tick.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>());
        std::fs::write(s.home.join(&file), serde_json::to_string(r).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        rules.ranks = None;
        rules.ranks_file = Some(file);
    }
    let club = crate::clubs::Club::from_rules(rules.clone(), ranks, &s.home).map_err(bad)?;
    std::fs::write(s.home.join("club.json"), serde_json::to_string_pretty(&rules).unwrap_or_default()).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let summary = club_summary(&club);
    *s.club.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(club));
    sync_bots(&s);
    println!("club: rules for \"{}\" loaded from the dashboard", rules.club);
    Ok(Json(summary))
}

/// Accepts ranks as {"id": rank} or as [{"id": .., "rank": ..}] (what a spreadsheet export usually looks like).
fn parse_ranks(v: serde_json::Value) -> anyhow::Result<std::collections::BTreeMap<String, u64>> {
    let mut out = std::collections::BTreeMap::new();
    match v {
        serde_json::Value::Object(m) => {
            for (k, r) in m {
                let rank = r.as_u64().or_else(|| r.as_f64().map(|f| f as u64)).ok_or_else(|| anyhow::anyhow!("rank for {k} isn't a number"))?;
                out.insert(k, rank);
            }
        }
        serde_json::Value::Array(a) => {
            for row in a {
                let id = row.get("id").or(row.get("tokenId")).or(row.get("token_id")).ok_or_else(|| anyhow::anyhow!("each row needs an id"))?;
                let id = id.as_u64().map(|n| n.to_string()).or_else(|| id.as_f64().map(|f| (f as u64).to_string())).or_else(|| id.as_str().map(String::from)).ok_or_else(|| anyhow::anyhow!("bad id {id}"))?;
                let r = row.get("rank").or(row.get("Rank")).and_then(|r| r.as_u64().or_else(|| r.as_f64().map(|f| f as u64))).ok_or_else(|| anyhow::anyhow!("row {id} needs a rank"))?;
                out.insert(id, r);
            }
        }
        _ => anyhow::bail!("ranks must be an object {{id: rank}} or a list of {{id, rank}} rows"),
    }
    if out.is_empty() {
        anyhow::bail!("the ranks list is empty");
    }
    Ok(out)
}

#[cfg(test)]
mod supervise_test {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_crashed_job_is_started_again() {
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        super::supervise("test job", move || {
            let r = r.clone();
            async move {
                if r.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("boom");
                }
                std::future::pending::<()>().await;
            }
        });
        for _ in 0..12 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2, "the job should have been started again after crashing");
    }
}

#[cfg(test)]
mod pass_word_test {
    use super::{clean_word, plural};

    #[test]
    fn words() {
        assert_eq!(plural("pass"), "passes");
        assert_eq!(plural("membership"), "memberships");
        assert_eq!(plural("entry"), "entries");
        assert_eq!(plural("key"), "keys");
        assert_eq!(plural("subscription"), "subscriptions");
        assert_eq!(clean_word("  Season   Ticket ").unwrap(), "season ticket");
        assert_eq!(clean_word("").unwrap(), "");
        assert!(clean_word("<b>x</b>").is_err());
        assert!(clean_word("an extremely long word for a pass").is_err());
    }
}

#[cfg(test)]
mod clean_html_test {
    use super::clean_html;

    #[test]
    fn keeps_formatting_and_drops_anything_that_runs() {
        assert_eq!(clean_html("<p><b>Hi</b> <i>there</i></p><ul><li>one</li></ul>"), "<p><b>Hi</b> <i>there</i></p><ul><li>one</li></ul>");
        assert_eq!(clean_html("<p>x<script>alert(1)</script></p>"), "<p>x</p>");
        assert_eq!(clean_html("<p onclick=\"alert(1)\" style=\"color:red\">x</p>"), "<p>x</p>");
        assert_eq!(clean_html("<img src=x onerror=alert(1)>"), "");
        let bad = clean_html("<a href=\"javascript:alert(1)\">x</a>");
        assert!(!bad.contains("href") && !bad.contains("javascript") && bad.contains("noopener"), "{bad}");
        let good = clean_html("<a href=\"https://kaspa.org\">k</a>");
        assert!(good.contains("href=\"https://kaspa.org\"") && good.contains("target=\"_blank\"") && good.contains("nofollow"), "{good}");
        assert!(!clean_html("<a href=\"/dashboard\">x</a>").contains("href"));
        assert_eq!(clean_html("<p><br></p>"), "");
        assert_eq!(clean_html("<h1>Big</h1><div>plain</div>"), "Bigplain");
    }
}
