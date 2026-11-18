//! Fable's review of update 89: renewals, top-ups and sales while the block reader waits for
//! confirmations, a same-id copy of a purchase, and a restart (these showed the problems in 89 as
//! first built; they now show the fixes).
//! They use the simulator; the Live block reader hands a transaction to the indexer only once its
//! chain block has more than CHAIN_CONFIRMATIONS (30) confirmations, and that delay is modelled
//! here by not scanning the newest transaction yet.

use super::*;

fn world(tag: &str) -> (Ctx, Chain) {
    let home = std::env::temp_dir().join(format!("kaspass-review89-{tag}-{}", std::process::id()));
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

async fn tier(cx: &Ctx, chain: &mut Chain, st: &mut State, remaining: i64) -> Result<()> {
    let voucher = wallet_from_secret("voucher", SecretKey::from_slice(&[0x77; 32])?);
    let recovery = wallet_from_secret("w66", SecretKey::from_slice(&[0x66; 32])?);
    let rid = sim_owner_record(cx, chain, &cx.home, &recovery).await?;
    let fee = Fee { address: cx.fee.addr.to_string(), bps: DEFAULT_FEE_BPS, min: SOMPI / 10 };
    let v5 = V4 { auto_window_ms: MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: shop::V5_MAX_DISCOUNT_BPS, record_id: Some(rid.to_string()), self_trial: false };
    let tier = Tier { period_ms: 10 * MINUTE, trial_ms: 0, resellable: false, royalty: 0, bond: SOMPI, max_ahead_ms: 20 * MINUTE, fee: Some(fee), v4: Some(v5) };
    let lane = Lane { voucher: None, price: 2 * SOMPI, life: 0, credit: false, remaining, term_price: 0, term_ms: 0 };
    op_launch(cx, chain, st, tier, lane).await
}

/// An accepted transaction: the coin set changes now. (On a live node the block reader hands it
/// to the indexer about three seconds later; a test models that by scanning later.)
async fn accept(chain: &mut Chain, tx: &Transaction, entries: &[UtxoEntry]) -> Result<Hash> {
    chain.submit(tx, entries).await
}

/// R1. A member renews. From the moment the renewal is accepted until the block reader hands it
/// over (about three seconds on a live node), the old coin is spent and the new one isn't known
/// yet. With the fix the pass keeps showing as it was last known, and the new coin takes over
/// when the reader hands the renewal over.
#[tokio::test]
async fn audit89_r1_pass_keeps_showing_after_a_coin_move_until_the_reader_catches_up() -> Result<()> {
    let (cx, mut chain) = world("r1");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    assert_eq!(ix.verified(&chain, now).await?.len(), 1, "alice is a member");

    // Alice renews; the network accepts it. The reader hasn't reached it yet.
    op_renew(&cx, &mut chain, &mut st, true).await?;
    let old = ix.verified(&chain, now).await?.remove(0);
    let seen_by_site = ix.verified(&chain, now + 1_000).await?;
    assert_eq!(seen_by_site.len(), 1, "FIXED: the pass keeps showing while the reader catches up");
    assert!(seen_by_site[0].held && seen_by_site[0].txid == old.txid && seen_by_site[0].value_sompi == old.value_sompi);

    // Once the reader hands the renewal over, the new coin takes over in the same step: one pass.
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let back = ix.verified(&chain, now + 6_000).await?;
    assert_eq!(back.len(), 1);
    assert!(!back[0].held);
    assert_eq!(back[0].txid, st.membership.as_ref().unwrap().at.txid);

    // A pass that is closed stops showing as soon as the reader hands the close over...
    op_close(&cx, &mut chain, &mut st, "alice").await?;
    assert_eq!(ix.verified(&chain, now + 7_000).await?.len(), 1, "held for the moment");
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    assert!(ix.verified(&chain, now + 8_000).await?.is_empty(), "gone once its close is read");

    // ...and a coin that vanished without any spend being read is given up after a minute.
    let mut st2 = State::default();
    let (cx2, mut chain2) = world("r1b");
    tier(&cx2, &mut chain2, &mut st2, -1).await?;
    op_sale(&cx2, &mut chain2, &mut st2, Sale::Monthly, false).await?;
    let card2 = make_card(&cx2, &st2)?;
    let mut ix2 = indexer::Indexer::new(card2.clone())?;
    let mut c2 = Some(card2.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix2, &chain2, &mut c2).await?;
    let p = ix2.verified(&chain2, now).await?.remove(0);
    if let Chain::Sim(s) = &mut chain2 {
        s.utxos.remove(&TransactionOutpoint::new(p.txid.parse().unwrap(), p.index)).unwrap();
    }
    assert_eq!(ix2.verified(&chain2, now + 30_000).await?.len(), 1);
    assert!(ix2.verified(&chain2, now + 61_000).await?.is_empty());
    Ok(())
}

/// R2. After a sale the lane coin moves. With the fix the lane it became is found where a sale
/// puts it (the same address for an unlimited plan, the "one fewer left" address for a limited
/// one) without waiting for the reader.
#[tokio::test]
async fn audit89_r2_the_lane_is_found_after_a_sale_before_the_reader_catches_up() -> Result<()> {
    // A plan with 5 passes.
    let (cx, mut chain) = world("r2a");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, 5).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1);
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    let l = ix.verified_lanes(&chain).await?;
    assert_eq!(l.len(), 1, "FIXED: the lane the sale made is found at its 'one fewer left' address");
    assert_eq!((l[0].txid.clone(), l[0].lane.remaining), (st.minter.as_ref().unwrap().txid.clone(), 4));
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1);
    // ...and the same again after the next sale: the "recent lanes" search can't help, because
    // each sale moves a limited lane to a new address.
    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.bob.id, "bob", Payer::Key(cx.bob.clone()), None).await?;
    accept(&mut chain, &b.tx, &b.entries).await?;
    let l = ix.verified_lanes(&chain).await?;
    assert_eq!((l.len(), l[0].txid.clone(), l[0].lane.remaining), (1, b.tx.id().to_string(), 3), "FIXED: and after every sale");
    // Two more sales before the reader catches up: still found (a few steps are tried).
    st.minter = Some(Live { txid: b.tx.id().to_string(), index: 0, value: b.tx.outputs[0].value });
    st.lane = Some(b.next_lane);
    let c = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.creator.id, "creator", Payer::Key(cx.creator.clone()), None).await?;
    accept(&mut chain, &c.tx, &c.entries).await?;
    let l = ix.verified_lanes(&chain).await?;
    assert_eq!((l.len(), l[0].txid.clone(), l[0].lane.remaining), (1, c.tx.id().to_string(), 2));
    // The reader catches up: the same single lane.
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let l = ix.verified_lanes(&chain).await?;
    assert_eq!((l.len(), l[0].txid.clone(), l[0].lane.remaining), (1, c.tx.id().to_string(), 2));

    // An unlimited plan, first sale after a (re)start: no lane either.
    let (cx, mut chain) = world("r2b");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1, "FIXED: unlimited plan, first sale since start: found at the same address");
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1);
    // Second sale in the same run: bridged by the search at the last lane's address.
    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.bob.id, "bob", Payer::Key(cx.bob.clone()), None).await?;
    accept(&mut chain, &b.tx, &b.entries).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1, "unlimited plan, later sales: found at the same address");
    Ok(())
}

