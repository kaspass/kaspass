//! Paying from any wallet with a QR code. Each order gets its own pay-in address (see
//! contracts/payin.sil). This file creates orders, shows their status, and runs the watcher that
//! completes an order as soon as its payment lands, or refunds it after its refund time.

use super::*;

const ORDERS_FILE: &str = "payin-orders.json";

/// How long the QR code's amount is quoted for.
fn valid_ms() -> i64 {
    crate::test_switch("KASPASS_PAYIN_VALID_MIN").and_then(|v| v.parse::<i64>().ok()).unwrap_or(15) * MINUTE_MS
}
/// When an unfinished order's payment goes back to the buyer.
fn refund_ms() -> i64 {
    crate::test_switch("KASPASS_PAYIN_REFUND_MIN").and_then(|v| v.parse::<i64>().ok()).unwrap_or(60) * MINUTE_MS
}
const MINUTE_MS: i64 = 60_000;
/// How long after the refund time refunds start (the chain's clock trails the wall clock).
fn grace_ms() -> i64 {
    crate::test_switch("KASPASS_PAYIN_GRACE_S").and_then(|v| v.parse::<i64>().ok()).unwrap_or(120) * 1000
}
/// No completing this close to the refund time (the two must never race).
fn margin_ms() -> i64 {
    crate::test_switch("KASPASS_PAYIN_MARGIN_S").and_then(|v| v.parse::<i64>().ok()).unwrap_or(300) * 1000
}
/// A payment may come up this short of the QR amount (a wallet taking its fee out of it) and still complete.
const SHORT_OK: u64 = crate::SOMPI / 2;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Order {
    pub id: String,
    pub level: String,
    pub buyer: String,
    /// "buy" or "renew".
    pub action: String,
    pub bps: u64,
    pub renew_ppm: u64,
    pub periods: u64,
    /// What the purchase takes from the payment (price, bond, prepaid), before the network fee.
    pub uses: u64,
    /// The contract's limit on what may leave for the purchase; also the QR amount.
    pub max_spend: u64,
    pub currency: String,
    pub price: f64,
    pub credit: f64,
    pub created_ms: i64,
    pub expires_ms: i64,
    pub refund_after_ms: i64,
    /// waiting, completed, refunded, expired
    pub status: String,
    #[serde(default)]
    pub paid: u64,
    #[serde(default)]
    pub txid: Option<String>,
    #[serde(default)]
    pub refunds: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub until_ms: Option<i64>,
    /// The one-time pay-in address (kept so it can be listed later, in `/v1/member`).
    #[serde(default)]
    pub pay_to: Option<String>,
}

/// Finished orders older than 30 days leave `payin-orders.json`; this keeps what's needed to
/// recognise their transactions later (for tax tools): who, which address, and what happened.
const HISTORY_FILE: &str = "payin-history.json";
const HISTORY_KEEP: usize = 20_000;

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Past {
    buyer: String,
    level: String,
    pay_to: String,
    created_ms: i64,
    status: String,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    refunds: Vec<String>,
}

