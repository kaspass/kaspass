//! Selling passes from the website: the server builds the purchase, the buyer's
//! wallet signs only its own coins, and the server checks and broadcasts it.
//!
//! The sales lane needs no signature to sell a pass (the Minter covenant itself
//! enforces the price and terms), so the creator's keys are never involved.
//!
//! Everything comes from the public tier card and the chain: prices and the sales lane
//! from the indexer. If the creator's state.json happens to sit in the same folder, it's
//! kept in step for the command line, but the website doesn't need it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use kaspa_addresses::{Address, Version};
use kaspa_consensus_core::tx::{ScriptPublicKey, Transaction, UtxoEntry};
use kaspa_txscript::{extract_script_pub_key_address, pay_to_address_script};
use serde_json::{Value, json};

use crate::indexer::{FoundLane, FoundPass, KnownLane, TierCard, owner_hash_of};
use crate::{
    Chain, Creator, Fee, LIFETIME, Lane, Live, MINUTE, Member, Pass, PassAction, Payer, SOMPI, Sale, SaleOpts, State, Tier, V4, Voucher, Wallet,
    close_tx_external, deposit_tx, human_ms, kas, launch_tx_external, pass_tx_v, pskt, sale_tx_full,
    transfer_tx_external, verify_all, when, withdraw_tx_external,
};

/// A tier being launched from the browser, waiting for the creator's signature.
struct PendingLaunch {
    card: TierCard,
    /// v5: the plan's price in its currency, saved once the launch is on chain.
    price: Option<crate::prices::LevelPrice>,
}

/// What came of a signed transaction.
pub struct Finished {
    pub txid: String,
    /// For a sale or price change: the sales lane it created, and the lane it replaced.
    pub new_lane: Option<(FoundLane, (String, u32))>,
    /// For a launch: the new tier card, and where it was saved.
    pub card: Option<(TierCard, String)>,
}

/// Settings for a new tier, from the dashboard's launch form.
#[derive(serde::Deserialize, Debug, Default)]
pub struct LaunchForm {
    pub price_kas: f64,
    pub lifetime_kas: f64,
    pub credit: bool,
    pub period_minutes: i64,
    pub trial_minutes: i64,
    pub resellable: bool,
    pub royalty_kas: f64,
    pub bond_kas: f64,
    pub max_ahead_minutes: i64,
    pub supply: i64,
    /// v4: auto-renew from a prepaid balance, a commitment term and holder discounts.
    #[serde(default)]
    pub v4: bool,
    /// Largest holder discount the contracts will ever allow, in percent.
    #[serde(default)]
    pub max_discount_pct: f64,
    /// One commitment term: its price and length in periods (0 = not offered).
    #[serde(default)]
    pub term_kas: f64,
    #[serde(default)]
    pub term_periods: i64,
    /// How close to the end auto-renew may run (0 = a third of the period, at most 3 days).
    #[serde(default)]
    pub auto_window_minutes: i64,
    /// v5: members can start the free trial themselves, with their first period prepaid into
    /// their own pass (charged when the trial ends unless they cancel). Needs a trial length.
    #[serde(default)]
    pub self_trial: bool,
    /// v5: the plan's price in a currency (CAD, USD, EUR). The on-chain price becomes a KAS
    /// ceiling of 3x today's KAS amount; buyers pay today's price.
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub founding_amount: Option<f64>,
    #[serde(default)]
    pub founding_until_ms: Option<i64>,
    /// v5: the KAS ceiling as a multiple of today's KAS amount (default 3, from 1.5 to 10).
    #[serde(default)]
    pub ceiling_times: Option<f64>,
    /// v5: a lifetime pass and a commitment term, in the plan's currency (each gets its own
    /// ceiling the same way). The term's length is `term_periods`.
    #[serde(default)]
    pub lifetime_amount: Option<f64>,
    #[serde(default)]
    pub term_amount: Option<f64>,
}

/// A built purchase waiting for the buyer's signature.
pub struct Quote {
    pub buyer: String,
    /// For actions that create a new pass: who will own it.
    pub new_pass_owner: Option<String>,
    launch: Option<PendingLaunch>,
    tx: Transaction,
    entries: Vec<UtxoEntry>,
    unsigned: Vec<usize>,
    /// Sales move the sales lane on: (lane spent, lane it becomes).
    next_lane: Option<Lane>,
    lane_from: Option<(String, u32)>,
    /// Renewals and upgrades move a pass: (old txid, old index, new terms).
    pass_move: Option<(String, u32, Pass)>,
    /// A close ends a pass: (txid, index).
    closes: Option<(String, u32)>,
    /// v5: the owner record as it will be once this is on chain (setup or handover).
    record: Option<crate::RecordInfo>,
    pub created: Instant,
}

impl Quote {
    /// The transaction's id (it doesn't cover signatures, so it's final before the wallet signs).
    pub fn txid(&self) -> String {
        self.tx.id().to_string()
    }
}

/// The owner record this site follows (v5).
pub const RECORD_FILE: &str = "record.json";

/// v5: transactions this server signed a price for (txid -> when), so a payment that used a
/// price approval it never made stands out (`signed-txids.json`; a txid doesn't cover signatures,
/// so it's known before the buyer signs). Kept for 2 days.
pub const SIGNED_FILE: &str = "signed-txids.json";
static SIGNED: std::sync::LazyLock<std::sync::Mutex<Option<std::collections::HashMap<String, i64>>>> = std::sync::LazyLock::new(Default::default);

