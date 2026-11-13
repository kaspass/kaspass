//! KasPass: creator memberships on Kaspa, built on the v5 covenants (contracts/*_v5*.sil,
//! owner_record.sil, vault.sil). Runs on mainnet and Testnet-10. Not audited by a professional
//! firm; see SECURITY.md for what members and creators have to trust.
//!
//! Every transaction is run through Kaspa's own script engine on your machine before it is
//! sent; if any input would fail, nothing is broadcast.
//!
//! Commands (run with --help on any of them for options):
//!   serve      the website, creator dashboard and member check API
//!   scan       find and verify every pass in a membership (from a tier card)
//!   check      does this address hold an active pass?
//!   name       show a friendly name for an address in scans
//!   keys       create or show this folder's test wallets
//!   simulate   run every contract and the website's flows offline, on a simulated chain

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use blake2b_simd::Params as Blake2b;
use clap::{Parser, Subcommand};
use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::config::params::{MAINNET_PARAMS, TESTNET_PARAMS};
use kaspa_consensus_core::hashing::sighash::{SigHashReusedValuesUnsync, calc_schnorr_signature_hash};
use kaspa_consensus_core::hashing::sighash_type::SIG_HASH_ALL;
use kaspa_consensus_core::mass::{ComputeBudget, MassCalculator};
use kaspa_consensus_core::tx::{
    CovenantBinding, GenesisCovenantGroup, PopulatedTransaction, ScriptPublicKey, Transaction, TransactionInput,
    TransactionOutpoint, TransactionOutput, UtxoEntry,
};
use kaspa_txscript::caches::Cache;
use kaspa_txscript::covenants::CovenantsContext;
use kaspa_txscript::script_builder::ScriptBuilder;
use kaspa_txscript::{EngineCtx, EngineFlags, TxScriptEngine, pay_to_address_script, pay_to_script_hash_script};
use secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use silverscript_abi::{ArtifactValue, SilAbiArtifact, encode_contract_covenant_decl_sig_script, encode_contract_entry_sig_script};
use silverscript_lang::compiler::{CompileOptions, compile_to_sil_abi_artifact_with_options};

mod api;
mod auth;
mod pskt;
mod prices;
mod shop;
mod indexer;
mod telegram;
mod discord;
mod clubs;
mod holdings;
mod bridge;
mod names;
mod v5sim;

const SOMPI: u64 = 100_000_000;
const MINUTE: i64 = 60_000;
const LIFETIME: i64 = 4_000_000_000_000_000;
const TX_VERSION: u16 = 1;
const MIN_CHANGE: u64 = SOMPI / 2; // smaller change is folded into the fee (storage-mass rules)
const MIN_FEERATE: f64 = 100.0; // sompi per gram, the post-Toccata floor

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(about = "KasPass: creator memberships on Kaspa (mainnet and Testnet-10; not audited)")]
struct Cli {
    /// Folder for keys/ and state.json
    #[arg(long, default_value = ".")]
    home: PathBuf,
    /// Connect to this wRPC node instead of the public resolver (e.g. ws://127.0.0.1:17210)
    #[arg(long)]
    node: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create or show this folder's test wallets
    Keys,
    /// Find and verify every pass in a tier (from a tier card)
    Scan {
        #[arg(long, default_value = "tier-card.json")]
        card: PathBuf,
    },
    /// Does this address hold an active pass?
    Check {
        address: String,
        #[arg(long, default_value = "tier-card.json")]
        card: PathBuf,
    },
    /// Run the indexer as a small web API for apps
    Serve {
        #[arg(long, default_value = "tier-card.json")]
        card: PathBuf,
        #[arg(long, default_value_t = 8787)]
        port: u16,
        /// KasPass fee address for tiers launched from the dashboard (default: this tier's)
        #[arg(long)]
        fee_address: Option<String>,
        /// A partner site the membership page may send people back to after they pay, e.g.
        /// `--allow-return https://example.com` (https only, repeatable). Pages link here with
        /// `?return=https://example.com/some/page`.
        #[arg(long = "allow-return")]
        allow_return: Vec<String>,
        /// Content-Security-Policy: `on` (default), `report` (browsers only warn), or `off`.
        #[arg(long, default_value = "on")]
        csp: String,
        /// Only the site's creator wallet may launch new levels from the dashboard.
        #[arg(long)]
        creator_only_launch: bool,
        /// The network for a new site with no membership yet: mainnet or testnet-10. (With a
        /// membership, the network always comes from its tier card.)
        #[arg(long)]
        network: Option<String>,
        /// The creator wallet of a new site with no membership yet. Only this wallet can sign in
        /// to the dashboard and launch the first one.
        #[arg(long)]
        creator: Option<String>,
        /// Memberships launched from this site's dashboard carry no KasPass fee.
        #[arg(long)]
        no_fee: bool,
        /// A second node to confirm every pass with (default: another public node). Use
        /// `--cross-check off` to trust the main node alone.
        #[arg(long = "cross-check")]
        cross_check: Option<String>,
    },
    /// Run every contract and the website's flows offline, on a simulated chain
    Simulate,
    /// Show a friendly name for an address in scans, e.g. `name kaspatest:... my-kaspire`
    Name {
        address: String,
        #[arg(num_args = 1.., required = true)]
        label: Vec<String>,
    },
}

/// Permanent settings, fixed at launch.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Tier {
    period_ms: i64,
    trial_ms: i64,
    resellable: bool,
    royalty: u64,
    bond: u64,
    max_ahead_ms: i64,
    /// KasPass protocol fee. Present on v3 tiers; v2 tiers (no fee) leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fee: Option<Fee>,
    /// v4 settings: prepaid balance with auto-renew, commitment terms, holder discounts.
    /// Present on v4 tiers only (v4 always has the protocol fee too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v4: Option<V4>,
}

/// Settings only v4 tiers have, fixed at launch and enforced by the contracts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct V4 {
    /// AUTORENEW is only allowed this close to the end of a period.
    auto_window_ms: i64,
    /// blake2b of the voucher key that may approve holder discounts (hex). All zeros = none.
    voucher_key_hash: String,
    /// The largest discount a voucher may give, in basis points (2000 = 20%).
    max_discount_bps: u64,
    /// v5: the creator's owner record (covenant id, hex). Payments then go to the payout
    /// vault it controls, and creator actions need the record instead of one fixed key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_id: Option<String>,
    /// v5: members can start the free trial themselves (minter_v5t.sil), with their first
    /// period prepaid into their own pass and charged by the keeper when the trial ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    self_trial: bool,
}

impl V4 {
    fn voucher_hash(&self) -> Result<Hash> {
        self.voucher_key_hash.parse().map_err(|_| anyhow!("bad voucher key hash in the tier settings"))
    }
}

/// A holder discount approved by the voucher key (the KasPass server's), for one transaction.
#[derive(Clone)]
struct Voucher {
    key: Wallet,
    bps: u64,
}

/// What's due after a discount, with the same arithmetic as the tier's contracts: v5 discounts
/// are in millionths and round in the creator's favour; older ones are in basis points.
fn discount_due(tier: &Tier, amount: u64, d: u64) -> u64 {
    if is_v5_tier(tier) { discounted_ppm(amount, d) } else { discounted(amount, d) }
}

fn is_v5_tier(t: &Tier) -> bool {
    t.v4.as_ref().is_some_and(|v| v.record_id.is_some())
}

/// v5 contracts' discounted(): the cut is split so it can't overflow, and rounds down.
fn discounted_ppm(amount: u64, ppm: u64) -> u64 {
    if ppm == 0 {
        return amount;
    }
    let d = ppm.min(1_000_000);
    let cut = (amount / 1_000_000) * d + ((amount % 1_000_000) * d) / 1_000_000;
    amount - cut
}

/// Same arithmetic as the v4 contracts' discounted().
fn discounted(amount: u64, bps: u64) -> u64 {
    if bps == 0 { amount } else { (amount as u128 * (10_000 - bps.min(10_000)) as u128 / 10_000) as u64 }
}

/// The voucher part of a v4 entry's arguments. With no discount, blank values (never checked).
fn voucher_args(v: Option<&Voucher>, sig: Vec<u8>) -> [ArtifactValue; 3] {
    match v {
        // A voucher signs whenever it's given (v5 auto-renewals need it even at no discount).
        Some(v) => [ArtifactValue::Int(v.bps as i64), ArtifactValue::Bytes(if sig.is_empty() { vec![0u8; 65] } else { sig }), ArtifactValue::Bytes(v.key.pk.clone())],
        _ => [ArtifactValue::Int(0), ArtifactValue::Bytes(vec![0u8; 65]), ArtifactValue::Bytes(vec![0u8; 32])],
    }
}

/// The KasPass protocol fee, fixed for a tier at launch and enforced by its contracts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Fee {
    /// Where fees go.
    address: String,
    /// Basis points of each payment: 200 = 2%.
    bps: u64,
    /// Fees smaller than this (sompi) are skipped, since tiny outputs are costly on Kaspa.
    min: u64,
}

impl Fee {
    fn spk(&self) -> Result<ScriptPublicKey> {
        let a: Address = self.address.as_str().try_into().map_err(|_| anyhow!("bad fee address {}", self.address))?;
        Ok(pay_to_address_script(&a))
    }
    /// Same arithmetic as the contracts' fee_of().
    fn of(&self, amount: u64) -> u64 {
        let f = amount * self.bps / 10_000;
        if f < self.min { 0 } else { f }
    }
}

/// Default protocol fee for new tiers: 2%. Fees under 0.5 KAS (`launch --fee-min`) are
/// skipped, so payments under 25 KAS pay none.
const DEFAULT_FEE_BPS: u64 = 200;

/// Outputs for paying `amount` to the creator: the creator's share, then the protocol fee.
/// This order is required by the contracts.
fn payment_outs(creator: &Creator, tier: &Tier, amount: u64) -> Result<Vec<TransactionOutput>> {
    if amount == 0 {
        return Ok(vec![]);
    }
    let fee = tier.fee.as_ref().map(|f| f.of(amount)).unwrap_or(0);
    let to = payee(creator, tier)?;
    let mut v = vec![TransactionOutput { value: amount - fee, script_public_key: to.spk, covenant: None }];
    if fee > 0 {
        v.push(TransactionOutput { value: fee, script_public_key: tier.fee.as_ref().unwrap().spk()?, covenant: None });
    }
    Ok(v)
}

/// A sales lane's changeable state.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
struct Lane {
    price: u64,
    life: u64,
    credit: bool,
    remaining: i64,
    /// v4: price of one commitment term (0 = not offered) and how long it runs.
    #[serde(default)]
    term_price: u64,
    #[serde(default)]
    term_ms: i64,
    /// v5: the voucher key this lane trusts (None = the one the tier launched with).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    voucher: Option<Hash>,
}

