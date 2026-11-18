//! The block reader and the indexer against transactions that sit in a block but never take
//! effect, a node whose coin list is a moment behind, and a lost sales lane. (From Fable's
//! security review of update 88, finding M1: these tests showed the problem; they now show the fix.)

use super::*;

fn sim_world(tag: &str) -> (Ctx, Chain) {
    let home = std::env::temp_dir().join(format!("kaspass-chain-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let cx = Ctx {
        creator: wallet_from_secret("creator", SecretKey::from_slice(&[0x11; 32]).unwrap()),
        alice: wallet_from_secret("alice", SecretKey::from_slice(&[0x22; 32]).unwrap()),
        bob: wallet_from_secret("bob", SecretKey::from_slice(&[0x33; 32]).unwrap()),
        fee: wallet_from_secret("fee", SecretKey::from_slice(&[0x44; 32]).unwrap()),
        home,
    };
    let mut utxos = HashMap::new();
    for (i, w) in [&cx.creator, &cx.alice, &cx.bob].into_iter().enumerate() {
        utxos.insert(TransactionOutpoint::new(Hash::from_bytes([i as u8 + 1; 32]), 0), UtxoEntry::new(1_000 * SOMPI, w.spk.clone(), 0, false, None));
    }
    (cx, Chain::Sim(Sim { utxos, now: 1_800_000_000_000, blocks: vec![], block_times: vec![], clock: None, rejected: Default::default() }))
}

/// A block the node hands out, holding a transaction that was never accepted (the coin set does
/// not change, and the node doesn't list it as accepted).
fn mined_but_not_accepted(chain: &mut Chain, tx: &Transaction) {
    if let Chain::Sim(s) = chain {
        s.blocks.push(tx.clone());
        s.block_times.push(s.now);
        s.rejected.insert(tx.id());
    }
}

/// An accepted transaction whose coins the node's coin list doesn't show yet (a node a moment behind).
fn accepted_but_coin_list_behind(chain: &mut Chain, tx: &Transaction) {
    if let Chain::Sim(s) = chain {
        s.blocks.push(tx.clone());
        s.block_times.push(s.now);
    }
}

/// The coin list catches up with `tx`.
fn coin_list_catches_up(chain: &mut Chain, tx: &Transaction) {
    let Chain::Sim(sim) = chain else { unreachable!() };
    for inp in &tx.inputs {
        sim.utxos.remove(&inp.previous_outpoint).expect("input was unspent");
    }
    for (i, o) in tx.outputs.iter().enumerate() {
        sim.utxos.insert(TransactionOutpoint::new(tx.id(), i as u32), UtxoEntry::new(o.value, o.script_public_key.clone(), 0, false, o.covenant.map(|c| c.covenant_id)));
    }
}

async fn v5_tier(cx: &Ctx, chain: &mut Chain, st: &mut State) -> Result<()> {
    let voucher = wallet_from_secret("voucher", SecretKey::from_slice(&[0x77; 32])?);
    let recovery = wallet_from_secret("w66", SecretKey::from_slice(&[0x66; 32])?);
    let rid = sim_owner_record(cx, chain, &cx.home, &recovery).await?;
    let fee = Fee { address: cx.fee.addr.to_string(), bps: DEFAULT_FEE_BPS, min: SOMPI / 10 };
    let v5 = V4 { auto_window_ms: MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: shop::V5_MAX_DISCOUNT_BPS, record_id: Some(rid.to_string()), self_trial: false };
    let tier = Tier { period_ms: 10 * MINUTE, trial_ms: 0, resellable: false, royalty: 0, bond: SOMPI, max_ahead_ms: 20 * MINUTE, fee: Some(fee), v4: Some(v5) };
    let lane = Lane { voucher: None, price: 2 * SOMPI, life: 0, credit: false, remaining: -1, term_price: 0, term_ms: 0 };
    op_launch(cx, chain, st, tier, lane).await
}

/// An unaccepted purchase (its funding coin was spent by a competing transaction first) changes
/// nothing: the lane stays, and no sale is logged.
#[tokio::test]
async fn unaccepted_purchase_changes_nothing() -> Result<()> {
    let (cx, mut chain) = sim_world("unaccepted");
    let mut st = State::default();
    v5_tier(&cx, &mut chain, &mut st).await?;
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    let card = make_card(&cx, &st)?;
    let start: Hash = card.start_hash.parse().unwrap();
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(start);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let lanes = ix.verified_lanes(&chain).await?;
    assert_eq!(lanes.len(), 1);
    let real_lane = (lanes[0].txid.clone(), lanes[0].index);
    let events_before = ix.events().len();

    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.bob.id, "bob", Payer::Key(cx.bob.clone()), None).await?;
    mined_but_not_accepted(&mut chain, &b.tx);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let after = ix.verified_lanes(&chain).await?;
    assert_eq!(after.iter().map(|l| (l.txid.clone(), l.index)).collect::<Vec<_>>(), vec![real_lane.clone()], "the lane is still known");
    assert_eq!(ix.events().len(), events_before, "no sale that never happened");

    // A full rescan agrees.
    let mut ix2 = indexer::Indexer::new(card)?;
    let mut c2 = Some(start);
    indexer::scan(&mut ix2, &chain, &mut c2).await?;
    assert_eq!(ix2.verified_lanes(&chain).await?.len(), 1);
    assert_eq!(ix2.events().len(), events_before);
    Ok(())
}