/// R3. A transaction's id does not cover its unlock data, so a block can carry a copy with the
/// same id that can't be read (only a miner can put an invalid copy in a block). With the fix
/// the reader hands over every different body it found for an accepted id, and an id only counts
/// as handled once a body was understood: the real purchase is read.
#[tokio::test]
async fn audit89_r3_a_same_id_copy_with_other_unlock_data_does_not_hide_a_real_purchase() -> Result<()> {
    for (tag, remaining) in [("r3a", -1i64), ("r3b", 5)] {
        let (cx, mut chain) = world(tag);
        let mut st = State::default();
        tier(&cx, &mut chain, &mut st, remaining).await?;
        let card = make_card(&cx, &st)?;
        let mut ix = indexer::Indexer::new(card.clone())?;
        let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
        indexer::scan(&mut ix, &chain, &mut cursor).await?;
        assert_eq!(ix.verified_lanes(&chain).await?.len(), 1);

        let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.alice.id, "alice", Payer::Key(cx.alice.clone()), None).await?;
        let mut copy = b.tx.clone();
        for i in copy.inputs.iter_mut() {
            i.signature_script = vec![0x51];
        }
        assert_eq!(copy.id(), b.tx.id(), "same id, different unlock data");
        // The real purchase is accepted on chain.
        let t = chain.now_ms().await?;
        if let Chain::Sim(s) = &mut chain {
            for inp in &b.tx.inputs {
                s.utxos.remove(&inp.previous_outpoint).unwrap();
            }
            for (i, o) in b.tx.outputs.iter().enumerate() {
                s.utxos.insert(TransactionOutpoint::new(b.tx.id(), i as u32), UtxoEntry::new(o.value, o.script_public_key.clone(), 0, false, o.covenant.map(|c| c.covenant_id)));
            }
        }
        // The reader hands over the copy's body for the accepted id (and, if it also came across
        // the real body, that one after it).
        ix.process_at(&copy, t);
        ix.process_at(&b.tx, t);
        let passes = ix.verified(&chain, t + 11 * 60_000).await?;
        assert_eq!(passes.len(), 1, "FIXED: the copy that can't be read doesn't make the real one be skipped");
        assert_eq!(ix.events().iter().filter(|e| e.txid == b.tx.id().to_string()).count(), 1, "one payment");
        let lanes = ix.verified_lanes(&chain).await?;
        assert_eq!(lanes.len(), 1, "the lane is known (limited or not)");
        assert_eq!(lanes[0].txid, b.tx.id().to_string());
        // Reading the same transaction again changes nothing.
        ix.process_at(&b.tx, t);
        ix.process_at(&copy, t);
        assert_eq!(ix.events().iter().filter(|e| e.txid == b.tx.id().to_string()).count(), 1);
    }
    Ok(())
}