/// What a pass carries, locked in when bought.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Pass {
    owner: String,
    expiry: i64,
    rate: u64,
    life: u64,
    credit: bool,
    paid: u64,
    /// v5: the voucher key this pass trusts (None = the one the tier launched with).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    voucher: Option<Hash>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Live {
    txid: String,
    index: u32,
    value: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Member {
    at: Live,
    pass: Pass,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct State {
    version: Option<u32>,
    tier: Option<Tier>,
    covenant_id: Option<String>,
    minter: Option<Live>,
    lane: Option<Lane>,
    membership: Option<Member>,
    #[serde(default)]
    start_hash: Option<String>,
    #[serde(default)]
    launch_txid: Option<String>,
}

impl State {
    fn load(home: &Path) -> Result<Self> {
        let p = home.join("state.json");
        if !p.exists() {
            return Ok(Self::default());
        }
        let st: State = serde_json::from_str(&std::fs::read_to_string(&p)?).unwrap_or_default();
        if st.version != Some(2) && st.covenant_id.is_some() {
            bail!("state.json is from the v1 tool. Rename it (e.g. state-v1.json) and run `launch` to start a v2 tier.");
        }
        Ok(st)
    }
    fn save(&self, home: &Path) -> Result<()> {
        let mut s = self.clone();
        s.version = Some(2);
        std::fs::write(home.join("state.json"), serde_json::to_string_pretty(&s)?)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wallets
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Wallet {
    name: String,
    kp: Keypair,
    pk: Vec<u8>,
    id: Hash,
    addr: Address,
    spk: ScriptPublicKey,
}

fn blake(data: &[u8]) -> Hash {
    Hash::from_slice(Blake2b::new().hash_length(32).to_state().update(data).finalize().as_bytes())
}

fn wallet_from_secret(name: &str, sk: SecretKey) -> Wallet {
    let kp = Keypair::from_secret_key(&Secp256k1::new(), &sk);
    let pk = kp.x_only_public_key().0.serialize().to_vec();
    let addr = Address::new(net_prefix(), Version::PubKey, &pk);
    let spk = pay_to_address_script(&addr);
    // A voucher key's signatures never leave the server inside a request for a wallet.
    if name.starts_with("voucher") {
        crate::pskt::add_server_key(&pk);
    }
    Wallet { name: name.into(), id: blake(&pk), kp, pk, addr, spk }
}

fn load_wallet(home: &Path, name: &str) -> Result<Wallet> {
    let dir = home.join("keys");
    std::fs::create_dir_all(&dir)?;
    // Only this account may open the keys folder.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| anyhow!("couldn't make {} private: {e}", dir.display()))?;
    }
    let p = dir.join(format!("{name}.key"));
    let sk = if p.exists() {
        let hex = std::fs::read_to_string(&p)?;
        let mut b = [0u8; 32];
        faster_hex::hex_decode(hex.trim().as_bytes(), &mut b).map_err(|_| anyhow!("bad key file {}", p.display()))?;
        SecretKey::from_slice(&b)?
    } else {
        let (sk, _) = Secp256k1::new().generate_keypair(&mut secp256k1::rand::thread_rng());
        // Created readable by this account only, from the first byte.
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        use std::io::Write;
        o.open(&p)?.write_all(faster_hex::hex_string(&sk.secret_bytes()).as_bytes())?;
        sk
    };
    Ok(wallet_from_secret(name, sk))
}

// ---------------------------------------------------------------------------
// Contracts
// ---------------------------------------------------------------------------

/// What the contracts need to know about a creator: where to pay, and whose key signs.
#[derive(Clone)]
struct Creator {
    spk: ScriptPublicKey,
    id: Hash,
}

impl Creator {
    fn from_address(addr: &Address) -> Self {
        Creator { spk: pay_to_address_script(addr), id: blake(&addr.payload) }
    }
}

impl Wallet {
    fn c(&self) -> Creator {
        Creator { spk: self.spk.clone(), id: self.id }
    }
}

fn creator_spk_bytes(creator: &Creator) -> Vec<u8> {
    // Introspected scriptPubKey bytes = version (u16) + script.
    let mut v = creator.spk.version().to_be_bytes().to_vec();
    v.extend_from_slice(creator.spk.script());
    v
}

#[derive(Clone)]
struct Compiled {
    art: SilAbiArtifact,
    name: String,
    bytecode: Vec<u8>,
}

impl Compiled {
    fn new(source: &str, args: Vec<ArtifactValue>) -> Result<Self> {
        let art = compile_to_sil_abi_artifact_with_options(source, &args, CompileOptions::default())
            .map_err(|e| anyhow!("compile failed: {e}"))?;
        let (name, c) = art.contracts.iter().next().ok_or_else(|| anyhow!("no contract"))?;
        let name = name.clone();
        let bytecode = c.compiled.bytecode.clone();
        Ok(Self { name, bytecode, art })
    }
    fn spk(&self) -> ScriptPublicKey {
        pay_to_script_hash_script(&self.bytecode)
    }
    fn template(&self) -> (Vec<u8>, Vec<u8>, Hash) {
        let c = self.art.contracts.values().next().unwrap();
        let s = c.compiled.state_span;
        let pre = self.bytecode[..s.offset].to_vec();
        let suf = self.bytecode[s.offset + s.len..].to_vec();
        (pre, suf, Hash::from_bytes(c.compiled.template_hash))
    }
    fn with_redeem(&self, mut sig: Vec<u8>) -> Vec<u8> {
        sig.extend(ScriptBuilder::with_flags(EngineFlags::default()).add_data(&self.bytecode).unwrap().drain());
        sig
    }
    fn entry_sigscript(&self, entry: &str, args: &[ArtifactValue]) -> Result<Vec<u8>> {
        let s = encode_contract_entry_sig_script(&self.art, &self.name, entry, args).map_err(|e| anyhow!("{e}"))?;
        Ok(self.with_redeem(s))
    }
    fn decl_sigscript(&self, func: &str, args: &[ArtifactValue]) -> Result<Vec<u8>> {
        let s = encode_contract_covenant_decl_sig_script(&self.art, &self.name, func, false, args).map_err(|e| anyhow!("{e}"))?;
        Ok(self.with_redeem(s))
    }
}

const MEMBERSHIP_V5_SRC: &str = include_str!("../contracts/membership_v5.sil");
const MINTER_V5_SRC: &str = include_str!("../contracts/minter_v5.sil");
/// Minter v5 plus SELF_TRIAL, for levels launched with self-serve trials.
const MINTER_V5T_SRC: &str = include_str!("../contracts/minter_v5t.sil");
const OWNER_RECORD_SRC: &str = include_str!("../contracts/owner_record.sil");
const VAULT_SRC: &str = include_str!("../contracts/vault.sil");
const PAYIN_SRC: &str = include_str!("../contracts/payin.sil");

/// blake2b of a scriptPubKey as contracts see it (version, big-endian, then the script).
fn spk_hash(spk: &ScriptPublicKey) -> Hash {
    let mut v = spk.version().to_be_bytes().to_vec();
    v.extend_from_slice(spk.script());
    blake(&v)
}

/// A creator's owner record, in a given state (owner and recovery wallets).
fn owner_record(owner: Hash, recovery: Hash) -> Result<Compiled> {
    Compiled::new(OWNER_RECORD_SRC, vec![hb(owner), hb(recovery)])
}

/// One order's pay-in address (paying from any wallet): the coins can only complete a purchase
/// or renewal on `level` that leaves a pass owned by `buyer`, with the rest back to `refund`,
/// or go back to `refund` after `refund_after`.
#[allow(clippy::too_many_arguments)]
fn payin(buyer: Hash, level: Hash, template: Hash, refund: &ScriptPublicKey, max_spend: u64, refund_after: i64, max_fee: u64) -> Result<Compiled> {
    let mut spk = refund.version().to_be_bytes().to_vec();
    spk.extend_from_slice(refund.script());
    Compiled::new(
        PAYIN_SRC,
        vec![hb(buyer), hb(level), hb(template), ArtifactValue::Bytes(spk), ArtifactValue::Int(max_spend as i64), ArtifactValue::Int(refund_after), ArtifactValue::Int(max_fee as i64)],
    )
}

/// A creator's payout vault: answers only to their owner record.
fn vault(record_id: Hash) -> Result<Compiled> {
    Compiled::new(VAULT_SRC, vec![hb(record_id)])
}

impl V4 {
    /// The owner record of a v5 tier (None on v4).
    fn record(&self) -> Result<Option<Hash>> {
        self.record_id.as_deref().map(|r| r.parse().map_err(|_| anyhow!("bad owner record id in tier card"))).transpose()
    }
}

/// Who a tier pays: the creator's wallet (v2 to v4), or their payout vault (v5).
fn payee(creator: &Creator, t: &Tier) -> Result<Creator> {
    match t.v4.as_ref().map(|v| v.record()).transpose()?.flatten() {
        Some(rid) => Ok(Creator { spk: vault(rid)?.spk(), id: creator.id }),
        None => Ok(Creator { spk: creator.spk.clone(), id: creator.id }),
    }
}

fn hb(h: Hash) -> ArtifactValue {
    ArtifactValue::Bytes(h.as_bytes().to_vec())
}

fn b(x: bool) -> i64 {
    if x { 1 } else { 0 }
}

/// Constructor arguments that describe the fee (v3 contracts only).
fn fee_args(f: &Fee) -> Result<Vec<ArtifactValue>> {
    let spk = f.spk()?;
    let mut bytes = spk.version().to_be_bytes().to_vec();
    bytes.extend_from_slice(spk.script());
    Ok(vec![ArtifactValue::Bytes(bytes), ArtifactValue::Int(f.bps as i64), ArtifactValue::Int(f.min as i64)])
}

fn membership(creator: &Creator, t: &Tier, owner: Hash, p: &Pass) -> Result<Compiled> {
    let creator = &payee(creator, t)?;
    let mut args = vec![
        ArtifactValue::Bytes(creator_spk_bytes(creator)),
        ArtifactValue::Int(t.period_ms),
        ArtifactValue::Int(b(t.resellable)),
        ArtifactValue::Int(t.royalty as i64),
        ArtifactValue::Int(t.max_ahead_ms),
    ];
    // v5 only: the owner record (for REKEY) and the cap in millionths; the voucher key is pass state.
    let (f, v, rid) = v5_parts(t)?;
    args.extend(fee_args(f)?);
    args.extend([ArtifactValue::Int(t.bond as i64), ArtifactValue::Int(v.auto_window_ms), hb(rid), ArtifactValue::Int(v.max_discount_bps as i64 * 100)]);
    args.extend([
            hb(owner),
            ArtifactValue::Int(p.expiry),
            ArtifactValue::Int(p.rate as i64),
            ArtifactValue::Int(p.life as i64),
            ArtifactValue::Int(b(p.credit)),
            ArtifactValue::Int(p.paid as i64),
    ]);
    args.push(hb(match p.voucher { Some(h) => h, None => v.voucher_hash()? }));
    Compiled::new(MEMBERSHIP_V5_SRC, args)
}

/// A v5 tier's fee, v5 settings and owner record. Older contract versions (v2, v3 and v4) are
/// retired: KasPass builds and reads v5 contracts only.
fn v5_parts(t: &Tier) -> Result<(&Fee, &V4, Hash)> {
    match (&t.fee, &t.v4) {
        (Some(f), Some(v)) => match v.record()? {
            Some(rid) => Ok((f, v, rid)),
            None => bail!("{RETIRED}"),
        },
        _ => bail!("{RETIRED}"),
    }
}

/// Why an old membership can't be used any more.
const RETIRED: &str = "this membership uses a retired contract version (v2, v3 or v4); KasPass now runs v5 memberships only";

fn blank_pass() -> Pass {
    Pass { voucher: None, owner: String::new(), expiry: 0, rate: 0, life: 0, credit: false, paid: 0 }
}

fn minter(creator: &Creator, t: &Tier, l: &Lane) -> Result<Compiled> {
    // The template hash does not depend on state, so any pass values work here.
    let (_, _, tmpl) = membership(creator, t, Hash::default(), &blank_pass())?.template();
    let (f, v, record) = v5_parts(t)?;
    let payout = payee(creator, t)?;
    let mut args = vec![
        // v5 lanes trust the creator's owner record.
        hb(record),
        ArtifactValue::Bytes(creator_spk_bytes(&payout)),
        ArtifactValue::Int(t.period_ms),
        ArtifactValue::Int(t.trial_ms),
        ArtifactValue::Int(t.bond as i64),
        hb(tmpl),
    ];
    args.extend(fee_args(f)?);
    // The cap in millionths; the voucher key is lane state (below).
    args.push(ArtifactValue::Int(v.max_discount_bps as i64 * 100));
    args.extend([
        ArtifactValue::Int(l.price as i64),
        ArtifactValue::Int(l.life as i64),
        ArtifactValue::Int(b(l.credit)),
        ArtifactValue::Int(l.remaining),
        ArtifactValue::Int(l.term_price as i64),
        ArtifactValue::Int(l.term_ms),
        hb(match l.voucher { Some(h) => h, None => v.voucher_hash()? }),
    ]);
    let self_trial = t.v4.as_ref().is_some_and(|v| v.self_trial);
    Compiled::new(if self_trial { MINTER_V5T_SRC } else { MINTER_V5_SRC }, args)
}

// ---------------------------------------------------------------------------
// Chain: live node or offline simulation
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Utxo {
    op: TransactionOutpoint,
    entry: UtxoEntry,
}

struct Sim {
    utxos: HashMap<TransactionOutpoint, UtxoEntry>,
    now: i64,
    blocks: Vec<Transaction>,
    /// When each simulated transaction was mined (the sim clock at submit).
    block_times: Vec<i64>,
    /// When set, the simulated clock runs with real time from this moment (for serving pages).
    clock: Option<std::time::Instant>,
}

enum Chain {
    Live(kaspa_wrpc_client::KaspaRpcClient),
    Sim(Sim),
}

/// The network the live node connection is on ("testnet-10" until `serve` says otherwise).
static NETWORK: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn network() -> &'static str {
    NETWORK.get().map(String::as_str).unwrap_or("testnet-10")
}

fn is_mainnet() -> bool {
    network() == "mainnet"
}

/// Text from outside (a group title, a display name) made safe to print in the server log: control
/// characters, including terminal escape codes, are dropped.
/// The environment settings that also apply on mainnet (all the others are test switches).
const SETTINGS: &[&str] = &["KASPASS_HOME_HOSTS", "KASPASS_WATCHDOG_S", "KASPASS_TRIAL_DAILY_MAX"];

pub fn log_safe(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// A test-only switch from the environment. Ignored on mainnet, so a stray variable on a live
/// server can't turn on a simulator route or switch off a protection.
pub fn test_switch(name: &str) -> Option<String> {
    if is_mainnet() {
        if std::env::var(name).is_ok() {
            eprintln!("warning: {name} is set but ignored on mainnet");
        }
        return None;
    }
    std::env::var(name).ok()
}

fn net_prefix() -> Prefix {
    if is_mainnet() { Prefix::Mainnet } else { Prefix::Testnet }
}

/// "mainnet" or "testnet-10"; nothing else is supported.
fn check_network(n: &str) -> Result<()> {
    anyhow::ensure!(n == "mainnet" || n == "testnet-10", "unknown network \"{n}\" (use mainnet or testnet-10)");
    Ok(())
}

impl Chain {
    async fn connect(node: Option<String>) -> Result<Self> {
        Self::connect_to(node, "testnet-10").await
    }

    /// Connects to a node on `network` ("mainnet" or "testnet-10"). The first connection fixes
    /// the network for the whole program: addresses and fees follow it.
    async fn connect_to(node: Option<String>, network_name: &str) -> Result<Self> {
        use kaspa_wrpc_client::prelude::*;
        check_network(network_name)?;
        let _ = NETWORK.set(network_name.to_string());
        anyhow::ensure!(network() == network_name, "this program is already on {}", network());
        let net = if network_name == "mainnet" { NetworkId::new(NetworkType::Mainnet) } else { NetworkId::with_suffix(NetworkType::Testnet, 10) };
        let label = if network_name == "mainnet" { "Mainnet" } else { "Testnet-10" };
        let client = if let Some(url) = &node {
            KaspaRpcClient::new(WrpcEncoding::Borsh, Some(url), None, Some(net), None)?
        } else {
            KaspaRpcClient::new(WrpcEncoding::Borsh, None, Some(Resolver::default()), Some(net), None)?
        };
        let opts = ConnectOptions {
            block_async_connect: true,
            strategy: ConnectStrategy::Fallback,
            connect_timeout: Some(std::time::Duration::from_secs(20)),
            ..Default::default()
        };
        client.connect(Some(opts)).await.with_context(|| format!("could not reach a {label} node"))?;
        let info = client.get_server_info().await?;
        if !info.has_utxo_index {
            bail!("connected node has no UTXO index; try another with --node");
        }
        println!("Connected to {} ({label}, synced: {})", client.url().unwrap_or_default(), info.is_synced);
        Ok(Chain::Live(client))
    }

    async fn utxos_at(&self, spk: &ScriptPublicKey) -> Result<Vec<Utxo>> {
        match self {
            Chain::Live(c) => {
                use kaspa_rpc_core::api::rpc::RpcApi;
                let addr = kaspa_txscript::extract_script_pub_key_address(spk, net_prefix())?;
                let list = c.get_utxos_by_addresses(vec![addr]).await?;
                Ok(list
                    .into_iter()
                    .map(|e| Utxo {
                        op: TransactionOutpoint::new(e.outpoint.transaction_id, e.outpoint.index),
                        entry: UtxoEntry::new(
                            e.utxo_entry.amount,
                            e.utxo_entry.script_public_key.clone(),
                            e.utxo_entry.block_daa_score,
                            e.utxo_entry.is_coinbase,
                            e.utxo_entry.covenant_id,
                        ),
                    })
                    .collect())
            }
            Chain::Sim(s) => Ok(s
                .utxos
                .iter()
                .filter(|(_, e)| &e.script_public_key == spk)
                .map(|(op, e)| Utxo { op: *op, entry: e.clone() })
                .collect()),
        }
    }

    /// The tier's sales lane. Prefers the exact coin we last saw; if that one is gone but a lane coin
    /// with the same terms and this tier's covenant ID sits at the same address (someone else bought
    /// from an unlimited lane, or state.json is behind), it follows that one instead.
    async fn find_lane(&self, spk: &ScriptPublicKey, live: &Live, cov: Hash) -> Result<Utxo> {
        let txid: Hash = live.txid.parse().map_err(|_| anyhow!("bad txid in state.json"))?;
        let want = TransactionOutpoint::new(txid, live.index);
        let here = self.utxos_at(spk).await?;
        if let Some(u) = here.iter().find(|u| u.op == want) {
            return Ok(u.clone());
        }
        let genuine: Vec<&Utxo> = here.iter().filter(|u| u.entry.covenant_id == Some(cov)).collect();
        if let Some(u) = genuine.first() {
            println!("  note: the sales lane moved to {}:{} (a sale happened that this tool didn't make); following it.", u.op.transaction_id, u.op.index);
            return Ok((*u).clone());
        }
        bail!(
            "can't see your tier's sales lane ({}:{}) on this node.\n  \
             Usually the public node is behind or missing data: wait a minute and run the command again (it may connect to a different node).\n  \
             If it keeps happening, run `membership-cli scan` and send me the output.",
            live.txid,
            live.index
        )
    }

    async fn find(&self, spk: &ScriptPublicKey, live: &Live) -> Result<Utxo> {
        let txid: Hash = live.txid.parse().map_err(|_| anyhow!("bad txid in state.json"))?;
        let want = TransactionOutpoint::new(txid, live.index);
        self.utxos_at(spk).await?.into_iter().find(|u| u.op == want).ok_or_else(|| {
            anyhow!("covenant output {}:{} not found on chain yet (wait a few seconds and retry)", live.txid, live.index)
        })
    }

    async fn now_ms(&self) -> Result<i64> {
        match self {
            Chain::Live(c) => {
                use kaspa_rpc_core::api::rpc::RpcApi;
                // Lock times must sit below the network's past median time.
                Ok(c.get_block_dag_info().await?.past_median_time as i64 - 1_000)
            }
            Chain::Sim(s) => Ok(s.now + s.clock.map(|c| c.elapsed().as_millis() as i64).unwrap_or(0)),
        }
    }

    async fn feerate(&self) -> Result<f64> {
        match self {
            Chain::Live(c) => {
                use kaspa_rpc_core::api::rpc::RpcApi;
                let est = c.get_fee_estimate().await?;
                // Capped: a node reporting a wild rate (or a short spike) can't make the keeper wallet
                // or a buyer pay a huge fee. At the cap a transaction still gets in, just later.
                Ok(est.normal_buckets.first().map(|b| b.feerate).unwrap_or(MIN_FEERATE).clamp(MIN_FEERATE, MIN_FEERATE * 20.0))
            }
            Chain::Sim(_) => Ok(MIN_FEERATE),
        }
    }

    /// A block at the current tip; anything submitted after this is in its future.
    async fn sink(&self) -> Result<Hash> {
        match self {
            Chain::Live(c) => {
                use kaspa_rpc_core::api::rpc::RpcApi;
                Ok(c.get_sink().await?.sink)
            }
            Chain::Sim(s) => Ok(sim_cursor(s.blocks.len())),
        }
    }

    /// One page of blocks from `cursor`: (transactions, next cursor, reached the tip).
    /// One page of blocks from `cursor`: (transactions with their block's time in ms, next cursor, reached the tip).
    async fn blocks_page(&self, cursor: Option<Hash>) -> Result<(Vec<(Transaction, i64)>, Option<Hash>, bool)> {
        match self {
            Chain::Live(c) => {
                use kaspa_rpc_core::api::rpc::RpcApi;
                let low = cursor.ok_or_else(|| anyhow!("no start block in the tier card"))?;
                let sink = c.get_sink().await?.sink;
                let resp = c.get_blocks(Some(low), true, true).await?;
                let at_tip = resp.block_hashes.contains(&sink);
                let mut best = (0u64, low);
                let mut txs = Vec::new();
                for blk in resp.blocks {
                    if blk.header.blue_score >= best.0 {
                        best = (blk.header.blue_score, blk.header.hash);
                    }
                    let t = blk.header.timestamp as i64;
                    for rtx in blk.transactions {
                        if let Ok(tx) = Transaction::try_from(rtx) {
                            if !tx.is_coinbase() {
                                txs.push((tx, t));
                            }
                        }
                    }
                }
                Ok((txs, Some(best.1), at_tip))
            }
            Chain::Sim(s) => {
                let from = cursor.map(sim_index).unwrap_or(0).min(s.blocks.len());
                let txs = s.blocks[from..].iter().cloned().zip(s.block_times[from..].iter().copied()).collect();
                Ok((txs, Some(sim_cursor(s.blocks.len())), true))
            }
        }
    }

    async fn submit(&mut self, tx: &Transaction, entries: &[UtxoEntry]) -> Result<Hash> {
        match self {
            Chain::Live(c) => {
                use kaspa_rpc_core::api::rpc::RpcApi;
                let id = c.submit_transaction(tx.into(), false).await?;
                Ok(id)
            }
            Chain::Sim(s) => {
                // Simulator only: KASPASS_BASELINE=<file> appends every transaction (with the coins it
                // spends) for the kaspa-redteam baseline format.
                if let Ok(path) = std::env::var("KASPASS_BASELINE") {
                    use std::io::Write;
                    let line = serde_json::json!({ "pastMedianTime": s.now, "tx": serde_json::to_value(tx)?, "utxos": serde_json::to_value(entries)? });
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                        let _ = writeln!(f, "{line}");
                    }
                }
                // Consensus-side checks the script engine does not do.
                let ins: u64 = entries.iter().map(|e| e.amount).sum();
                let outs: u64 = tx.outputs.iter().map(|o| o.value).sum();
                if outs > ins {
                    bail!("sim: outputs exceed inputs");
                }
                for (i, o) in tx.outputs.iter().enumerate() {
                    if let Some(b) = o.covenant {
                        let auth = entries.get(b.authorizing_input as usize).ok_or_else(|| anyhow!("sim: bad authorizing input"))?;
                        if auth.covenant_id == Some(b.covenant_id) {
                            continue; // continuation of an existing family
                        }
                        let group: Vec<(u32, &TransactionOutput)> = tx
                            .outputs
                            .iter()
                            .enumerate()
                            .filter(|(_, x)| x.covenant == Some(b))
                            .map(|(j, x)| (j as u32, x))
                            .collect();
                        let genesis = kaspa_consensus_core::hashing::covenant_id::covenant_id(
                            tx.inputs[b.authorizing_input as usize].previous_outpoint,
                            group.into_iter(),
                        );
                        if genesis != b.covenant_id {
                            bail!("sim: output {i} claims a covenant id it is not entitled to");
                        }
                    }
                }
                for inp in &tx.inputs {
                    s.utxos.remove(&inp.previous_outpoint).ok_or_else(|| anyhow!("sim: input already spent"))?;
                }
                for (i, o) in tx.outputs.iter().enumerate() {
                    s.utxos.insert(
                        TransactionOutpoint::new(tx.id(), i as u32),
                        UtxoEntry::new(o.value, o.script_public_key.clone(), 0, false, o.covenant.map(|b| b.covenant_id)),
                    );
                }
                s.blocks.push(tx.clone());
                s.block_times.push(s.now);
                Ok(tx.id())
            }
        }
    }
}

fn sim_cursor(i: usize) -> Hash {
    let mut b = [0u8; 32];
    b[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Hash::from_bytes(b)
}

fn sim_index(h: Hash) -> usize {
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap()) as usize
}

// ---------------------------------------------------------------------------
// Transaction builder
// ---------------------------------------------------------------------------

/// How to fill an input's signature script once the transaction is final.
enum Unlock {
    /// Plain wallet coin: push the wallet's signature.
    Key(Wallet),
    /// Covenant input. Gets this input's signature if `signer` is set (empty otherwise).
    Script { signer: Option<Wallet>, build: Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>> + Send + Sync> },
    /// Plain coin owned by someone else (a website buyer). Left unsigned; their wallet signs it later.
    External,
    /// Covenant input whose script needs an outside wallet's signature inside it (e.g. the owner
    /// closing a pass from the website). Built with a blank 65-byte slot the wallet fills in.
    ExternalScript { build: Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>> + Send + Sync>, budget: u16 },
}

impl Unlock {
    fn is_external(&self) -> bool {
        matches!(self, Unlock::External | Unlock::ExternalScript { .. })
    }
}

/// Who funds a transaction and receives the change.
#[derive(Clone)]
enum Payer {
    /// A wallet this tool holds the key for.
    Key(Wallet),
    /// Any address, such as a website buyer's. Their wallet signs afterwards.
    External { name: String, spk: ScriptPublicKey },
    /// One order's pay-in coin (paid from any wallet with a QR code). Nobody signs it: the
    /// pay-in contract checks the pass and the change itself.
    PayIn(PayInCoin),
}

#[derive(Clone)]
struct PayInCoin {
    pin: Compiled,
    coin: Utxo,
    /// The buyer's own address: where the change goes.
    refund: ScriptPublicKey,
}

/// Network fee allowance built into every pay-in order (what COMPLETE may use on top of the
/// purchase itself). It comes back as change when the fee is smaller.
const PAYIN_SLACK: u64 = SOMPI;
/// The most a pay-in REFUND may leave as network fee.
const PAYIN_REFUND_FEE: u64 = SOMPI / 10;

/// Puts a pay-in coin into a sale or pass transaction: the coin as the last input, completing
/// with the new pass at `pass_out_k` (the `k`-th output the covenant input 0 authorises), and
/// its change as the last output (the contract's "outputs mirror inputs" rule). `uses` is what
/// the transaction takes from it before the network fee. Other payers pass through unchanged.
#[allow(clippy::type_complexity)]
fn with_payin(tier: &Tier, mut spends: Vec<Spend>, mut outs: Vec<TransactionOutput>, payer: Payer, k: i64, pass: &Pass, member: &Compiled, uses: u64) -> Result<(Vec<Spend>, Vec<TransactionOutput>, Option<Payer>, Option<usize>)> {
    let pc = match payer {
        Payer::PayIn(pc) => pc,
        other => return Ok((spends, outs, Some(other), None)),
    };
    let v = match (pass.voucher, tier.v4.as_ref()) {
        (Some(h), _) => h,
        (None, Some(v4)) => v4.voucher_hash()?,
        (None, None) => bail!("pay-in orders are for v5 plans"),
    };
    if pc.coin.entry.amount < uses + MIN_CHANGE {
        bail!("the payment ({}) doesn't cover this ({} plus the network fee)", kas(pc.coin.entry.amount), kas(uses));
    }
    let (pre, suf, _) = member.template();
    let (expiry, rate, life, credit, paid) = (pass.expiry, pass.rate as i64, pass.life as i64, b(pass.credit), pass.paid as i64);
    let pin = pc.pin.clone();
    let unlock = Unlock::Script {
        signer: None,
        build: Box::new(move |_| {
            pin.entry_sigscript(
                "complete",
                &[ArtifactValue::Int(0), ArtifactValue::Int(k), ArtifactValue::Int(expiry), ArtifactValue::Int(rate), ArtifactValue::Int(life), ArtifactValue::Int(credit), ArtifactValue::Int(paid), hb(v), ArtifactValue::Bytes(pre.clone()), ArtifactValue::Bytes(suf.clone())],
            )
        }),
    };
    let change = pc.coin.entry.amount - uses;
    spends.push(Spend { utxo: pc.coin, unlock });
    outs.push(TransactionOutput { value: change, script_public_key: pc.refund, covenant: None });
    let last = outs.len() - 1;
    Ok((spends, outs, None, Some(last)))
}

/// REFUND a pay-in coin to its buyer (after the order's refund time). Nobody signs.
async fn payin_refund_tx(chain: &Chain, pc: PayInCoin, lock_time: u64) -> Result<(Transaction, Vec<UtxoEntry>, u64)> {
    let pin = pc.pin.clone();
    let unlock = Unlock::Script { signer: None, build: Box::new(move |_| pin.entry_sigscript("refund", &[])) };
    let outs = vec![TransactionOutput { value: pc.coin.entry.amount, script_public_key: pc.refund, covenant: None }];
    build_tx(chain, vec![Spend { utxo: pc.coin, unlock }], outs, None, lock_time, None, Some(0)).await
}

impl Payer {
    fn spk(&self) -> &ScriptPublicKey {
        match self {
            Payer::Key(w) => &w.spk,
            Payer::External { spk, .. } => spk,
            Payer::PayIn(pc) => &pc.refund,
        }
    }
    fn name(&self) -> &str {
        match self {
            Payer::Key(w) => &w.name,
            Payer::External { name, .. } => name,
            Payer::PayIn(_) => "the order's pay-in address",
        }
    }
    fn unlock(&self) -> Unlock {
        match self {
            Payer::Key(w) => Unlock::Key(w.clone()),
            // Never reached: `with_payin` turns a pay-in payer into an input of its own.
            Payer::External { .. } | Payer::PayIn(_) => Unlock::External,
        }
    }
}

struct Spend {
    utxo: Utxo,
    unlock: Unlock,
}

/// Compute used by a plain-wallet input we can't sign: measured with a stand-in key
/// whose script and signature have exactly the same shape and cost.
fn stand_in_units(tx: &Transaction, entries: &[UtxoEntry], idx: usize) -> Result<u64> {
    let w = wallet_from_secret("stand-in", SecretKey::from_slice(&[0x5a; 32])?);
    let mut e = entries.to_vec();
    e[idx] = UtxoEntry::new(e[idx].amount, w.spk.clone(), e[idx].block_daa_score, false, None);
    let mut t = tx.clone();
    t.inputs[idx].signature_script = ScriptBuilder::with_flags(EngineFlags::default()).add_data(&sign(&t, &e, idx, &w))?.drain();
    t.finalize();
    engine_run(&t, &e, idx, None)
}

/// Every input, under its committed compute limit, exactly as a node would check it.
fn verify_all(tx: &Transaction, entries: &[UtxoEntry]) -> Result<()> {
    for i in 0..tx.inputs.len() {
        let limit: u64 = tx.inputs[i].compute_commit.allowed_script_units().into();
        engine_run(tx, entries, i, Some(limit))?;
    }
    Ok(())
}

fn engine_run(tx: &Transaction, entries: &[UtxoEntry], idx: usize, limit: Option<u64>) -> Result<u64> {
    let reused = SigHashReusedValuesUnsync::new();
    let cache = Cache::new(1_000);
    let pop = PopulatedTransaction::new(tx, entries.to_vec());
    let cov = CovenantsContext::from_tx(&pop).map_err(|e| anyhow!("covenant context: {e:?}"))?;
    let input = &tx.inputs[idx];
    let ctx = EngineCtx::new(&cache).with_reused(&reused).with_covenants_ctx(&cov);
    let mut vm = match limit {
        Some(l) => TxScriptEngine::from_transaction_input_with_script_units_limit(
            &pop,
            input,
            idx,
            &entries[idx],
            ctx,
            EngineFlags::default(),
            l.into(),
        ),
        None => TxScriptEngine::from_transaction_input(&pop, input, idx, &entries[idx], ctx, EngineFlags::default()),
    };
    vm.execute().map_err(|e| anyhow!("input {idx} rejected by script engine: {e}"))?;
    Ok(vm.used_script_units().into())
}

fn sign(tx: &Transaction, entries: &[UtxoEntry], idx: usize, w: &Wallet) -> Vec<u8> {
    let reused = SigHashReusedValuesUnsync::new();
    let pop = PopulatedTransaction::new(tx, entries.to_vec());
    let h = calc_schnorr_signature_hash(&pop, idx, SIG_HASH_ALL, &reused);
    let sig = w.kp.sign_schnorr(Message::from_digest_slice(h.as_bytes().as_slice()).unwrap());
    let mut v = sig.as_ref().to_vec();
    v.push(SIG_HASH_ALL.to_u8());
    v
}

fn fill_sigscripts(tx: &mut Transaction, entries: &[UtxoEntry], spends: &[Spend], placeholder: bool) -> Result<()> {
    let mut scripts = Vec::with_capacity(spends.len());
    for (i, s) in spends.iter().enumerate() {
        let sig_for = |w: &Wallet| if placeholder { vec![0u8; 65] } else { sign(tx, entries, i, w) };
        let script = match &s.unlock {
            Unlock::Key(w) => ScriptBuilder::with_flags(EngineFlags::default()).add_data(&sig_for(w))?.drain(),
            Unlock::Script { signer, build } => build(signer.as_ref().map(sig_for).unwrap_or_default())?,
            // Same size as the real signature push, so fees and mass come out right.
            Unlock::External => ScriptBuilder::with_flags(EngineFlags::default()).add_data(&[0u8; 65])?.drain(),
            Unlock::ExternalScript { build, .. } => build(vec![0u8; 65])?,
        };
        scripts.push(script);
    }
    for (i, s) in scripts.into_iter().enumerate() {
        tx.inputs[i].signature_script = s;
    }
    tx.finalize();
    Ok(())
}

/// Builds, funds, signs, budgets and verifies a transaction.
/// `pre_outputs` come first (their order is part of the covenant rules);
/// change to `payer` is appended last.
async fn build_tx(
    chain: &Chain,
    spends: Vec<Spend>,
    pre_outputs: Vec<TransactionOutput>,
    payer: Option<&Wallet>,
    lock_time: u64,
    genesis: Option<GenesisCovenantGroup>,
    fee_from_output: Option<usize>,
) -> Result<(Transaction, Vec<UtxoEntry>, u64)> {
    build_tx_with(chain, spends, pre_outputs, payer.map(|w| Payer::Key(w.clone())), lock_time, genesis, fee_from_output).await
}

/// Like `build_tx`, but the payer can be an outside address. Its inputs are left unsigned
/// (placeholders of the right size); everything else is signed, budgeted and verified.
async fn build_tx_with(
    chain: &Chain,
    spends: Vec<Spend>,
    pre_outputs: Vec<TransactionOutput>,
    payer: Option<Payer>,
    lock_time: u64,
    genesis: Option<GenesisCovenantGroup>,
    fee_from_output: Option<usize>,
) -> Result<(Transaction, Vec<UtxoEntry>, u64)> {
    build_tx_inner(chain, spends, pre_outputs, payer, lock_time, genesis, fee_from_output, false).await
}

/// `small_coin`: pay from the payer's smallest coin that covers the fee (key swaps: a big coin
/// next to many small contract coins makes the storage mass far larger).
#[allow(clippy::too_many_arguments)]
async fn build_tx_inner(
    chain: &Chain,
    mut spends: Vec<Spend>,
    pre_outputs: Vec<TransactionOutput>,
    payer: Option<Payer>,
    lock_time: u64,
    genesis: Option<GenesisCovenantGroup>,
    fee_from_output: Option<usize>,
    small_coin: bool,
) -> Result<(Transaction, Vec<UtxoEntry>, u64)> {
    let feerate = chain.feerate().await?;
    let calc = MassCalculator::new_with_consensus_params(if is_mainnet() { &MAINNET_PARAMS } else { &TESTNET_PARAMS });
    let wallet_coins = match &payer {
        Some(p) => {
            let mut v: Vec<Utxo> = chain.utxos_at(p.spk()).await?.into_iter().filter(|u| u.entry.covenant_id.is_none()).collect();
            v.sort_by_key(|u| std::cmp::Reverse(u.entry.amount));
            if small_coin {
                if let Some(i) = v.iter().rposition(|u| u.entry.amount >= MIN_CHANGE + SOMPI / 5) {
                    let c = v.remove(i);
                    v.insert(0, c);
                }
            }
            v
        }
        None => vec![],
    };
    let out_total: u64 = pre_outputs.iter().map(|o| o.value).sum();
    let fixed_in: u64 = spends.iter().map(|s| s.utxo.entry.amount).sum();
    let n_fixed = spends.len();

    let mut fee: u64 = 0;
    let mut budgets: Option<Vec<u16>> = None;
    for _round in 0..8 {
        // Fund.
        spends.truncate(n_fixed);
        let fee_in = if fee_from_output.is_some() { 0 } else { fee };
        let mut in_total = fixed_in;
        for c in &wallet_coins {
            if in_total >= out_total + fee_in + MIN_CHANGE {
                break;
            }
            in_total += c.entry.amount;
            spends.push(Spend { utxo: c.clone(), unlock: payer.as_ref().unwrap().unlock() });
        }
        if in_total < out_total + fee_in {
            let who = payer.as_ref().map(|p| p.name()).unwrap_or("(no payer)");
            // A member's own wallet: say it the way they'd say it, before the wallet ever opens.
            if matches!(payer, Some(Payer::External { .. })) {
                bail!(
                    "You need about {:.2} KAS and this wallet has {:.2} KAS. {}",
                    (out_total + fee) as f64 / SOMPI as f64,
                    in_total as f64 / SOMPI as f64,
                    if is_mainnet() { "Add KAS to your wallet, then try again." } else { "Get free test KAS from the faucet, then try again." }
                );
            }
            bail!(
                "{who} needs {} but only has {}. {}",
                kas(out_total + fee),
                kas(in_total),
                if is_mainnet() { "Add KAS to it and try again." } else { "Get test KAS from the faucet for that address." }
            );
        }
        let mut outputs = pre_outputs.clone();
        if let Some(k) = fee_from_output {
            outputs[k].value = outputs[k].value.checked_sub(fee).filter(|v| *v > 0).ok_or_else(|| anyhow!("fee is larger than the output paying it"))?;
        }
        let change = in_total - out_total - fee_in;
        if change >= MIN_CHANGE {
            outputs.push(TransactionOutput { value: change, script_public_key: payer.as_ref().unwrap().spk().clone(), covenant: None });
        }

        let inputs: Vec<TransactionInput> = spends
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let default = match &s.unlock {
                    Unlock::Key(_) | Unlock::External => 10,
                    Unlock::Script { .. } => 200,
                    Unlock::ExternalScript { budget, .. } => *budget,
                };
                let b = budgets.as_ref().and_then(|v| v.get(i).copied()).unwrap_or(default);
                TransactionInput::new_with_compute_budget(s.utxo.op, vec![], 0, b)
            })
            .collect();
        let mut tx = Transaction::new_non_finalized(TX_VERSION, inputs, outputs, lock_time, Default::default(), 0, vec![]);
        if let Some(g) = &genesis {
            tx.populate_genesis_covenants(std::slice::from_ref(g)).map_err(|e| anyhow!("{e:?}"))?;
        }
        let entries: Vec<UtxoEntry> = spends.iter().map(|s| s.utxo.entry.clone()).collect();

        // Mass with placeholder signatures (same size as real ones).
        fill_sigscripts(&mut tx, &entries, &spends, true)?;
        let nc = calc.calc_non_contextual_masses(&tx);
        let storage = calc
            .calc_contextual_masses(&PopulatedTransaction::new(&tx, entries.clone()))
            .ok_or_else(|| anyhow!("storage mass too high (an output is too small)"))?
            .storage_mass;
        let mass = nc.compute_mass.max(nc.transient_mass).max(storage);
        let needed = (mass as f64 * feerate * 1.1).ceil() as u64 + 1_000;

        // Sign for real, then measure each input's actual compute use.
        fill_sigscripts(&mut tx, &entries, &spends, false)?;
        let mut measured = Vec::with_capacity(spends.len());
        for i in 0..spends.len() {
            if let Unlock::ExternalScript { budget, .. } = &spends[i].unlock {
                measured.push(*budget); // checked for real once signed (see verify_all)
                continue;
            }
            let used = match spends[i].unlock {
                Unlock::External => stand_in_units(&tx, &entries, i)?,
                _ => engine_run(&tx, &entries, i, None)?,
            };
            let b = ComputeBudget::checked_covering_script_units(used.into())
                .ok_or_else(|| anyhow!("input {i} needs more compute than one input can commit"))?;
            measured.push(b.0.saturating_add(1)); // one unit of headroom
        }

        let actual_fee = in_total - tx.outputs.iter().map(|o| o.value).sum::<u64>();
        if budgets.as_ref() == Some(&measured) && actual_fee >= needed {
            tx.set_storage_mass(storage);
            // Final check under the exact committed limits, just like a node.
            for i in 0..spends.len() {
                if spends[i].unlock.is_external() {
                    continue; // checked again once the outside wallet has signed (see verify_all)
                }
                let limit: u64 = tx.inputs[i].compute_commit.allowed_script_units().into();
                engine_run(&tx, &entries, i, Some(limit))?;
            }
            return Ok((tx, entries, actual_fee));
        }
        budgets = Some(measured);
        fee = fee.max(needed);
    }
    bail!("could not settle fee and budgets")
}

