//! Simulation checks for the v5 contracts: the owner record, the payout vault, creator
//! actions through the record, mandatory voucher on auto-renew, and handing the record to a
//! new owner with the recovery wallet. Every honest path must be accepted and every cheat
//! refused by the contracts themselves, on the same script engine a node runs.

use super::*;

/// Refused, and by the contract itself (the script engine), not by some other error.
fn refused<T>(label: &str, r: Result<T>) -> Result<()> {
    match r {
        Err(e) if format!("{e:#}").contains("rejected by script engine") => {
            println!("   refused as expected ({label})");
            Ok(())
        }
        Err(e) => bail!("{label}: failed for a reason other than the contract: {e:#}"),
        Ok(_) => bail!("{label}: should have been refused"),
    }
}

fn sim_utxo(chain: &Chain, op: TransactionOutpoint) -> Result<Utxo> {
    match chain {
        Chain::Sim(s) => s.utxos.get(&op).map(|e| Utxo { op, entry: e.clone() }).ok_or_else(|| anyhow!("utxo gone")),
        _ => bail!("sim only"),
    }
}

/// One plain coin of a wallet (no covenant), to prove the wallet signed this transaction.
async fn coin(chain: &Chain, w: &Wallet) -> Result<Utxo> {
    chain.utxos_at(&w.spk).await?.into_iter().filter(|u| u.entry.covenant_id.is_none()).max_by_key(|u| u.entry.amount).ok_or_else(|| anyhow!("{} has no coins", w.name))
}

fn fund(chain: &mut Chain, w: &Wallet, tag: u8) {
    if let Chain::Sim(s) = chain {
        s.utxos.insert(TransactionOutpoint::new(Hash::from_bytes([tag; 32]), 0), UtxoEntry::new(1_000 * SOMPI, w.spk.clone(), 0, false, None));
    }
}

fn set_now(chain: &mut Chain, t: i64) {
    if let Chain::Sim(s) = chain {
        s.now = t;
    }
}

#[derive(Clone, Copy)]
struct Rec {
    id: Hash,
    at: TransactionOutpoint,
    owner: Hash,
    recovery: Hash,
}

/// Spends the record with `use(wallet_in)` or `handover(wallet_in, next)`, and its next output.
fn record_spend(r: &Rec, entry: &'static str, wallet_in: usize, next: (Hash, Hash), out_auth: u16, chain: &Chain) -> Result<(Spend, TransactionOutput)> {
    let cur = owner_record(r.owner, r.recovery)?;
    let utxo = sim_utxo(chain, r.at)?;
    let out = cov_out(&owner_record(next.0, next.1)?, utxo.entry.amount, out_auth, r.id);
    let build: Box<dyn Fn(Vec<u8>) -> Result<Vec<u8>> + Send + Sync> = if entry == "use" {
        Box::new(move |_| cur.entry_sigscript("use", &[ArtifactValue::Int(wallet_in as i64)]))
    } else {
        Box::new(move |_| cur.entry_sigscript("handover", &[ArtifactValue::Int(wallet_in as i64), hb(next.0), hb(next.1)]))
    };
    Ok((Spend { utxo, unlock: Unlock::Script { signer: None, build } }, out))
}

struct W5 {
    tier: Tier,
    creator: Wallet,
    buyer: Wallet,
    keeper: Wallet,
    voucher: Wallet,
    cov: Hash,
    vault_spk: ScriptPublicKey,
}

type Built = (Transaction, Vec<UtxoEntry>);

impl W5 {
    async fn vault_balance(&self, chain: &Chain) -> Result<u64> {
        Ok(chain.utxos_at(&self.vault_spk).await?.iter().map(|u| u.entry.amount).sum())
    }

    /// BUY at `bps` off `lane.price`, signed by `signer` (None = no signature).
    async fn buy(&self, chain: &Chain, lane_at: TransactionOutpoint, lane: Lane, bps: i64, signer: Option<Wallet>, prepaid: u64) -> Result<(Built, Pass)> {
        let now = chain.now_ms().await?;
        let due = discounted_ppm(lane.price, bps as u64);
        let owner = self.buyer.id;
        let c = self.creator.c();
        let pass = Pass { voucher: lane.voucher, owner: String::new(), expiry: now + self.tier.period_ms, rate: lane.price, life: 0, credit: false, paid: due };
        let member = membership(&c, &self.tier, owner, &pass)?;
        let (pre, suf, _) = member.template();
        let m = minter(&c, &self.tier, &lane)?;
        let lane_utxo = sim_utxo(chain, lane_at)?;
        let vpk = self.voucher.pk.clone();
        let unlock = Unlock::Script {
            signer,
            build: Box::new(move |sig| {
                let s = if sig.is_empty() { vec![0u8; 65] } else { sig };
                m.entry_sigscript("buy", &[hb(owner), ArtifactValue::Int(now), ArtifactValue::Int(bps), ArtifactValue::Bytes(s), ArtifactValue::Bytes(vpk.clone()), ArtifactValue::Bytes(pre.clone()), ArtifactValue::Bytes(suf.clone())])
            }),
        };
        let next = minter(&c, &self.tier, &lane)?;
        let mut outs = vec![cov_out(&next, lane_utxo.entry.amount, 0, self.cov), cov_out(&member, self.tier.bond + prepaid, 0, self.cov)];
        outs.extend(payment_outs(&c, &self.tier, due)?);
        let (tx, entries, _) = build_tx(chain, vec![Spend { utxo: lane_utxo, unlock }], outs, Some(&self.buyer), now as u64, None, None).await?;
        Ok(((tx, entries), pass))
    }

