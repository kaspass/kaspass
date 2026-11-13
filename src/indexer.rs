//! KasPass indexer: finds every pass in a tier and proves each one against the chain.
//!
//! How it works
//! 1. A *tier card* (tier-card.json) describes a tier: its covenant ID, the
//!    creator's address and the permanent settings. The card is self-checking:
//!    if any setting is wrong, the computed templates won't match any real
//!    output, so a bad card simply finds nothing.
//! 2. The scanner walks blocks from the tier's launch point. For every input
//!    that spends a KasPass contract, it reads the revealed script (old state +
//!    the entry called + its arguments) and computes the exact new pass state.
//! 3. A candidate pass is only accepted if (a) an output carries this tier's
//!    covenant ID, (b) its script hash equals the one computed from that state,
//!    and (c) that very coin is still unspent on-chain right now.
//!
//! So the indexer can't invent or alter a pass: every answer is re-derivable
//! from the chain by anyone.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, anyhow, bail};
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::tx::{ScriptPublicKey, Transaction, TransactionOutpoint};
use kaspa_txscript::{extract_script_pub_key_address, pay_to_script_hash_script};
use serde::{Deserialize, Serialize};
use silverscript_abi::{ArtifactValue, encode_runtime_state_script};

use crate::{Chain, Creator, LIFETIME, Lane, Tier, blank_pass, kas, membership, minter, when};

/// Public description of a tier. Safe to share: contains no keys.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TierCard {
    pub kaspass: u32,
    pub network: String,
    pub covenant_id: String,
    pub creator_address: String,
    pub settings: Tier,
    /// A block at or before the launch; scanning starts here.
    pub start_hash: String,
    pub launch_txid: String,
    /// A sales lane and its prices at a known point (the launch, for new tiers). Only needed
    /// until the first sale: every sale reveals the lane's state on-chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_lane: Option<KnownLane>,
    /// A stand-in while a new site has no membership yet: nothing is on sale, and the creator
    /// launches the first one from the dashboard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placeholder: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct KnownLane {
    pub txid: String,
    pub index: u32,
    pub lane: Lane,
}

/// One payment to the creator (or a free trial), for the dashboard's earnings and activity.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub txid: String,
    /// "monthly", "lifetime", "term", "trial" (given by the creator), "self_trial" (started by the
    /// member), "renew", "autorenew", "upgrade" or "transfer".
    pub kind: String,
    pub owner_hash: String,
    /// What was paid, before the protocol fee.
    pub amount_sompi: u64,
    pub fee_sompi: u64,
    pub creator_sompi: u64,
    /// v5: the payment carried a voucher signature (a price below the ceiling, or an auto-renewal).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub voucher: bool,
    /// When its block was mined (ms; 0 when not known, for events read before this was kept).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub time_ms: i64,
}

fn is_zero(x: &i64) -> bool {
    *x == 0
}

/// A live, verified sales lane: where new passes are sold from, and at what prices.
#[derive(Serialize, Clone, Debug)]
pub struct FoundLane {
    pub txid: String,
    pub index: u32,
    pub value: u64,
    pub lane: Lane,
}

/// A pass the indexer has found and verified.
#[derive(Serialize, Clone, Debug)]
pub struct FoundPass {
    pub txid: String,
    pub index: u32,
    pub owner_hash: String,
    pub kind: String,
    pub active: bool,
    pub expiry_ms: i64,
    pub lifetime: bool,
    pub rate_sompi: u64,
    pub lifetime_price_sompi: u64,
    pub credit: bool,
    pub paid_sompi: u64,
    /// What the pass coin holds right now. On v4 tiers, anything above the bond is prepaid
    /// balance that auto-renew pays from.
    pub value_sompi: u64,
    pub balance_sompi: u64,
    /// Address that paid for the pass when it was first issued (from the purchase's change output).
    pub bought_by: Option<String>,
    /// The owner's address, when it's known (the buyer bought it for themselves).
    pub owner_address: Option<String>,
    /// v5: the voucher key this pass trusts (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voucher: Option<String>,
}

/// Saved scan progress, so the next scan only reads new blocks. It can only hide passes,
/// never invent them: every cached pass is re-checked against the chain before it's reported.
#[derive(Serialize, Deserialize)]
pub struct ScanCache {
    pub covenant_id: String,
    pub cursor: String,
    pub scanned_txs: usize,
    passes: Vec<CachedPass>,
    #[serde(default)]
    lanes: Vec<KnownLane>,
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Serialize, Deserialize)]
struct CachedPass {
    txid: String,
    index: u32,
    owner: String,
    expiry: i64,
    rate: i64,
    life: i64,
    credit: i64,
    paid: i64,
    bought_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    voucher: Option<String>,
}

/// Friendly names for addresses, shown instead of hashes.
#[derive(Default, Clone)]
pub struct Names(pub Vec<(String, String)>); // (label, address)