fn history(home: &std::path::Path) -> Vec<Past> {
    std::fs::read_to_string(home.join(HISTORY_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn keep_history(s: &AppState, gone: Vec<Order>) {
    if gone.is_empty() {
        return;
    }
    let mut all = history(&s.home);
    for o in gone {
        let pay_to = o.pay_to.clone().or_else(|| s.shop.as_ref().and_then(|sh| sh.payin_for(&o.buyer, o.max_spend, o.refund_after_ms).ok()).map(|x| x.2));
        let Some(pay_to) = pay_to else { continue };
        all.push(Past { buyer: o.buyer, level: o.level, pay_to, created_ms: o.created_ms, status: o.status, txid: o.txid, refunds: o.refunds });
    }
    if all.len() > HISTORY_KEEP {
        let extra = all.len() - HISTORY_KEEP;
        all.drain(..extra);
    }
    let tmp = s.home.join(format!("{HISTORY_FILE}.tmp"));
    if let Ok(t) = serde_json::to_string_pretty(&all) {
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, s.home.join(HISTORY_FILE));
        }
    }
}

/// Every pay-in order this wallet was given (open, finished, and older ones), oldest first.
/// `open_details`: show still-open orders' pay-in address and start time (only to the signed-in
/// buyer; together with public data they'd describe the order's contract).
pub fn for_buyer(s: &AppState, buyer: &str, open_details: bool) -> Vec<serde_json::Value> {
    let mut out: Vec<(i64, serde_json::Value)> = history(&s.home)
        .into_iter()
        .filter(|p| p.buyer == buyer)
        .map(|p| (p.created_ms, serde_json::json!({ "pay_to": p.pay_to, "level": p.level, "created_ms": p.created_ms, "status": p.status, "txid": p.txid, "refunds": p.refunds })))
        .collect();
    for o in load(&s.home).into_iter().filter(|o| o.buyer == buyer) {
        let open = o.status == "waiting";
        let pay_to = o.pay_to.clone().or_else(|| s.shop.as_ref().and_then(|sh| sh.payin_for(&o.buyer, o.max_spend, o.refund_after_ms).ok()).map(|x| x.2));
        let (pay_to, created) = if open && !open_details { (None, None) } else { (pay_to, Some(o.created_ms)) };
        out.push((o.created_ms, serde_json::json!({ "pay_to": pay_to, "level": o.level, "created_ms": created, "status": o.status, "txid": o.txid, "refunds": o.refunds })));
    }
    out.sort_by_key(|(t, _)| *t);
    out.into_iter().map(|(_, v)| v).collect()
}

static ORDERS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn load(home: &std::path::Path) -> Vec<Order> {
    std::fs::read_to_string(home.join(ORDERS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(home: &std::path::Path, orders: &[Order]) {
    let tmp = home.join(format!("{ORDERS_FILE}.tmp"));
    if let Ok(t) = serde_json::to_string_pretty(orders) {
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, home.join(ORDERS_FILE));
        }
    }
}

/// Changes one order on disk (others may be changing at the same time).
fn update(home: &std::path::Path, id: &str, f: impl FnOnce(&mut Order)) {
    let _g = ORDERS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(home);
    if let Some(o) = all.iter_mut().find(|o| o.id == id) {
        f(o);
    }
    save(home, &all);
}

fn kas_str(sompi: u64) -> String {
    let s = format!("{}.{:08}", sompi / crate::SOMPI, sompi % crate::SOMPI);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn view(o: &Order, address: &str) -> serde_json::Value {
    serde_json::json!({
        "id": o.id,
        "level": o.level,
        "action": o.action,
        "address": address,
        "amount_kas": kas_str(o.max_spend),
        "amount_sompi": o.max_spend,
        "uri": format!("{address}?amount={}", kas_str(o.max_spend)),
        "price": { "currency": o.currency, "amount": (o.price * 100.0).round() / 100.0, "credit": (o.credit * 100.0).round() / 100.0 },
        "expires_ms": o.expires_ms,
        "refund_after_ms": o.refund_after_ms,
        "status": o.status,
        "paid_kas": kas_str(o.paid),
        "short": o.status == "waiting" && o.paid > 0 && o.paid + SHORT_OK < o.max_spend,
        "txid": o.txid,
        "refunds": o.refunds,
        "until_ms": o.until_ms,
    })
}

#[derive(Deserialize)]
pub struct OrderReq {
    /// The wallet that gets the pass (the signed-in one).
    address: String,
    /// "buy" (a purchase or upgrade) or "renew".
    #[serde(default = "buy_word")]
    action: String,
    /// Renewals to prepay for auto-renew (purchases only).
    #[serde(default)]
    periods: u64,
}

fn buy_word() -> String {
    "buy".into()
}

/// The buyer's newest pass on this level.
async fn pass_of(s: &AppState, address: &str) -> Option<FoundPass> {
    let owner = owner_hash_of(address).ok()?.to_string();
    let snap = s.snap.read().await;
    snap.passes.iter().filter(|p| p.owner_hash == owner).max_by_key(|p| p.expiry_ms).cloned()
}

/// Paying by QR code is on for test networks only. This pay-in contract has a known flaw (see
/// SECURITY.md), so mainnet stays off in the code itself until a new contract version replaces it.
pub fn enabled(s: &AppState) -> bool {
    s.card.network != "mainnet"
}

/// POST /v1/payin/order?level=: a one-time pay-in address and the exact amount, for a QR code.
pub async fn create(State(s): State<AppState>, headers: axum::http::HeaderMap, Query(lq): Query<LevelQ>, Json(req): Json<OrderReq>) -> Result<Json<serde_json::Value>, ApiError> {
    let s = s.lv(&lq)?;
    super::signed_in_as(&s, &headers, &req.address)?;
    let shop = shop_of(&s)?;
    if !is_v5(&s.card) {
        return Err(bad("paying with a QR code is for plans priced in a currency"));
    }
    if !enabled(&s) {
        return Err((StatusCode::FORBIDDEN, "paying with a QR code isn't turned on for this site yet".into()));
    }
    let address = req.address.trim().to_string();
    let addr: kaspa_addresses::Address = address.as_str().try_into().map_err(|_| bad("not a valid Kaspa address"))?;
    if addr.prefix != expected_prefix(&s.card.network) {
        return Err(bad(format!("this membership lives on {}; switch your wallet to that network", s.card.network)));
    }
    members::remember(&s.home, &address);
    let now = wall_ms();
    {
        let all = load(&s.home);
        let open = all.iter().filter(|o| o.status == "waiting" && o.refund_after_ms > now);
        if open.clone().filter(|o| o.buyer == address).count() >= 3 {
            return Err((StatusCode::TOO_MANY_REQUESTS, "this wallet already has 3 open QR payments; use one of those or wait for them to expire".into()));
        }
        if open.count() >= 500 {
            return Err((StatusCode::TOO_MANY_REQUESTS, "too many open QR payments right now; try again in a few minutes".into()));
        }
    }
    let bond = s.card.settings.bond;
    let (q, uses, periods) = match req.action.as_str() {
        "buy" => {
            no_duplicate(&s, &address).await?;
            let q = v5_quote(&s, V5For::Buy(&address)).await?.ok_or_else(|| bad("no price for this plan"))?;
            let lane = current_lane(&s).await?;
            if lane.lane.remaining == 0 {
                return Err((StatusCode::CONFLICT, "sold out: every pass for this membership has been sold".into()));
            }
            let periods = req.periods.min(36);
            let (due, prepaid) = shop.payin_buy_cost(&lane, q.bps, q.renew_ppm, periods);
            (q, due + bond + prepaid, periods)
        }
        "renew" => {
            let p = pass_of(&s, &address).await.ok_or_else(|| bad("this wallet has no pass on this plan to renew"))?;
            if p.lifetime {
                return Err(bad("this pass never needs renewing"));
            }
            let opens = p.expiry_ms - s.card.settings.max_ahead_ms;
            let chain_now = s.snap.read().await.updated_ms;
            if p.active && chain_now < opens {
                return Err(bad("too early to renew this pass"));
            }
            let q = v5_quote(&s, V5For::Renew(&p, None)).await?.ok_or_else(|| bad("no price for this plan"))?;
            let due = crate::discounted_ppm(p.rate_sompi, q.bps);
            (q, due, 0)
        }
        _ => return Err(bad("action must be buy or renew")),
    };
    let max_spend = uses + crate::PAYIN_SLACK;
    let refund_after = now + refund_ms();
    let (_, _, pay_to) = shop.payin_for(&address, max_spend, refund_after).map_err(bad)?;
    let o = Order {
        id: crate::auth::random_token(),
        level: s.card.covenant_id.clone(),
        buyer: address.clone(),
        action: req.action.clone(),
        bps: q.bps,
        renew_ppm: q.renew_ppm,
        periods,
        uses,
        max_spend,
        currency: q.currency.clone(),
        price: q.amount,
        credit: q.credit,
        created_ms: now,
        expires_ms: (now + valid_ms()).min(refund_after - margin_ms()),
        refund_after_ms: refund_after,
        status: "waiting".into(),
        paid: 0,
        txid: None,
        refunds: vec![],
        error: None,
        until_ms: None,
        pay_to: Some(pay_to.clone()),
    };
    {
        let _g = ORDERS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let all = load(&s.home);
        // Keep the file small: orders finished more than 30 days ago move to the history.
        let (mut keep, gone): (Vec<Order>, Vec<Order>) = all.into_iter().partition(|x| x.status == "waiting" || now - x.refund_after_ms < 30 * 86_400_000);
        keep_history(&s, gone);
        keep.push(o.clone());
        save(&s.home, &keep);
    }
    println!("pay-in: new {} order {} for {} KAS", o.action, &o.id[..8], kas_str(o.max_spend));
    let mut out = view(&o, &pay_to);
    out["kas_today"] = serde_json::json!((q.kas_today * 100.0).round() / 100.0);
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct OrderQ {
    id: String,
}

/// GET /v1/payin/order?id=: where an order stands (for the page to poll).
pub async fn status(State(s): State<AppState>, Query(q): Query<OrderQ>) -> Result<Json<serde_json::Value>, ApiError> {
    let o = load(&s.home).into_iter().find(|o| o.id == q.id).ok_or_else(|| (StatusCode::NOT_FOUND, "no such order".to_string()))?;
    let s = s.at(Some(&o.level))?;
    let shop = shop_of(&s)?;
    let (_, _, pay_to) = shop.payin_for(&o.buyer, o.max_spend, o.refund_after_ms).map_err(bad)?;
    Ok(Json(view(&o, &pay_to)))
}

/// Background: completes paid orders on this level and refunds what can't be completed.
pub async fn watch_loop(s: AppState) {
    let every = crate::test_switch("KASPASS_PAYIN_EVERY_S").and_then(|v| v.parse().ok()).unwrap_or(5);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(every)).await;
        watch_once(&s).await;
    }
}

async fn watch_once(s: &AppState) {
    let Some(shop) = s.shop.clone() else { return };
    if !s.snap.read().await.synced {
        return;
    }
    let now = wall_ms();
    // Open orders, and finished ones for a while (a second payment to the same address goes back too).
    let keep = |o: &Order| o.status == "waiting" || now < o.refund_after_ms + grace_ms() + 30 * MINUTE_MS;
    let mine: Vec<Order> = load(&s.home).into_iter().filter(|o| o.level == s.card.covenant_id && keep(o)).collect();
    for o in mine {
        let Ok((pin, refund, _)) = shop.payin_for(&o.buyer, o.max_spend, o.refund_after_ms) else { continue };
        let coins = {
            let chain = s.chain.lock().await;
            match chain.utxos_at(&pin.spk()).await {
                Ok(c) => c,
                Err(_) => continue,
            }
        };
        if coins.is_empty() {
            // Nothing at the address: an unpaid order lapses at its refund time.
            if o.status == "waiting" && now > o.refund_after_ms {
                update(&s.home, &o.id, |x| x.status = "expired".into());
            }
            continue;
        }
        let total: u64 = coins.iter().map(|c| c.entry.amount).sum();
        if o.status == "waiting" && o.paid != total {
            update(&s.home, &o.id, |x| x.paid = total);
        }
        let mut done = o.status == "completed";
        for coin in coins {
            // Refunds wait for the refund time (plus a little, as the chain's clock trails).
            if done || now > o.refund_after_ms + grace_ms() {
                if now <= o.refund_after_ms + grace_ms() {
                    continue;
                }
                let res = {
                    let mut chain = s.chain.lock().await;
                    shop.payin_refund(&mut chain, pin.clone(), refund.clone(), coin).await
                };
                match res {
                    Ok((txid, back)) => {
                        println!("pay-in: refunded {} KAS on order {}, tx {txid}", kas_str(back), &o.id[..8]);
                        update(&s.home, &o.id, |x| {
                            x.refunds.push(txid);
                            if x.status == "waiting" {
                                x.status = "refunded".into();
                            }
                        });
                    }
                    Err(e) => update(&s.home, &o.id, |x| x.error = Some(format!("refund: {e:#}"))),
                }
                continue;
            }
            if coin.entry.amount + SHORT_OK < o.max_spend || now > o.refund_after_ms - margin_ms() {
                continue; // too little, or too close to the refund time: it goes back
            }
            // Within the quoted time: the order's price. Later: today's, if the payment still covers it.
            let mut bps = o.bps;
            if now > o.expires_ms {
                let fresh = match o.action.as_str() {
                    "renew" => match pass_of(s, &o.buyer).await {
                        Some(p) => v5_quote(s, V5For::Renew(&p, None)).await.ok().flatten().map(|q| (q.bps, crate::discounted_ppm(p.rate_sompi, q.bps))),
                        None => None,
                    },
                    _ => match (v5_quote(s, V5For::Buy(&o.buyer)).await.ok().flatten(), current_lane(s).await.ok()) {
                        (Some(q), Some(lane)) => {
                            let (due, prepaid) = shop.payin_buy_cost(&lane, q.bps, o.renew_ppm, o.periods);
                            Some((q.bps, due + s.card.settings.bond + prepaid))
                        }
                        _ => None,
                    },
                };
                match fresh {
                    Some((b, uses)) if uses + SHORT_OK <= coin.entry.amount.min(o.max_spend) => bps = b,
                    _ => continue,
                }
            }
            let res = match o.action.as_str() {
                "renew" => match pass_of(s, &o.buyer).await {
                    Some(p) => {
                        let mut chain = s.chain.lock().await;
                        shop.payin_renew(&mut chain, &p, bps, pin.clone(), refund.clone(), coin).await.map(|r| (r, None, Some(p.expiry_ms.max(now) + s.card.settings.period_ms)))
                    }
                    None => Err(anyhow::anyhow!("the pass to renew isn't showing yet")),
                },
                _ => {
                    if no_duplicate(s, &o.buyer).await.is_err() {
                        Err(anyhow::anyhow!("this wallet already has a pass; the payment goes back after the refund time"))
                    } else {
                        match current_lane(s).await {
                            Ok(lane) => {
                                let mut chain = s.chain.lock().await;
                                let next = FoundLane { txid: String::new(), index: 0, value: lane.value, lane: crate::Lane { remaining: if lane.lane.remaining > 0 { lane.lane.remaining - 1 } else { lane.lane.remaining }, ..lane.lane } };
                                let old = (lane.txid.clone(), lane.index);
                                shop.payin_buy(&mut chain, &o.buyer, &lane, bps, o.renew_ppm, o.periods, pin.clone(), refund.clone(), coin).await.map(|r| (r, Some((next, old)), Some(now + s.card.settings.period_ms)))
                            }
                            Err(e) => Err(anyhow::anyhow!(e.1)),
                        }
                    }
                }
            };
            match res {
                Ok(((txid, charge), new_lane, until)) => {
                    if let Some((mut lane, (old_txid, old_index))) = new_lane {
                        lane.txid = txid.clone();
                        let mut snap = s.snap.write().await;
                        snap.lanes.retain(|l| !(l.txid == old_txid && l.index == old_index));
                        snap.lanes.insert(0, lane);
                    }
                    let kind = if o.action == "renew" { "renewal" } else if o.credit > 0.0 { "upgrade" } else { "purchase" };
                    let amount = if bps == o.bps { o.price } else { 0.0 };
                    crate::prices::log_payment(&s.home, &txid, crate::prices::PayNote { currency: o.currency.clone(), amount, credit: o.credit, kind: kind.into() });
                    if o.action == "buy" {
                        crate::prices::log_paid(&s.home, &s.card.covenant_id, &owner_hash_of(&o.buyer).map(|h| h.to_string()).unwrap_or_default(), charge as i64, &o.currency, o.price);
                        s.recent_buys.lock().unwrap_or_else(|e| e.into_inner()).insert(format!("{}:{}", s.card.covenant_id, o.buyer), std::time::Instant::now());
                    }
                    println!("pay-in: completed {} order {} ({} KAS to the creator), tx {txid}", o.action, &o.id[..8], kas_str(charge));
                    update(&s.home, &o.id, |x| {
                        x.status = "completed".into();
                        x.txid = Some(txid);
                        x.until_ms = until;
                        x.error = None;
                    });
                    done = true;
                }
                Err(e) => {
                    let m = format!("{e:#}");
                    if o.error.as_deref() != Some(m.as_str()) {
                        eprintln!("pay-in: order {} not completed yet: {m}", &o.id[..8]);
                    }
                    update(&s.home, &o.id, |x| x.error = Some(m));
                }
            }
        }
    }
}

/// Simulator only: lets the test site's stand-in "phone wallet" send a plain payment.
pub static SIM_PAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Deserialize)]
pub struct SimPay {
    /// The test wallet's secret byte, as two hex digits ("66").
    from: String,
    to: String,
    sompi: u64,
}

/// POST /v1/sim/pay (simulator only).
pub async fn sim_pay(State(s): State<AppState>, Json(req): Json<SimPay>) -> Result<Json<serde_json::Value>, ApiError> {
    if !SIM_PAY.load(std::sync::atomic::Ordering::Relaxed) {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    let byte = u8::from_str_radix(&req.from, 16).map_err(|_| bad("from: two hex digits"))?;
    let w = crate::wallet_from_secret("phone", secp256k1::SecretKey::from_slice(&[byte; 32]).map_err(bad)?);
    let to: kaspa_addresses::Address = req.to.as_str().try_into().map_err(|_| bad("not a valid address"))?;
    let outs = vec![kaspa_consensus_core::tx::TransactionOutput { value: req.sompi, script_public_key: kaspa_txscript::pay_to_address_script(&to), covenant: None }];
    let mut chain = s.chain.lock().await;
    let (tx, entries, _) = crate::build_tx(&chain, vec![], outs, Some(&w), 0, None, None).await.map_err(bad)?;
    let id = chain.submit(&tx, &entries).await.map_err(bad)?;
    Ok(Json(serde_json::json!({ "txid": id.to_string() })))
}