    /// AUTORENEW from the pass balance at `bps` off its rate, signed by `signer`.
    async fn autorenew(&self, chain: &Chain, at: TransactionOutpoint, pass: &Pass, bps: i64, signer: Option<Wallet>) -> Result<(Built, Pass)> {
        let now = chain.now_ms().await?;
        let c = self.creator.c();
        let owner = self.buyer.id;
        let cur = membership(&c, &self.tier, owner, pass)?;
        let utxo = sim_utxo(chain, at)?;
        let due = discounted_ppm(pass.rate, bps as u64);
        let next_pass = Pass { expiry: pass.expiry.max(now) + self.tier.period_ms, paid: pass.paid + due, ..pass.clone() };
        let next = membership(&c, &self.tier, owner, &next_pass)?;
        let vpk = self.voucher.pk.clone();
        let unlock = Unlock::Script {
            signer,
            build: Box::new(move |sig| {
                let s = if sig.is_empty() { vec![0u8; 65] } else { sig };
                cur.decl_sigscript("autorenew", &[ArtifactValue::Int(now), ArtifactValue::Int(bps), ArtifactValue::Bytes(s), ArtifactValue::Bytes(vpk.clone())])
            }),
        };
        let mut outs = vec![cov_out(&next, utxo.entry.amount - due, 0, self.cov)];
        outs.extend(payment_outs(&c, &self.tier, due)?);
        let (tx, entries, _) = build_tx(chain, vec![Spend { utxo, unlock }], outs, Some(&self.keeper), now as u64, None, None).await?;
        Ok(((tx, entries), next_pass))
    }

    /// SET PRICES through the record, proven by `signer`'s coin. `record_in` overrides where the
    /// lane looks for the record (to test pointing it at something else).
    async fn set_price(&self, chain: &Chain, rec: &Rec, lane_at: TransactionOutpoint, lane: Lane, new_price: u64, signer: &Wallet, record_in: usize) -> Result<(Built, Lane)> {
        let c = self.creator.c();
        let (rspend, rout) = record_spend(rec, "use", 2, (rec.owner, rec.recovery), 1, chain)?;
        let m = minter(&c, &self.tier, &lane)?;
        let next_lane = Lane { price: new_price, ..lane };
        let next = minter(&c, &self.tier, &next_lane)?;
        let lane_utxo = sim_utxo(chain, lane_at)?;
        let unlock = Unlock::Script {
            signer: None,
            build: Box::new(move |_| {
                m.entry_sigscript("set_prices", &[ArtifactValue::Int(record_in as i64), ArtifactValue::Int(new_price as i64), ArtifactValue::Int(0), ArtifactValue::Int(0), ArtifactValue::Int(0), ArtifactValue::Int(0)])
            }),
        };
        let outs = vec![cov_out(&next, lane_utxo.entry.amount, 0, self.cov), rout];
        let spends = vec![Spend { utxo: lane_utxo, unlock }, rspend, Spend { utxo: coin(chain, signer).await?, unlock: Unlock::Key(signer.clone()) }];
        let (tx, entries, _) = build_tx(chain, spends, outs, Some(signer), 0, None, None).await?;
        Ok(((tx, entries), next_lane))
    }

    /// WITHDRAW everything in the vault to `signer`, with or without the record.
    async fn withdraw(&self, chain: &Chain, rec: &Rec, signer: &Wallet, with_record: bool) -> Result<(Built, u64)> {
        let coins = chain.utxos_at(&self.vault_spk).await?;
        let vc = vault(rec.id)?;
        let n = coins.len();
        let record_in = n as i64; // the record comes right after the vault coins
        let total: u64 = coins.iter().map(|u| u.entry.amount).sum();
        let mut spends: Vec<Spend> = coins
            .into_iter()
            .map(|u| {
                let vc = vc.clone();
                Spend { utxo: u, unlock: Unlock::Script { signer: None, build: Box::new(move |_| vc.entry_sigscript("withdraw", &[ArtifactValue::Int(record_in)])) } }
            })
            .collect();
        let mut outs = vec![TransactionOutput { value: total, script_public_key: signer.spk.clone(), covenant: None }];
        if with_record {
            // The wallet coin comes right after the record.
            let (rspend, rout) = record_spend(rec, "use", n + 1, (rec.owner, rec.recovery), n as u16, chain)?;
            spends.push(rspend);
            outs.push(rout);
        }
        spends.push(Spend { utxo: coin(chain, signer).await?, unlock: Unlock::Key(signer.clone()) });
        let (tx, entries, _) = build_tx(chain, spends, outs, Some(signer), 0, None, None).await?;
        Ok(((tx, entries), total))
    }
}

impl W5 {
    /// SET VOUCHER on the lane through the record (proven by `signer`'s coin).
    async fn set_voucher(&self, chain: &Chain, rec: &Rec, lane_at: TransactionOutpoint, lane: Lane, new: Hash, signer: &Wallet) -> Result<(Built, Lane)> {
        let c = self.creator.c();
        let (rspend, rout) = record_spend(rec, "use", 2, (rec.owner, rec.recovery), 1, chain)?;
        let m = minter(&c, &self.tier, &lane)?;
        let next_lane = Lane { voucher: Some(new), ..lane };
        let next = minter(&c, &self.tier, &next_lane)?;
        let lane_utxo = sim_utxo(chain, lane_at)?;
        let unlock = Unlock::Script { signer: None, build: Box::new(move |_| m.entry_sigscript("set_voucher", &[ArtifactValue::Int(1), hb(new)])) };
        let outs = vec![cov_out(&next, lane_utxo.entry.amount, 0, self.cov), rout];
        let spends = vec![Spend { utxo: lane_utxo, unlock }, rspend, Spend { utxo: coin(chain, signer).await?, unlock: Unlock::Key(signer.clone()) }];
        let (tx, entries, _) = build_tx(chain, spends, outs, Some(signer), 0, None, None).await?;
        Ok(((tx, entries), next_lane))
    }