/// A throwaway transaction that only names the lane coin as an input (a miner could put one in a
/// block; it can never be accepted) doesn't remove the lane.
#[tokio::test]
async fn junk_transaction_naming_the_lane_is_ignored() -> Result<()> {
    let (cx, mut chain) = sim_world("junk");
    let mut st = State::default();
    v5_tier(&cx, &mut chain, &mut st).await?;
    let card = make_card(&cx, &st)?;
    let start: Hash = card.start_hash.parse().unwrap();
    let mut ix = indexer::Indexer::new(card)?;
    let mut cursor = Some(start);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let lanes = ix.verified_lanes(&chain).await?;
    assert_eq!(lanes.len(), 1);
    let op = TransactionOutpoint::new(lanes[0].txid.parse().unwrap(), lanes[0].index);
    let mut junk = Transaction::new_non_finalized(
        TX_VERSION,
        vec![TransactionInput::new_with_compute_budget(op, vec![], 0, 0)],
        vec![TransactionOutput { value: 1, script_public_key: cx.bob.spk.clone(), covenant: None }],
        0,
        Default::default(),
        0,
        vec![],
    );
    junk.finalize();
    mined_but_not_accepted(&mut chain, &junk);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1, "the lane is kept");
    Ok(())
}

/// A pass the node's coin list doesn't show yet is waited for, and shows once the list catches up.
#[tokio::test]
async fn new_pass_is_waited_for_while_the_coin_list_is_behind() -> Result<()> {
    let (cx, mut chain) = sim_world("behind");
    let mut st = State::default();
    v5_tier(&cx, &mut chain, &mut st).await?;
    let card = make_card(&cx, &st)?;
    let start: Hash = card.start_hash.parse().unwrap();
    let mut ix = indexer::Indexer::new(card)?;
    let mut cursor = Some(start);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;

    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.alice.id, "alice", Payer::Key(cx.alice.clone()), None).await?;
    accepted_but_coin_list_behind(&mut chain, &b.tx);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    assert!(ix.verified(&chain, now).await?.is_empty(), "not shown while the coin isn't there");

    coin_list_catches_up(&mut chain, &b.tx);
    let passes = ix.verified(&chain, now + 60_000).await?;
    assert_eq!(passes.len(), 1, "alice's pass shows once the coin is there");
    assert_eq!(passes[0].txid, b.tx.id().to_string());

    // A coin that never shows is given up on after the grace period.
    let c = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.bob.id, "bob", Payer::Key(cx.bob.clone()), None).await?;
    accepted_but_coin_list_behind(&mut chain, &c.tx);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let later = now + 11 * 60_000;
    assert_eq!(ix.verified(&chain, later).await?.len(), 1, "only alice's");
    coin_list_catches_up(&mut chain, &c.tx);
    assert_eq!(ix.verified(&chain, later).await?.len(), 1, "bob's was given up on after 10 minutes");
    Ok(())
}

/// If the index ever loses the lane while its coin is still there, it's found again at its
/// address, and kept across a restart from saved progress.
#[tokio::test]
async fn lost_lane_is_found_again_at_its_address() -> Result<()> {
    let (cx, mut chain) = sim_world("refind");
    let mut st = State::default();
    v5_tier(&cx, &mut chain, &mut st).await?;
    let card = make_card(&cx, &st)?;
    let start: Hash = card.start_hash.parse().unwrap();
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(start);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let real = ix.verified_lanes(&chain).await?.remove(0);
    let op = TransactionOutpoint::new(real.txid.parse().unwrap(), real.index);
    // Make the index believe the lane was spent, while the coin stays (any mismatch between what
    // was read and the coin set).
    let mut junk = Transaction::new_non_finalized(
        TX_VERSION,
        vec![TransactionInput::new_with_compute_budget(op, vec![], 0, 0)],
        vec![TransactionOutput { value: 1, script_public_key: cx.bob.spk.clone(), covenant: None }],
        0,
        Default::default(),
        0,
        vec![],
    );
    junk.finalize();
    ix.process(&junk);
    let lanes = ix.verified_lanes(&chain).await?;
    assert_eq!(lanes.len(), 1, "found again");
    assert_eq!((lanes[0].txid.clone(), lanes[0].index), (real.txid.clone(), real.index));

    let file = cx.home.join("scan-cache-refind.json");
    ix.save_cache(&file, cursor.unwrap())?;
    let mut again = indexer::Indexer::new(indexer::TierCard { known_lane: None, ..card })?;
    let mut c2 = again.load_cache(&file);
    indexer::scan(&mut again, &chain, &mut c2).await?;
    assert_eq!(again.verified_lanes(&chain).await?.len(), 1, "kept after a restart");
    Ok(())
}