/// One coin moving to a new voucher key: a sales lane (`set_voucher`) or a pass (`rekey`).
struct RekeyCoin {
    utxo: Utxo,
    cur: Compiled,
    next: Compiled,
    cov: Hash,
    lane: bool,
}

/// The most coins one key-swap transaction tries; fewer when they don't fit.
const REKEY_MAX: usize = 8;
/// Mass a key-swap transaction may use (a node's standard limit is 100,000).
const REKEY_MASS: u64 = 90_000;

/// v5 key swap: moves lanes and passes to trust voucher key `new`, through the owner record
/// (proven by the owner's coin, which also pays the fee). Takes as many as fit in one
/// transaction, from the front; returns how many it took.
async fn rekey_tx_external(chain: &Chain, owner: &Address, rec: &RecordInfo, coins: &[RekeyCoin], new: Hash) -> Result<(Transaction, Vec<UtxoEntry>, u64, usize)> {
    let record = find_record(chain, rec).await?;
    let mut n = coins.len().min(REKEY_MAX);
    anyhow::ensure!(n > 0, "nothing to move to the new key");
    while n > 0 {
        let mut spends = Vec::new();
        let mut outs = Vec::new();
        for (i, c) in coins[..n].iter().enumerate() {
            let cur = c.cur.clone();
            let rin = n as i64;
            let lane = c.lane;
            let build: Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>> + Send + Sync> = Box::new(move |_| {
                let args = [ArtifactValue::Int(rin), hb(new)];
                if lane { cur.entry_sigscript("set_voucher", &args) } else { cur.decl_sigscript("rekey", &args) }
            });
            spends.push(Spend { utxo: c.utxo.clone(), unlock: Unlock::Script { signer: None, build } });
            outs.push(cov_out(&c.next, c.utxo.entry.amount, i as u16, c.cov));
        }
        let (rspend, rout) = record_spend_v5(record.clone(), rec, None, n + 1, n as u16)?;
        spends.push(rspend);
        outs.push(rout);
        let (tx, entries, fee) = build_tx_inner(chain, spends, outs, Some(external(owner)), 0, None, None, true).await?;
        if tx_mass(&tx, &entries) <= REKEY_MASS {
            return Ok((tx, entries, fee, n));
        }
        n -= 1;
    }
    bail!("nothing to move to the new key")
}

/// The owner's smallest plain coin that can pay a key-swap fee, if any is small enough to keep
/// batches large (see `build_tx_inner`); else None, and the owner first sets one aside.
async fn small_coin(chain: &Chain, owner: &Address) -> Result<Option<u64>> {
    let spk = pay_to_address_script(owner);
    Ok(chain.utxos_at(&spk).await?.into_iter().filter(|u| u.entry.covenant_id.is_none()).map(|u| u.entry.amount).filter(|&a| a >= MIN_CHANGE + SOMPI / 5 && a <= REKEY_COIN * 3).min())
}

/// Sets aside REKEY_COIN in the owner's own wallet (a payment to themselves).
async fn rekey_coin_tx(chain: &Chain, owner: &Address) -> Result<(Transaction, Vec<UtxoEntry>, u64)> {
    let outs = vec![TransactionOutput { value: REKEY_COIN, script_public_key: pay_to_address_script(owner), covenant: None }];
    build_tx_with(chain, vec![], outs, Some(external(owner)), 0, None, None).await
}

/// The coin the owner sets aside to pay key-swap fees (it stays theirs).
const REKEY_COIN: u64 = 2 * SOMPI;

/// A transaction's mass the way a node counts it (the standard limit is 100,000).
fn tx_mass(tx: &Transaction, entries: &[UtxoEntry]) -> u64 {
    let calc = MassCalculator::new_with_consensus_params(if is_mainnet() { &MAINNET_PARAMS } else { &TESTNET_PARAMS });
    let nc = calc.calc_non_contextual_masses(tx);
    let storage = calc.calc_contextual_masses(&PopulatedTransaction::new(tx, entries.to_vec())).map(|m| m.storage_mass).unwrap_or(u64::MAX);
    nc.compute_mass.max(nc.transient_mass).max(storage)
}

fn cov_out(c: &Compiled, value: u64, auth: u16, cov: Hash) -> TransactionOutput {
    TransactionOutput { value, script_public_key: c.spk(), covenant: Some(CovenantBinding::new(auth, cov)) }
}

fn pay_out(w: &Wallet, value: u64) -> TransactionOutput {
    TransactionOutput { value, script_public_key: w.spk.clone(), covenant: None }
}

fn kas(v: u64) -> String {
    format!("{:.4} KAS", v as f64 / SOMPI as f64)
}