    /// REKEY a pass through the record (`record_in` overrides where it looks for the record).
    async fn rekey(&self, chain: &Chain, rec: &Rec, at: TransactionOutpoint, pass: &Pass, new: Hash, signer: &Wallet, record_in: usize) -> Result<(Built, Pass)> {
        let c = self.creator.c();
        let owner = self.buyer.id;
        let (rspend, rout) = record_spend(rec, "use", 2, (rec.owner, rec.recovery), 1, chain)?;
        let cur = membership(&c, &self.tier, owner, pass)?;
        let next_pass = Pass { voucher: Some(new), ..pass.clone() };
        let next = membership(&c, &self.tier, owner, &next_pass)?;
        let utxo = sim_utxo(chain, at)?;
        let unlock = Unlock::Script { signer: None, build: Box::new(move |_| cur.decl_sigscript("rekey", &[ArtifactValue::Int(record_in as i64), hb(new)])) };
        let outs = vec![cov_out(&next, utxo.entry.amount, 0, self.cov), rout];
        let spends = vec![Spend { utxo, unlock }, rspend, Spend { utxo: coin(chain, signer).await?, unlock: Unlock::Key(signer.clone()) }];
        let (tx, entries, _) = build_tx(chain, spends, outs, Some(signer), 0, None, None).await?;
        Ok(((tx, entries), next_pass))
    }
}

/// The same tier with another buyer, still holding the old voucher key.
fn w3_old(w: &W5, buyer: &Wallet) -> W5 {
    W5 { voucher: w.voucher.clone(), tier: w.tier.clone(), creator: w.creator.clone(), buyer: buyer.clone(), keeper: w.keeper.clone(), cov: w.cov, vault_spk: w.vault_spk.clone() }
}

/// HANDOVER of the record to `to`, proven by `signer`'s coin.
async fn handover(chain: &Chain, rec: &Rec, signer: &Wallet, to: (Hash, Hash)) -> Result<Built> {
    let (rspend, rout) = record_spend(rec, "handover", 1, to, 0, chain)?;
    let spends = vec![rspend, Spend { utxo: coin(chain, signer).await?, unlock: Unlock::Key(signer.clone()) }];
    let (tx, entries, _) = build_tx(chain, spends, vec![rout], Some(signer), 0, None, None).await?;
    Ok((tx, entries))
}