/// R4. With the fix the 10-minute wait for a new pass coin is saved with the scan progress, so
/// it survives a restart (an upgrade, a crash, the watchdog).
#[tokio::test]
async fn audit89_r4_the_wait_for_a_new_pass_survives_a_restart() -> Result<()> {
    let (cx, mut chain) = world("r4");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.alice.id, "alice", Payer::Key(cx.alice.clone()), None).await?;
    // Accepted, read by the reader, but this node's coin list doesn't show it yet.
    if let Chain::Sim(s) = &mut chain {
        s.blocks.push(b.tx.clone());
        s.block_times.push(s.now);
    }
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    assert!(ix.verified(&chain, now).await?.is_empty());
    let file = cx.home.join("scan-cache-r4.json");
    ix.save_cache(&file, cursor.unwrap())?;

    // Restart: saved progress is loaded, the coin list is still a moment behind.
    let mut again = indexer::Indexer::new(card)?;
    let mut c2 = again.load_cache(&file);
    indexer::scan(&mut again, &chain, &mut c2).await?;
    assert!(again.verified(&chain, now + 2_000).await?.is_empty());
    // The coin list catches up a moment later.
    if let Chain::Sim(s) = &mut chain {
        for inp in &b.tx.inputs {
            s.utxos.remove(&inp.previous_outpoint).unwrap();
        }
        for (i, o) in b.tx.outputs.iter().enumerate() {
            s.utxos.insert(TransactionOutpoint::new(b.tx.id(), i as u32), UtxoEntry::new(o.value, o.script_public_key.clone(), 0, false, o.covenant.map(|c| c.covenant_id)));
        }
    }
    assert_eq!(again.verified(&chain, now + 4_000).await?.len(), 1, "FIXED: the wait survives a restart; the pass shows once the coin is there");
    Ok(())
}