fn when(ms: i64, now: i64) -> String {
    let d = (ms - now) / 1000;
    if d >= 0 { format!("in {}m {}s", d / 60, d % 60) } else { format!("{}m {}s ago", -d / 60, -d % 60) }
}
// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

struct Ctx {
    home: PathBuf,
    creator: Wallet,
    alice: Wallet,
    bob: Wallet,
    /// Stand-in for the KasPass fee address on testnet.
    fee: Wallet,
}

impl Ctx {
    fn who(&self, name: &str) -> Result<&Wallet> {
        match name {
            "creator" => Ok(&self.creator),
            "alice" => Ok(&self.alice),
            "bob" => Ok(&self.bob),
            _ => bail!("unknown wallet {name}"),
        }
    }
}

fn cov_id(st: &State) -> Result<Hash> {
    st.covenant_id.as_ref().ok_or_else(|| anyhow!("run launch first"))?.parse().map_err(|_| anyhow!("bad covenant id"))
}

fn tier_and_lane(st: &State) -> Result<(Tier, Lane, Live, Hash)> {
    Ok((
        st.tier.clone().ok_or_else(|| anyhow!("run launch first"))?,
        st.lane.ok_or_else(|| anyhow!("run launch first"))?,
        st.minter.clone().ok_or_else(|| anyhow!("run launch first"))?,
        cov_id(st)?,
    ))
}

fn pass_kind(p: &Pass) -> &'static str {
    if p.expiry >= LIFETIME {
        "lifetime"
    } else if p.paid == 0 {
        "trial"
    } else {
        "monthly"
    }
}

fn upgrade_cost(p: &Pass) -> u64 {
    let credit = if p.credit { p.paid } else { 0 };
    p.life.saturating_sub(credit)
}

async fn op_launch(cx: &Ctx, chain: &mut Chain, st: &mut State, tier: Tier, lane: Lane) -> Result<()> {
    if st.minter.is_some() {
        bail!("a tier is already launched (rename state.json to start a new one)");
    }
    let m = minter(&cx.creator.c(), &tier, &lane)?;
    let start = chain.sink().await?;
    let lane_value = SOMPI; // 1 KAS locked in the sales lane
    let outs = vec![TransactionOutput { value: lane_value, script_public_key: m.spk(), covenant: None }];
    let (tx, entries, fee) =
        build_tx(chain, vec![], outs, Some(&cx.creator), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let cov = tx.outputs[0].covenant.unwrap().covenant_id;
    let id = chain.submit(&tx, &entries).await?;
    st.tier = Some(tier);
    st.lane = Some(lane);
    st.covenant_id = Some(cov.to_string());
    st.minter = Some(Live { txid: id.to_string(), index: 0, value: lane_value });
    st.membership = None;
    st.start_hash = Some(start.to_string());
    st.launch_txid = Some(id.to_string());
    st.save(&cx.home)?;
    write_card(cx, st)?;
    println!("Launched tier. Covenant ID (your tier's brand):\n  {cov}\n  tx {id}  fee {}", kas(fee));
    println!("  Tier card written to tier-card.json (safe to share; no keys inside).");
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sale {
    Monthly,
    Lifetime,
    Trial,
    /// v4: a commitment term, paid once.
    Term,
}

/// v4 extras for a sale: prepaid balance put into the new pass (for auto-renew), and a
/// holder discount approved by the voucher key.
#[derive(Clone, Default)]
struct SaleOpts {
    prepaid: u64,
    voucher: Option<Voucher>,
    /// How many renewals `prepaid` was sized for (for the buyer's summary).
    periods: u64,
    /// The cushion added to each prepaid renewal in case KAS falls, in percent.
    cushion_pct: u64,
}

/// "Yearly", "Monthly", ... for a period, as buyers read it.
fn period_word(ms: i64) -> &'static str {
    let d = ms as f64 / 86_400_000.0;
    if (360.0..=366.0).contains(&d) { "Yearly" } else if (28.0..=31.0).contains(&d) { "Monthly" } else if d == 7.0 { "Weekly" } else if d == 1.0 { "Daily" } else { "Monthly" }
}

async fn op_sale(cx: &Ctx, chain: &mut Chain, st: &mut State, kind: Sale, force: bool) -> Result<()> {
    op_sale_to(cx, chain, st, kind, force, None).await
}

/// `gift`: issue the pass to any address. The tool doesn't hold that key, so it doesn't track the pass afterwards.
async fn op_sale_to(cx: &Ctx, chain: &mut Chain, st: &mut State, kind: Sale, force: bool, gift: Option<&str>) -> Result<()> {
    let tier = st.tier.clone().ok_or_else(|| anyhow!("run launch first"))?;
    let (to_id, to_name) = match gift {
        Some(a) => {
            let addr: Address = a.trim().try_into().map_err(|_| anyhow!("--for: not a valid Kaspa address"))?;
            if addr.prefix != Prefix::Testnet {
                bail!("--for: this tool is testnet only; use a kaspatest: address");
            }
            (indexer::owner_hash_of(a.trim())?, a.trim().to_string())
        }
        None => (cx.alice.id, cx.alice.name.clone()),
    };
    if gift.is_none() && st.membership.is_some() {
        bail!("this tool tracks one pass at a time; close the current one first");
    }
    // Buyers pay for their own pass; the creator funds the bond on a trial.
    let (payer, signer) = if kind == Sale::Trial {
        (Payer::Key(cx.creator.clone()), Some(cx.creator.clone()))
    } else {
        (Payer::Key(cx.alice.clone()), None)
    };
    let b = sale_tx(&cx.creator.c(), chain, st, kind, force, to_id, &to_name, payer, signer).await?;
    let id = chain.submit(&b.tx, &b.entries).await?;
    st.minter = Some(Live { txid: id.to_string(), index: 0, value: b.tx.outputs[0].value });
    st.lane = Some(b.next_lane);
    if gift.is_none() {
        st.membership = Some(Member { at: Live { txid: id.to_string(), index: 1, value: tier.bond }, pass: b.pass.clone() });
    }
    st.save(&cx.home)?;
    let what = match kind {
        Sale::Monthly => format!("{to_name} bought a monthly pass for {}. Expires {}.", kas(b.charge), when(b.pass.expiry, b.now)),
        Sale::Lifetime => format!("{to_name} bought a lifetime pass for {}. Never expires.", kas(b.charge)),
        Sale::Trial => format!("creator gave {to_name} a free trial. Expires {}.", when(b.pass.expiry, b.now)),
        Sale::Term => format!("{to_name} bought a commitment term for {}. Expires {}.", kas(b.charge), when(b.pass.expiry, b.now)),
    };
    let left = if b.next_lane.remaining >= 0 { format!("  ({} left)", b.next_lane.remaining) } else { String::new() };
    println!("{what}{left}\n  tx {id}  fee {}", kas(b.fee));
    Ok(())
}

/// A built (not yet sent) sale.
struct SaleBuild {
    tx: Transaction,
    entries: Vec<UtxoEntry>,
    fee: u64,
    charge: u64,
    now: i64,
    pass: Pass,
    next_lane: Lane,
    /// Where the creator's signature goes in input 0, for a trial signed from the website.
    sig_offset: Option<usize>,
}

/// Builds a sale from the tier's sales lane. Shared by the command line and the website.
/// `trial_signer` is the creator's wallet, needed only for free trials.
#[allow(clippy::too_many_arguments)]
async fn sale_tx(
    creator: &Creator,
    chain: &Chain,
    st: &State,
    kind: Sale,
    force: bool,
    owner_id: Hash,
    owner_name: &str,
    payer: Payer,
    trial_signer: Option<Wallet>,
) -> Result<SaleBuild> {
    let (tier, lane, live, cov) = tier_and_lane(st)?;
    sale_tx_at(creator, chain, &tier, lane, &live, cov, kind, force, owner_id, owner_name, payer, trial_signer).await
}

/// `sale_tx` from an explicit sales lane (the website gets it from the indexer, not state.json).
#[allow(clippy::too_many_arguments)]
async fn sale_tx_at(
    creator: &Creator,
    chain: &Chain,
    tier: &Tier,
    lane: Lane,
    live: &Live,
    cov: Hash,
    kind: Sale,
    force: bool,
    owner_id: Hash,
    owner_name: &str,
    payer: Payer,
    trial_signer: Option<Wallet>,
) -> Result<SaleBuild> {
    sale_tx_full(creator, chain, tier, lane, live, cov, kind, force, owner_id, owner_name, payer, trial_signer, None, SaleOpts::default()).await
}

/// Compute committed for creator-signed sales-lane actions from an outside wallet (trial
/// measured at 12, price change at 11, plus headroom). Fully checked once signed.
const CREATOR_ACTION_BUDGET: u16 = 16;

/// `external_creator`: the creator's public key, when the creator signs a trial from their own
/// wallet (the website). The signature slot is left blank for the wallet to fill.
#[allow(clippy::too_many_arguments)]
async fn sale_tx_full(
    creator: &Creator,
    chain: &Chain,
    tier: &Tier,
    lane: Lane,
    live: &Live,
    cov: Hash,
    kind: Sale,
    force: bool,
    owner_id: Hash,
    owner_name: &str,
    payer: Payer,
    trial_signer: Option<Wallet>,
    external_creator: Option<Vec<u8>>,
    opts: SaleOpts,
) -> Result<SaleBuild> {
    let tier = tier.clone();
    let live = live.clone();
    let v4 = tier.v4.is_some();
    if lane.remaining == 0 && !force {
        bail!("sold out: every pass for this membership has been sold");
    }
    if kind == Sale::Lifetime && lane.life == 0 && !force {
        bail!("this membership doesn't offer lifetime passes");
    }
    if kind == Sale::Trial && tier.trial_ms == 0 && !force {
        bail!("this membership doesn't offer free trials");
    }
    if kind == Sale::Term && !(v4 && lane.term_price > 0 && lane.term_ms > 0) && !force {
        bail!("this membership doesn't offer a commitment plan");
    }
    if !v4 && (opts.prepaid > 0 || opts.voucher.is_some()) {
        bail!("prepaid balances and holder discounts need a v4 membership");
    }
    if let (Some(v), Some(t4)) = (&opts.voucher, &tier.v4) {
        let cap = if is_v5_tier(&tier) { t4.max_discount_bps * 100 } else { t4.max_discount_bps };
        if v.bps > cap && !force {
            bail!("that discount is over this membership's {}% cap", t4.max_discount_bps as f64 / 100.0);
        }
    }
    let voucher = opts.voucher.clone().filter(|v| v.bps > 0 && kind != Sale::Trial);
    let bps = voucher.as_ref().map(|v| v.bps).unwrap_or(0);
    let m = minter(creator, &tier, &lane)?;
    let lane_utxo = chain.find_lane(&m.spk(), &live, cov).await?;
    let now = chain.now_ms().await?;
    let next_lane = Lane { remaining: if lane.remaining > 0 { lane.remaining - 1 } else { lane.remaining }, ..lane };
    let next_m = minter(creator, &tier, &next_lane)?;

    let (expiry, charge) = match kind {
        Sale::Monthly => (now + tier.period_ms, discount_due(&tier, lane.price, bps)),
        Sale::Lifetime => (LIFETIME, discount_due(&tier, lane.life, bps)),
        Sale::Term => (now + lane.term_ms, discount_due(&tier, lane.term_price, bps)),
        Sale::Trial => (now + tier.trial_ms, 0),
    };
    // The pass keeps the full monthly price as its rate; a holder discount is re-checked at each renewal.
    // v5: the pass trusts the same voucher key as the lane it was sold from.
    let pass = Pass { voucher: lane.voucher, owner: owner_name.to_string(), expiry, rate: lane.price, life: lane.life, credit: lane.credit, paid: charge };
    let member = membership(creator, &tier, owner_id, &pass)?;
    let (pre, suf, _) = member.template();

    let mm = minter(creator, &tier, &lane)?;
    let mut sig_offset = None;
    // v4 sales carry the voucher (discount, signature, key) after the buyer and join time.
    let sale_args = move |entry: &str, sig: Vec<u8>, v: Option<&Voucher>| -> Vec<ArtifactValue> {
        let mut a = vec![hb(owner_id)];
        if entry != "buy_lifetime" {
            a.push(ArtifactValue::Int(now));
        }
        if v4 {
            a.extend(voucher_args(v, sig));
        }
        a.push(ArtifactValue::Bytes(pre.clone()));
        a.push(ArtifactValue::Bytes(suf.clone()));
        a
    };
    let (pre, suf) = (member.template().0, member.template().1);
    let unlock = match kind {
        Sale::Monthly | Sale::Lifetime | Sale::Term => {
            let entry = match kind {
                Sale::Monthly => "buy",
                Sale::Lifetime => "buy_lifetime",
                _ => "buy_term",
            };
            let v = voucher.clone();
            Unlock::Script {
                signer: voucher.as_ref().map(|v| v.key.clone()),
                build: Box::new(move |sig| mm.entry_sigscript(entry, &sale_args(entry, sig, v.as_ref()))),
            }
        }
        Sale::Trial if trial_signer.is_none() && external_creator.is_some() => {
            let creator_pk = external_creator.clone().unwrap();
            let script = |sig: Vec<u8>| {
                mm.entry_sigscript(
                    "trial",
                    &[
                        ArtifactValue::Bytes(sig),
                        ArtifactValue::Bytes(creator_pk.clone()),
                        hb(owner_id),
                        ArtifactValue::Int(now),
                        ArtifactValue::Bytes(pre.clone()),
                        ArtifactValue::Bytes(suf.clone()),
                    ],
                )
            };
            sig_offset = Some(sig_slot(&script(vec![0u8; 65])?, &script(vec![1u8; 65])?)?);
            let mm2 = mm.clone();
            let (pre2, suf2) = (pre.clone(), suf.clone());
            Unlock::ExternalScript {
                build: Box::new(move |sig| {
                    mm2.entry_sigscript(
                        "trial",
                        &[
                            ArtifactValue::Bytes(sig),
                            ArtifactValue::Bytes(creator_pk.clone()),
                            hb(owner_id),
                            ArtifactValue::Int(now),
                            ArtifactValue::Bytes(pre2.clone()),
                            ArtifactValue::Bytes(suf2.clone()),
                        ],
                    )
                }),
                budget: CREATOR_ACTION_BUDGET,
            }
        }
        Sale::Trial => {
            let signer = trial_signer.ok_or_else(|| anyhow!("only the creator can issue free trials"))?;
            let creator_pk = signer.pk.clone();
            Unlock::Script {
                signer: Some(signer),
                build: Box::new(move |sig| {
                    mm.entry_sigscript(
                        "trial",
                        &[
                            ArtifactValue::Bytes(sig),
                            ArtifactValue::Bytes(creator_pk.clone()),
                            hb(owner_id),
                            ArtifactValue::Int(now),
                            ArtifactValue::Bytes(pre.clone()),
                            ArtifactValue::Bytes(suf.clone()),
                        ],
                    )
                }),
            }
        }
    };
    let mut outs = vec![cov_out(&next_m, lane_utxo.entry.amount, 0, cov), cov_out(&member, tier.bond + opts.prepaid, 0, cov)];
    outs.extend(payment_outs(creator, &tier, charge)?);
    let uses = charge + tier.bond + opts.prepaid;
    let (spends, outs, payer, fee_out) = with_payin(&tier, vec![Spend { utxo: lane_utxo, unlock }], outs, payer, 1, &pass, &member, uses)?;
    let (tx, entries, fee) = build_tx_with(chain, spends, outs, payer, now as u64, None, fee_out).await?;
    Ok(SaleBuild { tx, entries, fee, charge, now, pass, next_lane, sig_offset })
}

fn current(cx: &Ctx, st: &State) -> Result<(Tier, Member, Wallet, Hash)> {
    let tier = st.tier.clone().ok_or_else(|| anyhow!("run launch first"))?;
    let mem = st.membership.clone().ok_or_else(|| anyhow!("no pass yet; run buy, buy-lifetime, or trial"))?;
    let owner = cx.who(&mem.pass.owner)?.clone();
    Ok((tier, mem, owner, cov_id(st)?))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PassAction {
    Renew,
    Upgrade,
    /// v4: one period paid from the pass's own prepaid balance (the keeper submits it).
    AutoRenew,
}

/// A built (not yet sent) renewal or upgrade.
struct PassBuild {
    tx: Transaction,
    entries: Vec<UtxoEntry>,
    fee: u64,
    charge: u64,
    now: i64,
    next_pass: Pass,
}

/// Renew or upgrade the pass at `at`. Anyone may pay (gifts work), so the payer can be
/// any wallet, including a website visitor's. Shared by the command line and the website.
#[allow(clippy::too_many_arguments)]
async fn pass_tx(
    creator: &Creator,
    chain: &Chain,
    tier: &Tier,
    cov: Hash,
    at: &Live,
    owner_id: Hash,
    p: &Pass,
    action: PassAction,
    force: bool,
    payer: Payer,
) -> Result<PassBuild> {
    pass_tx_v(creator, chain, tier, cov, at, owner_id, p, action, force, payer, None).await
}

/// `pass_tx` with an optional holder discount (v4 renewals and auto-renewals).
#[allow(clippy::too_many_arguments)]
async fn pass_tx_v(
    creator: &Creator,
    chain: &Chain,
    tier: &Tier,
    cov: Hash,
    at: &Live,
    owner_id: Hash,
    p: &Pass,
    action: PassAction,
    force: bool,
    payer: Payer,
    voucher: Option<Voucher>,
) -> Result<PassBuild> {
    let cur = membership(creator, tier, owner_id, p)?;
    let utxo = chain.find(&cur.spk(), at).await?;
    let now = chain.now_ms().await?;
    let v4 = tier.v4.clone();
    if v4.is_none() && (voucher.is_some() || action == PassAction::AutoRenew) {
        bail!("auto-renew and holder discounts need a v4 membership");
    }
    // v5 auto-renewals keep the voucher even at no discount: the contract needs its signature.
    let keep_zero = is_v5_tier(tier) && action == PassAction::AutoRenew;
    let voucher = voucher.filter(|v| v.bps > 0 || keep_zero);
    // Caps: basis points on older tiers, millionths on v5.
    let cap = |t4: &V4| if is_v5_tier(tier) { t4.max_discount_bps * 100 } else { t4.max_discount_bps };
    if let (Some(v), Some(t4)) = (&voucher, &v4) {
        if v.bps > cap(t4) && !force {
            bail!("that discount is over this membership's {}% cap", t4.max_discount_bps as f64 / 100.0);
        }
    }
    let mut pass_value = utxo.entry.amount;
    let (next_pass, charge, lock_time, unlock) = match action {
        PassAction::Renew | PassAction::AutoRenew => {
            if p.expiry >= LIFETIME && !force {
                bail!("this is a lifetime pass; it never needs renewing");
            }
            let auto = action == PassAction::AutoRenew;
            let base = p.expiry.max(now);
            if auto {
                let window = v4.as_ref().map(|v| v.auto_window_ms).unwrap_or(0);
                if p.expiry - now > window && !force {
                    bail!("too early to auto-renew: it opens {} before the end (expires {})", human_ms(window), when(p.expiry, now));
                }
            } else if base - now > tier.max_ahead_ms && !force {
                bail!(
                    "too early to renew: this pass is paid up and expires {} (renewals open when {} min or less remain)",
                    when(p.expiry, now),
                    tier.max_ahead_ms / MINUTE
                );
            }
            let due = discount_due(tier, p.rate, voucher.as_ref().map(|v| v.bps).unwrap_or(0));
            if auto {
                let bond = tier.bond;
                if utxo.entry.amount < bond + due && !force {
                    bail!("the pass balance ({}) doesn't cover a renewal ({})", kas(utxo.entry.amount.saturating_sub(bond)), kas(due));
                }
                pass_value = utxo.entry.amount.saturating_sub(due);
            }
            let next = Pass { expiry: base.saturating_add(tier.period_ms), paid: p.paid + due, ..p.clone() };
            let func = if auto { "autorenew" } else { "renew" };
            let unlock = if v4.is_some() {
                let v = voucher.clone();
                Unlock::Script {
                    signer: voucher.as_ref().map(|v| v.key.clone()),
                    build: Box::new(move |sig| {
                        let mut a = vec![ArtifactValue::Int(now)];
                        a.extend(voucher_args(v.as_ref(), sig));
                        cur.decl_sigscript(func, &a)
                    }),
                }
            } else {
                Unlock::Script { signer: None, build: Box::new(move |_| cur.decl_sigscript("renew", &[ArtifactValue::Int(now)])) }
            };
            (next, due, now as u64, unlock)
        }
        PassAction::Upgrade => {
            if p.life == 0 {
                bail!("this pass has no lifetime option");
            }
            if p.expiry >= LIFETIME {
                bail!("already a lifetime pass");
            }
            let cost = upgrade_cost(p);
            let next = Pass { expiry: LIFETIME, paid: p.paid + cost, ..p.clone() };
            let unlock = Unlock::Script { signer: None, build: Box::new(move |_| cur.decl_sigscript("upgrade", &[])) };
            (next, cost, 0, unlock)
        }
    };
    let next = membership(creator, tier, owner_id, &next_pass)?;
    let mut outs = vec![cov_out(&next, pass_value, 0, cov)];
    outs.extend(payment_outs(creator, tier, charge)?);
    let (spends, outs, payer, fee_out) = with_payin(tier, vec![Spend { utxo, unlock }], outs, payer, 0, &next_pass, &next, charge)?;
    let (tx, entries, fee) = build_tx_with(chain, spends, outs, payer, lock_time, None, fee_out).await?;
    Ok(PassBuild { tx, entries, fee, charge, now, next_pass })
}

/// "3 days", "2 hours", "5 minutes".
fn human_ms(ms: i64) -> String {
    let m = (ms / MINUTE).max(1);
    if m >= 1440 && m % 1440 == 0 {
        format!("{} day{}", m / 1440, if m == 1440 { "" } else { "s" })
    } else if m >= 60 && m % 60 == 0 {
        format!("{} hour{}", m / 60, if m == 60 { "" } else { "s" })
    } else {
        format!("{m} minute{}", if m == 1 { "" } else { "s" })
    }
}

/// v4 DEPOSIT: anyone adds prepaid balance to a pass (topping up auto-renew).
#[allow(clippy::too_many_arguments)]
async fn deposit_tx(creator: &Creator, chain: &Chain, tier: &Tier, cov: Hash, at: &Live, owner_id: Hash, p: &Pass, add: u64, payer: Payer) -> Result<(Transaction, Vec<UtxoEntry>, u64, u64)> {
    if tier.v4.is_none() {
        bail!("prepaid balances need a v4 tier");
    }
    if add == 0 {
        bail!("add at least some KAS");
    }
    let cur = membership(creator, tier, owner_id, p)?;
    let utxo = chain.find(&cur.spk(), at).await?;
    let value = utxo.entry.amount + add;
    let same = cur.clone();
    let unlock = Unlock::Script { signer: None, build: Box::new(move |_| cur.decl_sigscript("deposit", &[])) };
    let outs = vec![cov_out(&same, value, 0, cov)];
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }], outs, Some(payer), 0, None, None).await?;
    Ok((tx, entries, fee, value))
}