impl Names {
    pub fn for_owner(&self, owner_hash: &str) -> Option<String> {
        self.0.iter().find(|(_, a)| owner_hash_of(a).map(|h| h.to_string() == owner_hash).unwrap_or(false)).map(|(n, _)| n.clone())
    }
    pub fn for_address(&self, address: &str) -> Option<String> {
        self.0.iter().find(|(_, a)| a == address).map(|(n, _)| n.clone())
    }
}

/// One contract template: everything except the state in the middle.
struct Template {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    art: silverscript_abi::SilAbiArtifact,
    name: String,
    tags: HashMap<[u8; 4], String>,
}

impl Template {
    fn from(c: &crate::Compiled) -> Self {
        let (prefix, suffix, _) = c.template();
        let ca = c.art.contracts.get(&c.name).unwrap();
        let mut tags = HashMap::new();
        for (entry, e) in &ca.entries {
            tags.insert(*e.dispatch_tag.as_bytes(), entry.clone());
        }
        // Map generated covenant wrappers back to their declared names.
        for (decl, entry) in &ca.cov_decl_to_abi {
            if let Some(e) = ca.entries.get(entry) {
                tags.insert(*e.dispatch_tag.as_bytes(), decl.clone());
            }
        }
        Self { prefix, suffix, art: c.art.clone(), name: c.name.clone(), tags }
    }

    /// If `redeem` is an instance of this template, decode its state.
    fn decode(&self, redeem: &[u8]) -> Option<BTreeMap<String, ArtifactValue>> {
        if redeem.len() < self.prefix.len() + self.suffix.len() || !redeem.starts_with(&self.prefix) || !redeem.ends_with(&self.suffix) {
            return None;
        }
        let state = &redeem[self.prefix.len()..redeem.len() - self.suffix.len()];
        let rs = &self.art.contracts.get(&self.name)?.runtime_state;
        silverscript_abi::decode_runtime_state_script(&self.art, rs, state).ok()
    }