pub async fn run(cx: &Ctx, chain: &mut Chain) -> Result<()> {
    println!("\n== v5: owner record, payout vault, record-controlled lanes, voucher-only auto-renew, handover");
    let owner_w = cx.creator.clone();
    let recovery_w = cx.bob.clone();
    let voucher = wallet_from_secret("voucher5", SecretKey::from_slice(&[0x79; 32])?);
    let keeper = wallet_from_secret("keeper5", SecretKey::from_slice(&[0x7a; 32])?);
    let new_owner = wallet_from_secret("new-owner", SecretKey::from_slice(&[0x5a; 32])?);
    let new_recovery = wallet_from_secret("new-recovery", SecretKey::from_slice(&[0x5b; 32])?);
    fund(chain, &voucher, 0xb1);
    fund(chain, &new_owner, 0xb2);
    fund(chain, &new_recovery, 0xb3);
    fund(chain, &keeper, 0xb4);

    // 1. The owner record: created once, by the owner.
    let (oh, rh) = (spk_hash(&owner_w.spk), spk_hash(&recovery_w.spk));
    let outs = vec![TransactionOutput { value: SOMPI, script_public_key: owner_record(oh, rh)?.spk(), covenant: None }];
    let (tx, entries, _) = build_tx(chain, vec![], outs, Some(&owner_w), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let rid = tx.outputs[0].covenant.ok_or_else(|| anyhow!("record output has no covenant"))?.covenant_id;
    let id = chain.submit(&tx, &entries).await?;
    let mut rec = Rec { id: rid, at: TransactionOutpoint::new(id, 0), owner: oh, recovery: rh };
    println!("   ok: owner record {}… created by the owner", &rid.to_string()[..12]);

    // 2. A v5 tier: fee-free, payments to the vault, creator actions through the record.
    let fee = Fee { address: owner_w.addr.to_string(), bps: 0, min: 0 };
    let v4 = V4 { auto_window_ms: 10 * MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: 9999, record_id: Some(rid.to_string()), self_trial: false };
    let tier = Tier { period_ms: 30 * MINUTE, trial_ms: 0, resellable: false, royalty: 0, bond: SOMPI, max_ahead_ms: 60 * MINUTE, fee: Some(fee), v4: Some(v4) };
    let lane = Lane { voucher: None, price: 300 * SOMPI, life: 0, credit: false, remaining: -1, term_price: 0, term_ms: 0 };
    let m = minter(&owner_w.c(), &tier, &lane)?;
    let outs = vec![TransactionOutput { value: SOMPI, script_public_key: m.spk(), covenant: None }];
    let (tx, entries, _) = build_tx(chain, vec![], outs, Some(&owner_w), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let cov = tx.outputs[0].covenant.ok_or_else(|| anyhow!("launch output has no covenant"))?.covenant_id;
    let id = chain.submit(&tx, &entries).await?;
    let mut lane_at = TransactionOutpoint::new(id, 0);
    let w = W5 { tier, creator: owner_w.clone(), buyer: cx.alice.clone(), keeper, voucher: voucher.clone(), cov, vault_spk: vault(rid)?.spk() };
    println!("   ok: v5 tier {}… launched (ceiling 300 KAS, no KasPass fee, pays the vault)", &cov.to_string()[..12]);

    // 3. Buying at today's price: the voucher signs a discount off the ceiling (100 KAS today).
    let before = w.vault_balance(chain).await?;
    let ((tx, entries), mut pass) = w.buy(chain, lane_at, lane, 666_700, Some(voucher.clone()), 300 * SOMPI).await?;
    let id = chain.submit(&tx, &entries).await?;
    lane_at = TransactionOutpoint::new(id, 0);
    let mut pass_at = TransactionOutpoint::new(id, 1);
    let got = w.vault_balance(chain).await? - before;
    if got != discounted_ppm(lane.price, 666_700) {
        bail!("vault got {} instead of today's price", kas(got));
    }
    println!("   ok: bought at today's price ({}), paid into the vault, 300 KAS prepaid for auto-renew", kas(got));
    let r = match w.buy(chain, lane_at, lane, 666_700, None, 0).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("discount without the voucher's signature", r)?;

    // 4. Auto-renew: only with the voucher's signature, even with no discount.
    set_now(chain, pass.expiry - 5 * MINUTE);
    let r = match w.autorenew(chain, pass_at, &pass, 0, None).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("auto-renew at the full ceiling without the server's signature", r)?;
    let r = match w.autorenew(chain, pass_at, &pass, 666_700, Some(cx.alice.clone())).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("auto-renew signed by someone other than the voucher key", r)?;
    let ((tx, entries), next) = w.autorenew(chain, pass_at, &pass, 666_700, Some(voucher.clone())).await?;
    let id = chain.submit(&tx, &entries).await?;
    pass_at = TransactionOutpoint::new(id, 0);
    pass = next;
    println!("   ok: auto-renewed at today's price with the server's signature");

    // 5. Creator actions go through the record: change the ceiling.
    let r = match w.set_price(chain, &rec, lane_at, lane, 400 * SOMPI, &cx.alice, 1).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("price change proven by someone else's coin", r)?;
    let r = match w.set_price(chain, &rec, lane_at, lane, 400 * SOMPI, &owner_w, 2).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("price change pointing at a plain coin instead of the record", r)?;
    let ((tx, entries), _) = w.set_price(chain, &rec, lane_at, lane, 400 * SOMPI, &owner_w, 1).await?;
    let id = chain.submit(&tx, &entries).await?;
    lane_at = TransactionOutpoint::new(id, 0);
    rec.at = TransactionOutpoint::new(id, 1);
    println!("   ok: the owner changed the ceiling to 400 KAS through the record");

    // 6. Withdraw from the vault: needs the record, which needs the owner.
    let r = match w.withdraw(chain, &rec, &cx.alice, false).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("vault emptied without the owner record", r)?;
    let r = match w.withdraw(chain, &rec, &cx.alice, true).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("vault emptied with the record but someone else's coin", r)?;
    let ((tx, entries), total) = w.withdraw(chain, &rec, &owner_w, true).await?;
    let id = chain.submit(&tx, &entries).await?;
    rec.at = TransactionOutpoint::new(id, 1);
    println!("   ok: the owner withdrew {} from the vault", kas(total));

    // 7. Handover: only the recovery wallet can name a new owner.
    let to = (spk_hash(&new_owner.spk), spk_hash(&new_recovery.spk));
    let r = match handover(chain, &rec, &owner_w, to).await { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("handover by the owner (only the recovery wallet may)", r)?;
    let r = match handover(chain, &rec, &new_owner, to).await { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("handover by a stranger", r)?;
    let (tx, entries) = handover(chain, &rec, &recovery_w, to).await?;
    let id = chain.submit(&tx, &entries).await?;
    rec = Rec { at: TransactionOutpoint::new(id, 0), owner: to.0, recovery: to.1, ..rec };
    println!("   ok: the recovery wallet handed the record to a new owner and a new recovery wallet");

    // 8. After the handover: existing passes pay the same vault; old owner out, new owner in.
    set_now(chain, pass.expiry - 5 * MINUTE);
    let before = w.vault_balance(chain).await?;
    let ((tx, entries), next) = w.autorenew(chain, pass_at, &pass, 600_000, Some(voucher.clone())).await?;
    let id = chain.submit(&tx, &entries).await?;
    pass_at = TransactionOutpoint::new(id, 0);
    pass = next;
    println!("   ok: an existing pass auto-renewed into the same vault after the handover (+{})", kas(w.vault_balance(chain).await? - before));
    let r = match w.withdraw(chain, &rec, &owner_w, true).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("old owner withdrawing after the handover", r)?;
    let r = match handover(chain, &rec, &recovery_w, (oh, rh)).await { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("old recovery wallet taking it back after the handover", r)?;
    let ((tx, entries), total) = w.withdraw(chain, &rec, &new_owner, true).await?;
    let id = chain.submit(&tx, &entries).await?;
    rec.at = TransactionOutpoint::new(id, 1);
    println!("   ok: the new owner withdrew {} from the same vault", kas(total));

    // 9. Replacing a leaked voucher key: the lane and each pass, through the record.
    let new_key = wallet_from_secret("voucher5b", SecretKey::from_slice(&[0x7b; 32])?);
    let lane_now = Lane { price: 400 * SOMPI, ..lane };
    let r = match w.set_voucher(chain, &rec, lane_at, lane_now, new_key.id, &owner_w).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("old owner re-keying the lane", r)?;
    let ((tx, entries), lane2) = w.set_voucher(chain, &rec, lane_at, lane_now, new_key.id, &new_owner).await?;
    let id = chain.submit(&tx, &entries).await?;
    lane_at = TransactionOutpoint::new(id, 0);
    rec.at = TransactionOutpoint::new(id, 1);
    println!("   ok: the owner pointed the lane at a new voucher key");
    let r = match w3_old(&w, &new_owner).buy(chain, lane_at, lane2, 600_000, Some(voucher.clone()), 0).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("discount signed with the old key after the lane was re-keyed", r)?;
    let w2 = W5 { voucher: new_key.clone(), tier: w.tier.clone(), creator: w.creator.clone(), buyer: cx.alice.clone(), keeper: w.keeper.clone(), cov, vault_spk: w.vault_spk.clone() };
    // A different buyer, so alice keeps her test KAS for the later sections.
    let w3 = W5 { voucher: new_key.clone(), tier: w.tier.clone(), creator: w.creator.clone(), buyer: new_owner.clone(), keeper: w.keeper.clone(), cov, vault_spk: w.vault_spk.clone() };
    let ((tx, entries), _) = w3.buy(chain, lane_at, lane2, 600_000, Some(new_key.clone()), 0).await?;
    let id = chain.submit(&tx, &entries).await?;
    lane_at = TransactionOutpoint::new(id, 0);
    println!("   ok: a new purchase with the new key works, and the pass trusts the new key");
    // The existing pass (still on the old key) is re-keyed. About 80 KAS of its prepaid
    // balance is left, so the renewals below are at 80% off the 300 KAS ceiling.
    let r = match w.rekey(chain, &rec, pass_at, &pass, new_key.id, &new_owner, 2).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("re-keying a pass without the owner record", r)?;
    let r = match w.rekey(chain, &rec, pass_at, &pass, new_key.id, &cx.alice, 1).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("re-keying a pass with the record but someone else's coin", r)?;
    let ((tx, entries), pass2) = w.rekey(chain, &rec, pass_at, &pass, new_key.id, &new_owner, 1).await?;
    let id = chain.submit(&tx, &entries).await?;
    let pass2_at = TransactionOutpoint::new(id, 0);
    println!("   ok: the owner re-keyed an existing pass");
    set_now(chain, pass2.expiry - 5 * MINUTE);
    let r = match w.autorenew(chain, pass2_at, &pass2, 800_000, Some(voucher.clone())).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("auto-renew signed with the old (leaked) key after the re-key", r)?;
    let ((tx, entries), _) = w2.autorenew(chain, pass2_at, &pass2, 800_000, Some(new_key.clone())).await?;
    chain.submit(&tx, &entries).await?;
    println!("   ok: the re-keyed pass auto-renews with the new key");

    payin_checks(&w2, chain, lane_at, lane2, &new_key).await?;
    self_trial_checks(cx, chain, rid, &owner_w, &voucher).await
}

/// SELF_TRIAL (minter_v5t): a member starts the free trial themselves with the voucher key's
/// approval, prepaying their first period into their own pass. `join` and `expiry` let a test
/// lie about the pass, `pass_value` about the deposit.
#[allow(clippy::too_many_arguments)]
async fn self_trial(w: &W5, chain: &Chain, lane_at: TransactionOutpoint, lane: Lane, signer: Option<Wallet>, prepaid: u64, join_shift: i64, expiry_extra: i64, pass_value: Option<u64>) -> Result<(Built, Pass)> {
    let now = chain.now_ms().await?;
    let join = now + join_shift;
    let owner = w.buyer.id;
    let c = w.creator.c();
    let pass = Pass { voucher: lane.voucher, owner: String::new(), expiry: join + w.tier.trial_ms + expiry_extra, rate: lane.price, life: 0, credit: false, paid: 0 };
    let member = membership(&c, &w.tier, owner, &pass)?;
    let (pre, suf, _) = member.template();
    let m = minter(&c, &w.tier, &lane)?;
    let lane_utxo = sim_utxo(chain, lane_at)?;
    let vpk = w.voucher.pk.clone();
    let unlock = Unlock::Script {
        signer,
        build: Box::new(move |sig| {
            let s = if sig.is_empty() { vec![0u8; 65] } else { sig };
            m.entry_sigscript("self_trial", &[hb(owner), ArtifactValue::Int(join), ArtifactValue::Bytes(s), ArtifactValue::Bytes(vpk.clone()), ArtifactValue::Bytes(pre.clone()), ArtifactValue::Bytes(suf.clone())])
        }),
    };
    let next = minter(&c, &w.tier, &Lane { remaining: lane.remaining, ..lane })?;
    let outs = vec![cov_out(&next, lane_utxo.entry.amount, 0, w.cov), cov_out(&member, pass_value.unwrap_or(w.tier.bond + prepaid), 0, w.cov)];
    let (tx, entries, _) = build_tx(chain, vec![Spend { utxo: lane_utxo, unlock }], outs, Some(&w.buyer), now as u64, None, None).await?;
    Ok(((tx, entries), pass))
}

async fn self_trial_checks(cx: &Ctx, chain: &mut Chain, rid: Hash, owner_w: &Wallet, voucher: &Wallet) -> Result<()> {
    println!("\n== v5: self-serve free trials (minter_v5t): the member starts it, the server approves, the first period sits prepaid in their own pass");
    let fee = Fee { address: owner_w.addr.to_string(), bps: 0, min: 0 };
    let v4 = V4 { auto_window_ms: 10 * MINUTE, voucher_key_hash: voucher.id.to_string(), max_discount_bps: 9999, record_id: Some(rid.to_string()), self_trial: true };
    let tier = Tier { period_ms: 30 * MINUTE, trial_ms: 7 * MINUTE, resellable: false, royalty: 0, bond: SOMPI, max_ahead_ms: 60 * MINUTE, fee: Some(fee), v4: Some(v4) };
    let lane = Lane { voucher: None, price: 300 * SOMPI, life: 0, credit: false, remaining: -1, term_price: 0, term_ms: 0 };
    let m = minter(&owner_w.c(), &tier, &lane)?;
    let outs = vec![TransactionOutput { value: SOMPI, script_public_key: m.spk(), covenant: None }];
    let (tx, entries, _) = build_tx(chain, vec![], outs, Some(owner_w), 0, Some(GenesisCovenantGroup::new(0, vec![0])), None).await?;
    let cov = tx.outputs[0].covenant.ok_or_else(|| anyhow!("launch output has no covenant"))?.covenant_id;
    let id = chain.submit(&tx, &entries).await?;
    let mut lane_at = TransactionOutpoint::new(id, 0);
    let keeper = wallet_from_secret("keeper5t", SecretKey::from_slice(&[0x7c; 32])?);
    fund(chain, &keeper, 0xb7);
    let w = W5 { tier, creator: owner_w.clone(), buyer: cx.alice.clone(), keeper, voucher: voucher.clone(), cov, vault_spk: vault(rid)?.spk() };
    println!("   ok: tier {}… launched with a 7-minute self-serve trial", &cov.to_string()[..12]);
    let prepaid = 100 * SOMPI;
    let try_ = |r: Result<(Built, Pass)>| r.map(|(b, _)| b);
    // Cheats first.
    let r = match try_(self_trial(&w, chain, lane_at, lane, None, prepaid, 0, 0, None).await) { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("self-serve trial without the server's signature", r)?;
    let r = match try_(self_trial(&w, chain, lane_at, lane, Some(cx.alice.clone()), prepaid, 0, 0, None).await) { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("self-serve trial signed by someone other than the voucher key", r)?;
    let r = match try_(self_trial(&w, chain, lane_at, lane, Some(voucher.clone()), prepaid, 0, w.tier.period_ms, None).await) { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("a trial pass claiming a longer expiry than the trial", r)?;
    let r = match try_(self_trial(&w, chain, lane_at, lane, Some(voucher.clone()), prepaid, 60 * MINUTE, 0, None).await) { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("a trial starting in the future (to stretch it)", r)?;
    let r = match try_(self_trial(&w, chain, lane_at, lane, Some(voucher.clone()), 0, 0, 0, Some(SOMPI / 2)).await) { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("a trial pass holding less than the deposit", r)?;
    // The honest trial: nothing reaches the vault; the pass holds the deposit plus the first period.
    let before = w.vault_balance(chain).await?;
    let ((tx, entries), pass) = self_trial(&w, chain, lane_at, lane, Some(voucher.clone()), prepaid, 0, 0, None).await?;
    let id = chain.submit(&tx, &entries).await?;
    lane_at = TransactionOutpoint::new(id, 0);
    let pass_at = TransactionOutpoint::new(id, 1);
    if w.vault_balance(chain).await? != before {
        bail!("a free trial paid the creator something");
    }
    if tx.outputs[1].value != SOMPI + prepaid {
        bail!("the trial pass holds {} instead of the deposit plus the prepaid period", kas(tx.outputs[1].value));
    }
    println!("   ok: the member started a 7-minute trial; nothing paid, {} in their own pass", kas(SOMPI + prepaid));
    let _ = lane_at;
    // At the end of the trial the keeper charges the first period from the pass, with the server's approval.
    set_now(chain, pass.expiry - MINUTE);
    let ((tx, entries), next) = w.autorenew(chain, pass_at, &pass, 666_700, Some(voucher.clone())).await?;
    chain.submit(&tx, &entries).await?;
    let got = w.vault_balance(chain).await? - before;
    if got != discounted_ppm(lane.price, 666_700) || next.expiry != pass.expiry + w.tier.period_ms {
        bail!("the first paid period after the trial went wrong ({} to the vault)", kas(got));
    }
    println!("   ok: at the end of the trial the keeper charged the first period from the pass ({}); the paid month starts when the trial ends", kas(got));
    Ok(())
}

/// The pay-in address of one order for `buyer` on this tier (pays up to `max_spend`).
fn order(w: &W5, buyer: &Wallet, max_spend: u64, refund_after: i64) -> Result<Compiled> {
    let (_, _, tmpl) = membership(&w.creator.c(), &w.tier, Hash::default(), &blank_pass())?.template();
    payin(buyer.id, w.cov, tmpl, &buyer.spk, max_spend, refund_after, PAYIN_MAX_FEE)
}

const PAYIN_MAX_FEE: u64 = SOMPI / 10;

/// A phone wallet sends `amount` to a pay-in address (a plain payment, nothing to sign but its own coin).
async fn phone_pays(chain: &mut Chain, phone: &Wallet, to: &Compiled, amount: u64) -> Result<Utxo> {
    let outs = vec![TransactionOutput { value: amount, script_public_key: to.spk(), covenant: None }];
    let (tx, entries, _) = build_tx(chain, vec![], outs, Some(phone), 0, None, None).await?;
    let id = chain.submit(&tx, &entries).await?;
    sim_utxo(chain, TransactionOutpoint::new(id, 0))
}

/// COMPLETE an order: BUY on the lane, funded only by the pay-in coin. The pass goes to
/// `owner` (the order's buyer, unless testing a cheat); `short` takes that much from the change.
#[allow(clippy::too_many_arguments)]
async fn payin_buy(w: &W5, chain: &Chain, lane_at: TransactionOutpoint, lane: Lane, bps: i64, key: &Wallet, prepaid: u64, pin: &Compiled, coin: Utxo, owner: Hash, short: u64) -> Result<(Built, Pass)> {
    let now = chain.now_ms().await?;
    let due = discounted_ppm(lane.price, bps as u64);
    let c = w.creator.c();
    let pass = Pass { voucher: lane.voucher, owner: String::new(), expiry: now + w.tier.period_ms, rate: lane.price, life: 0, credit: false, paid: due };
    let member = membership(&c, &w.tier, owner, &pass)?;
    let (pre, suf, _) = member.template();
    let m = minter(&c, &w.tier, &lane)?;
    let lane_utxo = sim_utxo(chain, lane_at)?;
    let vpk = key.pk.clone();
    let (pre2, suf2) = (pre.clone(), suf.clone());
    let lane_unlock = Unlock::Script {
        signer: Some(key.clone()),
        build: Box::new(move |sig| {
            let s = if sig.is_empty() { vec![0u8; 65] } else { sig };
            m.entry_sigscript("buy", &[hb(owner), ArtifactValue::Int(now), ArtifactValue::Int(bps), ArtifactValue::Bytes(s), ArtifactValue::Bytes(vpk.clone()), ArtifactValue::Bytes(pre.clone()), ArtifactValue::Bytes(suf.clone())])
        }),
    };
    let v = lane.voucher.ok_or_else(|| anyhow!("lane has no voucher"))?;
    let p = pin.clone();
    let expiry = pass.expiry;
    let rate = pass.rate as i64;
    let pin_unlock = Unlock::Script {
        signer: None,
        build: Box::new(move |_| {
            p.entry_sigscript(
                "complete",
                &[ArtifactValue::Int(0), ArtifactValue::Int(1), ArtifactValue::Int(expiry), ArtifactValue::Int(rate), ArtifactValue::Int(0), ArtifactValue::Int(0), ArtifactValue::Int(due as i64), hb(v), ArtifactValue::Bytes(pre2.clone()), ArtifactValue::Bytes(suf2.clone())],
            )
        }),
    };
    let paid_in = coin.entry.amount;
    let next = minter(&c, &w.tier, &lane)?;
    let mut outs = vec![cov_out(&next, lane_utxo.entry.amount, 0, w.cov), cov_out(&member, w.tier.bond + prepaid, 0, w.cov)];
    outs.extend(payment_outs(&c, &w.tier, due)?);
    let change = paid_in - due - w.tier.bond - prepaid - short;
    if short > 0 {
        // The cheat: the skimmed part goes to another wallet.
        outs.push(TransactionOutput { value: short, script_public_key: w.keeper.spk.clone(), covenant: None });
    }
    outs.push(TransactionOutput { value: change, script_public_key: w.buyer.spk.clone(), covenant: None });
    let last = outs.len() - 1;
    let (tx, entries, _) = build_tx(chain, vec![Spend { utxo: lane_utxo, unlock: lane_unlock }, Spend { utxo: coin, unlock: pin_unlock }], outs, None, now as u64, None, Some(last)).await?;
    Ok(((tx, entries), pass))
}

/// COMPLETE an order with a RENEW of the buyer's pass, funded only by the pay-in coin.
#[allow(clippy::too_many_arguments)]
async fn payin_renew(w: &W5, chain: &Chain, pass_at: TransactionOutpoint, pass: &Pass, bps: i64, key: &Wallet, pin: &Compiled, coin: Utxo) -> Result<(Built, Pass)> {
    let now = chain.now_ms().await?;
    let c = w.creator.c();
    let owner = w.buyer.id;
    let cur = membership(&c, &w.tier, owner, pass)?;
    let utxo = sim_utxo(chain, pass_at)?;
    let due = discounted_ppm(pass.rate, bps as u64);
    let next_pass = Pass { expiry: pass.expiry.max(now) + w.tier.period_ms, paid: pass.paid + due, ..pass.clone() };
    let next = membership(&c, &w.tier, owner, &next_pass)?;
    let (pre, suf, _) = next.template();
    let vpk = key.pk.clone();
    let pass_unlock = Unlock::Script {
        signer: Some(key.clone()),
        build: Box::new(move |sig| {
            let s = if sig.is_empty() { vec![0u8; 65] } else { sig };
            cur.decl_sigscript("renew", &[ArtifactValue::Int(now), ArtifactValue::Int(bps), ArtifactValue::Bytes(s), ArtifactValue::Bytes(vpk.clone())])
        }),
    };
    let v = next_pass.voucher.ok_or_else(|| anyhow!("pass has no voucher"))?;
    let p = pin.clone();
    let (expiry, rate, paid) = (next_pass.expiry, next_pass.rate as i64, next_pass.paid as i64);
    let pin_unlock = Unlock::Script {
        signer: None,
        build: Box::new(move |_| {
            p.entry_sigscript(
                "complete",
                &[ArtifactValue::Int(0), ArtifactValue::Int(0), ArtifactValue::Int(expiry), ArtifactValue::Int(rate), ArtifactValue::Int(0), ArtifactValue::Int(0), ArtifactValue::Int(paid), hb(v), ArtifactValue::Bytes(pre.clone()), ArtifactValue::Bytes(suf.clone())],
            )
        }),
    };
    let mut outs = vec![cov_out(&next, utxo.entry.amount, 0, w.cov)];
    outs.extend(payment_outs(&c, &w.tier, due)?);
    outs.push(TransactionOutput { value: coin.entry.amount - due, script_public_key: w.buyer.spk.clone(), covenant: None });
    let last = outs.len() - 1;
    let (tx, entries, _) = build_tx(chain, vec![Spend { utxo, unlock: pass_unlock }, Spend { utxo: coin, unlock: pin_unlock }], outs, None, now as u64, None, Some(last)).await?;
    Ok(((tx, entries), next_pass))
}

/// REFUND an order to `to` (the buyer, unless testing a cheat), with `lock` as the chain time.
async fn payin_refund(chain: &Chain, pin: &Compiled, coin: Utxo, to: &Wallet, lock: i64) -> Result<Built> {
    let p = pin.clone();
    let unlock = Unlock::Script { signer: None, build: Box::new(move |_| p.entry_sigscript("refund", &[])) };
    let outs = vec![TransactionOutput { value: coin.entry.amount, script_public_key: to.spk.clone(), covenant: None }];
    let (tx, entries, _) = build_tx(chain, vec![Spend { utxo: coin, unlock }], outs, None, lock as u64, None, Some(0)).await?;
    Ok((tx, entries))
}

/// 10. Paying from any wallet (a QR code): the pass goes to the signed-in wallet, the money
/// comes from a phone wallet through a one-time pay-in address.
async fn payin_checks(w: &W5, chain: &mut Chain, mut lane_at: TransactionOutpoint, lane: Lane, key: &Wallet) -> Result<()> {
    println!("\n== pay from any wallet: one-time pay-in addresses (QR code)");
    let kaspire = wallet_from_secret("kaspire-buyer", SecretKey::from_slice(&[0x6a; 32])?);
    let phone = wallet_from_secret("phone-wallet", SecretKey::from_slice(&[0x6b; 32])?);
    let thief = wallet_from_secret("thief", SecretKey::from_slice(&[0x6c; 32])?);
    fund(chain, &phone, 0xc1);
    let wb = W5 { voucher: key.clone(), tier: w.tier.clone(), creator: w.creator.clone(), buyer: kaspire.clone(), keeper: w.keeper.clone(), cov: w.cov, vault_spk: w.vault_spk.clone() };
    let bps = 600_000;
    let due = discounted_ppm(lane.price, bps as u64);
    let prepaid = 50 * SOMPI;
    let cost = due + w.tier.bond + prepaid;
    let max_spend = cost + SOMPI; // up to 1 KAS of network fee
    let now = chain.now_ms().await?;

    // An order, paid a little too much from the phone (wallets round).
    let pin = order(&wb, &kaspire, max_spend, now + 60 * MINUTE)?;
    let coin = phone_pays(chain, &phone, &pin, cost + 2 * SOMPI).await?;
    println!("   ok: the phone wallet paid {} to the order's pay-in address", kas(coin.entry.amount));

    let r = match payin_buy(&wb, chain, lane_at, lane, bps, key, prepaid, &pin, coin.clone(), thief.id, 0).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("completing the order with the pass going to someone else", r)?;
    let r = match payin_buy(&wb, chain, lane_at, lane, bps, key, prepaid, &pin, coin.clone(), kaspire.id, 3 * SOMPI / 2).await { Ok(((tx, entries), _)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("completing the order but keeping more than it allows", r)?;
    let r = match payin_refund(chain, &pin, coin.clone(), &kaspire, now).await { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("refunding before the refund time", r)?;

    let before = w.vault_balance(chain).await?;
    let ((tx, entries), pass) = payin_buy(&wb, chain, lane_at, lane, bps, key, prepaid, &pin, coin, kaspire.id, 0).await?;
    let id = chain.submit(&tx, &entries).await?;
    lane_at = TransactionOutpoint::new(id, 0);
    let pass_coin = sim_utxo(chain, TransactionOutpoint::new(id, 1))?;
    let got = w.vault_balance(chain).await? - before;
    let back: u64 = chain.utxos_at(&kaspire.spk).await?.iter().map(|u| u.entry.amount).sum();
    if got != due || pass_coin.entry.amount != w.tier.bond + prepaid || back < 2 * SOMPI - SOMPI {
        bail!("pay-in purchase: vault +{}, pass {}, change {}", kas(got), kas(pass_coin.entry.amount), kas(back));
    }
    println!("   ok: completed: today's price into the vault, the pass (with {} prepaid) owned by the signed-in wallet, {} change back to it", kas(prepaid), kas(back));
    let pass_at = TransactionOutpoint::new(id, 1);

    // Renewing that pass the same way: the order spends the pass instead of the lane.
    set_now(chain, pass.expiry - 5 * MINUTE);
    let now = chain.now_ms().await?;
    let due_r = discounted_ppm(pass.rate, bps as u64);
    let pin3 = order(&wb, &kaspire, due_r + SOMPI, now + 60 * MINUTE)?;
    let coin3 = phone_pays(chain, &phone, &pin3, due_r + SOMPI / 2).await?;
    let before = w.vault_balance(chain).await?;
    let ((tx, entries), renewed) = payin_renew(&wb, chain, pass_at, &pass, bps, key, &pin3, coin3).await?;
    chain.submit(&tx, &entries).await?;
    let got = w.vault_balance(chain).await? - before;
    if got != due_r || renewed.expiry != pass.expiry + w.tier.period_ms {
        bail!("pay-in renewal: vault +{}", kas(got));
    }
    println!("   ok: renewed from the phone through a pay-in address ({} into the vault, one period added)", kas(got));
    let back: u64 = chain.utxos_at(&kaspire.spk).await?.iter().map(|u| u.entry.amount).sum();

    // A second order that never completes: refunded after the refund time, only to the buyer.
    let now = chain.now_ms().await?;
    let pin2 = order(&wb, &kaspire, max_spend, now + 60 * MINUTE)?;
    let coin2 = phone_pays(chain, &phone, &pin2, 40 * SOMPI).await?;
    let later = now + 61 * MINUTE;
    set_now(chain, later);
    let r = match payin_refund(chain, &pin2, coin2.clone(), &thief, later).await { Ok((tx, entries)) => chain.submit(&tx, &entries).await.map(|_| ()), Err(e) => Err(e) };
    refused("refunding to someone else", r)?;
    let before = back;
    let (tx, entries) = payin_refund(chain, &pin2, coin2, &kaspire, later).await?;
    chain.submit(&tx, &entries).await?;
    let back: u64 = chain.utxos_at(&kaspire.spk).await?.iter().map(|u| u.entry.amount).sum();
    if back - before < 40 * SOMPI - PAYIN_MAX_FEE {
        bail!("refund returned only {}", kas(back - before));
    }
    println!("   ok: an unfinished order refunded after the refund time ({} back to the signed-in wallet)", kas(back - before));
    let _ = lane_at;
    Ok(())
}