fn signed_load(dir: &Path) -> std::collections::HashMap<String, i64> {
    std::fs::read_to_string(dir.join(SIGNED_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The cushions a member can add to prepaid renewals in case KAS falls, in percent.
pub const CUSHIONS: [u64; 3] = [0, 10, 25];
pub const DEFAULT_CUSHION: u64 = 10;

/// Prices signed while preparing, not sent yet: (txid, when). They're written to
/// `signed-txids.json` only in `finish`, the moment the signature leaves the server.
static PREPARED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = std::sync::LazyLock::new(Default::default);

/// How long a sent signature is remembered (the leaked-key alert ignores anything older).
pub const SIGNED_KEEP_MS: i64 = 30 * 24 * 3600 * 1000;

/// Remembers that this server signed a price for `txid` (call it just before sending).
pub fn note_signed(dir: &Path, txid: &str) {
    if crate::test_switch("KASPASS_SIM_FORGET_SIGNED").is_some() {
        return; // tests: act as if someone else signed every price
    }
    let mut g = SIGNED.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(|| signed_load(dir));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    m.retain(|_, t| now - *t < SIGNED_KEEP_MS);
    m.insert(txid.to_string(), now);
    let _ = crate::prices::write_atomic(&dir.join(SIGNED_FILE), serde_json::to_string(&*m).unwrap_or_default().as_bytes());
}

/// A price was signed for `txid` while preparing; it's recorded if and when it's sent.
fn note_prepared(txid: &str) {
    let mut g = PREPARED.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(20 * 60));
    g.insert(txid.to_string(), std::time::Instant::now());
}

/// Whether this server signed a price for `txid` (in the last 2 days).
pub fn was_signed(dir: &Path, txid: &str) -> bool {
    let mut g = SIGNED.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(|| signed_load(dir)).contains_key(txid)
}

/// v5: the KAS ceiling is this many times today's KAS amount.
pub const V5_CEILING_TIMES: f64 = 3.0;

/// v5: the largest discount off the KAS ceiling the server may sign (99.99%; the contracts get it in millionths).
pub const V5_MAX_DISCOUNT_BPS: u64 = 9999;

pub struct Shop {
    /// The creator's folder, if this server runs there (only used to keep state.json in step).
    pub home: Option<PathBuf>,
    /// Where this website runs (new tier cards from the launch form are saved here).
    pub dir: PathBuf,
    pub card: TierCard,
    /// Where protocol fees go for tiers launched from this site.
    pub fee_address: Option<String>,
    /// Memberships launched from this site's dashboard carry no KasPass fee (`serve --no-fee`).
    pub no_fee: bool,
    /// This server's voucher key: it approves holder discounts on v4 tiers. Lives only on the server.
    pub voucher: Option<Wallet>,
    /// v5: replacement voucher keys made by "Replace server key" (keys/voucher-2.key, -3, ...),
    /// oldest first, shared by every level on this server. The newest signs for new plans; the
    /// older ones still sign for lanes and passes that haven't moved over yet.
    pub voucher_ring: std::sync::Arc<std::sync::RwLock<Vec<Wallet>>>,
    /// This server's keeper wallet: it pays the network fee for auto-renewals.
    pub keeper: Option<Wallet>,
}

impl Shop {
    pub fn open(home: &Path, card: &TierCard) -> Shop {
        let matches = State::load(home).map(|st| st.covenant_id.as_deref() == Some(card.covenant_id.as_str())).unwrap_or(false);
        let fee_address = card.settings.fee.as_ref().map(|f| f.address.clone());
        Shop { home: matches.then(|| home.to_path_buf()), dir: home.to_path_buf(), card: card.clone(), fee_address, no_fee: false, voucher: None, voucher_ring: Default::default(), keeper: None }
    }

    /// The tier's v4 settings, if it's a v4 tier.
    pub fn v4(&self) -> Option<&V4> {
        self.tier().v4.as_ref()
    }

    /// Whether this server holds the key the tier's contracts accept for holder discounts.
    pub fn can_discount(&self) -> bool {
        match (self.v4(), &self.voucher) {
            (Some(v), Some(w)) if self.is_v5() => v.max_discount_bps > 0 && v.voucher_hash().is_ok_and(|h| h == w.id || self.voucher_ring.read().unwrap_or_else(|e| e.into_inner()).iter().any(|k| k.id == h)),
            (Some(v), Some(w)) => v.max_discount_bps > 0 && v.voucher_hash().ok() == Some(w.id),
            _ => false,
        }
    }

    /// v5: the voucher key new plans and moved lanes and passes trust (the newest one).
    pub fn v5_key(&self) -> Option<Wallet> {
        self.voucher_ring.read().unwrap_or_else(|e| e.into_inner()).last().cloned().or_else(|| self.voucher.clone())
    }

    /// v5: makes a new voucher key on this server (keys/voucher-N.key) and makes it the newest.
    /// The old ones stay until nothing on chain trusts them.
    pub fn new_v5_key(&self) -> Result<Wallet> {
        let mut ring = self.voucher_ring.write().unwrap_or_else(|e| e.into_inner());
        let n = ring.len() + 2;
        let name = format!("voucher-{n}");
        if self.dir.join("keys").join(format!("{name}.key")).exists() {
            bail!("keys/{name}.key already exists; restart the server to load it");
        }
        let w = crate::load_wallet(&self.dir, &name)?;
        ring.push(w.clone());
        Ok(w)
    }

    /// v5: the key a lane or pass trusts (`set`, or the plan's launch key when None).
    pub fn trusted_key(&self, set: Option<kaspa_consensus_core::Hash>) -> Option<kaspa_consensus_core::Hash> {
        set.or_else(|| self.v4().and_then(|v| v.voucher_hash().ok()))
    }

    /// A voucher for `bps` off (capped by the tier), or None if no discount applies.
    /// The voucher for a discount of `d` (basis points, or millionths on v5). On v5 the key is
    /// the one the lane or pass trusts (`want`, None = the one the tier launched with), and it
    /// signs even at no discount (v5 auto-renewals need it).
    fn voucher_for_v5(&self, d: u64, want: Option<kaspa_consensus_core::Hash>) -> Result<Option<Voucher>> {
        let v = self.v4().ok_or_else(|| anyhow!("not a v5 membership"))?;
        let want = match want {
            Some(h) => h,
            None => v.voucher_hash()?,
        };
        let ring = self.voucher_ring.read().unwrap_or_else(|e| e.into_inner());
        let key = self.voucher.iter().chain(ring.iter())
            .find(|w| w.id == want)
            .cloned()
            .ok_or_else(|| anyhow!("this server doesn't hold the key this membership trusts (was the server key replaced?)"))?;
        Ok(Some(Voucher { key, bps: d.min(v.max_discount_bps * 100) }))
    }

    /// Prepaid renewals on a plan priced in a currency get a cushion in case KAS falls (10%
    /// unless the member picks another); a plan priced in KAS always costs the same, so it
    /// needs none.
    pub fn prepaid_margin(&self, per: u64) -> u64 {
        self.prepaid_with(per, DEFAULT_CUSHION)
    }

    pub fn prepaid_with(&self, per: u64, cushion_pct: u64) -> u64 {
        if self.priced_in_kas() { per } else { per * (100 + cushion_pct) / 100 }
    }

    fn priced_in_kas(&self) -> bool {
        crate::prices::level_prices(&self.dir).get(&self.card.covenant_id).is_some_and(|p| p.currency == "KAS")
    }

    fn is_v5(&self) -> bool {
        self.v4().is_some_and(|v| v.record_id.is_some())
    }

    fn voucher_for(&self, bps: u64) -> Result<Option<Voucher>> {
        if bps == 0 {
            return Ok(None);
        }
        if !self.can_discount() {
            bail!("holder discounts aren't available on this membership");
        }
        let cap = self.v4().map(|v| v.max_discount_bps).unwrap_or(0);
        Ok(Some(Voucher { key: self.voucher.clone().unwrap(), bps: bps.min(cap) }))
    }

    /// The site's owner: the owner record's current owner (v5), else the membership's creator.
    pub fn is_creator(&self, address: &str) -> bool {
        match self.record() {
            Some(r) => address == r.owner,
            None => address == self.card.creator_address,
        }
    }

    /// v5: the owner record this site follows (record.json), if it has one.
    pub fn record(&self) -> Option<crate::RecordInfo> {
        std::fs::read_to_string(self.dir.join(RECORD_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    fn quote_of(buyer: &str, tx: Transaction, entries: Vec<UtxoEntry>, unsigned: Vec<usize>, record: Option<crate::RecordInfo>) -> Quote {
        Quote { buyer: buyer.to_string(), new_pass_owner: None, launch: None, tx, entries, unsigned, next_lane: None, lane_from: None, pass_move: None, closes: None, record, created: Instant::now() }
    }

    /// v5: the creator sets up their owner record (once): their wallet as owner, a separate
    /// recovery wallet that can hand it over later. The owner's wallet pays (1 KAS stays in it).
    pub async fn prepare_record(&self, chain: &Chain, owner_addr: &str, recovery_addr: &str) -> Result<(Quote, Value)> {
        if let Some(r) = self.record() {
            bail!("this site already has an owner record ({}…)", &r.record_id[..12]);
        }
        let owner: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        let recovery: Address = recovery_addr.trim().try_into().map_err(|_| anyhow!("the recovery wallet isn't a valid Kaspa address"))?;
        if owner.prefix != self.prefix() || recovery.prefix != self.prefix() {
            bail!("both wallets must be on {}", self.card.network);
        }
        let (tx, entries, fee, id) = crate::record_tx_external(chain, &owner, &recovery).await?;
        let spk = pay_to_address_script(&owner);
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, owner_addr, &unsigned);
        let rec = crate::RecordInfo { record_id: id.to_string(), owner: owner_addr.to_string(), recovery: recovery.to_string() };
        let mut summary = json!({
            "action": "record",
            "price_sompi": 0,
            "fee_sompi": fee,
            "total_sompi": SOMPI + fee,
            "record_id": id.to_string(),
            "text": format!("Turn on payouts: 1 KAS stays locked in your owner record, plus a {} network fee. Backup wallet: {}", kas(fee), short(&rec.recovery)),
        });
        summary["outputs"] = self.outputs(&tx, Some(&spk), &["Your owner record (contract)"], "Change, back to your wallet");
        Ok((Self::quote_of(owner_addr, tx, entries, unsigned, Some(rec)), json!({ "request": request, "summary": summary })))
    }

    /// v5: the owner takes everything in the payout vault to their wallet.
    pub async fn prepare_vault_withdraw(&self, chain: &Chain, owner_addr: &str) -> Result<(Quote, Value)> {
        let rec = self.record().ok_or_else(|| anyhow!("this site has no owner record"))?;
        if rec.owner != owner_addr {
            bail!("only the owner record's current owner can withdraw");
        }
        let owner: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        let (tx, entries, fee, total) = crate::vault_withdraw_tx_external(chain, &owner, &rec).await?;
        let spk = pay_to_address_script(&owner);
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, owner_addr, &unsigned);
        let mut summary = json!({
            "action": "withdraw",
            "price_sompi": 0,
            "fee_sompi": fee,
            "total_sompi": fee,
            "amount_sompi": total,
            "text": format!("Withdraw {} from your payout vault to this wallet, {} network fee", kas(total), kas(fee)),
        });
        summary["outputs"] = self.outputs(&tx, Some(&spk), &["Your owner record (contract, unchanged)"], "Change, back to your wallet");
        Ok((Self::quote_of(owner_addr, tx, entries, unsigned, None), json!({ "request": request, "summary": summary })))
    }

    /// v5: the recovery wallet names a new owner and a new recovery wallet.
    pub async fn prepare_handover(&self, chain: &Chain, recovery_addr: &str, new_owner: &str, new_recovery: &str) -> Result<(Quote, Value)> {
        let rec = self.record().ok_or_else(|| anyhow!("this site has no owner record"))?;
        if rec.recovery != recovery_addr {
            bail!("only the recovery wallet can hand the owner record over");
        }
        let recovery: Address = recovery_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        let no: Address = new_owner.trim().try_into().map_err(|_| anyhow!("the new owner isn't a valid Kaspa address"))?;
        let nr: Address = new_recovery.trim().try_into().map_err(|_| anyhow!("the new recovery wallet isn't a valid Kaspa address"))?;
        if no.prefix != self.prefix() || nr.prefix != self.prefix() {
            bail!("both new wallets must be on {}", self.card.network);
        }
        let (tx, entries, fee) = crate::handover_tx_external(chain, &recovery, &rec, &no, &nr).await?;
        let spk = pay_to_address_script(&recovery);
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, recovery_addr, &unsigned);
        let next = crate::RecordInfo { record_id: rec.record_id.clone(), owner: no.to_string(), recovery: nr.to_string() };
        let mut summary = json!({
            "action": "handover",
            "price_sompi": 0,
            "fee_sompi": fee,
            "total_sompi": fee,
            "text": format!("Hand the owner record to {} (new recovery wallet {}), {} network fee. The old owner loses access to payouts and prices.", short(&next.owner), short(&next.recovery), kas(fee)),
        });
        summary["outputs"] = self.outputs(&tx, Some(&spk), &["Your owner record with its new wallets (contract)"], "Change, back to your wallet");
        Ok((Self::quote_of(recovery_addr, tx, entries, unsigned, Some(next)), json!({ "request": request, "summary": summary })))
    }

    /// v5 key swap: the lanes and passes of this level that don't trust the newest key yet.
    pub fn rekey_left(&self, lanes: &[FoundLane], passes: &[FoundPass]) -> (usize, usize) {
        let Some(new) = self.v5_key().map(|w| w.id) else { return (0, 0) };
        let l = lanes.iter().filter(|l| self.trusted_key(l.lane.voucher) != Some(new)).count();
        let p = passes.iter().filter(|p| self.trusted_key(p.voucher.as_deref().and_then(|v| v.parse().ok())) != Some(new)).count();
        (l, p)
    }

    /// v5 key swap: this level's coins still on an older key, lanes first, as they are on chain
    /// now (coins already moved, but not yet seen by the indexer, are skipped).
    async fn rekey_coins(&self, chain: &Chain, lanes: &[FoundLane], passes: &[FoundPass], want: usize) -> Result<Vec<crate::RekeyCoin>> {
        let new = self.v5_key().ok_or_else(|| anyhow!("this server has no voucher key"))?.id;
        let tier = self.tier().clone();
        let cov = self.cov()?;
        let creator = self.creator()?;
        let mut out = Vec::new();
        for l in lanes {
            if out.len() >= want {
                return Ok(out);
            }
            if self.trusted_key(l.lane.voucher) == Some(new) {
                continue;
            }
            let cur = crate::minter(&creator, &tier, &l.lane)?;
            let next = crate::minter(&creator, &tier, &Lane { voucher: Some(new), ..l.lane })?;
            let live = Live { txid: l.txid.clone(), index: l.index, value: l.value };
            if let Ok(utxo) = chain.find(&cur.spk(), &live).await {
                out.push(crate::RekeyCoin { utxo, cur, next, cov, lane: true });
            }
        }
        for p in passes {
            if out.len() >= want {
                break;
            }
            let pass = Self::pass_of(p);
            if self.trusted_key(pass.voucher) == Some(new) {
                continue;
            }
            let owner_id = p.owner_hash.parse().map_err(|_| anyhow!("bad owner"))?;
            let cur = crate::membership(&creator, &tier, owner_id, &pass)?;
            let next = crate::membership(&creator, &tier, owner_id, &Pass { voucher: Some(new), ..pass.clone() })?;
            let live = Live { txid: p.txid.clone(), index: p.index, value: p.value_sompi };
            if let Ok(utxo) = chain.find(&cur.spk(), &live).await {
                out.push(crate::RekeyCoin { utxo, cur, next, cov, lane: false });
            }
        }
        Ok(out)
    }

    /// v5 key swap: one transaction moving as many lanes and passes as fit (across `levels`,
    /// each a shop with its chain snapshot) to the newest voucher key. The owner approves it.
    pub async fn prepare_rekey(&self, chain: &Chain, owner_addr: &str, levels: &[(&Shop, Vec<FoundLane>, Vec<FoundPass>)]) -> Result<(Quote, Value)> {
        let rec = self.record().ok_or_else(|| anyhow!("this site has no owner record"))?;
        if rec.owner != owner_addr {
            bail!("only the owner record's current owner can move plans and passes to a new key");
        }
        let owner: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        let new = self.v5_key().ok_or_else(|| anyhow!("this server has no voucher key"))?.id;
        let mut coins = Vec::new();
        for (shop, lanes, passes) in levels {
            if !shop.is_v5() || shop.record().map(|r| r.record_id) != Some(rec.record_id.clone()) {
                continue;
            }
            coins.extend(shop.rekey_coins(chain, lanes, passes, 4 * crate::REKEY_MAX - coins.len()).await?);
            if coins.len() >= 4 * crate::REKEY_MAX {
                break;
            }
        }
        if coins.is_empty() {
            bail!("everything already trusts the new key (or the last batch is still confirming; try again in a minute)");
        }
        let spk = pay_to_address_script(&owner);
        // Several passes fit in one approval only when the fee comes from a small coin; set one
        // aside first if the owner has none (it stays theirs, and pays for every batch).
        if coins.len() > 1 && crate::small_coin(chain, &owner).await?.is_none() {
            let (tx, entries, fee) = crate::rekey_coin_tx(chain, &owner).await?;
            let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
            let request = pskt::sign_request(&tx, &entries, owner_addr, &unsigned);
            let mut summary = json!({
                "action": "rekey_coin",
                "price_sompi": 0,
                "fee_sompi": fee,
                "total_sompi": fee,
                "moved": 0,
                "text": format!("First, set aside {} in your own wallet for the key swap's network fees (it stays yours), {} network fee. Each approval after this moves several passes at once", kas(crate::REKEY_COIN), kas(fee)),
            });
            summary["outputs"] = self.outputs(&tx, Some(&spk), &[], "Change, back to your wallet");
            summary["outputs"][0]["label"] = json!("Set aside for the key swap (your own wallet)");
            return Ok((Self::quote_of(owner_addr, tx, entries, unsigned, None), json!({ "request": request, "summary": summary })));
        }
        // Coins of similar value batch best (storage mass): smallest first.
        coins.sort_by_key(|c| c.utxo.entry.amount);
        let (tx, entries, fee, n) = crate::rekey_tx_external(chain, &owner, &rec, &coins, new).await?;
        let lanes = coins[..n].iter().filter(|c| c.lane).count();
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, owner_addr, &unsigned);
        let mut labels: Vec<&str> = vec![];
        for c in &coins[..n] {
            labels.push(if c.lane { "A plan's sales lane, now trusting the new key (contract)" } else { "A member's pass, now trusting the new key (contract)" });
        }
        labels.push("Your owner record (contract, unchanged)");
        let mut summary = json!({
            "action": "rekey",
            "price_sompi": 0,
            "fee_sompi": fee,
            "total_sompi": fee,
            "moved": n,
            "text": format!("Move {} to the new server key, {} network fee. Members keep everything; only the key that approves prices changes", match (lanes, n - lanes) {
                (0, p) => format!("{p} member pass{}", if p == 1 { "" } else { "es" }),
                (l, 0) => format!("{l} plan{}", if l == 1 { "" } else { "s" }),
                (l, p) => format!("{l} plan{} and {p} member pass{}", if l == 1 { "" } else { "s" }, if p == 1 { "" } else { "es" }),
            }, kas(fee)),
        });
        summary["outputs"] = self.outputs(&tx, Some(&spk), &labels, "Change, back to your wallet");
        Ok((Self::quote_of(owner_addr, tx, entries, unsigned, None), json!({ "request": request, "summary": summary })))
    }

    fn prefix(&self) -> kaspa_addresses::Prefix {
        if self.card.network.starts_with("testnet") { kaspa_addresses::Prefix::Testnet } else { kaspa_addresses::Prefix::Mainnet }
    }

    /// Every output, numbered and labelled the way the buyer will see it in Kaspire, so they
    /// can match the wallet's review line by line. `contract` names the covenant outputs in order.
    fn outputs(&self, tx: &Transaction, payer: Option<&ScriptPublicKey>, contract: &[&str], change: &str) -> Value {
        let prefix = if self.card.network.starts_with("testnet") { kaspa_addresses::Prefix::Testnet } else { kaspa_addresses::Prefix::Mainnet };
        let creator = self.creator().ok().map(|c| c.spk);
        let fee = self.tier().fee.as_ref().and_then(|f| f.spk().ok());
        let vault = self.record().filter(|_| self.is_v5()).and_then(|r| r.id().ok()).and_then(|id| crate::vault(id).ok()).map(|v| v.spk());
        let mut contract = contract.iter();
        let last = tx.outputs.len().saturating_sub(1);
        let rows: Vec<Value> = tx
            .outputs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let label = if o.covenant.is_some() {
                    contract.next().copied().unwrap_or("KasPass contract").to_string()
                } else if i == last && Some(&o.script_public_key) == payer {
                    // Change always comes last, even if the payer is the creator.
                    change.to_string()
                } else if Some(&o.script_public_key) == creator.as_ref() {
                    "To the creator".into()
                } else if Some(&o.script_public_key) == vault.as_ref() {
                    "To the creator's payout vault".into()
                } else if Some(&o.script_public_key) == fee.as_ref() {
                    format!("KasPass fee ({}%)", self.tier().fee.as_ref().map(|f| f.bps as f64 / 100.0).unwrap_or(0.0))
                } else if Some(&o.script_public_key) == payer {
                    change.to_string()
                } else {
                    "Other".into()
                };
                json!({
                    "index": i,
                    "sompi": o.value,
                    "kas": kas_exact(o.value),
                    "address": extract_script_pub_key_address(&o.script_public_key, prefix).map(|a| a.to_string()).unwrap_or_default(),
                    "label": label,
                })
            })
            .collect();
        Value::Array(rows)
    }

    fn tier(&self) -> &Tier {
        &self.card.settings
    }

    fn cov(&self) -> Result<kaspa_consensus_core::Hash> {
        self.card.covenant_id.parse().map_err(|_| anyhow!("bad covenant id in tier card"))
    }

    /// What's for sale right now, from the lane the indexer found on-chain.
    pub fn offer(&self, lane: &FoundLane) -> Result<Value> {
        let tier = self.tier();
        let lane = lane.lane;
        Ok(json!({
            "available": lane.remaining != 0,
            "monthly_sompi": lane.price,
            "lifetime_sompi": lane.life,
            "credit": lane.credit,
            "remaining": lane.remaining,
            "period_ms": tier.period_ms,
            "bond_sompi": tier.bond,
            "v4": tier.v4.as_ref().map(|v| json!({
                "term_sompi": lane.term_price,
                "term_ms": lane.term_ms,
                "auto_window_ms": v.auto_window_ms,
                "max_discount_bps": v.max_discount_bps,
                "discounts": self.can_discount(),
                "auto_renew": self.keeper.is_some(),
            })),
        }))
    }

    /// A purchase on a v4 tier: `prepaid_periods` months put into the pass for auto-renew, and a
    /// holder discount of `bps` (the caller has checked the buyer qualifies).
    /// `renew_ppm` (v5): what a renewal costs today, in millionths off the ceiling; prepaid
    /// renewals are sized on it (not on this purchase's founding or upgrade price).
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_v4(&self, chain: &Chain, buyer: &str, kind: Sale, lane: &FoundLane, prepaid_periods: u64, cushion_pct: u64, bps: u64, renew_ppm: Option<u64>) -> Result<(Quote, Value)> {
        if kind == Sale::Trial {
            bail!("free trials are given by the creator");
        }
        if prepaid_periods > 36 {
            bail!("prepay at most 36 periods");
        }
        if kind == Sale::Lifetime && prepaid_periods > 0 {
            bail!("a lifetime pass never renews, so there's nothing to prepay");
        }
        // Prepaid renewals cost what a renewal costs: today's price on v5 (a discount off the
        // ceiling, with the cushion the member picked for price moves), the locked rate otherwise.
        let per = if self.is_v5() { self.prepaid_with(crate::discounted_ppm(lane.lane.price, renew_ppm.unwrap_or(bps)), cushion_pct) } else { lane.lane.price };
        let voucher = if self.is_v5() { self.voucher_for_v5(bps, lane.lane.voucher)?.filter(|v| v.bps > 0) } else { self.voucher_for(bps)? };
        let opts = SaleOpts { prepaid: prepaid_periods * per, voucher, periods: prepaid_periods, cushion_pct };
        self.prepare_for_opts(chain, buyer, buyer, kind, lane, opts).await
    }

    /// v5: a pass paid by `payer_addr` for `owner_addr` at today's price (`bps`, in millionths
    /// off the ceiling), not at the KAS ceiling.
    pub async fn prepare_gift_v5(&self, chain: &Chain, payer_addr: &str, owner_addr: &str, lane: &FoundLane, bps: u64, kind: Sale) -> Result<(Quote, Value)> {
        let voucher = self.voucher_for_v5(bps, lane.lane.voucher)?.filter(|v| v.bps > 0);
        self.prepare_for_opts(chain, payer_addr, owner_addr, kind, lane, SaleOpts { voucher, ..Default::default() }).await
    }

    /// v5 free trial for `member_addr`, given by the owner record's current owner, who pays the
    /// pass's refundable deposit and the network fee.
    /// Whether members can start this level's free trial themselves (minter_v5t).
    pub fn self_trial(&self) -> bool {
        self.tier().trial_ms > 0 && self.v4().is_some_and(|v| v.self_trial)
    }

    /// v5 self-serve trial: the member's wallet funds the pass's deposit plus `prepaid` (their first
    /// period, charged by the keeper when the trial ends unless they take it back) and the network
    /// fee; the voucher key approves. Nothing goes to the creator now.
    pub async fn prepare_self_trial(&self, chain: &Chain, member_addr: &str, lane: &FoundLane, prepaid: u64) -> Result<(Quote, Value)> {
        if !self.self_trial() {
            bail!("this plan doesn't offer a free trial members can start themselves");
        }
        let addr: Address = member_addr.trim().try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if addr.version != Version::PubKey || addr.prefix != self.prefix() {
            bail!("sign in with a regular wallet on this site's network");
        }
        let member_id = owner_hash_of(member_addr.trim())?;
        let voucher = self.voucher_for_v5(0, lane.lane.voucher)?.ok_or_else(|| anyhow!("this server can't approve trials right now"))?;
        let creator: Address = self.card.creator_address.as_str().try_into().map_err(|_| anyhow!("bad creator address in tier card"))?;
        let live = Live { txid: lane.txid.clone(), index: lane.index, value: lane.value };
        let (tx, entries, fee, pass) = crate::self_trial_tx_v5(chain, &addr, &creator, self.tier(), self.cov()?, lane.lane, &live, member_id, member_addr.trim(), prepaid, &voucher).await?;
        note_prepared(&tx.id().to_string());
        let spk = pay_to_address_script(&addr);
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, member_addr.trim(), &unsigned);
        let bond = self.tier().bond;
        let mut summary = json!({
            "action": "self_trial",
            "kind": "trial",
            "price_sompi": 0,
            "bond_sompi": bond,
            "prepaid_sompi": prepaid,
            "fee_sompi": fee,
            "total_sompi": bond + prepaid + fee,
            "expiry_ms": pass.expiry,
            "text": format!(
                "Free trial for {}: you set aside {} (your first month, taken only if you stay after the trial) and a {} refundable deposit, plus a {} network fee. Nothing is paid to the creator now, and all of it comes back if you cancel in time",
                human_ms(self.tier().trial_ms), kas(prepaid), kas(bond), kas(fee)
            ),
        });
        summary["outputs"] = self.outputs(&tx, Some(&spk), &["Kept by the plan itself (not a charge)", "Your trial membership, holding the money you set aside and your deposit"], "Change, back to your wallet");
        let next_lane = Lane { remaining: if lane.lane.remaining > 0 { lane.lane.remaining - 1 } else { lane.lane.remaining }, ..lane.lane };
        let quote = Quote {
            buyer: member_addr.trim().to_string(),
            new_pass_owner: Some(member_addr.trim().to_string()),
            launch: None,
            tx,
            entries,
            unsigned,
            next_lane: Some(next_lane),
            lane_from: Some((lane.txid.clone(), lane.index)),
            pass_move: None,
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    pub async fn prepare_trial_v5(&self, chain: &Chain, owner_addr: &str, member_addr: &str, lane: &FoundLane) -> Result<(Quote, Value)> {
        let rec = self.record().ok_or_else(|| anyhow!("this site has no owner record (record.json)"))?;
        if rec.owner != owner_addr {
            bail!("only the owner record's current owner can give free trials");
        }
        let addr: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        let member: Address = member_addr.trim().try_into().map_err(|_| anyhow!("the recipient isn't a valid Kaspa address"))?;
        if member.prefix != self.prefix() {
            bail!("the recipient address is for a different network");
        }
        let member_id = owner_hash_of(member_addr.trim()).map_err(|_| anyhow!("the recipient must be a regular (P2PK) address"))?;
        let live = Live { txid: lane.txid.clone(), index: lane.index, value: lane.value };
        let (tx, entries, fee, _pass) = crate::trial_tx_v5(chain, &addr, &rec, self.tier(), self.cov()?, lane.lane, &live, member_id, member_addr.trim()).await?;
        let spk = pay_to_address_script(&addr);
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, owner_addr, &unsigned);
        let bond = self.tier().bond;
        let mut summary = json!({
            "action": "trial",
            "price_sompi": 0,
            "fee_sompi": fee,
            "total_sompi": bond + fee,
            "text": format!(
                "Give a free trial: {} goes into the new membership as its refundable deposit (it goes back to the member when they end it), {} network fee. The trial lasts {}.",
                kas(bond), kas(fee), human_ms(self.tier().trial_ms)
            ),
        });
        summary["outputs"] = self.outputs(&tx, Some(&spk), &["The sales lane (contract)", "The new trial membership, holding its refundable deposit", "Your owner record (contract, unchanged)"], "Change, back to your wallet");
        let next_lane = Lane { remaining: if lane.lane.remaining > 0 { lane.lane.remaining - 1 } else { lane.lane.remaining }, ..lane.lane };
        let quote = Quote {
            buyer: owner_addr.to_string(),
            new_pass_owner: Some(member_addr.trim().to_string()),
            launch: None,
            tx,
            entries,
            unsigned,
            next_lane: Some(next_lane),
            lane_from: Some((lane.txid.clone(), lane.index)),
            pass_move: None,
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    async fn prepare_for_opts(&self, chain: &Chain, payer_addr: &str, owner_addr: &str, kind: Sale, lane: &FoundLane, opts: SaleOpts) -> Result<(Quote, Value)> {
        let addr: Address = payer_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if addr.version != Version::PubKey {
            bail!("only regular (P2PK) addresses can pay for a pass");
        }
        let owner: Address = owner_addr.trim().try_into().map_err(|_| anyhow!("the recipient isn't a valid Kaspa address"))?;
        if owner.prefix != self.prefix() {
            bail!("the recipient address is for a different network");
        }
        let owner_id = owner_hash_of(owner_addr.trim()).map_err(|_| anyhow!("the recipient must be a regular (P2PK) address"))?;
        let trial = kind == Sale::Trial;
        if trial && !self.is_creator(payer_addr) {
            bail!("only the creator can give free trials");
        }
        let buyer = owner_addr.trim();
        let creator = self.creator()?;
        let payer = Payer::External { name: "your wallet".into(), spk: pay_to_address_script(&addr) };
        let live = Live { txid: lane.txid.clone(), index: lane.index, value: lane.value };
        let creator_pk = trial.then(|| addr.payload.to_vec());
        let prepaid = opts.prepaid;
        let periods = opts.periods;
        let cushion_pct = opts.cushion_pct;
        let bps = opts.voucher.as_ref().map(|v| v.bps).unwrap_or(0);
        let signs = opts.voucher.is_some();
        let b = sale_tx_full(&creator, chain, self.tier(), lane.lane, &live, self.cov()?, kind, false, owner_id, buyer, payer.clone(), None, creator_pk, opts).await?;
        if signs {
            note_prepared(&b.tx.id().to_string());
        }
        let unsigned: Vec<usize> = (1..b.tx.inputs.len()).filter(|&i| &b.entries[i].script_public_key == payer.spk()).collect();
        let slots: Vec<pskt::ScriptSlot> = b
            .sig_offset
            .map(|offset| pskt::ScriptSlot { input: 0, prebuilt: b.tx.inputs[0].signature_script.clone(), offset })
            .into_iter()
            .collect();
        let request = pskt::sign_request_with_scripts(&b.tx, &b.entries, payer_addr, &unsigned, &slots);
        let mut unsigned = unsigned;
        if !slots.is_empty() {
            unsigned.insert(0, 0);
        }
        let bond = self.tier().bond;
        let fee = self.tier().fee.as_ref().map(|f| f.of(b.charge)).unwrap_or(0);
        let what = match kind {
            Sale::Lifetime => "Lifetime",
            Sale::Term => "Commitment",
            _ => "Monthly",
        };
        // What the buyer reads: the plan's own period ("Yearly pass").
        let what_text = if what == "Monthly" { crate::period_word(self.tier().period_ms) } else { what };
        let extras = {
            let mut v = Vec::new();
            if bps > 0 && self.is_v5() {
                v.push("today's price".to_string());
            } else if bps > 0 {
                v.push(format!("holder price, {}% off", bps as f64 / 100.0));
            }
            if prepaid > 0 {
                let n = if self.is_v5() { periods } else { prepaid / lane.lane.price.max(1) };
                let at = if self.is_v5() && self.prepaid_with(100, cushion_pct) > 100 { format!(" at today's price, plus {cushion_pct}% in case KAS falls") } else if self.is_v5() && !self.priced_in_kas() { " at today's price, no cushion".to_string() } else { String::new() };
                v.push(format!("{} prepaid for auto-renew ({} renewal{}{}, yours to take back any time)", kas(prepaid), n, if n == 1 { "" } else { "s" }, at));
            }
            let t = v.join("; ");
            let mut c = t.chars();
            match c.next() {
                Some(f) => format!(". {}{}", f.to_uppercase(), c.as_str()),
                None => String::new(),
            }
        };
        let summary = json!({
            "kind": what.to_lowercase(),
            "price_sompi": b.charge,
            "bond_sompi": bond,
            "prepaid_sompi": prepaid,
            "discount_bps": bps,
            "fee_sompi": b.fee,
            "protocol_fee_sompi": fee,
            "total_sompi": b.charge + bond + prepaid + b.fee,
            "expiry_ms": b.pass.expiry,
            "lifetime": kind == Sale::Lifetime,
            "text": if trial {
                format!("Free trial for {}: you pay its {} refundable deposit, {} network fee", short(buyer), kas(bond), kas(b.fee))
            } else if buyer != payer_addr {
                format!(
                    "Gift a {} membership to {}: {}, {} refundable deposit, {} network fee",
                    if kind == Sale::Lifetime { "lifetime" } else { "monthly" },
                    short(buyer),
                    split_text(b.charge, fee),
                    kas(bond),
                    kas(b.fee)
                )
            } else {
                format!(
                    "{what_text} membership: {}, {} refundable deposit, {} network fee{extras}",
                    split_text(b.charge, fee),
                    kas(bond),
                    kas(b.fee)
                )
            },
        });
        let mut summary = summary;
        summary["outputs"] = self.outputs(
            &b.tx,
            Some(payer.spk()),
            &[
                "Kept by the plan itself (not a charge)",
                if prepaid > 0 {
                    "Your membership, holding your deposit and the amount paid ahead"
                } else if buyer == payer_addr {
                    "Your membership, holding your refundable deposit"
                } else {
                    "The new membership, holding its refundable deposit"
                },
            ],
            "Change, back to your wallet",
        );
        let quote = Quote {
            buyer: payer_addr.to_string(),
            new_pass_owner: Some(buyer.to_string()),
            launch: None,
            tx: b.tx,
            entries: b.entries,
            unsigned,
            next_lane: Some(b.next_lane),
            lane_from: Some((lane.txid.clone(), lane.index)),
            pass_move: None,
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    fn creator(&self) -> Result<Creator> {
        let a: Address = self.card.creator_address.as_str().try_into().map_err(|_| anyhow!("bad creator address in tier card"))?;
        Ok(Creator::from_address(&a))
    }

    /// The terms a verified pass carries, as the builders need them.
    fn pass_of(pass: &FoundPass) -> Pass {
        Pass { voucher: pass.voucher.as_deref().and_then(|v| v.parse().ok()), owner: pass.owner_hash.clone(), expiry: pass.expiry_ms, rate: pass.rate_sompi, life: pass.lifetime_price_sompi, credit: pass.credit, paid: pass.paid_sompi }
    }

    /// Renew or upgrade, with a holder discount of `bps` on a v4 renewal.
    pub async fn prepare_pass_v4(&self, chain: &Chain, payer_addr: &str, pass: &FoundPass, action: PassAction, bps: u64) -> Result<(Quote, Value)> {
        if action == PassAction::AutoRenew {
            bail!("auto-renewals are sent by the server");
        }
        let voucher = match action {
            PassAction::Renew if self.is_v5() => self.voucher_for_v5(bps, pass.voucher.as_deref().and_then(|v| v.parse().ok()))?.filter(|v| v.bps > 0),
            PassAction::Renew => self.voucher_for(bps)?,
            _ => None,
        };
        let bps = voucher.as_ref().map(|v| v.bps).unwrap_or(0);
        let addr: Address = payer_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if addr.version != Version::PubKey {
            bail!("only regular (P2PK) addresses can pay");
        }
        let tier = self.tier().clone();
        let cov = self.cov()?;
        let owner_id = pass.owner_hash.parse().map_err(|_| anyhow!("bad owner"))?;
        let p = Pass { voucher: pass.voucher.as_deref().and_then(|v| v.parse().ok()),
            owner: pass.owner_hash.clone(),
            expiry: pass.expiry_ms,
            rate: pass.rate_sompi,
            life: pass.lifetime_price_sompi,
            credit: pass.credit,
            paid: pass.paid_sompi,
        };
        // The bond is whatever the pass coin holds; `find` checks the exact coin anyway.
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: tier.bond };
        let payer = Payer::External { name: "your wallet".into(), spk: pay_to_address_script(&addr) };
        let signs = voucher.is_some();
        let b = pass_tx_v(&self.creator()?, chain, &tier, cov, &at, owner_id, &p, action, false, payer.clone(), voucher).await?;
        if signs {
            note_prepared(&b.tx.id().to_string());
        }
        let unsigned: Vec<usize> = (0..b.tx.inputs.len()).filter(|&i| &b.entries[i].script_public_key == payer.spk()).collect();
        let request = pskt::sign_request(&b.tx, &b.entries, payer_addr, &unsigned);
        let fee = tier.fee.as_ref().map(|f| f.of(b.charge)).unwrap_or(0);
        let text = match action {
            PassAction::Renew => format!(
                "Renew: {} ({}), {} network fee. New expiry {}",
                split_text(b.charge, fee),
                if bps > 0 && self.is_v5() { "today's price".into() } else if bps > 0 { format!("holder price, {}% off your locked rate", bps as f64 / 100.0) } else { "your locked rate".into() },
                kas(b.fee),
                when(b.next_pass.expiry, b.now)
            ),
            PassAction::Upgrade => format!("Upgrade to lifetime: {}, {} network fee. Never expires", split_text(b.charge, fee), kas(b.fee)),
            PassAction::AutoRenew => unreachable!(),
        };
        let summary = json!({
            "action": format!("{action:?}").to_lowercase(),
            "price_sompi": b.charge,
            "discount_bps": bps,
            "fee_sompi": b.fee,
            "protocol_fee_sompi": fee,
            "total_sompi": b.charge + b.fee,
            "expiry_ms": b.next_pass.expiry,
            "lifetime": b.next_pass.expiry >= LIFETIME,
            "text": text,
        });
        let mut summary = summary;
        summary["outputs"] = self.outputs(&b.tx, Some(payer.spk()), &["Your membership, updated, still holding your deposit and any balance"], "Change, back to your wallet");
        let quote = Quote {
            buyer: payer_addr.to_string(),
            new_pass_owner: None,
            launch: None,
            tx: b.tx,
            entries: b.entries,
            unsigned,
            next_lane: None,
            lane_from: None,
            pass_move: Some((pass.txid.clone(), pass.index, b.next_pass)),
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    /// The owner closes their pass and gets the bond back. Their wallet signs inside the
    /// pass contract's own script, which proves they own it.
    pub async fn prepare_close(&self, chain: &Chain, owner_addr: &str, pass: &FoundPass) -> Result<(Quote, Value)> {
        let addr: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if owner_hash_of(owner_addr)?.to_string() != pass.owner_hash {
            bail!("only the pass owner can close it");
        }
        let tier = self.tier().clone();
        let p = Pass { voucher: pass.voucher.as_deref().and_then(|v| v.parse().ok()),
            owner: pass.owner_hash.clone(),
            expiry: pass.expiry_ms,
            rate: pass.rate_sompi,
            life: pass.lifetime_price_sompi,
            credit: pass.credit,
            paid: pass.paid_sompi,
        };
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: tier.bond };
        let b = close_tx_external(&self.creator()?, chain, &tier, &at, &addr, &p).await?;
        let slot = pskt::ScriptSlot { input: 0, prebuilt: b.tx.inputs[0].signature_script.clone(), offset: b.sig_offset };
        let request = pskt::sign_request_with_scripts(&b.tx, &b.entries, owner_addr, &[], &[slot]);
        let summary = json!({
            "action": "close",
            "price_sompi": 0,
            "fee_sompi": b.fee,
            "refund_sompi": b.refund,
            "total_sompi": b.fee,
            "text": format!("End this membership: {} comes back to you ({}, {} network fee taken from it). Your access ends now", kas(b.refund + b.fee), if b.refund + b.fee > tier.bond { "its deposit and prepaid balance" } else { "its deposit" }, kas(b.fee)),
        });
        let mut summary = summary;
        summary["outputs"] = self.outputs(&b.tx, Some(&pay_to_address_script(&addr)), &[], "Your deposit, back to your wallet (minus the network fee)");
        let quote = Quote {
            buyer: owner_addr.to_string(),
            new_pass_owner: None,
            launch: None,
            tx: b.tx,
            entries: b.entries,
            unsigned: vec![0],
            next_lane: None,
            lane_from: None,
            pass_move: None,
            record: None,
            closes: Some((pass.txid.clone(), pass.index)),
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    /// The owner sends their pass to another wallet. They sign inside the pass contract,
    /// and their wallet pays the creator's royalty (and the protocol fee on it).
    pub async fn prepare_transfer(&self, chain: &Chain, owner_addr: &str, pass: &FoundPass, to_addr: &str) -> Result<(Quote, Value)> {
        let addr: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if owner_hash_of(owner_addr)?.to_string() != pass.owner_hash {
            bail!("only the pass owner can transfer it");
        }
        let to: Address = to_addr.trim().try_into().map_err(|_| anyhow!("the recipient isn't a valid Kaspa address"))?;
        if to.prefix != addr.prefix {
            bail!("the recipient address is for a different network");
        }
        let new_owner = owner_hash_of(to_addr.trim()).map_err(|_| anyhow!("the recipient must be a regular (P2PK) address"))?;
        let tier = self.tier().clone();
        let cov = self.cov()?;
        let p = Pass { voucher: pass.voucher.as_deref().and_then(|v| v.parse().ok()),
            owner: pass.owner_hash.clone(),
            expiry: pass.expiry_ms,
            rate: pass.rate_sompi,
            life: pass.lifetime_price_sompi,
            credit: pass.credit,
            paid: pass.paid_sompi,
        };
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: tier.bond };
        let b = transfer_tx_external(&self.creator()?, chain, &tier, cov, &at, &addr, &p, new_owner).await?;
        let owner_spk = pay_to_address_script(&addr);
        let unsigned: Vec<usize> = (1..b.tx.inputs.len()).filter(|&i| b.entries[i].script_public_key == owner_spk).collect();
        let slot = pskt::ScriptSlot { input: 0, prebuilt: b.tx.inputs[0].signature_script.clone(), offset: b.sig_offset };
        let request = pskt::sign_request_with_scripts(&b.tx, &b.entries, owner_addr, &unsigned, &[slot]);
        let royalty = tier.royalty;
        let fee = tier.fee.as_ref().map(|f| f.of(royalty)).unwrap_or(0);
        let summary = json!({
            "action": "transfer",
            "price_sompi": royalty,
            "fee_sompi": b.fee,
            "protocol_fee_sompi": fee,
            "total_sompi": royalty + b.fee,
            "text": format!(
                "Give this membership to {}: royalty {}, {} network fee. They get the rest of your time; you lose access",
                short(to_addr.trim()),
                if royalty > 0 { split_text(royalty, fee) } else { "none".into() },
                kas(b.fee)
            ),
        });
        let mut summary = summary;
        summary["outputs"] = self.outputs(&b.tx, Some(&owner_spk), &["The membership, now theirs"], "Change, back to your wallet");
        let mut all = unsigned.clone();
        all.insert(0, 0);
        let quote = Quote {
            buyer: owner_addr.to_string(),
            new_pass_owner: None,
            launch: None,
            tx: b.tx,
            entries: b.entries,
            unsigned: all,
            next_lane: None,
            lane_from: None,
            pass_move: None,
            record: None,
            closes: Some((pass.txid.clone(), pass.index)),
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    /// v4: anyone adds prepaid balance to a pass (`periods` renewals at its locked rate, or `add` sompi).
    pub async fn prepare_deposit(&self, chain: &Chain, payer_addr: &str, pass: &FoundPass, add: u64) -> Result<(Quote, Value)> {
        let addr: Address = payer_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if addr.version != Version::PubKey {
            bail!("only regular (P2PK) addresses can pay");
        }
        if pass.lifetime {
            bail!("a lifetime pass never renews, so it doesn't need a balance");
        }
        let tier = self.tier().clone();
        let owner_id = pass.owner_hash.parse().map_err(|_| anyhow!("bad owner"))?;
        let p = Self::pass_of(pass);
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: pass.value_sompi };
        let payer = Payer::External { name: "your wallet".into(), spk: pay_to_address_script(&addr) };
        let (tx, entries, net_fee, value) = deposit_tx(&self.creator()?, chain, &tier, self.cov()?, &at, owner_id, &p, add, payer.clone()).await?;
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| &entries[i].script_public_key == payer.spk()).collect();
        let request = pskt::sign_request(&tx, &entries, payer_addr, &unsigned);
        let balance = value.saturating_sub(tier.bond);
        let covers = balance / pass.rate_sompi.max(1);
        let mut summary = json!({
            "action": "deposit",
            "price_sompi": add,
            "fee_sompi": net_fee,
            "total_sompi": add + net_fee,
            "balance_sompi": balance,
            "text": format!(
                "Add {} to your membership's balance ({} network fee). Balance becomes {}, enough for {} auto-renewal{}. It stays yours: take it back any time",
                kas(add), kas(net_fee), kas(balance), covers, if covers == 1 { "" } else { "s" }
            ),
        });
        summary["outputs"] = self.outputs(&tx, Some(payer.spk()), &["Your membership, holding your deposit and the larger balance"], "Change, back to your wallet");
        let quote = Quote {
            buyer: payer_addr.to_string(),
            new_pass_owner: None,
            launch: None,
            tx,
            entries,
            unsigned,
            next_lane: None,
            lane_from: None,
            pass_move: Some((pass.txid.clone(), pass.index, p)),
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    /// v4: the owner takes their prepaid balance back, which cancels auto-renew. Access is unchanged.
    pub async fn prepare_withdraw(&self, chain: &Chain, owner_addr: &str, pass: &FoundPass) -> Result<(Quote, Value)> {
        let addr: Address = owner_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if owner_hash_of(owner_addr)?.to_string() != pass.owner_hash {
            bail!("only the pass owner can take its balance back");
        }
        let tier = self.tier().clone();
        let p = Self::pass_of(pass);
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: pass.value_sompi };
        let b = withdraw_tx_external(&self.creator()?, chain, &tier, self.cov()?, &at, &addr, &p).await?;
        let slot = pskt::ScriptSlot { input: 0, prebuilt: b.tx.inputs[0].signature_script.clone(), offset: b.sig_offset };
        let request = pskt::sign_request_with_scripts(&b.tx, &b.entries, owner_addr, &[], &[slot]);
        let mut summary = json!({
            "action": "withdraw",
            "price_sompi": 0,
            "fee_sompi": b.fee,
            "refund_sompi": b.refund,
            "total_sompi": b.fee,
            "text": format!(
                "Cancel auto-renew: your {} balance comes back to you ({} network fee taken from it). Your membership keeps its deposit and your access stays until it ends ({})",
                kas(b.refund + b.fee),
                kas(b.fee),
                if pass.lifetime { "never".to_string() } else { when(pass.expiry_ms, chain.now_ms().await.unwrap_or(0)) }
            ),
        });
        summary["outputs"] = self.outputs(&b.tx, Some(&pay_to_address_script(&addr)), &["Your membership, keeping your deposit"], "Your balance, back to you (minus the network fee)");
        let quote = Quote {
            buyer: owner_addr.to_string(),
            new_pass_owner: None,
            launch: None,
            tx: b.tx,
            entries: b.entries,
            unsigned: vec![0],
            next_lane: None,
            lane_from: None,
            pass_move: Some((pass.txid.clone(), pass.index, p)),
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    /// The keeper renews a v4 pass from its own balance (within the auto-renew window) and sends
    /// it. The keeper only pays the network fee; the payment can only go to the creator.
    pub async fn autorenew(&self, chain: &mut Chain, pass: &FoundPass, bps: u64) -> Result<(String, u64, u64)> {
        let keeper = self.keeper.clone().ok_or_else(|| anyhow!("this server has no keeper wallet"))?;
        // v5 auto-renewals always carry the server's signature, even at no discount.
        let voucher = if self.is_v5() { self.voucher_for_v5(bps, pass.voucher.as_deref().and_then(|v| v.parse().ok()))? } else { self.voucher_for(bps)? };
        let tier = self.tier().clone();
        let owner_id = pass.owner_hash.parse().map_err(|_| anyhow!("bad owner"))?;
        let p = Self::pass_of(pass);
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: pass.value_sompi };
        let b = pass_tx_v(&self.creator()?, chain, &tier, self.cov()?, &at, owner_id, &p, PassAction::AutoRenew, false, Payer::Key(keeper), voucher).await?;
        verify_all(&b.tx, &b.entries)?;
        note_signed(&self.dir, &b.tx.id().to_string());
        let id = chain.submit(&b.tx, &b.entries).await?;
        Ok((id.to_string(), b.charge, b.tx.outputs[0].value.saturating_sub(tier.bond)))
    }

    /// One order's pay-in contract and address (paying from any wallet with a QR code): the
    /// coins can only buy or renew on this level for `buyer`, or go back to `buyer` after
    /// `refund_after` (ms).
    pub fn payin_for(&self, buyer: &str, max_spend: u64, refund_after: i64) -> Result<(crate::Compiled, ScriptPublicKey, String)> {
        let addr: Address = buyer.trim().try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if addr.prefix != self.prefix() {
            bail!("that address is for a different network");
        }
        let owner = owner_hash_of(buyer.trim())?;
        let (_, _, tmpl) = crate::membership(&self.creator()?, self.tier(), kaspa_consensus_core::Hash::default(), &crate::blank_pass())?.template();
        let refund = pay_to_address_script(&addr);
        let pin = crate::payin(owner, self.cov()?, tmpl, &refund, max_spend, refund_after, crate::PAYIN_REFUND_FEE)?;
        let spk = pin.spk();
        let at = extract_script_pub_key_address(&spk, self.prefix()).map_err(|e| anyhow!("{e}"))?.to_string();
        Ok((pin, refund, at))
    }

    /// What a pay-in purchase uses from the payment (price, bond, prepaid), as `prepare_v4` sizes it.
    pub fn payin_buy_cost(&self, lane: &FoundLane, bps: u64, renew_ppm: u64, periods: u64) -> (u64, u64) {
        let due = crate::discounted_ppm(lane.lane.price, bps);
        let prepaid = periods * self.prepaid_margin(crate::discounted_ppm(lane.lane.price, renew_ppm));
        (due, prepaid)
    }

    /// COMPLETE a pay-in order with a purchase: signed by this server's voucher key, checked
    /// like a node would, and sent. Returns the txid and what went to the creator.
    #[allow(clippy::too_many_arguments)]
    pub async fn payin_buy(&self, chain: &mut Chain, buyer: &str, lane: &FoundLane, bps: u64, renew_ppm: u64, periods: u64, pin: crate::Compiled, refund: ScriptPublicKey, coin: crate::Utxo) -> Result<(String, u64)> {
        if periods > 36 {
            bail!("prepay at most 36 periods");
        }
        let (_, prepaid) = self.payin_buy_cost(lane, bps, renew_ppm, periods);
        let voucher = self.voucher_for_v5(bps, lane.lane.voucher)?.filter(|v| v.bps > 0);
        let owner_id = owner_hash_of(buyer.trim())?;
        let live = Live { txid: lane.txid.clone(), index: lane.index, value: lane.value };
        let opts = SaleOpts { prepaid, voucher, periods, cushion_pct: DEFAULT_CUSHION };
        let payer = Payer::PayIn(crate::PayInCoin { pin, coin, refund });
        let b = sale_tx_full(&self.creator()?, chain, self.tier(), lane.lane, &live, self.cov()?, Sale::Monthly, false, owner_id, buyer.trim(), payer, None, None, opts).await?;
        verify_all(&b.tx, &b.entries)?;
        note_signed(&self.dir, &b.tx.id().to_string());
        let id = chain.submit(&b.tx, &b.entries).await?;
        Ok((id.to_string(), b.charge))
    }

    /// A promotion's free pass: the keeper wallet buys a pass on this level for `member` at
    /// today's price less `bps` (about 1 KAS, which goes to the creator's vault) and pays the
    /// pass's refundable deposit (the member's when they close it) and the network fee. Signed by
    /// this server, checked like a node would, and sent. Returns the txid and what went to the vault.
    /// On failure, the txid comes back too when the transaction was built and handed to the
    /// network (it may have gone through); None means nothing was sent.
    pub async fn promo_buy(&self, chain: &mut Chain, member: &str, lane: &FoundLane, bps: u64) -> std::result::Result<(String, u64), (Option<String>, anyhow::Error)> {
        let ns = |e: anyhow::Error| (None, e);
        let keeper = self.keeper.clone().ok_or_else(|| ns(anyhow!("this server has no keeper wallet")))?;
        if !self.is_v5() {
            return Err(ns(anyhow!("promotions need a v5 membership")));
        }
        let addr: Address = member.trim().try_into().map_err(|_| ns(anyhow!("not a valid Kaspa address")))?;
        if addr.prefix != self.prefix() {
            return Err(ns(anyhow!("that address is for a different network")));
        }
        let owner_id = owner_hash_of(member.trim()).map_err(|_| ns(anyhow!("the member must use a regular (P2PK) address")))?;
        let voucher = self.voucher_for_v5(bps, lane.lane.voucher).map_err(ns)?.filter(|v| v.bps > 0);
        let live = Live { txid: lane.txid.clone(), index: lane.index, value: lane.value };
        let b = sale_tx_full(&self.creator().map_err(ns)?, chain, self.tier(), lane.lane, &live, self.cov().map_err(ns)?, Sale::Monthly, false, owner_id, member.trim(), Payer::Key(keeper), None, None, SaleOpts { voucher, ..Default::default() }).await.map_err(ns)?;
        verify_all(&b.tx, &b.entries).map_err(ns)?;
        let txid = b.tx.id().to_string();
        note_signed(&self.dir, &txid);
        let id = chain.submit(&b.tx, &b.entries).await.map_err(|e| (Some(txid.clone()), e))?;
        Ok((id.to_string(), b.charge))
    }

    /// COMPLETE a pay-in order with a renewal of the buyer's pass.
    pub async fn payin_renew(&self, chain: &mut Chain, pass: &FoundPass, bps: u64, pin: crate::Compiled, refund: ScriptPublicKey, coin: crate::Utxo) -> Result<(String, u64)> {
        let voucher = self.voucher_for_v5(bps, pass.voucher.as_deref().and_then(|v| v.parse().ok()))?.filter(|v| v.bps > 0);
        let owner_id = pass.owner_hash.parse().map_err(|_| anyhow!("bad owner"))?;
        let p = Self::pass_of(pass);
        let at = Live { txid: pass.txid.clone(), index: pass.index, value: pass.value_sompi };
        let payer = Payer::PayIn(crate::PayInCoin { pin, coin, refund });
        let b = pass_tx_v(&self.creator()?, chain, self.tier(), self.cov()?, &at, owner_id, &p, PassAction::Renew, false, payer, voucher).await?;
        verify_all(&b.tx, &b.entries)?;
        note_signed(&self.dir, &b.tx.id().to_string());
        let id = chain.submit(&b.tx, &b.entries).await?;
        Ok((id.to_string(), b.charge))
    }

    /// REFUND a pay-in coin to its buyer (only once the order's refund time has passed).
    pub async fn payin_refund(&self, chain: &mut Chain, pin: crate::Compiled, refund: ScriptPublicKey, coin: crate::Utxo) -> Result<(String, u64)> {
        let now = chain.now_ms().await?;
        let (tx, entries, _) = crate::payin_refund_tx(chain, crate::PayInCoin { pin, coin, refund }, now as u64).await?;
        verify_all(&tx, &entries)?;
        let back = tx.outputs[0].value;
        let id = chain.submit(&tx, &entries).await?;
        Ok((id.to_string(), back))
    }

    /// Takes the wallet's signatures, checks the whole transaction like a node would, sends it.
    pub async fn finish(&self, chain: &mut Chain, mut q: Quote, signed_json: &str) -> Result<Finished> {
        pskt::apply_signatures(&mut q.tx, signed_json, &q.unsigned)?;
        verify_all(&q.tx, &q.entries).map_err(|e| anyhow!("the signed transaction didn't pass the final check ({e}); nothing was sent"))?;
        let prepared = PREPARED.lock().unwrap_or_else(|e| e.into_inner()).remove(&q.txid()).is_some();
        if prepared {
            note_signed(&self.dir, &q.txid());
        }
        let id = chain.submit(&q.tx, &q.entries).await.map_err(|e| {
            let m = e.to_string();
            if m.contains("already spent") || m.contains("orphan") || m.contains("missing") {
                anyhow!("someone else bought at the same moment; please try again")
            } else {
                anyhow!("the network refused the purchase: {m}")
            }
        })?;
        let new_lane = match (q.next_lane, q.lane_from.clone()) {
            (Some(lane), Some(from)) => Some((FoundLane { txid: id.to_string(), index: 0, value: q.tx.outputs[0].value, lane }, from)),
            _ => None,
        };
        // A launch: save the new tier's public card next to this website.
        let card = match q.launch.take() {
            Some(l) => {
                let name = format!("tier-card-{}.json", &l.card.covenant_id[..12]);
                std::fs::write(self.dir.join(&name), serde_json::to_string_pretty(&l.card)?)?;
                if let Some(p) = l.price {
                    let mut all = crate::prices::level_prices(&self.dir);
                    all.insert(l.card.covenant_id.clone(), p);
                    crate::prices::save_level_prices(&self.dir, &all)?;
                }
                Some((l.card, name))
            }
            None => None,
        };
        // v5: the owner record this server follows, as it is now on chain.
        if let Some(r) = q.record.take() {
            std::fs::write(self.dir.join(RECORD_FILE), serde_json::to_string_pretty(&r)?)?;
        }
        // The command line's records, if this is the creator's folder. Best effort.
        if let Some(home) = &self.home {
            let _ = keep_state_in_step(home, &q, &id.to_string());
        }
        Ok(Finished { txid: id.to_string(), new_lane, card })
    }

    /// New prices, including the commitment term on a v4 tier.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_prices_v4(&self, chain: &Chain, creator_addr: &str, lane: &FoundLane, price: u64, life: u64, credit: bool, term_price: u64, term_ms: i64) -> Result<(Quote, Value)> {
        let v4 = self.v4().is_some();
        let (term_price, term_ms) = if v4 { (term_price, if term_price > 0 { term_ms } else { 0 }) } else { (0, 0) };
        if term_price > 0 && term_ms <= 0 {
            bail!("a commitment term needs a length");
        }
        if !self.is_creator(creator_addr) {
            bail!("only the membership's creator can change prices");
        }
        let addr: Address = creator_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        let live = Live { txid: lane.txid.clone(), index: lane.index, value: lane.value };
        let new = Lane { voucher: lane.lane.voucher, price, life, credit, remaining: lane.lane.remaining, term_price, term_ms };
        let spk = pay_to_address_script(&addr);
        // v5: through the owner record, proven by the owner's own coin (no signature in a script).
        let rec = self.record().ok_or_else(|| anyhow!("this site has no owner record (record.json)"))?;
        if rec.owner != creator_addr {
            bail!("only the owner record's current owner can change prices");
        }
        let (tx, entries, fee, next) = crate::set_prices_tx_v5(chain, &addr, &rec, self.tier(), self.cov()?, lane.lane, &live, new).await?;
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, creator_addr, &unsigned);
        let b = crate::CloseBuild { tx, entries, fee, refund: 0, sig_offset: 0 };
        let mut summary = json!({
            "action": "prices",
            "price_sompi": 0,
            "fee_sompi": b.fee,
            "total_sompi": b.fee,
            "text": format!(
                "New prices for new buyers: {} monthly, {} lifetime, credit {}{}. Existing passes keep theirs. {} network fee",
                kas(price),
                if life > 0 { kas(life) } else { "not offered".into() },
                if credit { "on" } else { "off" },
                if !v4 {
                    String::new()
                } else if term_price > 0 {
                    format!(", commitment {} for {}", kas(term_price), human_ms(term_ms))
                } else {
                    ", no commitment term".into()
                },
                kas(b.fee)
            ),
        });
        summary["outputs"] = self.outputs(&b.tx, Some(&spk), &["The sales lane with the new prices (contract)", "Your owner record (contract, unchanged)"], "Change, back to your wallet");
        let all = unsigned;
        let quote = Quote {
            buyer: creator_addr.to_string(),
            new_pass_owner: None,
            launch: None,
            tx: b.tx,
            entries: b.entries,
            unsigned: all,
            next_lane: Some(next),
            lane_from: Some((lane.txid.clone(), lane.index)),
            pass_move: None,
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }

    /// Launches a new tier with the signed-in wallet as its creator.
    pub async fn prepare_launch(&self, chain: &Chain, creator_addr: &str, f: &LaunchForm) -> Result<(Quote, Value)> {
        let addr: Address = creator_addr.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
        if addr.version != Version::PubKey || addr.prefix != self.prefix() {
            bail!("launch with a regular wallet address on {}", self.card.network);
        }
        let k = |x: f64| -> Result<u64> {
            if !(0.0..=1e9).contains(&x) {
                bail!("amounts must be between 0 and 1,000,000,000 KAS");
            }
            Ok((x * SOMPI as f64).round() as u64)
        };
        if f.period_minutes < 1 || f.trial_minutes < 0 || f.max_ahead_minutes < 0 {
            bail!("the period must be at least 1 minute");
        }
        if f.bond_kas < 0.5 {
            bail!("the bond must be at least 0.5 KAS (smaller outputs are costly on Kaspa)");
        }
        // No fee: the contracts still carry fee settings, but at 0% they never ask for a fee output.
        let fee = match (&self.fee_address, self.no_fee) {
            (_, true) => Some(Fee { address: creator_addr.to_string(), bps: 0, min: 0 }),
            (Some(a), false) => Some(Fee { address: a.clone(), bps: crate::DEFAULT_FEE_BPS, min: SOMPI / 2 }),
            (None, false) => bail!("this website has no KasPass fee address set; start it with --fee-address or --no-fee"),
        };
        let period_ms = f.period_minutes * MINUTE;
        // New memberships are v5: they need the creator's owner record (dashboard, Payouts tab).
        let Some(rec) = self.record() else {
            bail!("set up your owner record first (dashboard, Payouts tab): new memberships pay into its vault");
        };
        if rec.owner != creator_addr {
            bail!("only this site's owner can launch new levels here");
        }
        let record_id = Some(rec.record_id);
        let v4 = if f.v4 || record_id.is_some() {
            let max_discount_bps = if record_id.is_some() {
                // v5: the on-chain price is a KAS ceiling and the server signs today's price
                // as a discount off it (up to the plan's largest discount, 99.99% by default). Auto-renew always needs its signature.
                V5_MAX_DISCOUNT_BPS
            } else {
                if !(0.0..=50.0).contains(&f.max_discount_pct) {
                    bail!("the largest holder discount must be between 0% and 50%");
                }
                (f.max_discount_pct * 100.0).round() as u64
            };
            // A tier with discounts trusts this server's voucher key; without, nobody's.
            // (v5 plans trust the newest key, if the server key was ever replaced.)
            let key = if record_id.is_some() { self.v5_key() } else { self.voucher.clone() };
            let voucher_key_hash = match (&key, max_discount_bps > 0) {
                (Some(w), true) => w.id.to_string(),
                (None, true) => bail!("this server has no voucher key, so it can't offer holder discounts"),
                (_, false) => kaspa_consensus_core::Hash::default().to_string(),
            };
            let auto_window_ms = if f.auto_window_minutes > 0 { f.auto_window_minutes * MINUTE } else { (period_ms / 3).min(3 * 24 * 60 * MINUTE) };
            if auto_window_ms >= period_ms {
                bail!("the auto-renew window must be shorter than the period");
            }
            // v5 when this site has an owner record: payouts to its vault, actions through it.
            if f.self_trial && (record_id.is_none() || f.trial_minutes <= 0) {
                bail!("a free trial members start themselves needs a trial length");
            }
            if f.self_trial && f.trial_minutes * MINUTE >= period_ms {
                bail!("the free trial must be shorter than the billing period");
            }
            // Every pass takes one from a limited supply and trials can't give theirs back, so
            // free trials could use the whole supply up.
            if f.self_trial && f.supply > 0 {
                bail!("free trials members start themselves can't be combined with a limited number of passes: each trial would use one up for good. Choose unlimited passes, or switch the free trial off");
            }
            if f.self_trial && self.card.network == "mainnet" && f.trial_minutes < 24 * 60 {
                bail!("a free trial members start themselves must be at least 1 day long");
            }
            Some(V4 { auto_window_ms, voucher_key_hash, max_discount_bps, record_id: record_id.clone(), self_trial: f.self_trial })
        } else {
            None
        };
        if v4.is_none() && (f.term_kas > 0.0 || f.max_discount_pct > 0.0) {
            bail!("commitment plans and holder discounts need a v4 membership");
        }
        if f.term_kas > 0.0 && !(1..=120).contains(&f.term_periods) {
            bail!("a commitment term runs 1 to 120 periods");
        }
        // v5: price in a currency; the lane holds a KAS ceiling (3x today), never a lifetime or term.
        let v5_price = match (&record_id, &f.currency, f.amount) {
            (Some(_), Some(cur), Some(amount)) => {
                if !crate::prices::PLAN_CURRENCIES.contains(&cur.as_str()) {
                    bail!("currency must be one of {}", crate::prices::PLAN_CURRENCIES.join(", "));
                }
                if !(0.5..=1e7).contains(&amount) {
                    bail!("the price must be between 0.50 and 10,000,000");
                }
                let founding = match (f.founding_amount, f.founding_until_ms) {
                    (Some(a), Some(u)) if a > 0.0 => {
                        if a >= amount {
                            bail!("the founding price must be lower than the regular price");
                        }
                        Some(crate::prices::Founding { amount: a, until_ms: u })
                    }
                    _ => None,
                };
                for (what, a) in [("lifetime", f.lifetime_amount), ("commitment", f.term_amount)] {
                    if let Some(a) = a {
                        if !(0.5..=1e8).contains(&a) {
                            bail!("the {what} price must be between 0.50 and 100,000,000");
                        }
                    }
                }
                if f.term_amount.is_some() && !(2..=120).contains(&f.term_periods) {
                    bail!("a commitment term runs 2 to 120 periods");
                }
                Some(crate::prices::LevelPrice { currency: cur.clone(), amount, founding, lifetime: f.lifetime_amount, term: f.term_amount })
            }
            _ => bail!("choose the plan's currency and price"),
        };
        // On-chain prices. Priced in a currency: each one is a KAS ceiling (today's amount times
        // `ceiling_times`) and buyers pay today's price under it. Priced in KAS: the real prices.
        let (ceiling_kas, lifetime_kas, term_kas) = match &v5_price {
            Some(p) if p.currency == "KAS" => (p.amount, p.lifetime.unwrap_or(0.0), p.term.unwrap_or(0.0)),
            Some(p) => {
                let rates = crate::prices::fresh().ok_or_else(|| anyhow!("the KAS price is unavailable right now; try again shortly"))?;
                let times = f.ceiling_times.unwrap_or(V5_CEILING_TIMES);
                if !(1.5..=10.0).contains(&times) {
                    bail!("the KAS ceiling must be between 1.5 and 10 times today's price");
                }
                let ceil = |amount: f64| -> Result<f64> { Ok((rates.kas(&p.currency, amount).ok_or_else(|| anyhow!("no rate for {}", p.currency))? * times).ceil()) };
                (ceil(p.amount)?, p.lifetime.map(ceil).transpose()?.unwrap_or(0.0), p.term.map(ceil).transpose()?.unwrap_or(0.0))
            }
            None => (f.price_kas, f.lifetime_kas, f.term_kas),
        };
        let term_price = k(term_kas)?;
        let tier = Tier {
            period_ms,
            trial_ms: f.trial_minutes * MINUTE,
            resellable: f.resellable,
            royalty: if f.resellable { k(f.royalty_kas)? } else { 0 },
            bond: k(f.bond_kas)?,
            max_ahead_ms: f.max_ahead_minutes * MINUTE,
            fee,
            v4,
        };
        let lane = Lane { voucher: None,
            price: k(ceiling_kas)?,
            life: k(lifetime_kas)?,
            credit: f.credit,
            remaining: if f.supply > 0 { f.supply } else { -1 },
            term_price,
            term_ms: if term_price > 0 { f.term_periods.checked_mul(period_ms).filter(|&ms| ms > 0).ok_or_else(|| anyhow!("that commitment term is too long"))? } else { 0 },
        };
        let start = chain.sink().await?;
        let (tx, entries, net_fee, cov) = launch_tx_external(chain, &addr, &tier, &lane).await?;
        let spk = pay_to_address_script(&addr);
        let unsigned: Vec<usize> = (0..tx.inputs.len()).filter(|&i| entries[i].script_public_key == spk).collect();
        let request = pskt::sign_request(&tx, &entries, creator_addr, &unsigned);
        let txid = tx.id().to_string();
        let card = TierCard {
            kaspass: if tier.v4.as_ref().is_some_and(|v| v.record_id.is_some()) { 5 } else if tier.v4.is_some() { 4 } else { 3 },
            network: self.card.network.clone(),
            covenant_id: cov.to_string(),
            creator_address: creator_addr.to_string(),
            settings: tier,
            start_hash: start.to_string(),
            launch_txid: txid.clone(),
            known_lane: Some(KnownLane { txid, index: 0, lane }),
            placeholder: false,
        };
        let mut summary = json!({
            "action": "launch",
            "price_sompi": 0,
            "fee_sompi": net_fee,
            "total_sompi": SOMPI + net_fee,
            "covenant_id": cov.to_string(),
            "text": format!("Launch your membership: 1 KAS goes into its sales lane (yours, and it stays in the contract), {} network fee", kas(net_fee)),
        });
        if v5_price.is_some() {
            // v5: the most KAS a purchase or renewal of this plan can ever charge.
            summary["ceiling_kas"] = json!(ceiling_kas);
            summary["text"] = json!(format!("{} The plan's KAS ceiling is {} KAS.", summary["text"].as_str().unwrap_or_default(), ceiling_kas));
        }
        summary["outputs"] = self.outputs(&tx, Some(&spk), &["Your new membership's sales lane (contract)"], "Change, back to your wallet");
        let quote = Quote {
            buyer: creator_addr.to_string(),
            new_pass_owner: None,
            launch: Some(PendingLaunch { card, price: v5_price }),
            tx,
            entries,
            unsigned,
            next_lane: None,
            lane_from: None,
            pass_move: None,
            closes: None,
            record: None,
            created: Instant::now(),
        };
        Ok((quote, json!({ "request": request, "summary": summary })))
    }
}

fn short(a: &str) -> String {
    a.split(':').nth(1).filter(|x| x.len() > 10).map(|x| format!("{}…{}", &x[..6], &x[x.len() - 4..])).unwrap_or_else(|| a.to_string())
}

fn keep_state_in_step(home: &Path, q: &Quote, id: &str) -> Result<()> {
    let id = id.to_string();
    {
        let mut st = State::load(home)?;
        if let Some(lane) = q.next_lane {
            st.minter = Some(Live { txid: id.clone(), index: 0, value: q.tx.outputs[0].value });
            st.lane = Some(lane);
        }
        // If the command line was tracking this pass, keep its records in step.
        if let (Some((old_txid, old_index, next)), Some(m)) = (q.pass_move.clone(), st.membership.as_mut()) {
            if m.at.txid == old_txid && m.at.index == old_index {
                *m = Member { at: Live { txid: id.clone(), index: 0, value: q.tx.outputs[0].value }, pass: Pass { owner: m.pass.owner.clone(), ..next } };
            }
        }
        if let (Some((t, i)), Some(m)) = (&q.closes, st.membership.as_ref()) {
            if &m.at.txid == t && m.at.index == *i {
                st.membership = None;
            }
        }
        st.save(home)?;
    }
    Ok(())
}

/// "20 KAS (19.6 to the creator, 0.4 KasPass fee)", or just "2 KAS to the creator".
fn split_text(amount: u64, fee: u64) -> String {
    if fee == 0 {
        format!("{} to the creator", kas(amount))
    } else {
        format!("{} ({} to the creator, {} KasPass fee)", kas(amount), kas(amount - fee), kas(fee))
    }
}

/// KAS with every significant digit, the way Kaspire prints it (e.g. 967.8384461).
fn kas_exact(sompi: u64) -> String {
    let whole = sompi / 100_000_000;
    let frac = sompi % 100_000_000;
    if frac == 0 {
        format!("{whole} KAS")
    } else {
        format!("{whole}.{} KAS", format!("{frac:08}").trim_end_matches('0'))
    }
}