/// R5 (a good result). Reading transactions again after a restart or an upgrade from update 88's
/// cursor (the indexer forgets which ids it handled) does not count a payment twice, and the
/// lanes and passes come out the same.
#[tokio::test]
async fn audit89_r5_reading_again_after_a_restart_does_not_double_count() -> Result<()> {
    let (cx, mut chain) = world("r5");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    op_renew(&cx, &mut chain, &mut st, true).await?;
    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.bob.id, "bob", Payer::Key(cx.bob.clone()), None).await?;
    accept(&mut chain, &b.tx, &b.entries).await?;
    let card = make_card(&cx, &st)?;
    let start: Hash = card.start_hash.parse().unwrap();
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(start);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    let passes = ix.verified(&chain, now).await?;
    let lanes = ix.verified_lanes(&chain).await?;
    let events = ix.events().to_vec();
    assert_eq!((passes.len(), lanes.len(), events.len()), (2, 1, 3));

    // Saved with an older cursor (as after an upgrade from 88, or a crash before saving).
    let file = cx.home.join("scan-cache-r5.json");
    ix.save_cache(&file, start)?;
    let mut again = indexer::Indexer::new(card)?;
    let mut c2 = again.load_cache(&file);
    indexer::scan(&mut again, &chain, &mut c2).await?;
    let passes2 = again.verified(&chain, now).await?;
    let lanes2 = again.verified_lanes(&chain).await?;
    assert_eq!(again.events(), &events[..], "no payment counted twice");
    assert_eq!(passes2.iter().map(|p| (p.txid.clone(), p.index)).collect::<Vec<_>>(), passes.iter().map(|p| (p.txid.clone(), p.index)).collect::<Vec<_>>());
    assert_eq!(lanes2.iter().map(|l| (l.txid.clone(), l.index)).collect::<Vec<_>>(), lanes.iter().map(|l| (l.txid.clone(), l.index)).collect::<Vec<_>>());
    // Only the "scanned transactions" figure counts them again (shown on /v1/health; harmless).
    assert!(again.scanned_txs > ix.scanned_txs);
    Ok(())
}

/// R1b. Anyone can add to a pass's balance. A stranger who does it again and again, each time
/// before the reader has caught up, must not make the member's pass vanish from the site.
#[tokio::test]
async fn audit89_r1b_a_stranger_topping_up_again_and_again_does_not_hide_the_pass() -> Result<()> {
    let (cx, mut chain) = world("r1c");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    let first = ix.verified(&chain, now).await?.remove(0);
    let t = st.tier.clone().unwrap();
    let cov = cov_id(&st)?;
    let mem = st.membership.clone().unwrap();
    let mut at = mem.at.clone();
    let mut sent: Vec<Transaction> = vec![];
    // Bob adds 0.01 KAS four times; the reader is always one transaction behind.
    for round in 0..4i64 {
        let (tx, entries, _, value) = deposit_tx(&cx.creator.c(), &chain, &t, cov, &at, cx.alice.id, &mem.pass, SOMPI / 100, Payer::Key(cx.bob.clone())).await?;
        accept(&mut chain, &tx, &entries).await?;
        at = Live { txid: tx.id().to_string(), index: 0, value };
        if let Some(prev) = sent.last() {
            ix.process_at(prev, now);
        }
        sent.push(tx);
        let seen = ix.verified(&chain, now + 3_000 * (round + 1)).await?;
        assert_eq!(seen.len(), 1, "alice's pass shows in round {round}");
        assert!(seen[0].held, "as it was last known");
        assert_eq!((seen[0].owner_hash.clone(), seen[0].expiry_ms), (first.owner_hash.clone(), first.expiry_ms), "same owner, same end date");
        if round > 0 {
            assert_eq!(seen[0].same_as, Some((first.txid.clone(), first.index)), "the same pass as the coin that was seen");
        }
    }
    // Bob stops; the reader catches up: the pass is the last coin, in the coin list.
    ix.process_at(sent.last().unwrap(), now);
    let seen = ix.verified(&chain, now + 20_000).await?;
    assert_eq!(seen.len(), 1);
    assert!(!seen[0].held);
    assert_eq!(seen[0].txid, at.txid);
    assert_eq!(seen[0].balance_sompi, 4 * (SOMPI / 100));
    Ok(())
}