// ---- The live block reader's choice of what to read, against node answers it may give ----

fn tx_paying(v: u64, coinbase: bool) -> Transaction {
    let mut t = Transaction::new_non_finalized(
        TX_VERSION,
        vec![],
        vec![TransactionOutput { value: v, script_public_key: ScriptPublicKey::from_vec(0, vec![0x51]), covenant: None }],
        0,
        if coinbase { kaspa_consensus_core::subnets::SUBNETWORK_ID_COINBASE } else { Default::default() },
        0,
        vec![],
    );
    t.finalize();
    t
}

fn chain_hash(n: u8) -> Hash {
    Hash::from_bytes([n; 32])
}

/// Two confirmed chain blocks, everything in hand: both are read, coinbases are left out, and the
/// reader reports it's caught up (so the server's round ends even while new blocks keep coming).
#[test]
fn reader_reports_caught_up_once_everything_listed_is_read() {
    let (a, b) = (chain_hash(1), chain_hash(2));
    let (t1, t2, cb) = (tx_paying(1, false), tx_paying(2, false), tx_paying(3, true));
    let bodies: HashMap<TransactionId, Vec<(Transaction, i64)>> = [(t1.id(), vec![(t1.clone(), 10)]), (t2.id(), vec![(t2.clone(), 20)]), (cb.id(), vec![(cb.clone(), 20)])].into_iter().collect();
    let accepted = vec![(a, vec![cb.id(), t1.id()]), (b, vec![t2.id()])];
    let in_bulk = [a, b].into_iter().collect();
    let (txs, last, caught_up) = pick_accepted(chain_hash(0), &[a, b], &accepted, false, &in_bulk, &bodies).unwrap();
    assert_eq!(txs.iter().map(|(t, _)| t.id()).collect::<Vec<_>>(), vec![t1.id(), t2.id()]);
    assert_eq!(last, b);
    assert!(caught_up);
}

/// The node lists more chain blocks than the bulk read holds, or pages its accepted list shorter
/// than its chain list: the reader stops at the last complete block and says there's more.
#[test]
fn reader_stops_at_the_last_complete_block_and_says_more_is_coming() {
    let (a, b, c) = (chain_hash(1), chain_hash(2), chain_hash(3));
    let (t1, t2) = (tx_paying(1, false), tx_paying(2, false));
    let bodies: HashMap<TransactionId, Vec<(Transaction, i64)>> = [(t1.id(), vec![(t1.clone(), 10)])].into_iter().collect();
    // b's block isn't in the bulk read.
    let in_bulk = [a].into_iter().collect();
    let (txs, last, caught_up) = pick_accepted(chain_hash(0), &[a, b], &[(a, vec![t1.id()]), (b, vec![t2.id()])], false, &in_bulk, &bodies).unwrap();
    assert_eq!((txs.len(), last, caught_up), (1, a, false));
    // The accepted list covers fewer chain blocks than the chain list.
    let in_bulk = [a, b, c].into_iter().collect();
    let (_, last, caught_up) = pick_accepted(chain_hash(0), &[a, b, c], &[(a, vec![t1.id()])], false, &in_bulk, &bodies).unwrap();
    assert_eq!((last, caught_up), (a, false));
    // A body missing for the second block: stop before it.
    let (_, last, caught_up) = pick_accepted(chain_hash(0), &[a, b], &[(a, vec![t1.id()]), (b, vec![t2.id()])], false, &[a, b].into_iter().collect(), &bodies).unwrap();
    assert_eq!((last, caught_up), (a, false));
}

/// After the cursor fell off the chain only the first new chain block is read (its merge set was
/// fetched on its own); a node that lists chain blocks without accepted lists is an error, not a skip.
#[test]
fn reader_handles_an_off_chain_cursor_and_a_wrong_answer() {
    let (a, b) = (chain_hash(1), chain_hash(2));
    let (t1, t2) = (tx_paying(1, false), tx_paying(2, false));
    let bodies: HashMap<TransactionId, Vec<(Transaction, i64)>> = [(t1.id(), vec![(t1.clone(), 10)]), (t2.id(), vec![(t2.clone(), 20)])].into_iter().collect();
    let in_bulk = [a, b].into_iter().collect();
    let (txs, last, caught_up) = pick_accepted(chain_hash(0), &[a, b], &[(a, vec![t1.id()]), (b, vec![t2.id()])], true, &in_bulk, &bodies).unwrap();
    assert_eq!((txs.len(), last, caught_up), (1, a, false));
    assert!(pick_accepted(chain_hash(0), &[a, b], &[], false, &in_bulk, &bodies).is_err());
    // The first block's transaction isn't in hand even after its merge set was read: an error, so
    // the cursor doesn't move past a purchase it couldn't read.
    let t3 = tx_paying(3, false);
    assert!(pick_accepted(chain_hash(0), &[a], &[(a, vec![t3.id()])], false, &in_bulk, &bodies).is_err());
}