    fn spk(&self, values: &BTreeMap<String, ArtifactValue>) -> Result<ScriptPublicKey> {
        let rs = &self.art.contracts.get(&self.name).unwrap().runtime_state;
        let state = encode_runtime_state_script(&self.art, rs, values).map_err(|e| anyhow!("{e}"))?;
        let mut code = self.prefix.clone();
        code.extend(state);
        code.extend(&self.suffix);
        Ok(pay_to_script_hash_script(&code))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PassState {
    owner: Hash,
    expiry: i64,
    rate: i64,
    life: i64,
    credit: i64,
    paid: i64,
    /// v5: the voucher key this pass trusts (part of the contract state).
    voucher: Option<Hash>,
    /// Not part of the contract state; carried along for display.
    bought_by: Option<String>,
}

impl PassState {
    fn values(&self) -> BTreeMap<String, ArtifactValue> {
        BTreeMap::from([
            ("owner".into(), ArtifactValue::Bytes(self.owner.as_bytes().to_vec())),
            ("expiry".into(), ArtifactValue::Int(self.expiry)),
            ("rate".into(), ArtifactValue::Int(self.rate)),
            ("life_price".into(), ArtifactValue::Int(self.life)),
            ("credit".into(), ArtifactValue::Int(self.credit)),
            ("paid".into(), ArtifactValue::Int(self.paid)),
        ])
        .into_iter()
        .chain(self.voucher.map(|h| ("voucher".to_string(), ArtifactValue::Bytes(h.as_bytes().to_vec()))))
        .collect()
    }
    fn from_values(v: &BTreeMap<String, ArtifactValue>) -> Option<Self> {
        Some(Self {
            owner: hash_of(v.get("owner")?)?,
            expiry: int_of(v.get("expiry")?)?,
            rate: int_of(v.get("rate")?)?,
            life: int_of(v.get("life_price")?)?,
            credit: int_of(v.get("credit")?)?,
            paid: int_of(v.get("paid")?)?,
            voucher: v.get("voucher").and_then(hash_of),
            bought_by: None,
        })
    }
}

fn int_of(v: &ArtifactValue) -> Option<i64> {
    match v {
        ArtifactValue::Int(i) => Some(*i),
        _ => None,
    }
}
fn hash_of(v: &ArtifactValue) -> Option<Hash> {
    match v {
        ArtifactValue::Bytes(b) if b.len() == 32 => Some(Hash::from_slice(b)),
        _ => None,
    }
}

/// Splits a signature script into its pushes (data of each push).
pub fn pushes(script: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < script.len() {
        let op = script[i];
        i += 1;
        let len = match op {
            0x00 => {
                out.push(vec![]);
                continue;
            }
            0x01..=0x4b => op as usize,
            0x4c => {
                let l = *script.get(i)? as usize;
                i += 1;
                l
            }
            0x4d => {
                let l = u16::from_le_bytes([*script.get(i)?, *script.get(i + 1)?]) as usize;
                i += 2;
                l
            }
            0x4e => {
                let l = u32::from_le_bytes(script.get(i..i + 4)?.try_into().ok()?) as usize;
                i += 4;
                l
            }
            0x4f => {
                out.push(vec![0x81]); // -1 as a script number
                continue;
            }
            0x51..=0x60 => {
                out.push(vec![op - 0x50]);
                continue;
            }
            _ => return None,
        };
        out.push(script.get(i..i + len)?.to_vec());
        i += len;
    }
    Some(out)
}

/// Decodes a minimally-encoded script number.
pub fn script_num(b: &[u8]) -> Option<i64> {
    if b.is_empty() {
        return Some(0);
    }
    if b.len() > 8 {
        return None;
    }
    let mut v: i64 = 0;
    for (i, byte) in b.iter().enumerate() {
        v |= (*byte as i64) << (8 * i);
    }
    let last = b[b.len() - 1];
    if last & 0x80 != 0 {
        v &= !(0x80i64 << (8 * (b.len() - 1)));
        v = -v;
    }
    Some(v)
}

pub struct Indexer {
    /// The time of the block being read (ms), for the events it creates.
    block_time: i64,
    pub card: TierCard,
    cov: Hash,
    tier: Tier,
    member: Template,
    minter: Template,
    creator_spk: ScriptPublicKey,
    fee_spk: Option<ScriptPublicKey>,
    prefix: kaspa_addresses::Prefix,
    seen: HashSet<Hash>,
    candidates: HashMap<TransactionOutpoint, PassState>,
    lanes: HashMap<TransactionOutpoint, Lane>,
    events: Vec<Event>,
    pub unmatched: usize,
    pub scanned_txs: usize,
}

impl Indexer {
    pub fn new(card: TierCard) -> Result<Self> {
        let addr: kaspa_addresses::Address =
            card.creator_address.as_str().try_into().map_err(|_| anyhow!("bad creator address in tier card"))?;
        if addr.version != kaspa_addresses::Version::PubKey {
            bail!("creator address must be a regular (P2PK) address");
        }
        let creator = Creator::from_address(&addr);
        let tier = card.settings.clone();
        let card_tier_for_payee = tier.clone();
        let fee_spk = tier.fee.as_ref().map(|f| f.spk()).transpose()?;
        let member = Template::from(&membership(&creator, &tier, Hash::default(), &blank_pass())?);
        let lane = Lane { remaining: -1, ..Lane::default() };
        let minter = Template::from(&minter(&creator, &tier, &lane)?);
        let cov = card.covenant_id.parse().map_err(|_| anyhow!("bad covenant id in tier card"))?;
        let prefix = if card.network.starts_with("testnet") { kaspa_addresses::Prefix::Testnet } else { kaspa_addresses::Prefix::Mainnet };
        Ok(Self {
            block_time: 0,
            card,
            cov,
            tier,
            member,
            minter,
            creator_spk: crate::payee(&creator, &card_tier_for_payee)?.spk,
            fee_spk,
            prefix,
            seen: HashSet::new(),
            candidates: HashMap::new(),
            lanes: HashMap::new(),
            events: Vec::new(),
            unmatched: 0,
            scanned_txs: 0,
        })
        .map(|mut ix: Self| {
            if let Some(k) = ix.card.known_lane.clone() {
                ix.seed_lane(&k);
            }
            ix
        })
    }

    /// A lane's contract state, as the minter template encodes it.
    fn lane_values(&self, l: &Lane) -> BTreeMap<String, ArtifactValue> {
        let mut v = BTreeMap::from([
            ("price".to_string(), ArtifactValue::Int(l.price as i64)),
            ("life_price".to_string(), ArtifactValue::Int(l.life as i64)),
            ("credit".to_string(), ArtifactValue::Int(l.credit as i64)),
            ("remaining".to_string(), ArtifactValue::Int(l.remaining)),
        ]);
        if let Some(t4) = &self.tier.v4 {
            v.insert("term_price".into(), ArtifactValue::Int(l.term_price as i64));
            v.insert("term_length".into(), ArtifactValue::Int(l.term_ms));
            if t4.record_id.is_some() {
                let h = l.voucher.or_else(|| t4.voucher_key_hash.parse().ok()).unwrap_or_default();
                v.insert("voucher".into(), ArtifactValue::Bytes(h.as_bytes().to_vec()));
            }
        }
        v
    }

    /// Tells the indexer about a sales lane it can't learn from the chain yet (no sale since launch).
    pub fn seed_lane(&mut self, k: &KnownLane) {
        if let Ok(txid) = k.txid.parse() {
            self.lanes.insert(TransactionOutpoint::new(txid, k.index), k.lane);
        }
    }

    /// Restores saved progress for this tier. Returns where to continue scanning from.
    pub fn load_cache(&mut self, path: &std::path::Path) -> Option<Hash> {
        let c: ScanCache = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        if c.covenant_id != self.card.covenant_id {
            return None;
        }
        for p in c.passes {
            let op = TransactionOutpoint::new(p.txid.parse().ok()?, p.index);
            let st = PassState { owner: p.owner.parse().ok()?, expiry: p.expiry, rate: p.rate, life: p.life, credit: p.credit, paid: p.paid, voucher: p.voucher.and_then(|v| v.parse().ok()), bought_by: p.bought_by };
            self.candidates.insert(op, st);
        }
        for k in &c.lanes {
            self.seed_lane(k);
        }
        self.events = c.events;
        self.scanned_txs = c.scanned_txs;
        c.cursor.parse().ok()
    }

    pub fn save_cache(&self, path: &std::path::Path, cursor: Hash) -> Result<()> {
        let passes = self
            .candidates
            .iter()
            .map(|(op, s)| CachedPass {
                txid: op.transaction_id.to_string(),
                index: op.index,
                owner: s.owner.to_string(),
                expiry: s.expiry,
                rate: s.rate,
                life: s.life,
                credit: s.credit,
                paid: s.paid,
                bought_by: s.bought_by.clone(),
                voucher: s.voucher.map(|h| h.to_string()),
            })
            .collect();
        let lanes = self.lanes.iter().map(|(op, l)| KnownLane { txid: op.transaction_id.to_string(), index: op.index, lane: *l }).collect();
        let c = ScanCache {
            covenant_id: self.card.covenant_id.clone(),
            cursor: cursor.to_string(),
            scanned_txs: self.scanned_txs,
            passes,
            lanes,
            events: self.events.clone(),
        };
        std::fs::write(path, serde_json::to_string_pretty(&c)?)?;
        Ok(())
    }

    /// Reads one transaction and records any pass it creates.
    /// Like `process`, for a transaction mined in a block with this time (ms).
    pub fn process_at(&mut self, tx: &Transaction, block_time_ms: i64) {
        self.block_time = block_time_ms;
        self.process(tx);
    }

    pub fn process(&mut self, tx: &Transaction) {
        let id = tx.id();
        // A lane that's spent is gone (sales recreate it as a new output below).
        if !self.lanes.is_empty() {
            for inp in &tx.inputs {
                self.lanes.remove(&inp.previous_outpoint);
            }
        }
        let ours: Vec<usize> =
            tx.outputs.iter().enumerate().filter(|(_, o)| o.covenant.map(|c| c.covenant_id) == Some(self.cov)).map(|(i, _)| i).collect();
        if ours.is_empty() {
            self.scanned_txs += 1;
            return;
        }
        // Only our own transactions are remembered, so memory stays small on a busy network.
        if !self.seen.insert(id) {
            return;
        }
        self.scanned_txs += 1;
        for (i, input) in tx.inputs.iter().enumerate() {
            let Some(p) = pushes(&input.signature_script) else { continue };
            if p.len() < 2 {
                continue;
            }
            let redeem = &p[p.len() - 1];
            let Ok(tag): Result<[u8; 4], _> = p[p.len() - 2].as_slice().try_into() else { continue };
            let args = &p[..p.len() - 2];
            let from_sale = self.minter.tags.contains_key(&tag);
            if let Some(entry) = self.minter.tags.get(&tag).cloned() {
                if let Some(prev) = self.minter.decode(redeem) {
                    self.follow_lane(tx, i, &entry, &prev, args, &ours);
                }
            }
            let mut event: Option<(String, i64)> = None; // (kind, amount)
            let v5 = self.tier.v4.as_ref().is_some_and(|v| v.record_id.is_some());
            let mut signed = false;
            let next = if let Some(entry) = self.minter.tags.get(&tag).cloned() {
                let lane = self.minter.decode(redeem);
                let price_of = |k: &str| lane.as_ref().and_then(|l| l.get(k)).and_then(int_of).unwrap_or(0);
                let (ceiling, life_ceiling, term_ceiling) = (price_of("price"), price_of("life_price"), price_of("term_price"));
                let n = lane.and_then(|lane| self.from_minter(&entry, &lane, args));
                if let Some(st) = &n {
                    let kind = match entry.as_str() {
                        "trial" => "trial",
                        "self_trial" => "self_trial",
                        "buy_lifetime" => "lifetime",
                        "buy_term" => "term",
                        _ => "monthly",
                    };
                    // Which uses needed this server's price approval: anything below the lane's own
                    // price for that kind, and every self-serve trial.
                    signed = v5
                        && match kind {
                            "monthly" => st.paid < ceiling,
                            "lifetime" => st.paid < life_ceiling,
                            "term" => st.paid < term_ceiling,
                            "self_trial" => true,
                            _ => false,
                        };
                    event = Some((kind.into(), st.paid));
                }
                n
            } else if let Some(entry) = self.member.tags.get(&tag).cloned() {
                let carried = self.candidates.get(&input.previous_outpoint).and_then(|c| c.bought_by.clone());
                let prev = self.member.decode(redeem).and_then(|v| PassState::from_values(&v));
                let n = prev.clone().and_then(|prev| self.from_pass(&entry, PassState { bought_by: carried, ..prev }, args));
                if let (Some(p), Some(st)) = (&prev, &n) {
                    let amount = match entry.as_str() {
                        "renew" | "autorenew" | "upgrade" => st.paid - p.paid,
                        "transfer" => self.tier.royalty as i64,
                        _ => 0,
                    };
                    signed = v5 && (entry == "autorenew" || (entry == "renew" && amount < p.rate));
                    // Topping up or taking back a prepaid balance isn't a payment to the creator.
                    if !matches!(entry.as_str(), "deposit" | "withdraw") {
                        event = Some((entry.clone(), amount));
                    }
                }
                n
            } else {
                None
            };
            let Some(state) = next else { continue };
            let Ok(spk) = self.member.spk(&state.values()) else { continue };
            let hit = ours.iter().find(|&&j| {
                let o = &tx.outputs[j];
                o.covenant.map(|c| c.authorizing_input as usize) == Some(i) && o.script_public_key == spk
            });
            match hit {
                Some(&j) => {
                    let mut state = state;
                    if from_sale {
                        state.bought_by = self.payer_of(tx, j, state.paid);
                    }
                    if let Some((kind, amount)) = event {
                        let amount = amount.max(0) as u64;
                        let fee = self.tier.fee.as_ref().map(|f| f.of(amount)).unwrap_or(0);
                        let ev = Event {
                            txid: id.to_string(),
                            kind,
                            owner_hash: state.owner.to_string(),
                            amount_sompi: amount,
                            fee_sompi: fee,
                            creator_sompi: amount - fee,
                            voucher: signed,
                            time_ms: self.block_time,
                        };
                        if !self.events.iter().any(|e| e.txid == ev.txid && e.kind == ev.kind && e.owner_hash == ev.owner_hash) {
                            self.events.push(ev);
                        }
                    }
                    self.candidates.insert(TransactionOutpoint::new(id, j as u32), state);
                }
                None => self.unmatched += 1,
            }
        }
    }

    /// A sales lane was spent: work out the lane(s) it became and find them in the outputs.
    fn follow_lane(&mut self, tx: &Transaction, input: usize, entry: &str, prev: &BTreeMap<String, ArtifactValue>, args: &[Vec<u8>], ours: &[usize]) {
        let get = |k: &str| prev.get(k).and_then(int_of);
        let (Some(price), Some(life), Some(credit), Some(remaining)) = (get("price"), get("life_price"), get("credit"), get("remaining")) else {
            return;
        };
        let (tp, tl) = (get("term_price").unwrap_or(0), get("term_length").unwrap_or(0));
        let v5 = self.tier.v4.as_ref().is_some_and(|v| v.record_id.is_some());
        let cur_voucher = if v5 { prev.get("voucher").and_then(hash_of) } else { None };
        let lane = |price: i64, life: i64, credit: i64, remaining: i64, tp: i64, tl: i64| Lane { voucher: cur_voucher,
            price: price as u64,
            life: life as u64,
            credit: credit == 1,
            remaining,
            term_price: tp.max(0) as u64,
            term_ms: tl,
        };
        let after_sale = if remaining > 0 { remaining - 1 } else { remaining };
        let next: Vec<Lane> = match entry {
            "buy" | "buy_lifetime" | "buy_term" | "trial" | "self_trial" => vec![lane(price, life, credit, after_sale, tp, tl)],
            "set_prices" => {
                // v2-v4 args start with (sig, key); v5 with (record input).
                let off = if v5 { 1 } else { 2 };
                let n = |i: usize| args.get(i + off - 2).and_then(|b| script_num(b));
                let v4 = self.tier.v4.is_some();
                match (n(2), n(3), n(4)) {
                    (Some(p), Some(l), Some(c)) if !v4 => vec![lane(p, l, c, remaining, 0, 0)],
                    (Some(p), Some(l), Some(c)) => match (n(5), n(6)) {
                        (Some(t), Some(len)) => vec![lane(p, l, c, remaining, t, len)],
                        _ => vec![],
                    },
                    _ => vec![],
                }
            }
            "set_voucher" => match args.get(1).filter(|b| b.len() == 32) {
                Some(k) => vec![Lane { voucher: Some(Hash::from_slice(k)), ..lane(price, life, credit, remaining, tp, tl) }],
                None => vec![],
            },
            "fork" => match args.get(if v5 { 1 } else { 2 }).and_then(|b| script_num(b)) {
                Some(left) => {
                    let right = if remaining == -1 { -1 } else { remaining - left };
                    vec![lane(price, life, credit, left, tp, tl), lane(price, life, credit, right, tp, tl)]
                }
                None => vec![],
            },
            _ => vec![], // retire
        };
        let id = tx.id();
        for l in next {
            let values = self.lane_values(&l);
            let Ok(spk) = self.minter.spk(&values) else { continue };
            for &j in ours {
                let o = &tx.outputs[j];
                if o.covenant.map(|c| c.authorizing_input as usize) == Some(input) && o.script_public_key == spk {
                    self.lanes.insert(TransactionOutpoint::new(id, j as u32), l);
                }
            }
        }
    }

    /// The tier's live sales lanes, each checked unspent on-chain right now. (A lane the node
    /// can't see yet stays known and shows up on a later check.)
    pub async fn verified_lanes(&self, chain: &Chain) -> Result<Vec<FoundLane>> {
        let mut out = vec![];
        for (op, l) in &self.lanes {
            let spk = self.minter.spk(&self.lane_values(l))?;
            match chain.utxos_at(&spk).await?.into_iter().find(|u| u.op == *op && u.entry.covenant_id == Some(self.cov)) {
                Some(u) => out.push(FoundLane { txid: op.transaction_id.to_string(), index: op.index, value: u.entry.amount, lane: *l }),
                None => {}
            }
        }
        out.sort_by(|a, b| a.txid.cmp(&b.txid).then(a.index.cmp(&b.index)));
        Ok(out)
    }

    /// Sales send their change back to whoever paid: the first plain output after the
    /// pass (and after the creator's payment, if there was one).
    fn payer_of(&self, tx: &Transaction, member_out: usize, charge: i64) -> Option<String> {
        let mut start = member_out + 1;
        if charge > 0 && tx.outputs.get(start).map(|o| o.script_public_key == self.creator_spk).unwrap_or(false) {
            start += 1;
            // ...and the protocol fee right after it, when there is one (a tier whose fee is 0%
            // has no fee output, and its fee address may be the buyer's own wallet: the creator
            // buying their own pass).
            let fee_due = self.tier.fee.as_ref().map(|f| f.of(charge.max(0) as u64)).unwrap_or(0);
            if let (Some(fee), true) = (&self.fee_spk, fee_due > 0) {
                if tx.outputs.get(start).map(|o| &o.script_public_key == fee).unwrap_or(false) {
                    start += 1;
                }
            }
        }
        let o = tx.outputs.iter().skip(start).find(|o| o.covenant.is_none())?;
        extract_script_pub_key_address(&o.script_public_key, self.prefix).ok().map(|a| a.to_string())
    }

    fn from_minter(&self, entry: &str, lane: &BTreeMap<String, ArtifactValue>, args: &[Vec<u8>]) -> Option<PassState> {
        let price = int_of(lane.get("price")?)?;
        let life = int_of(lane.get("life_price")?)?;
        let credit = int_of(lane.get("credit")?)?;
        let h = |b: &Vec<u8>| (b.len() == 32).then(|| Hash::from_slice(b));
        let v5 = self.tier.v4.as_ref().is_some_and(|v| v.record_id.is_some());
        // v5: new passes trust the lane's voucher key.
        let lane_voucher = if v5 { lane.get("voucher").and_then(hash_of) } else { None };
        if self.tier.v4.is_some() {
            // v4 sales: (buyer, [join time], discount, voucher sig, voucher key, prefix, suffix).
            // The discount is checked by the contract; a wrong one simply won't match the output.
            // v5 discounts are in millionths and round in the creator's favour.
            let disc = |amount: i64, d: i64| -> Option<i64> {
                if v5 {
                    (0..=1_000_000).contains(&d).then(|| crate::discounted_ppm(amount.max(0) as u64, d as u64) as i64)
                } else {
                    (0..=10_000).contains(&d).then(|| (amount as i128 * (10_000 - d) as i128 / 10_000) as i64)
                }
            };
            let term_price = lane.get("term_price").and_then(int_of).unwrap_or(0);
            let term_len = lane.get("term_length").and_then(int_of).unwrap_or(0);
            // Sales start with the buyer; a trial's first argument is the creator's proof instead.
            let owner = || h(args.first()?);
            return match entry {
                "buy" => Some(PassState {
                    owner: owner()?,
                    expiry: script_num(args.get(1)?)? + self.tier.period_ms,
                    rate: price,
                    life,
                    credit,
                    paid: disc(price, script_num(args.get(2)?)?)?,
                    voucher: lane_voucher,
                    bought_by: None,
                }),
                "buy_lifetime" => Some(PassState { owner: owner()?, expiry: LIFETIME, rate: price, life, credit, paid: disc(life, script_num(args.get(1)?)?)?, voucher: lane_voucher, bought_by: None }),
                "buy_term" => Some(PassState {
                    owner: owner()?,
                    expiry: script_num(args.get(1)?)? + term_len,
                    rate: price,
                    life,
                    credit,
                    paid: disc(term_price, script_num(args.get(2)?)?)?,
                    voucher: lane_voucher,
                    bought_by: None,
                }),
                // Self-serve trial (minter_v5t): (member, join, voucher sig, voucher key, prefix, suffix).
                "self_trial" => Some(PassState {
                    owner: owner()?,
                    expiry: script_num(args.get(1)?)? + self.tier.trial_ms,
                    rate: price,
                    life,
                    credit,
                    paid: 0,
                    voucher: lane_voucher,
                    bought_by: None,
                }),
                // v4 trial: (sig, key, member, join); v5: (record input, member, join).
                "trial" => {
                    let k = if v5 { 1 } else { 2 };
                    Some(PassState {
                        owner: h(args.get(k)?)?,
                        expiry: script_num(args.get(k + 1)?)? + self.tier.trial_ms,
                        rate: price,
                        life,
                        credit,
                        paid: 0,
                        voucher: lane_voucher,
                        bought_by: None,
                    })
                }
                _ => None,
            };
        }
        match entry {
            "buy" => Some(PassState {
                owner: h(args.first()?)?,
                expiry: script_num(args.get(1)?)? + self.tier.period_ms,
                rate: price,
                life,
                credit,
                paid: price,
                voucher: None,
                bought_by: None,
            }),
            "buy_lifetime" => Some(PassState { owner: h(args.first()?)?, expiry: LIFETIME, rate: price, life, credit, paid: life, voucher: None, bought_by: None }),
            "trial" => Some(PassState {
                owner: h(args.get(2)?)?,
                expiry: script_num(args.get(3)?)? + self.tier.trial_ms,
                rate: price,
                life,
                credit,
                paid: 0,
                voucher: None,
                bought_by: None,
            }),
            _ => None,
        }
    }

    fn from_pass(&self, entry: &str, prev: PassState, args: &[Vec<u8>]) -> Option<PassState> {
        match entry {
            "renew" | "autorenew" => {
                let t = script_num(args.first()?)?;
                let base = prev.expiry.max(t);
                // v4 renewals may carry a holder discount (checked by the contract).
                let bps = if self.tier.v4.is_some() { script_num(args.get(1)?)? } else { 0 };
                let due = if self.tier.v4.as_ref().is_some_and(|v| v.record_id.is_some()) {
                    if !(0..=1_000_000).contains(&bps) {
                        return None;
                    }
                    crate::discounted_ppm(prev.rate.max(0) as u64, bps as u64) as i64
                } else {
                    if !(0..=10_000).contains(&bps) {
                        return None;
                    }
                    (prev.rate as i128 * (10_000 - bps) as i128 / 10_000) as i64
                };
                Some(PassState { expiry: base + self.tier.period_ms, paid: prev.paid + due, ..prev })
            }
            // Prepaid balance changes; the pass terms stay exactly the same.
            "deposit" | "withdraw" => Some(prev),
            "upgrade" => {
                let mut cost = prev.life;
                if prev.credit == 1 {
                    cost -= prev.paid;
                }
                let cost = cost.max(0);
                Some(PassState { expiry: LIFETIME, paid: prev.paid + cost, ..prev })
            }
            "transfer" => {
                let to = args.get(2)?;
                (to.len() == 32).then(|| PassState { owner: Hash::from_slice(to), ..prev })
            }
            // v5: (record input, new voucher key).
            "rekey" => {
                let k = args.get(1)?;
                (k.len() == 32).then(|| PassState { voucher: Some(Hash::from_slice(k)), ..prev })
            }
            _ => None,
        }
    }

    /// Keeps only candidates whose exact coin is unspent on-chain right now.
    pub async fn verified(&mut self, chain: &Chain, now: i64) -> Result<Vec<FoundPass>> {
        let mut by_spk: HashMap<ScriptPublicKey, Vec<TransactionOutpoint>> = HashMap::new();
        for (op, st) in &self.candidates {
            by_spk.entry(self.member.spk(&st.values())?).or_default().push(*op);
        }
        let mut live: HashMap<TransactionOutpoint, u64> = HashMap::new();
        for (spk, ops) in &by_spk {
            let utxos = chain.utxos_at(spk).await?;
            for op in ops {
                if let Some(u) = utxos.iter().find(|u| u.op == *op && u.entry.covenant_id == Some(self.cov)) {
                    live.insert(*op, u.entry.amount);
                }
            }
        }
        // Spent or never-accepted candidates are dropped for good.
        self.candidates.retain(|op, _| live.contains_key(op));
        let bond = self.tier.bond;
        let v4 = self.tier.v4.is_some();
        let mut out: Vec<FoundPass> = self
            .candidates
            .iter()
            .map(|(op, s)| FoundPass {
                txid: op.transaction_id.to_string(),
                index: op.index,
                owner_hash: s.owner.to_string(),
                kind: if s.expiry >= LIFETIME { "lifetime" } else if s.paid == 0 { "trial" } else { "monthly" }.into(),
                active: s.expiry > now,
                expiry_ms: s.expiry,
                lifetime: s.expiry >= LIFETIME,
                rate_sompi: s.rate as u64,
                lifetime_price_sompi: s.life as u64,
                credit: s.credit == 1,
                paid_sompi: s.paid as u64,
                value_sompi: live.get(op).copied().unwrap_or(bond),
                balance_sompi: if v4 { live.get(op).copied().unwrap_or(bond).saturating_sub(bond) } else { 0 },
                bought_by: s.bought_by.clone(),
                owner_address: s.bought_by.clone().filter(|a| owner_hash_of(a).map(|h| h == s.owner).unwrap_or(false)),
                voucher: s.voucher.map(|h| h.to_string()),
            })
            .collect();
        out.sort_by(|a, b| a.txid.cmp(&b.txid).then(a.index.cmp(&b.index)));
        Ok(out)
    }

    /// Which of `passes` another node also sees, as (txid, index): the same unspent coin, with
    /// this tier's covenant id, at the address its state implies. Asks only; changes nothing.
    pub async fn confirmed_on(&self, chain: &Chain, passes: &[FoundPass]) -> Result<std::collections::HashSet<(String, u32)>> {
        let want: std::collections::HashSet<(String, u32)> = passes.iter().map(|p| (p.txid.clone(), p.index)).collect();
        let mut by_spk: HashMap<ScriptPublicKey, Vec<TransactionOutpoint>> = HashMap::new();
        for (op, st) in &self.candidates {
            if want.contains(&(op.transaction_id.to_string(), op.index)) {
                by_spk.entry(self.member.spk(&st.values())?).or_default().push(*op);
            }
        }
        let mut ok = std::collections::HashSet::new();
        for (spk, ops) in &by_spk {
            let utxos = chain.utxos_at(spk).await?;
            for op in ops {
                if utxos.iter().any(|u| u.op == *op && u.entry.covenant_id == Some(self.cov)) {
                    ok.insert((op.transaction_id.to_string(), op.index));
                }
            }
        }
        Ok(ok)
    }

    /// Every payment and trial seen so far, oldest first.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn tier(&self) -> &Tier {
        &self.tier
    }
}

/// Owner hash for an address (P2PK addresses carry the public key).
pub fn owner_hash_of(address: &str) -> Result<Hash> {
    let addr: kaspa_addresses::Address = address.try_into().map_err(|_| anyhow!("not a valid Kaspa address"))?;
    if addr.version != kaspa_addresses::Version::PubKey {
        bail!("only regular (P2PK) addresses can hold passes");
    }
    Ok(crate::blake(&addr.payload))
}

/// Reads one page of blocks. Returns true once it has reached the tip.
#[allow(dead_code)]
pub async fn scan_step(ix: &mut Indexer, chain: &Chain, cursor: &mut Option<Hash>) -> Result<bool> {
    let (txs, next, at_tip) = chain.blocks_page(*cursor).await?;
    for (tx, t) in &txs {
        ix.process_at(tx, *t);
    }
    let advanced = next != *cursor;
    *cursor = next;
    Ok(at_tip || !advanced)
}

/// Walks the chain from the card's start block, feeding every transaction in.
pub async fn scan(ix: &mut Indexer, chain: &Chain, cursor: &mut Option<Hash>) -> Result<usize> {
    let mut pages = 0;
    loop {
        let (txs, next, at_tip) = chain.blocks_page(*cursor).await?;
        for (tx, t) in &txs {
            ix.process_at(tx, *t);
        }
        pages += 1;
        let advanced = next != *cursor;
        *cursor = next;
        if at_tip || !advanced {
            return Ok(pages);
        }
    }
}

pub fn print_passes(ix: &Indexer, passes: &[FoundPass], now: i64, names: &Names) {
    let t = ix.tier();
    println!(
        "Tier {}\n  period {} min, trials {}, resellable {}, royalty {}, bond {}",
        ix.card.covenant_id,
        t.period_ms / crate::MINUTE,
        if t.trial_ms > 0 { format!("{} min", t.trial_ms / crate::MINUTE) } else { "off".into() },
        if t.resellable { "yes" } else { "no" },
        kas(t.royalty),
        kas(t.bond)
    );
    println!("  scanned {} transactions, found {} live pass(es)\n", ix.scanned_txs, passes.len());
    let short = |a: &str| {
        let body = a.split(':').nth(1).unwrap_or(a);
        format!("{}…{}", &body[..6.min(body.len())], &body[body.len().saturating_sub(4)..])
    };
    for p in passes {
        let who = names.for_owner(&p.owner_hash).unwrap_or_else(|| format!("{}…", &p.owner_hash[..12]));
        let exp = if p.lifetime { "never expires".to_string() } else { format!("expires {}", when(p.expiry_ms, now)) };
        let payer = match &p.bought_by {
            Some(a) => names.for_address(a).unwrap_or_else(|| short(a)),
            None => "?".into(),
        };
        println!(
            "  {:14} {:8} {:26} {:7}  paid {:>13}  bought by {:14} {}:{}",
            who,
            p.kind,
            exp,
            if p.active { "ACTIVE" } else { "EXPIRED" },
            kas(p.paid_sompi),
            payer,
            &p.txid[..12],
            p.index
        );
    }
}