/// Compute committed for a withdraw signed by the owner's wallet (checked for real once signed).
const WITHDRAW_BUDGET: u16 = 14;

/// v4 WITHDRAW: the owner takes the prepaid balance back (cancels auto-renew), keeping the bond
/// in the pass. They sign inside the pass script; the network fee comes out of the refund.
async fn withdraw_tx_external(creator: &Creator, chain: &Chain, tier: &Tier, cov: Hash, at: &Live, owner_addr: &Address, p: &Pass) -> Result<CloseBuild> {
    if tier.v4.is_none() {
        bail!("prepaid balances need a v4 tier");
    }
    let owner_pk = owner_addr.payload.to_vec();
    let owner_id = blake(&owner_pk);
    let cur = membership(creator, tier, owner_id, p)?;
    let utxo = chain.find(&cur.spk(), at).await?;
    let back = utxo.entry.amount.saturating_sub(tier.bond);
    if back < MIN_CHANGE {
        bail!("there's no prepaid balance to take back (the {} bond stays with the pass)", kas(tier.bond));
    }
    let script = |fill: u8| cur.decl_sigscript("withdraw", &[ArtifactValue::Bytes(vec![fill; 65]), ArtifactValue::Bytes(owner_pk.clone())]);
    let sig_offset = sig_slot(&script(0)?, &script(1)?)?;
    let same = cur.clone();
    let pk = owner_pk.clone();
    let unlock = Unlock::ExternalScript {
        build: Box::new(move |sig| cur.decl_sigscript("withdraw", &[ArtifactValue::Bytes(sig), ArtifactValue::Bytes(pk.clone())])),
        budget: WITHDRAW_BUDGET,
    };
    let outs = vec![cov_out(&same, tier.bond, 0, cov), TransactionOutput { value: back, script_public_key: pay_to_address_script(owner_addr), covenant: None }];
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }], outs, None, 0, None, Some(1)).await?;
    let refund = tx.outputs[1].value;
    Ok(CloseBuild { tx, entries, fee, refund, sig_offset })
}

/// Compute committed for a close signed by an outside wallet. Measured at 10 units
/// (plus headroom) in the simulation; the signed transaction is fully checked before sending.
const CLOSE_BUDGET: u16 = 14;

/// A close built for an outside wallet (the pass owner) to sign from the website.
struct CloseBuild {
    tx: Transaction,
    entries: Vec<UtxoEntry>,
    fee: u64,
    refund: u64,
    /// Where the 65-byte signature goes inside input 0's script.
    sig_offset: usize,
}

/// The owner ends a pass and gets the bond back (minus the network fee).
async fn close_tx_external(creator: &Creator, chain: &Chain, tier: &Tier, at: &Live, owner_addr: &Address, p: &Pass) -> Result<CloseBuild> {
    let owner_pk = owner_addr.payload.to_vec();
    let owner_id = blake(&owner_pk);
    let cur = membership(creator, tier, owner_id, p)?;
    let utxo = chain.find(&cur.spk(), at).await?;
    let bond = utxo.entry.amount;
    // Locate the signature slot by building the script with two different fillers.
    let a = cur.entry_sigscript("close", &[ArtifactValue::Bytes(vec![0u8; 65]), ArtifactValue::Bytes(owner_pk.clone())])?;
    let b = cur.entry_sigscript("close", &[ArtifactValue::Bytes(vec![1u8; 65]), ArtifactValue::Bytes(owner_pk.clone())])?;
    let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    let sig_offset = *diff.first().ok_or_else(|| anyhow!("could not find the signature slot"))?;
    if diff.len() != 65 || diff[64] != sig_offset + 64 {
        bail!("unexpected close script layout");
    }
    let pk = owner_pk.clone();
    let unlock = Unlock::ExternalScript {
        build: Box::new(move |sig| cur.entry_sigscript("close", &[ArtifactValue::Bytes(sig), ArtifactValue::Bytes(pk.clone())])),
        budget: CLOSE_BUDGET,
    };
    let refund_out = TransactionOutput { value: bond, script_public_key: pay_to_address_script(owner_addr), covenant: None };
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }], vec![refund_out], None, 0, None, Some(0)).await?;
    let refund = tx.outputs[0].value;
    Ok(CloseBuild { tx, entries, fee, refund, sig_offset })
}

/// A tier launched from the creator's own wallet (the website): their wallet funds the
/// sales lane and the network fee. Returns the transaction and the new tier's covenant ID.
async fn launch_tx_external(chain: &Chain, creator_addr: &Address, tier: &Tier, lane: &Lane) -> Result<(Transaction, Vec<UtxoEntry>, u64, Hash)> {
    let creator = Creator::from_address(creator_addr);
    let m = minter(&creator, tier, lane)?;
    let outs = vec![TransactionOutput { value: SOMPI, script_public_key: m.spk(), covenant: None }];
    let payer = Payer::External { name: "your wallet".into(), spk: pay_to_address_script(creator_addr) };
    let (tx, entries, fee) = build_tx_with(chain, vec![], outs, Some(payer), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let cov = tx.outputs[0].covenant.ok_or_else(|| anyhow!("launch output has no covenant"))?.covenant_id;
    Ok((tx, entries, fee, cov))
}

// ---------------------------------------------------------------------------
// v5: owner record and payout vault (transactions signed by outside wallets)
// ---------------------------------------------------------------------------

/// A creator's owner record as this server knows it (record.json). The wallets are addresses;
/// the contract itself only stores hashes of their scripts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RecordInfo {
    pub record_id: String,
    pub owner: String,
    pub recovery: String,
}

impl RecordInfo {
    fn id(&self) -> Result<Hash> {
        self.record_id.parse().map_err(|_| anyhow!("bad owner record id"))
    }
    fn hashes(&self) -> Result<(Hash, Hash)> {
        let a = |s: &str| -> Result<ScriptPublicKey> {
            let addr: Address = s.try_into().map_err(|_| anyhow!("bad address in record.json: {s}"))?;
            Ok(pay_to_address_script(&addr))
        };
        Ok((spk_hash(&a(&self.owner)?), spk_hash(&a(&self.recovery)?)))
    }
    pub fn vault_address(&self) -> Result<String> {
        let spk = vault(self.id()?)?.spk();
        Ok(kaspa_txscript::extract_script_pub_key_address(&spk, net_prefix())?.to_string())
    }
}

/// The owner record isn't where record.json says (a typed error, so the dashboard's lock doesn't
/// depend on the wording of a message).
#[derive(Debug)]
pub struct RecordMissing;
impl std::fmt::Display for RecordMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the owner record isn't on chain with these wallets (was it handed over?)")
    }
}
impl std::error::Error for RecordMissing {}

/// The record's live coin, found by its state's address and its covenant id.
async fn find_record(chain: &Chain, r: &RecordInfo) -> Result<Utxo> {
    let (oh, rh) = r.hashes()?;
    let id = r.id()?;
    let here = chain.utxos_at(&owner_record(oh, rh)?.spk()).await?;
    here.into_iter().find(|u| u.entry.covenant_id == Some(id)).ok_or_else(|| anyhow::Error::new(RecordMissing))
}

/// Spends the record with `use` (no change) or `handover` (to `next`), wallet coin at `wallet_in`.
fn record_spend_v5(utxo: Utxo, r: &RecordInfo, handover_to: Option<(Hash, Hash)>, wallet_in: usize, out_auth: u16) -> Result<(Spend, TransactionOutput)> {
    let (oh, rh) = r.hashes()?;
    let cur = owner_record(oh, rh)?;
    let next = handover_to.unwrap_or((oh, rh));
    let out = cov_out(&owner_record(next.0, next.1)?, utxo.entry.amount, out_auth, r.id()?);
    let build: Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>> + Send + Sync> = match handover_to {
        None => Box::new(move |_| cur.entry_sigscript("use", &[ArtifactValue::Int(wallet_in as i64)])),
        Some(n) => Box::new(move |_| cur.entry_sigscript("handover", &[ArtifactValue::Int(wallet_in as i64), hb(n.0), hb(n.1)])),
    };
    Ok((Spend { utxo, unlock: Unlock::Script { signer: None, build } }, out))
}

fn external(addr: &Address) -> Payer {
    Payer::External { name: "your wallet".into(), spk: pay_to_address_script(addr) }
}

/// Creates the owner record. The owner's wallet pays (1 KAS stays in the record).
async fn record_tx_external(chain: &Chain, owner: &Address, recovery: &Address) -> Result<(Transaction, Vec<UtxoEntry>, u64, Hash)> {
    anyhow::ensure!(owner.version == Version::PubKey && recovery.version == Version::PubKey, "both wallets must be regular Kaspa addresses");
    anyhow::ensure!(owner != recovery, "the recovery wallet must be a different wallet from the owner");
    let rec = owner_record(spk_hash(&pay_to_address_script(owner)), spk_hash(&pay_to_address_script(recovery)))?;
    let outs = vec![TransactionOutput { value: SOMPI, script_public_key: rec.spk(), covenant: None }];
    let (tx, entries, fee) = build_tx_with(chain, vec![], outs, Some(external(owner)), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let id = tx.outputs[0].covenant.ok_or_else(|| anyhow!("record output has no covenant"))?.covenant_id;
    Ok((tx, entries, fee, id))
}

/// v5 price change: the lane, the record (proven by the owner's coin, which pays the fee).
#[allow(clippy::too_many_arguments)]
async fn set_prices_tx_v5(chain: &Chain, owner: &Address, rec: &RecordInfo, tier: &Tier, cov: Hash, lane: Lane, live: &Live, new: Lane) -> Result<(Transaction, Vec<UtxoEntry>, u64, Lane)> {
    let creator = Creator::from_address(owner);
    let m = minter(&creator, tier, &lane)?;
    let utxo = chain.find_lane(&m.spk(), live, cov).await?;
    // The voucher key never changes with a price change (that is set_voucher).
    let next = Lane { remaining: lane.remaining, voucher: lane.voucher, ..new };
    let next_m = minter(&creator, tier, &next)?;
    let args = set_prices_args_v5(1, &next);
    let unlock = Unlock::Script { signer: None, build: Box::new(move |_| m.entry_sigscript("set_prices", &args)) };
    let (rspend, rout) = record_spend_v5(find_record(chain, rec).await?, rec, None, 2, 1)?;
    let outs = vec![cov_out(&next_m, utxo.entry.amount, 0, cov), rout];
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }, rspend], outs, Some(external(owner)), 0, None, None).await?;
    Ok((tx, entries, fee, next))
}

/// v5 free trial: the creator gives `member` a trial pass through the owner record (proven by the
/// owner's coin, which also pays the pass's deposit and the network fee). Inputs: [lane][record]
/// [owner's coins]; outputs: [next lane][trial pass][record](change).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn trial_tx_v5(chain: &Chain, owner: &Address, rec: &RecordInfo, tier: &Tier, cov: Hash, lane: Lane, live: &Live, member: Hash, member_name: &str) -> Result<(Transaction, Vec<UtxoEntry>, u64, Pass)> {
    anyhow::ensure!(tier.trial_ms > 0, "this membership has no free trial (its trial length is 0)");
    let creator = Creator::from_address(owner);
    let m = minter(&creator, tier, &lane)?;
    let utxo = chain.find_lane(&m.spk(), live, cov).await?;
    let now = chain.now_ms().await?;
    let next_lane = Lane { remaining: if lane.remaining > 0 { lane.remaining - 1 } else { lane.remaining }, ..lane };
    let next_m = minter(&creator, tier, &next_lane)?;
    let pass = Pass { voucher: lane.voucher, owner: member_name.to_string(), expiry: now + tier.trial_ms, rate: lane.price, life: lane.life, credit: lane.credit, paid: 0 };
    let pass_c = membership(&creator, tier, member, &pass)?;
    let (pre, suf, _) = pass_c.template();
    let args = vec![ArtifactValue::Int(1), hb(member), ArtifactValue::Int(now), ArtifactValue::Bytes(pre), ArtifactValue::Bytes(suf)];
    let unlock = Unlock::Script { signer: None, build: Box::new(move |_| m.entry_sigscript("trial", &args)) };
    let (rspend, rout) = record_spend_v5(find_record(chain, rec).await?, rec, None, 2, 1)?;
    let outs = vec![cov_out(&next_m, utxo.entry.amount, 0, cov), cov_out(&pass_c, tier.bond, 0, cov), rout];
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }, rspend], outs, Some(external(owner)), now as u64, None, None).await?;
    Ok((tx, entries, fee, pass))
}

/// v5 self-serve trial (minter_v5t): the member starts the free trial themselves, approved by the
/// voucher key. The member's own coins pay the pass's deposit, `prepaid` on top of it (their first
/// paid period, charged by the keeper when the trial ends unless they take it back) and the network
/// fee. Inputs: [lane][member's coins]; outputs: [next lane][trial pass](change).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn self_trial_tx_v5(chain: &Chain, payer: &Address, creator_addr: &Address, tier: &Tier, cov: Hash, lane: Lane, live: &Live, member: Hash, member_name: &str, prepaid: u64, voucher: &Voucher) -> Result<(Transaction, Vec<UtxoEntry>, u64, Pass)> {
    anyhow::ensure!(tier.trial_ms > 0, "this membership has no free trial");
    anyhow::ensure!(tier.v4.as_ref().is_some_and(|v| v.self_trial), "this plan doesn't offer a free trial members can start themselves");
    let creator = Creator::from_address(creator_addr);
    let m = minter(&creator, tier, &lane)?;
    let utxo = chain.find_lane(&m.spk(), live, cov).await?;
    let now = chain.now_ms().await?;
    let next_lane = Lane { remaining: if lane.remaining > 0 { lane.remaining - 1 } else { lane.remaining }, ..lane };
    let next_m = minter(&creator, tier, &next_lane)?;
    let pass = Pass { voucher: lane.voucher, owner: member_name.to_string(), expiry: now + tier.trial_ms, rate: lane.price, life: lane.life, credit: lane.credit, paid: 0 };
    let pass_c = membership(&creator, tier, member, &pass)?;
    let (pre, suf, _) = pass_c.template();
    let vpk = voucher.key.pk.clone();
    let unlock = Unlock::Script {
        signer: Some(voucher.key.clone()),
        build: Box::new(move |sig| {
            let s = if sig.is_empty() { vec![0u8; 65] } else { sig };
            m.entry_sigscript("self_trial", &[hb(member), ArtifactValue::Int(now), ArtifactValue::Bytes(s), ArtifactValue::Bytes(vpk.clone()), ArtifactValue::Bytes(pre.clone()), ArtifactValue::Bytes(suf.clone())])
        }),
    };
    let outs = vec![cov_out(&next_m, utxo.entry.amount, 0, cov), cov_out(&pass_c, tier.bond + prepaid, 0, cov)];
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }], outs, Some(external(payer)), now as u64, None, None).await?;
    Ok((tx, entries, fee, pass))
}

fn set_prices_args_v5(record_in: i64, next: &Lane) -> Vec<ArtifactValue> {
    vec![
        ArtifactValue::Int(record_in),
        ArtifactValue::Int(next.price as i64),
        ArtifactValue::Int(next.life as i64),
        ArtifactValue::Int(b(next.credit)),
        ArtifactValue::Int(next.term_price as i64),
        ArtifactValue::Int(next.term_ms),
    ]
}