/// R6 (small). When a renewal is read a second time after a restart (the saved cursor was behind
/// it) and the coin it replaced is no longer known, the pass keeps its "bought by" address.
#[tokio::test]
async fn audit89_r6_reading_a_renewal_again_keeps_who_bought_the_pass() -> Result<()> {
    let (cx, mut chain) = world("r6");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    let after_sale = chain.sink().await?;
    op_renew(&cx, &mut chain, &mut st, true).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let now = chain.now_ms().await?;
    let p = ix.verified(&chain, now).await?.remove(0);
    assert_eq!(p.bought_by.as_deref(), Some(cx.alice.addr.to_string().as_str()));
    assert!(p.owner_address.is_some());
    // Later (the spent first coin has been given up on), progress is saved with the cursor just
    // before the renewal; then a restart.
    let now = now + 11 * 60_000;
    assert_eq!(ix.verified(&chain, now).await?.len(), 1);
    let file = cx.home.join("scan-cache-r6.json");
    ix.save_cache(&file, after_sale)?;
    let mut again = indexer::Indexer::new(card)?;
    let mut c2 = again.load_cache(&file);
    indexer::scan(&mut again, &chain, &mut c2).await?;
    let p2 = again.verified(&chain, now).await?.remove(0);
    assert_eq!(p2.txid, p.txid);
    assert_eq!((p2.bought_by, p2.owner_address), (p.bought_by, p.owner_address), "FIXED: still known");
    Ok(())
}

/// R2b. An unlimited plan after the owner changes the price: with the fix the first sale at the
/// new price doesn't leave the site without a lane.
#[tokio::test]
async fn audit89_r2b_the_lane_is_found_after_the_first_sale_following_a_price_change() -> Result<()> {
    let (cx, mut chain) = world("r2c");
    let mut st = State::default();
    tier(&cx, &mut chain, &mut st, -1).await?;
    let card = make_card(&cx, &st)?;
    let mut ix = indexer::Indexer::new(card.clone())?;
    let mut cursor = Some(card.start_hash.parse::<Hash>().unwrap());
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    // A sale at the old price, read by the reader (so the recent-lanes list isn't empty).
    op_sale(&cx, &mut chain, &mut st, Sale::Monthly, false).await?;
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    assert_eq!(ix.verified_lanes(&chain).await?.len(), 1);
    // The owner raises the price; the reader reads that too.
    let rec: RecordInfo = serde_json::from_str(&std::fs::read_to_string(cx.home.join(shop::RECORD_FILE))?)?;
    let t = st.tier.clone().unwrap();
    let old = st.lane.unwrap();
    let (tx, entries, _, next) = set_prices_tx_v5(&chain, &cx.creator.addr, &rec, &t, cov_id(&st)?, old, st.minter.as_ref().unwrap(), Lane { price: 3 * SOMPI, ..old }).await?;
    accept(&mut chain, &tx, &entries).await?;
    st.minter = Some(Live { txid: tx.id().to_string(), index: 0, value: tx.outputs[0].value });
    st.lane = Some(next);
    indexer::scan(&mut ix, &chain, &mut cursor).await?;
    let l = ix.verified_lanes(&chain).await?;
    assert_eq!((l.len(), l[0].lane.price), (1, 3 * SOMPI));
    // The first sale at the new price: accepted, not yet handed over by the reader.
    let b = sale_tx(&cx.creator.c(), &chain, &st, Sale::Monthly, false, cx.bob.id, "bob", Payer::Key(cx.bob.clone()), None).await?;
    accept(&mut chain, &b.tx, &b.entries).await?;
    let l = ix.verified_lanes(&chain).await?;
    assert_eq!((l.len(), l[0].txid.clone(), l[0].lane.price), (1, b.tx.id().to_string(), 3 * SOMPI), "FIXED: found at the same address");
    Ok(())
}