/// Withdraws everything in the vault to the owner (who also pays the network fee).
async fn vault_withdraw_tx_external(chain: &Chain, owner: &Address, rec: &RecordInfo) -> Result<(Transaction, Vec<UtxoEntry>, u64, u64)> {
    let vc = vault(rec.id()?)?;
    let mut coins = chain.utxos_at(&vc.spk()).await?;
    anyhow::ensure!(!coins.is_empty(), "the vault is empty");
    // Plenty for one transaction; the rest stays for the next withdrawal.
    coins.sort_by_key(|u| std::cmp::Reverse(u.entry.amount));
    coins.truncate(80);
    let n = coins.len();
    let total: u64 = coins.iter().map(|u| u.entry.amount).sum();
    let mut spends: Vec<Spend> = coins
        .into_iter()
        .map(|u| {
            let vc = vc.clone();
            Spend { utxo: u, unlock: Unlock::Script { signer: None, build: Box::new(move |_| vc.entry_sigscript("withdraw", &[ArtifactValue::Int(n as i64)])) } }
        })
        .collect();
    let (rspend, rout) = record_spend_v5(find_record(chain, rec).await?, rec, None, n + 1, n as u16)?;
    spends.push(rspend);
    let outs = vec![TransactionOutput { value: total, script_public_key: pay_to_address_script(owner), covenant: None }, rout];
    let (tx, entries, fee) = build_tx_with(chain, spends, outs, Some(external(owner)), 0, None, None).await?;
    Ok((tx, entries, fee, total))
}

/// The recovery wallet hands the record to a new owner and recovery wallet (and pays the fee).
async fn handover_tx_external(chain: &Chain, recovery: &Address, rec: &RecordInfo, new_owner: &Address, new_recovery: &Address) -> Result<(Transaction, Vec<UtxoEntry>, u64)> {
    anyhow::ensure!(new_owner.version == Version::PubKey && new_recovery.version == Version::PubKey, "both new wallets must be regular Kaspa addresses");
    anyhow::ensure!(new_owner != new_recovery, "the new recovery wallet must differ from the new owner");
    let to = (spk_hash(&pay_to_address_script(new_owner)), spk_hash(&pay_to_address_script(new_recovery)));
    let (rspend, rout) = record_spend_v5(find_record(chain, rec).await?, rec, Some(to), 1, 0)?;
    let (tx, entries, fee) = build_tx_with(chain, vec![rspend], vec![rout], Some(external(recovery)), 0, None, None).await?;
    Ok((tx, entries, fee))
}

/// Compute committed for a transfer signed by an outside wallet. Measured at 10 units
/// (plus headroom) on v3; the signed transaction is fully checked before sending.
const TRANSFER_BUDGET: u16 = 14;

/// Where a 65-byte signature goes in a script: build it with two different fillers.
fn sig_slot(a: &[u8], b: &[u8]) -> Result<usize> {
    let diff: Vec<usize> = (0..a.len().min(b.len())).filter(|&i| a[i] != b[i]).collect();
    let at = *diff.first().ok_or_else(|| anyhow!("could not find the signature slot"))?;
    if a.len() != b.len() || diff.len() != 65 || diff[64] != at + 64 {
        bail!("unexpected script layout");
    }
    Ok(at)
}

/// A transfer built for the owner to sign from the website: they sign inside the pass
/// script (proving ownership) and their wallet pays the royalty, fee and network fee.
async fn transfer_tx_external(
    creator: &Creator,
    chain: &Chain,
    tier: &Tier,
    cov: Hash,
    at: &Live,
    owner_addr: &Address,
    p: &Pass,
    new_owner: Hash,
) -> Result<CloseBuild> {
    if !tier.resellable {
        bail!("this tier's passes can't be resold or transferred");
    }
    let owner_pk = owner_addr.payload.to_vec();
    let owner_id = blake(&owner_pk);
    if new_owner == owner_id {
        bail!("that's the address that already owns this pass");
    }
    let cur = membership(creator, tier, owner_id, p)?;
    let utxo = chain.find(&cur.spk(), at).await?;
    let script = |fill: u8| cur.decl_sigscript("transfer", &[ArtifactValue::Bytes(vec![fill; 65]), ArtifactValue::Bytes(owner_pk.clone()), hb(new_owner)]);
    let sig_offset = sig_slot(&script(0)?, &script(1)?)?;
    let next = membership(creator, tier, new_owner, p)?;
    let pk = owner_pk.clone();
    let unlock = Unlock::ExternalScript {
        build: Box::new(move |sig| cur.decl_sigscript("transfer", &[ArtifactValue::Bytes(sig), ArtifactValue::Bytes(pk.clone()), hb(new_owner)])),
        budget: TRANSFER_BUDGET,
    };
    let mut outs = vec![cov_out(&next, utxo.entry.amount, 0, cov)];
    outs.extend(payment_outs(creator, tier, tier.royalty)?);
    let payer = Payer::External { name: "your wallet".into(), spk: pay_to_address_script(owner_addr) };
    let (tx, entries, fee) = build_tx_with(chain, vec![Spend { utxo, unlock }], outs, Some(payer), 0, None, None).await?;
    Ok(CloseBuild { tx, entries, fee, refund: tier.royalty, sig_offset })
}

async fn op_renew(cx: &Ctx, chain: &mut Chain, st: &mut State, force: bool) -> Result<()> {
    let (tier, mem, owner, cov) = current(cx, st)?;
    let was_trial = pass_kind(&mem.pass) == "trial";
    let b = pass_tx(&cx.creator.c(), chain, &tier, cov, &mem.at, owner.id, &mem.pass, PassAction::Renew, force, Payer::Key(owner.clone())).await?;
    let id = chain.submit(&b.tx, &b.entries).await?;
    st.membership = Some(Member { at: Live { txid: id.to_string(), index: 0, value: b.tx.outputs[0].value }, pass: b.next_pass.clone() });
    st.save(&cx.home)?;
    let note = if was_trial { " Trial converted to a paid pass." } else { "" };
    println!("Renewed for {} (locked rate).{note} Now expires {}.\n  tx {id}  fee {}", kas(b.charge), when(b.next_pass.expiry, b.now), kas(b.fee));
    Ok(())
}

async fn op_upgrade(cx: &Ctx, chain: &mut Chain, st: &mut State) -> Result<()> {
    let (tier, mem, owner, cov) = current(cx, st)?;
    let p = mem.pass.clone();
    let b = pass_tx(&cx.creator.c(), chain, &tier, cov, &mem.at, owner.id, &p, PassAction::Upgrade, false, Payer::Key(owner.clone())).await?;
    let id = chain.submit(&b.tx, &b.entries).await?;
    st.membership = Some(Member { at: Live { txid: id.to_string(), index: 0, value: b.tx.outputs[0].value }, pass: b.next_pass });
    st.save(&cx.home)?;
    let credit = if p.credit { format!(" ({} credited from past payments)", kas(p.paid.min(p.life))) } else { String::new() };
    println!("Upgraded to lifetime for {}{credit}. Never expires.\n  tx {id}  fee {}", kas(b.charge), kas(b.fee));
    Ok(())
}

async fn op_transfer(cx: &Ctx, chain: &mut Chain, st: &mut State) -> Result<()> {
    let (tier, mem, from, cov) = current(cx, st)?;
    if !tier.resellable {
        bail!("this tier's passes can't be resold or transferred");
    }
    let to = if from.name == "alice" { cx.bob.clone() } else { cx.alice.clone() };
    let p = mem.pass.clone();
    let cur = membership(&cx.creator.c(), &tier, from.id, &p)?;
    let utxo = chain.find(&cur.spk(), &mem.at).await?;
    let next_pass = Pass { owner: to.name.clone(), ..p.clone() };
    let next = membership(&cx.creator.c(), &tier, to.id, &next_pass)?;
    let (pk, to_id) = (from.pk.clone(), to.id);
    let unlock = Unlock::Script {
        signer: Some(from.clone()),
        build: Box::new(move |sig| cur.decl_sigscript("transfer", &[ArtifactValue::Bytes(sig), ArtifactValue::Bytes(pk.clone()), hb(to_id)])),
    };
    let mut outs = vec![cov_out(&next, utxo.entry.amount, 0, cov)];
    outs.extend(payment_outs(&cx.creator.c(), &tier, tier.royalty)?);
    let (tx, entries, fee) = build_tx(chain, vec![Spend { utxo, unlock }], outs, Some(&from), 0, None, None).await?;
    let id = chain.submit(&tx, &entries).await?;
    st.membership = Some(Member { at: Live { txid: id.to_string(), index: 0, value: tx.outputs[0].value }, pass: next_pass });
    st.save(&cx.home)?;
    println!("{} transferred the pass to {} ({} royalty to creator).\n  tx {id}  fee {}", from.name, to.name, kas(tier.royalty), kas(fee));
    Ok(())
}

async fn op_close(cx: &Ctx, chain: &mut Chain, st: &mut State, as_who: &str) -> Result<()> {
    let (tier, mem, owner, _) = current(cx, st)?;
    let signer = cx.who(as_who)?.clone();
    let cur = membership(&cx.creator.c(), &tier, owner.id, &mem.pass)?;
    let utxo = chain.find(&cur.spk(), &mem.at).await?;
    let bond = utxo.entry.amount;
    let pk = signer.pk.clone();
    let unlock = Unlock::Script {
        signer: Some(signer.clone()),
        build: Box::new(move |sig| cur.entry_sigscript("close", &[ArtifactValue::Bytes(sig), ArtifactValue::Bytes(pk.clone())])),
    };
    let (tx, entries, fee) = build_tx(chain, vec![Spend { utxo, unlock }], vec![pay_out(&signer, bond)], None, 0, None, Some(0)).await?;
    let id = chain.submit(&tx, &entries).await?;
    st.membership = None;
    st.save(&cx.home)?;
    println!("{} closed the pass and got {} back.\n  tx {id}  fee {}", signer.name, kas(tx.outputs[0].value), kas(fee));
    Ok(())
}

async fn op_status(cx: &Ctx, chain: &Chain, st: &State) -> Result<()> {
    for w in [&cx.creator, &cx.alice, &cx.bob, &cx.fee] {
        let bal: u64 = chain.utxos_at(&w.spk).await?.iter().map(|u| u.entry.amount).sum();
        println!("{:8} {:>14}  {}", w.name, kas(bal), w.addr);
    }
    let now = chain.now_ms().await?;
    match (&st.tier, &st.lane, &st.covenant_id) {
        (Some(t), Some(l), Some(c)) => {
            println!("\nTier (covenant {c})");
            println!(
                "  For new buyers: monthly {}, lifetime {}, credit {}",
                kas(l.price),
                if l.life > 0 { kas(l.life) } else { "not offered".into() },
                if l.credit { "on" } else { "off" }
            );
            println!(
                "  Permanent: period {} min, trials {}, resellable {}, royalty {}, bond {}, supply {}",
                t.period_ms / MINUTE,
                if t.trial_ms > 0 { format!("{} min", t.trial_ms / MINUTE) } else { "off".into() },
                if t.resellable { "yes" } else { "no" },
                kas(t.royalty),
                kas(t.bond),
                if l.remaining >= 0 { format!("{} left", l.remaining) } else { "unlimited".into() }
            );
            match &t.fee {
                Some(f) => println!(
                    "  KasPass fee: {}% of each payment (skipped under {}), to {}  [v3 contracts]",
                    f.bps as f64 / 100.0,
                    kas(f.min),
                    f.address
                ),
                None => println!("  KasPass fee: none  [v2 contracts]"),
            }
        }
        _ => println!("\nNo tier launched yet."),
    }
    match &st.membership {
        Some(m) => {
            let p = &m.pass;
            let active = p.expiry > now;
            let exp = if p.expiry >= LIFETIME { "never expires".to_string() } else { format!("expires {}", when(p.expiry, now)) };
            println!(
                "\nPass: {} pass owned by {}, {} -> {}",
                pass_kind(p),
                p.owner,
                exp,
                if active { "ACTIVE (player would unlock)" } else { "EXPIRED (player would lock)" }
            );
            println!("  Locked rate {}, paid so far {}", kas(p.rate), kas(p.paid));
            if p.life > 0 && p.expiry < LIFETIME {
                println!("  Lifetime upgrade would cost {} (credit {})", kas(upgrade_cost(p)), if p.credit { "on" } else { "off" });
            }
        }
        None => println!("\nNo live pass."),
    }
    Ok(())
}

fn make_card(cx: &Ctx, st: &State) -> Result<indexer::TierCard> {
    let tier = st.tier.as_ref();
    Ok(indexer::TierCard {
        kaspass: if tier.is_some_and(|t| t.v4.is_some()) { 4 } else if tier.is_some_and(|t| t.fee.is_some()) { 3 } else { 2 },
        network: "testnet-10".into(),
        covenant_id: st.covenant_id.clone().ok_or_else(|| anyhow!("run launch first"))?,
        creator_address: cx.creator.addr.to_string(),
        settings: st.tier.clone().ok_or_else(|| anyhow!("run launch first"))?,
        start_hash: st
            .start_hash
            .clone()
            .ok_or_else(|| anyhow!("this tier was launched before the tool recorded launch points; launch a new tier to index it"))?,
        launch_txid: st.launch_txid.clone().unwrap_or_default(),
        // Where the sales lane was when this card was written, so a website can sell even
        // before the first sale has revealed the lane on-chain.
        known_lane: match (&st.minter, st.lane) {
            (Some(m), Some(lane)) => Some(indexer::KnownLane { txid: m.txid.clone(), index: m.index, lane }),
            _ => None,
        },
        placeholder: false,
    })
}

fn write_card(cx: &Ctx, st: &State) -> Result<()> {
    let card = make_card(cx, st)?;
    std::fs::write(cx.home.join("tier-card.json"), serde_json::to_string_pretty(&card)?)?;
    Ok(())
}

fn read_card(path: &Path) -> Result<indexer::TierCard> {
    let text = std::fs::read_to_string(path).map_err(|_| anyhow!("can't read {} (run `card` or `launch` first)", path.display()))?;
    let card: indexer::TierCard = serde_json::from_str(&text)?;
    if !card.placeholder && !is_v5_tier(&card.settings) {
        bail!("{RETIRED}");
    }
    Ok(card)
}

/// The three test wallets, plus any names saved with `membership-cli name`.
fn known_names(cx: &Ctx) -> indexer::Names {
    let mut v: Vec<(String, String)> = [&cx.creator, &cx.alice, &cx.bob, &cx.fee].iter().map(|w| (w.name.clone(), w.addr.to_string())).collect();
    for line in std::fs::read_to_string(cx.home.join(NAMES_FILE)).unwrap_or_default().lines() {
        if let Some((addr, label)) = line.trim().split_once(' ') {
            v.push((label.trim().to_string(), addr.to_string()));
        }
    }
    indexer::Names(v)
}

const NAMES_FILE: &str = "names.txt";

fn op_name(cx: &Ctx, address: &str, label: &str) -> Result<()> {
    indexer::owner_hash_of(address)?; // must be a regular Kaspa address
    let label = label.trim();
    if label.is_empty() {
        bail!("give the address a name, e.g. membership-cli name kaspatest:... my-kaspire");
    }
    let path = cx.home.join(NAMES_FILE);
    let mut lines: Vec<String> =
        std::fs::read_to_string(&path).unwrap_or_default().lines().filter(|l| !l.starts_with(&format!("{address} "))).map(String::from).collect();
    lines.push(format!("{address} {label}"));
    std::fs::write(&path, lines.join("\n") + "\n")?;
    println!("Saved: {address} is now shown as \"{label}\" (in {NAMES_FILE}, which stays on this computer).");
    Ok(())
}

async fn run_scan(card: indexer::TierCard, chain: &Chain) -> Result<(indexer::Indexer, Vec<indexer::FoundPass>, i64)> {
    run_scan_cached(card, chain, None).await
}

fn cache_path(home: &Path, card: &indexer::TierCard) -> PathBuf {
    home.join(format!("scan-cache-{}.json", &card.covenant_id[..12.min(card.covenant_id.len())]))
}

/// Scans a tier. With a cache file, continues from where the last scan stopped.
async fn run_scan_cached(card: indexer::TierCard, chain: &Chain, cache: Option<&Path>) -> Result<(indexer::Indexer, Vec<indexer::FoundPass>, i64)> {
    let start: Hash = card.start_hash.parse().map_err(|_| anyhow!("bad start hash in tier card"))?;
    let mut ix = indexer::Indexer::new(card)?;
    let resumed = cache.and_then(|p| ix.load_cache(p));
    let mut cursor = Some(resumed.unwrap_or(start));
    let t0 = std::time::Instant::now();
    let pages = indexer::scan(&mut ix, chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    let passes = ix.verified(chain, now).await?;
    if let (Some(p), Some(c)) = (cache, cursor) {
        ix.save_cache(p, c)?;
    }
    if matches!(chain, Chain::Live(_)) {
        let how = if resumed.is_some() { "new blocks since the last scan" } else { "blocks since launch" };
        println!("Scanned {pages} page(s) of {how} in {:.1}s", t0.elapsed().as_secs_f64());
    }
    Ok((ix, passes, now))
}

// ---------------------------------------------------------------------------
// Offline end-to-end simulation
// ---------------------------------------------------------------------------

async fn expect_refusal(label: &str, r: Result<()>) -> Result<()> {
    match r {
        Err(e) => {
            println!("   refused as expected ({label}): {e}");
            Ok(())
        }
        Ok(_) => bail!("{label}: should have been refused"),
    }
}




/// Tries to cheat the fee on a v3 tier: the contracts themselves must refuse.
async fn fee_tamper_test(cx: &Ctx, chain: &mut Chain, home: &Path, fee: Fee, v5: V4) -> Result<()> {
    println!("== protocol fee can't be skipped: the CONTRACTS refuse every cheat");
    let mut st = State::default();
    let tier = Tier { period_ms: 10 * MINUTE, trial_ms: 0, resellable: true, royalty: SOMPI, bond: SOMPI, max_ahead_ms: 20 * MINUTE, fee: Some(fee), v4: Some(v5) };
    op_launch(cx, chain, &mut st, tier.clone(), Lane { voucher: None, price: 2 * SOMPI, life: 20 * SOMPI, credit: false, remaining: -1, term_price: 0, term_ms: 0 }).await?;
    let b = sale_tx(&cx.creator.c(), chain, &st, Sale::Lifetime, false, cx.bob.id, "bob", Payer::Key(cx.alice.clone()), None).await?;
    // Outputs: [0 lane][1 pass][2 creator 19.6][3 fee 0.4][4 change]
    if b.tx.outputs[2].value != 196 * SOMPI / 10 || b.tx.outputs[3].value != 4 * SOMPI / 10 || b.tx.outputs[3].script_public_key != cx.fee.spk {
        bail!("unexpected sale layout");
    }
    engine_run(&b.tx, &b.entries, 0, None)?;
    let tamper = |label: &str, f: &dyn Fn(&mut Transaction)| -> Result<()> {
        let mut t = b.tx.clone();
        f(&mut t);
        t.finalize();
        match engine_run(&t, &b.entries, 0, None) {
            Err(_) => {
                println!("   refused as expected ({label})");
                Ok(())
            }
            Ok(_) => bail!("{label}: the contract should have refused"),
        }
    };
    tamper("fee 1 sompi short", &|t| t.outputs[3].value -= 1)?;
    tamper("fee sent to someone else", &|t| t.outputs[3].script_public_key = cx.bob.spk.clone())?;
    tamper("fee output left out", &|t| {
        t.outputs.remove(3);
    })?;
    tamper("creator paid 1 sompi short", &|t| t.outputs[2].value -= 1)?;
    tamper("fee and creator swapped", &|t| t.outputs.swap(2, 3))?;
    chain.submit(&b.tx, &b.entries).await?;
    st = State::load(home)?;

    // The pass contract checks the fee too: upgrade a monthly pass (20 KAS, credit off).
    let m = sale_tx(&cx.creator.c(), chain, &st, Sale::Monthly, false, cx.alice.id, "alice", Payer::Key(cx.alice.clone()), None).await?;
    let mid = chain.submit(&m.tx, &m.entries).await?;
    let at = Live { txid: mid.to_string(), index: 1, value: tier.bond };
    let cov = cov_id(&st)?;
    let u = pass_tx(&cx.creator.c(), chain, &tier, cov, &at, cx.alice.id, &m.pass, PassAction::Upgrade, false, Payer::Key(cx.alice.clone())).await?;
    // Outputs: [0 pass][1 creator 19.6][2 fee 0.4][3 change]
    if u.tx.outputs[1].value != 196 * SOMPI / 10 || u.tx.outputs[2].value != 4 * SOMPI / 10 {
        bail!("unexpected upgrade layout");
    }
    engine_run(&u.tx, &u.entries, 0, None)?;
    for (label, f) in [
        ("upgrade fee short", Box::new(|t: &mut Transaction| t.outputs[2].value -= 1) as Box<dyn Fn(&mut Transaction)>),
        ("upgrade fee redirected", Box::new(|t: &mut Transaction| t.outputs[2].script_public_key = cx.bob.spk.clone())),
        ("upgrade fee left out", Box::new(|t: &mut Transaction| {
            t.outputs.remove(2);
        })),
    ] {
        let mut t = u.tx.clone();
        f(&mut t);
        t.finalize();
        if engine_run(&t, &u.entries, 0, None).is_ok() {
            bail!("{label}: the contract should have refused");
        }
        println!("   refused as expected ({label})");
    }
    chain.submit(&u.tx, &u.entries).await?;
    println!("   ok: honest sale and upgrade accepted; every short, redirected, missing or swapped fee refused");
    Ok(())
}

async fn check_indexer(cx: &Ctx, st: &State, chain: &Chain) -> Result<()> {
    let (_, passes, _) = run_scan(make_card(cx, st)?, chain).await?;
    let got: Vec<_> = passes.iter().map(|p| (p.txid.clone(), p.index, p.owner_hash.clone(), p.rate_sompi, p.lifetime_price_sompi, p.credit, p.paid_sompi, p.expiry_ms)).collect();
    let want: Vec<_> = match &st.membership {
        Some(m) => {
            let owner = cx.who(&m.pass.owner)?.id.to_string();
            vec![(m.at.txid.clone(), m.at.index, owner, m.pass.rate, m.pass.life, m.pass.credit, m.pass.paid, m.pass.expiry)]
        }
        None => vec![],
    };
    if got != want {
        bail!("indexer disagrees.\n  chain says: {got:?}\n  tool says:  {want:?}");
    }
    println!("   indexer agrees ({} live pass)", got.len());
    Ok(())
}

async fn simulate() -> Result<()> {
    let home = std::env::temp_dir().join(format!("kaspass-sim-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    let cx = Ctx {
        creator: wallet_from_secret("creator", SecretKey::from_slice(&[0x11; 32])?),
        alice: wallet_from_secret("alice", SecretKey::from_slice(&[0x22; 32])?),
        bob: wallet_from_secret("bob", SecretKey::from_slice(&[0x33; 32])?),
        fee: wallet_from_secret("fee", SecretKey::from_slice(&[0x44; 32])?),
        home: home.clone(),
    };
    let mut utxos = HashMap::new();
    for (i, w) in [&cx.creator, &cx.alice, &cx.bob].into_iter().enumerate() {
        utxos.insert(TransactionOutpoint::new(Hash::from_bytes([i as u8 + 1; 32]), 0), UtxoEntry::new(1_000 * SOMPI, w.spk.clone(), 0, false, None));
    }
    let mut chain = Chain::Sim(Sim { utxos, now: 1_800_000_000_000, blocks: vec![], block_times: vec![], clock: None });
    let mut st = State::default();
    let adv = |c: &mut Chain, mins: i64| {
        if let Chain::Sim(s) = c {
            s.now += mins * MINUTE;
        }
    };
    // A v5 membership with the protocol fee, paying the creator's vault through an owner record.
    // The fee minimum is lowered to 0.1 KAS here so the small test prices exercise both cases:
    // fees paid (upgrade, lifetime) and fees skipped (monthly, royalty).
    let voucher = wallet_from_secret("voucher", SecretKey::from_slice(&[0x77; 32])?);
    let recovery = wallet_from_secret("w66", SecretKey::from_slice(&[0x66; 32])?);
    let rid = sim_owner_record(&cx, &mut chain, &home, &recovery).await?;
    let sim_fee = Fee { address: cx.fee.addr.to_string(), bps: DEFAULT_FEE_BPS, min: SOMPI / 10 };
    let v5 = V4 { auto_window_ms: MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: shop::V5_MAX_DISCOUNT_BPS, record_id: Some(rid.to_string()), self_trial: false };
    let tier = Tier { period_ms: 10 * MINUTE, trial_ms: 5 * MINUTE, resellable: true, royalty: SOMPI, bond: SOMPI, max_ahead_ms: 20 * MINUTE, fee: Some(sim_fee.clone()), v4: Some(v5.clone()) };
    let lane = Lane { voucher: None, price: 2 * SOMPI, life: 20 * SOMPI, credit: true, remaining: 3, term_price: 0, term_ms: 0 };

    println!("== launch (v5): 2 KAS monthly, 20 KAS lifetime, credit on, supply 3");
    op_launch(&cx, &mut chain, &mut st, tier.clone(), lane).await?;
    let launch_card = make_card(&cx, &st)?;
    println!("== alice buys monthly");
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    check_indexer(&cx, &st, &chain).await?;
    op_status(&cx, &chain, &st).await?;
    adv(&mut chain, 11);
    println!("== the pass lapsed (+11 min); alice renews it");
    op_renew(&cx, &mut chain, &mut st, false).await?;
    check_indexer(&cx, &st, &chain).await?;
    println!("== alice upgrades to lifetime: 20 KAS minus 4 KAS already paid");
    op_upgrade(&cx, &mut chain, &mut st).await?;
    check_indexer(&cx, &st, &chain).await?;
    println!("== renewing a lifetime pass, skipping the tool's check: the CONTRACT must refuse");
    expect_refusal("lifetime renew", op_renew(&cx, &mut chain, &mut st, true).await).await?;
    println!("== bob tries to close alice's pass");
    expect_refusal("stranger close", op_close(&cx, &mut chain, &mut st, "bob").await).await?;
    println!("== alice transfers to bob");
    op_transfer(&cx, &mut chain, &mut st).await?;
    check_indexer(&cx, &st, &chain).await?;
    println!("== bob closes it");
    op_close(&cx, &mut chain, &mut st, "bob").await?;
    check_indexer(&cx, &st, &chain).await?;
    println!("== indexer check 1: finds nothing live (the only pass was closed)");
    {
        let card = make_card(&cx, &st)?;
        let (_, passes, _) = run_scan(card, &chain).await?;
        if !passes.is_empty() {
            bail!("indexer should find no live passes, found {}", passes.len());
        }
        println!("   ok: 0 live passes");
    }
    println!("== alice buys monthly again");
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    op_status(&cx, &chain, &st).await?;
    println!("== indexer check 2: finds alice's pass with the exact locked terms, from the launch card and the chain alone");
    {
        let (ix, passes, now) = run_scan(launch_card.clone(), &chain).await?;
        indexer::print_passes(&ix, &passes, now, &known_names(&cx));
        let m = st.membership.as_ref().unwrap();
        let want = (m.at.txid.clone(), m.at.index, cx.alice.id.to_string(), m.pass.rate, m.pass.paid, m.pass.expiry);
        let got: Vec<_> = passes.iter().map(|p| (p.txid.clone(), p.index, p.owner_hash.clone(), p.rate_sompi, p.paid_sompi, p.expiry_ms)).collect();
        if got != vec![want] {
            bail!("indexer disagrees with the chain: {got:?}");
        }
        let owner = indexer::owner_hash_of(&cx.alice.addr.to_string())?;
        if owner != cx.alice.id {
            bail!("address -> owner lookup mismatch");
        }
        println!("   ok: matches exactly, and alice's address maps to her pass");
    }
    op_close(&cx, &mut chain, &mut st, "alice").await?;
    println!("== alice buys lifetime outright (last pass in the supply)");
    op_sale(&cx, &mut chain, &mut st, Sale::Lifetime, false).await?;
    check_indexer(&cx, &st, &chain).await?;
    op_close(&cx, &mut chain, &mut st, "alice").await?;
    println!("== sold out: tool refuses, and the CONTRACT refuses too");
    expect_refusal("tool sold-out check", op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await).await?;
    expect_refusal("contract sold-out check", op_sale(&cx, &mut chain, &mut st, Sale::Monthly, true).await).await?;
    println!("== final status");
    op_status(&cx, &chain, &st).await?;
    let fees: u64 = chain.utxos_at(&cx.fee.spk).await?.iter().map(|u| u.entry.amount).sum();
    // 2% of: 16 KAS upgrade (0.32) + 20 KAS lifetime (0.4). Skipped (under 0.1 KAS): the 2 KAS
    // monthly payments (0.04) and the 1 KAS royalty (0.02).
    if fees != 72 * SOMPI / 100 {
        bail!("protocol fees should total 0.72 KAS, got {}", kas(fees));
    }
    println!("== protocol fee: 0.72 KAS collected (2% of the 16 KAS upgrade and 20 KAS lifetime; small payments skipped)");
    {
        let (ix, _, _) = run_scan(launch_card.clone(), &chain).await?;
        let ev = ix.events();
        let kinds: Vec<&str> = ev.iter().map(|e| e.kind.as_str()).collect();
        let gross: u64 = ev.iter().map(|e| e.amount_sompi).sum();
        let fees: u64 = ev.iter().map(|e| e.fee_sompi).sum();
        if kinds != ["monthly", "renew", "upgrade", "transfer", "monthly", "lifetime"] || gross != 43 * SOMPI || fees != 72 * SOMPI / 100 {
            bail!("dashboard earnings disagree with the chain: {kinds:?}, gross {}, fees {}", kas(gross), kas(fees));
        }
        println!("   ok: dashboard earnings from the chain: 6 events, 43 KAS paid, 42.28 KAS to the creator, 0.72 KAS in fees");
    }
    fee_tamper_test(&cx, &mut chain, &home, sim_fee.clone(), v5.clone()).await?;
    auth::self_test()?;
    println!("== wallet sign-in: KIP-5 test vector verified, wrong message rejected");
    v5sim::run(&cx, &mut chain).await?;
    println!("\nSIMULATION PASSED");
    // Developer hook: keep serving a simulated tier over the web API.
    if let Ok(port) = std::env::var("KASPASS_SIM_SERVE") {
        let site = std::env::temp_dir().join("kaspass-sim-telegram");
        if let Ok(r) = std::env::var("KASPASS_SIM_RETURN") {
            api::set_return_origins(&[r])?;
        }
        if let Ok(m) = std::env::var("KASPASS_SIM_CSP") {
            api::set_csp_mode(&m)?;
        }
        api::set_creator_only_launch(std::env::var("KASPASS_SIM_CREATOR_ONLY").is_ok());
        api::set_sim_pay();
        {
            // v5 levels with a real-time clock: 2-minute periods, auto-renew in the last minute.
            let voucher = wallet_from_secret("voucher", SecretKey::from_slice(&[0x77; 32])?);
            let keeper = wallet_from_secret("keeper", SecretKey::from_slice(&[0x78; 32])?);
            if let Chain::Sim(s) = &mut chain {
                for (i, w) in [&keeper, &wallet_from_secret("w55", SecretKey::from_slice(&[0x55; 32])?), &wallet_from_secret("w66", SecretKey::from_slice(&[0x66; 32])?)].into_iter().enumerate() {
                    s.utxos.insert(TransactionOutpoint::new(Hash::from_bytes([0xa0 + i as u8; 32]), 0), UtxoEntry::new(1_000 * SOMPI, w.spk.clone(), 0, false, None));
                }
                s.clock = Some(std::time::Instant::now());
            }
            // KASPASS_SIM_EMPTY=1: a new site with no membership yet, for the creator to launch one
            // (KASPASS_SIM_NOFEE=1 makes those launches fee-free).
            if std::env::var("KASPASS_SIM_EMPTY").is_ok() {
                let card = placeholder_card("testnet-10", &cx.creator.addr.to_string(), chain.sink().await?)?;
                let ix = indexer::Indexer::new(card)?;
                let mut shop = shop::Shop::open(&site, &ix.card);
                shop.fee_address = Some(cx.fee.addr.to_string());
                shop.no_fee = std::env::var("KASPASS_SIM_NOFEE").is_ok();
                shop.voucher = Some(voucher.clone());
                shop.keeper = Some(keeper.clone());
                println!("empty sim site for {}", cx.creator.addr);
                return api::serve(vec![api::LevelSetup { ix, shop: Some(shop), cache: None }], chain, port.parse()?, api::DEFAULT_MEMBERS_HTML.to_string(), telegram::Bot::open(&site).await?, discord::Discord::open(&site).await?, clubs::Club::load(&site)?, { prices::start(site.clone(), true); site }, api::SecondNode::default()).await;
            }
            // KASPASS_SIM_V5=1: v5 levels like Kaspa Vision. The creator already has an owner record
            // (recovery wallet: the 0x66 test key), two fee-free levels priced in CAD (100 and 300,
            // with a 200 founding price on the second), payouts to the vault. Rates come from
            // sim-rates.json in the site folder (tests write it; no file = the feed is down).
            if std::env::var("KASPASS_SIM_V5").is_ok() {
                let recovery = wallet_from_secret("w66", SecretKey::from_slice(&[0x66; 32])?);
                let rid = sim_owner_record(&cx, &mut chain, &site, &recovery).await?;
                // 1 KAS = 0.10 CAD in the simulation's launch, so the ceilings are 3,000 and 9,000 KAS.
                let founding_until: i64 = std::env::var("KASPASS_SIM_FOUNDING_UNTIL").ok().and_then(|v| v.parse().ok()).unwrap_or(4_102_444_800_000);
                // Test plans only: names and prices are made-up examples for the simulator, not
                // anyone's real price list.
                // (name, currency, price, founding, lifetime, commitment price, commitment periods, trial)
                type Plan = (&'static str, &'static str, f64, Option<f64>, Option<f64>, Option<f64>, i64, i64);
                let mut plans: Vec<Plan> = vec![("Kaspa Vision", "CAD", 100.0, None, None, None, 0, 0), ("Kaspa Vision + KasTax", "CAD", 300.0, Some(200.0), None, None, 0, 0)];
                // KASPASS_SIM_V5X=1 adds two plans with everything the older versions offered: one
                // priced in KAS, one in CAD, each with a lifetime price and a commitment term.
                if std::env::var("KASPASS_SIM_V5X").is_ok() {
                    plans.push(("KAS Club", "KAS", 50.0, None, Some(400.0), Some(450.0), 6, 3 * MINUTE));
                    plans.push(("CAD Extras", "CAD", 20.0, None, Some(150.0), Some(180.0), 6, 0));
                }
                // KASPASS_SIM_TRIAL=1 adds a plan whose members start a 1-minute free trial themselves.
                if std::env::var("KASPASS_SIM_TRIAL").is_ok() {
                    let trial_min: i64 = std::env::var("KASPASS_SIM_TRIAL_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
                    plans.push(("Trial Club", "CAD", 30.0, None, None, None, 0, trial_min * MINUTE));
                }
                let (mut setups, mut names, mut files, mut prices) = (Vec::new(), serde_json::Map::new(), Vec::new(), std::collections::HashMap::new());
                let ring: std::sync::Arc<std::sync::RwLock<Vec<Wallet>>> = Default::default();
                for (name, cur, cad, founding, life_amt, term_amt, term_n, trial_ms) in plans {
                    let mut st = State::default();
                    let fee = Fee { address: cx.creator.addr.to_string(), bps: 0, min: 0 };
                    let v4 = V4 { auto_window_ms: MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: shop::V5_MAX_DISCOUNT_BPS, record_id: Some(rid.to_string()), self_trial: name == "Trial Club" };
                    let tier = Tier { period_ms: 2 * MINUTE, trial_ms, resellable: false, royalty: 0, bond: SOMPI, max_ahead_ms: 4 * MINUTE, fee: Some(fee), v4: Some(v4) };
                    // In KAS, the on-chain prices are the prices; in CAD, ceilings at 0.10 CAD a KAS.
                    let on_chain = |a: f64| if cur == "KAS" { (a * SOMPI as f64) as u64 } else { (a / 0.10 * shop::V5_CEILING_TIMES).ceil() as u64 * SOMPI };
                    let ceiling = on_chain(cad);
                    let (life, term_price) = (life_amt.map(on_chain).unwrap_or(0), term_amt.map(on_chain).unwrap_or(0));
                    op_launch(&cx, &mut chain, &mut st, tier, Lane { voucher: None, price: ceiling, life, credit: false, remaining: -1, term_price, term_ms: if term_price > 0 { term_n * 2 * MINUTE } else { 0 } }).await?;
                    let mut card = make_card(&cx, &st)?;
                    card.kaspass = 5;
                    let ix = indexer::Indexer::new(card)?;
                    let file = format!("tier-card-{}.json", &ix.card.covenant_id[..12]);
                    std::fs::write(site.join(&file), serde_json::to_string_pretty(&ix.card)?)?;
                    files.push(file);
                    names.insert(ix.card.covenant_id.clone(), serde_json::json!(name));
                    prices.insert(ix.card.covenant_id.clone(), prices::LevelPrice { currency: cur.into(), amount: cad, founding: founding.map(|a| prices::Founding { amount: a, until_ms: founding_until }), lifetime: life_amt, term: term_amt });
                    let mut shop = shop::Shop::open(&site, &ix.card);
                    shop.no_fee = true;
                    shop.voucher = Some(voucher.clone());
                    shop.voucher_ring = ring.clone();
                    shop.keeper = Some(keeper.clone());
                    println!("v5 sim level: {} {}", name, ix.card.covenant_id);
                    setups.push(api::LevelSetup { ix, shop: Some(shop), cache: None });
                }
                std::fs::write(site.join("tier-names.json"), serde_json::to_string_pretty(&names)?)?;
                std::fs::write(site.join("levels.json"), serde_json::to_string_pretty(&serde_json::json!({ "levels": files }))?)?;
                prices::save_level_prices(&site, &prices)?;
                return api::serve(setups, chain, port.parse()?, api::DEFAULT_MEMBERS_HTML.to_string(), telegram::Bot::open(&site).await?, discord::Discord::open(&site).await?, clubs::Club::load(&site)?, { prices::start(site.clone(), true); site }, api::SecondNode::default()).await;
            }
            // KASPASS_SIM_LEVELS=3 sells three levels (10, 20 and 30 KAS a month), named in tier-names.json.
            // They're v5 levels priced in KAS, with the KasPass fee, resale and a 1 KAS royalty, so
            // the creator has an owner record (recovery: the 0x66 test key) and payouts go to its vault.
            let n: u64 = std::env::var("KASPASS_SIM_LEVELS").ok().and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 5);
            let recovery = wallet_from_secret("w66", SecretKey::from_slice(&[0x66; 32])?);
            let rid = sim_owner_record(&cx, &mut chain, &site, &recovery).await?;
            let mut setups = Vec::new();
            let mut names = serde_json::Map::new();
            let mut files = Vec::new();
            let mut prices = std::collections::HashMap::new();
            let ring: std::sync::Arc<std::sync::RwLock<Vec<Wallet>>> = Default::default();
            for i in 0..n {
                let mut st = State::default();
                let fee = Fee { address: cx.fee.addr.to_string(), bps: DEFAULT_FEE_BPS, min: SOMPI / 2 };
                let v4 = V4 { auto_window_ms: MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: shop::V5_MAX_DISCOUNT_BPS, record_id: Some(rid.to_string()), self_trial: false };
                let tier = Tier { period_ms: 2 * MINUTE, trial_ms: 0, resellable: true, royalty: SOMPI, bond: SOMPI, max_ahead_ms: 4 * MINUTE, fee: Some(fee), v4: Some(v4) };
                let k = i + 1;
                op_launch(&cx, &mut chain, &mut st, tier, Lane { voucher: None, price: 10 * k * SOMPI, life: 100 * k * SOMPI, credit: false, remaining: -1, term_price: 100 * k * SOMPI, term_ms: 24 * MINUTE }).await?;
                let mut card = make_card(&cx, &st)?;
                card.kaspass = 5;
                let ix = indexer::Indexer::new(card)?;
                // Like a real site: its tier cards live in the folder it serves from.
                let file = format!("tier-card-{}.json", &ix.card.covenant_id[..12]);
                std::fs::write(site.join(&file), serde_json::to_string_pretty(&ix.card)?)?;
                files.push(file);
                if n > 1 {
                    names.insert(ix.card.covenant_id.clone(), serde_json::json!(["Bronze", "Silver", "Gold", "Platinum", "Diamond"][i as usize]));
                }
                let kas = |x: u64| (10 * x * k) as f64;
                prices.insert(ix.card.covenant_id.clone(), prices::LevelPrice { currency: "KAS".into(), amount: kas(1), founding: None, lifetime: Some(kas(10)), term: Some(kas(10)) });
                let mut shop = shop::Shop::open(&site, &ix.card);
                shop.fee_address = Some(cx.fee.addr.to_string());
                shop.voucher = Some(voucher.clone());
                shop.voucher_ring = ring.clone();
                shop.keeper = Some(keeper.clone());
                println!("sim level: {}", ix.card.covenant_id);
                setups.push(api::LevelSetup { ix, shop: Some(shop), cache: None });
            }
            if n > 1 {
                std::fs::write(site.join("tier-names.json"), serde_json::to_string_pretty(&names)?)?;
            }
            std::fs::write(site.join("levels.json"), serde_json::to_string_pretty(&serde_json::json!({ "levels": files }))?)?;
            prices::save_level_prices(&site, &prices)?;
            return api::serve(setups, chain, port.parse()?, api::DEFAULT_MEMBERS_HTML.to_string(), telegram::Bot::open(&site).await?, discord::Discord::open(&site).await?, clubs::Club::load(&site)?, { prices::start(site.clone(), true); site }, api::SecondNode::default()).await;
        }
    }
    std::fs::remove_dir_all(&home).ok();
    Ok(())
}

/// The simulated site's owner record for the creator (written to the site folder like a real one).
async fn sim_owner_record(cx: &Ctx, chain: &mut Chain, site: &Path, recovery: &Wallet) -> Result<Hash> {
    // Plenty of test KAS for every test wallet (purchases here can cost hundreds of KAS). Each
    // call funds fresh coins, so every record (and its vault) is a different one.
    static ROUND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    let round = ROUND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Chain::Sim(sim) = chain {
        for (i, k) in [0x11u8, 0x22, 0x33, 0x55, 0x66].into_iter().enumerate() {
            let w = wallet_from_secret("t", SecretKey::from_slice(&[k; 32])?);
            for j in 0..3u8 {
                let mut tag = [0xc0 + i as u8; 32];
                tag[31] = round;
                sim.utxos.insert(TransactionOutpoint::new(Hash::from_bytes(tag), j as u32), UtxoEntry::new(5_000 * SOMPI, w.spk.clone(), 0, false, None));
            }
        }
    }
    let (oh, rh) = (spk_hash(&cx.creator.spk), spk_hash(&recovery.spk));
    let outs = vec![TransactionOutput { value: SOMPI, script_public_key: owner_record(oh, rh)?.spk(), covenant: None }];
    let (tx, entries, _) = build_tx(chain, vec![], outs, Some(&cx.creator), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let rid = tx.outputs[0].covenant.ok_or_else(|| anyhow!("record output has no covenant"))?.covenant_id;
    chain.submit(&tx, &entries).await?;
    let rec = RecordInfo { record_id: rid.to_string(), owner: cx.creator.addr.to_string(), recovery: recovery.addr.to_string() };
    std::fs::write(site.join(shop::RECORD_FILE), serde_json::to_string_pretty(&rec)?)?;
    Ok(rid)
}

/// The website needs only the public tier card: no keys are loaded or created.
/// The tier the creator picked in the dashboard ("Make this the live tier"), if any.
const ACTIVE_CARD: &str = "active-card.txt";

/// The memberships this site sells, lowest level first: levels.json if the dashboard saved one,
/// else the card chosen in the dashboard (active-card.txt), else `serve --card`.
fn cards_to_serve(home: &Path, card: &Path) -> Result<Vec<indexer::TierCard>> {
    if let Some(files) = api::read_levels(home) {
        let mut out: Vec<indexer::TierCard> = Vec::new();
        for f in &files {
            match read_card(&home.join(f)) {
                Ok(c) if out.first().is_some_and(|first| first.creator_address != c.creator_address || first.network != c.network) => {
                    eprintln!("Skipping level {f}: it belongs to another creator or network.");
                }
                Ok(c) if out.iter().any(|o| o.covenant_id == c.covenant_id) => {}
                Ok(c) => out.push(c),
                Err(e) => eprintln!("Skipping level {f}: {e}"),
            }
        }
        if !out.is_empty() {
            println!("Serving {} level{} (from levels.json).", out.len(), if out.len() == 1 { "" } else { "s" });
            return Ok(out);
        }
        eprintln!("levels.json lists no readable membership; falling back.");
    }
    let chosen = std::fs::read_to_string(home.join(ACTIVE_CARD)).ok().map(|t| t.trim().to_string()).filter(|t| api::safe_card_name(t));
    let card = match chosen.as_deref().map(|f| read_card(&home.join(f))) {
        Some(Ok(c)) => {
            println!("Serving {} (chosen in the dashboard).", chosen.as_deref().unwrap_or_default());
            c
        }
        Some(Err(e)) => {
            eprintln!("The tier chosen in the dashboard can't be read ({e}); serving {} instead.", card.display());
            read_card(&home.join(card)).or_else(|_| read_card(card))?
        }
        None => read_card(&home.join(card)).or_else(|_| read_card(card))?,
    };
    Ok(vec![card])
}

struct ServeOpts {
    network: Option<String>,
    creator: Option<String>,
    no_fee: bool,
    cross_check: Option<String>,
}

/// A stand-in card for a new site with no membership yet: v4 settings, a covenant id that can
/// never appear on chain, and the creator who may launch the first real one.
fn placeholder_card(network: &str, creator: &str, start: Hash) -> Result<indexer::TierCard> {
    let a: Address = creator.try_into().map_err(|_| anyhow!("--creator is not a valid Kaspa address"))?;
    let want = if network == "mainnet" { Prefix::Mainnet } else { Prefix::Testnet };
    anyhow::ensure!(a.prefix == want && a.version == Version::PubKey, "--creator must be a regular {network} wallet address");
    Ok(indexer::TierCard {
        kaspass: 5,
        network: network.to_string(),
        covenant_id: Hash::default().to_string(),
        creator_address: creator.to_string(),
        settings: Tier {
            period_ms: 30 * 24 * 60 * MINUTE,
            trial_ms: 0,
            resellable: false,
            royalty: 0,
            bond: SOMPI,
            max_ahead_ms: 0,
            fee: Some(Fee { address: creator.to_string(), bps: 0, min: 0 }),
            // A v5 shape with no real owner record: nothing is ever sold from a placeholder.
            v4: Some(V4 { auto_window_ms: 3 * 24 * 60 * MINUTE, voucher_key_hash: Hash::default().to_string(), max_discount_bps: 0, record_id: Some(Hash::default().to_string()), self_trial: false }),
        },
        start_hash: start.to_string(),
        launch_txid: String::new(),
        known_lane: None,
        placeholder: true,
    })
}

async fn run_serve(cli: &Cli, card: &Path, port: u16, fee_address: Option<String>, opts: ServeOpts) -> Result<()> {
    let cards = match cards_to_serve(&cli.home, card) {
        Ok(c) => Some(c),
        Err(e) if opts.creator.is_some() => {
            println!("No membership here yet ({e}); serving an empty site for the creator to launch one.");
            None
        }
        Err(e) => return Err(e),
    };
    let net = match (&cards, &opts.network) {
        (Some(c), Some(n)) if &c[0].network != n => bail!("--network {n} doesn't match the membership's network ({})", c[0].network),
        (Some(c), _) => c[0].network.clone(),
        (None, Some(n)) => n.clone(),
        (None, None) => bail!("a new site needs --network (mainnet or testnet-10) and --creator"),
    };
    check_network(&net)?;
    let chain = Chain::connect_to(cli.node.clone(), &net).await?;
    // Every setting from the environment, said once at start (test-only ones are ignored on mainnet).
    let mut set: Vec<String> = std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("KASPASS_")).collect();
    set.sort();
    for k in set {
        println!("setting: {k} is set{}", if is_mainnet() && !SETTINGS.contains(&k.as_str()) { " (a test switch: ignored on mainnet)" } else { "" });
    }
    let main_url = match &chain {
        Chain::Live(c) => c.url().unwrap_or_default(),
        Chain::Sim(_) => String::new(),
    };
    let cards = match cards {
        Some(c) => c,
        None => vec![placeholder_card(&net, opts.creator.as_deref().unwrap_or_default(), chain.sink().await?)?],
    };
    if opts.no_fee {
        println!("Memberships launched from this dashboard carry no KasPass fee.");
    }
    let second = match opts.cross_check.as_deref() {
        Some("off") => {
            println!("Second-node pass check is OFF (--cross-check off).");
            None
        }
        url => match connect_second(url.map(str::to_string), &net, &main_url).await {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("Second-node pass check: no second node yet ({e:#}); new passes wait until one answers.");
                None
            }
        },
    };
    let members = std::fs::read_to_string(cli.home.join("members.html")).unwrap_or_else(|_| api::DEFAULT_MEMBERS_HTML.to_string());
    if let Some(f) = &fee_address {
        Fee { address: f.clone(), bps: DEFAULT_FEE_BPS, min: 0 }.spk()?;
    }
    // The server's own keys, created here on first start and never leaving this folder:
    // the voucher key approves holder discounts (only on tiers launched to trust it), and the
    // keeper wallet pays the small network fee when it auto-renews a pass from its balance.
    let voucher = load_wallet(&cli.home, "voucher")?;
    let keeper = load_wallet(&cli.home, "keeper")?;
    // v5: replacement voucher keys from "Replace server key", oldest first.
    let mut ring = Vec::new();
    for n in 2.. {
        let name = format!("voucher-{n}");
        if !cli.home.join("keys").join(format!("{name}.key")).exists() {
            break;
        }
        ring.push(load_wallet(&cli.home, &name)?);
    }
    let voucher_ring = std::sync::Arc::new(std::sync::RwLock::new(ring));
    let mut setups = Vec::new();
    for card in cards {
        let mut ix = indexer::Indexer::new(card)?;
        // An older tier card may not record the sales lane; the creator's state.json can, if it's here.
        if let Ok(st) = State::load(&cli.home) {
            if st.covenant_id.as_deref() == Some(ix.card.covenant_id.as_str()) {
                if let (Some(m), Some(lane)) = (&st.minter, st.lane) {
                    ix.seed_lane(&indexer::KnownLane { txid: m.txid.clone(), index: m.index, lane });
                }
            }
        }
        let mut shop = shop::Shop::open(&cli.home, &ix.card);
        // The card's own fee address is the default (Shop::open); --fee-address only overrides it.
        if fee_address.is_some() || ix.card.placeholder {
            shop.fee_address = fee_address.clone();
        }
        shop.no_fee = opts.no_fee;
        if let Some(v) = &ix.card.settings.v4 {
            println!("v4 tier {}: auto-renew and commitment terms are ON. Keeper wallet (fund it with a little {} for network fees):\n  {}", &ix.card.covenant_id[..12.min(ix.card.covenant_id.len())], if is_mainnet() { "KAS" } else { "test KAS" }, keeper.addr);
            if v.max_discount_bps == 0 {
                println!("  Holder discounts: off for this tier.");
            } else if v.voucher_hash().ok() == Some(voucher.id) {
                println!("  Holder discounts: ON (up to {}%), approved by this server's voucher key.", v.max_discount_bps as f64 / 100.0);
            } else {
                println!("  Holder discounts: OFF. This tier trusts a different voucher key than this server's.");
            }
        }
        shop.voucher = Some(voucher.clone());
        shop.voucher_ring = voucher_ring.clone();
        shop.keeper = Some(keeper.clone());
        let cache = (!ix.card.placeholder).then(|| cache_path(&cli.home, &ix.card));
        setups.push(api::LevelSetup { ix, shop: Some(shop), cache });
    }
    // A bad bot or club setting turns that feature off; it never stops the website.
    let bot = telegram::Bot::open(&cli.home).await.unwrap_or_else(|e| {
        eprintln!("Telegram bot is OFF: {e}");
        None
    });
    match &bot {
        Some(b) => println!("Telegram bot @{} is ON. Add it to your private group as an admin (Invite users + Ban users).", b.username),
        None => println!("Telegram bot is off (put a bot token in telegram.token to turn it on)."),
    }
    let discord = discord::Discord::open(&cli.home).await.unwrap_or_else(|e| {
        eprintln!("Discord is OFF: {e}");
        None
    });
    let club = clubs::Club::load(&cli.home).unwrap_or_else(|e| {
        eprintln!("Holder club is OFF: {e}");
        None
    });
    if let Some(c) = &club {
        println!("Holder club \"{}\" is ON ({} {}, {} tiers).", c.rules.club, c.rules.collection.network, c.rules.collection.tick, c.tiers.len());
    }
    match &discord {
        Some(d) => println!("Discord bot {} is ON. Add it to your server: {}", d.bot_name, d.invite_bot_url()),
        None => println!("Discord is off (put bot_token and client_secret in discord.json to turn it on)."),
    }
    prices::start(cli.home.clone(), false);
    api::serve(setups, chain, port, members, bot, discord, club, cli.home.clone(), api::SecondNode { chain: second, net: net.clone(), url: opts.cross_check.clone(), main_url }).await
}

/// A second node for confirming passes: the one given, or a public node other than the main one.
pub(crate) async fn connect_second(url: Option<String>, net: &str, main_url: &str) -> Result<Chain> {
    for _ in 0..6 {
        let c = Chain::connect_to(url.clone(), net).await?;
        let u = match &c {
            Chain::Live(c) => c.url().unwrap_or_default(),
            Chain::Sim(_) => String::new(),
        };
        if u != main_url {
            println!("Second-node pass check is ON: every pass is confirmed on {u} too.");
            return Ok(c);
        }
        if url.is_some() {
            bail!("--cross-check points at the main node itself");
        }
        if let Chain::Live(c) = c {
            let _ = c.disconnect().await;
        }
    }
    bail!("the public node list kept giving the main node")
}

// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::Simulate = cli.cmd {
        return simulate().await;
    }
    if let Cmd::Serve { card, port, fee_address, allow_return, csp, creator_only_launch, network, creator, no_fee, cross_check } = &cli.cmd {
        api::set_return_origins(allow_return)?;
        api::set_csp_mode(csp)?;
        api::set_creator_only_launch(*creator_only_launch);
        if *creator_only_launch {
            println!("Launching new levels: creator wallet only.");
        }
        let opts = ServeOpts { network: network.clone(), creator: creator.clone(), no_fee: *no_fee, cross_check: cross_check.clone() };
        return run_serve(&cli, card, *port, fee_address.clone(), opts).await;
    }
    std::fs::create_dir_all(&cli.home)?;
    let cx = Ctx {
        creator: load_wallet(&cli.home, "creator")?,
        alice: load_wallet(&cli.home, "alice")?,
        bob: load_wallet(&cli.home, "bob")?,
        fee: load_wallet(&cli.home, "fee")?,
        home: cli.home.clone(),
    };
    if let Cmd::Keys = cli.cmd {
        println!("Test wallets (keys are in {}). TESTNET ONLY, never send real KAS here.\n", cli.home.join("keys").display());
        for w in [&cx.creator, &cx.alice, &cx.bob, &cx.fee] {
            println!("{:8} {}", w.name, w.addr);
        }
        println!("\n(\"fee\" stands in for the KasPass fee address on testnet; it receives the 2% protocol fee.)");
        println!("\nGet test KAS for creator and alice from the Testnet-10 faucet (faucet-testnet.kaspanet.io).");
        return Ok(());
    }
    if let Cmd::Name { address, label } = &cli.cmd {
        return op_name(&cx, address, &label.join(" "));
    }
    let chain = Chain::connect(cli.node.clone()).await?;
    match cli.cmd {
        Cmd::Scan { card } => {
            let card = read_card(&card)?;
            let cache = cache_path(&cli.home, &card);
            let (ix, passes, now) = run_scan_cached(card, &chain, Some(&cache)).await?;
            indexer::print_passes(&ix, &passes, now, &known_names(&cx));
            if ix.unmatched > 0 {
                println!("\n  note: {} spend(s) didn't match a known pass shape and were ignored", ix.unmatched);
            }
        }
        Cmd::Check { address, card } => {
            let owner = indexer::owner_hash_of(&address)?.to_string();
            let card = read_card(&card)?;
            let cache = cache_path(&cli.home, &card);
            let (_, passes, now) = run_scan_cached(card, &chain, Some(&cache)).await?;
            let mine: Vec<_> = passes.iter().filter(|p| p.owner_hash == owner).collect();
            let active = mine.iter().any(|p| p.active);
            println!("{address}\n  {}", if active { "ACTIVE: holds a valid pass (unlock)" } else { "NOT ACTIVE: no valid pass (lock)" });
            for p in mine {
                let exp = if p.lifetime { "lifetime".to_string() } else { format!("expires {}", when(p.expiry_ms, now)) };
                println!("  {} pass, {exp}, {}:{}", p.kind, &p.txid[..12], p.index);
            }
        }
        Cmd::Name { .. } | Cmd::Keys | Cmd::Simulate | Cmd::Serve { .. } => unreachable!(),
    }
    Ok(())
}

#[cfg(test)]
mod contract_compile {
    #[test]
    fn payin_compiles() {
        let spk = super::pay_to_script_hash_script(&[0x51]);
        super::payin(super::Hash::default(), super::Hash::default(), super::Hash::default(), &spk, 1, 1_800_000_000_000, 3).expect("pay-in contract compiles");
    }
}
