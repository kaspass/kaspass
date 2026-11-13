//! Hand a transaction to a browser wallet (Kaspire) for signing, and take it back.
//!
//! Kaspire signs "SafeJSON" transactions: every integer may be a string, scripts are hex,
//! and each input carries the coin it spends so the wallet can check everything itself.
//! It signs only the inputs we select, and never broadcasts generic transactions,
//! so the server sends it after checking the signatures.

use anyhow::{Result, anyhow, bail};
use kaspa_consensus_core::hashing::sighash::{SigHashReusedValuesUnsync, calc_schnorr_signature_hash};
use kaspa_consensus_core::hashing::sighash_type::SIG_HASH_ALL;
use kaspa_consensus_core::tx::{
    PopulatedTransaction, ScriptPublicKey, Transaction,
    UtxoEntry,
};
use kaspa_hashes::Hash;
use serde_json::{Value, json};

fn spk_hex(spk: &ScriptPublicKey) -> String {
    let mut b = spk.version().to_be_bytes().to_vec();
    b.extend_from_slice(spk.script());
    faster_hex::hex_string(&b)
}

fn hex(b: &[u8]) -> String {
    faster_hex::hex_string(b)
}

/// The transaction as SafeJSON. Inputs listed in `unsigned` get an empty signature script.
pub fn safe_json(tx: &Transaction, entries: &[UtxoEntry], unsigned: &[usize]) -> Value {
    let inputs: Vec<Value> = tx
        .inputs
        .iter()
        .zip(entries)
        .enumerate()
        .map(|(i, (inp, e))| {
            let mut utxo = json!({
                "amount": e.amount.to_string(),
                "scriptPublicKey": spk_hex(&e.script_public_key),
                "blockDaaScore": e.block_daa_score.to_string(),
                "isCoinbase": e.is_coinbase,
            });
            if let Some(c) = e.covenant_id {
                utxo["covenantId"] = json!(c.to_string());
            }
            json!({
                "transactionId": inp.previous_outpoint.transaction_id.to_string(),
                "index": inp.previous_outpoint.index,
                "signatureScript": if unsigned.contains(&i) { String::new() } else { hex(&inp.signature_script) },
                "sequence": inp.sequence.to_string(),
                "computeBudget": inp.compute_commit.compute_budget().unwrap_or_default(),
                "utxo": utxo,
            })
        })
        .collect();
    let outputs: Vec<Value> = tx
        .outputs
        .iter()
        .map(|o| {
            json!({
                "value": o.value.to_string(),
                "scriptPublicKey": spk_hex(&o.script_public_key),
                "covenant": o.covenant.map(|c| json!({
                    "authorizingInput": c.authorizing_input,
                    "covenantId": c.covenant_id.to_string(),
                })),
            })
        })
        .collect();
    json!({
        "version": tx.version,
        "inputs": inputs,
        "outputs": outputs,
        "lockTime": tx.lock_time.to_string(),
        "subnetworkId": tx.subnetwork_id.to_string(),
        "gas": tx.gas.to_string(),
        "payload": hex(&tx.payload),
        "storageMass": tx.storage_mass().to_string(),
    })
}

/// The exact hash the wallet will sign for input `idx`. Kaspire recomputes it and refuses
/// to show the approval if its own reading of the transaction disagrees.
pub fn sighash(tx: &Transaction, entries: &[UtxoEntry], idx: usize) -> Hash {
    let pop = PopulatedTransaction::new(tx, entries.to_vec());
    calc_schnorr_signature_hash(&pop, idx, SIG_HASH_ALL, &SigHashReusedValuesUnsync::new())
}

/// The server's voucher keys (x-only). Their signatures stay on the server: a request for a
/// wallet carries a blank in their place, and the server's own copy of the transaction keeps the
/// real one until `/v1/submit`, so an approval can't be kept and used later.
static SERVER_KEYS: std::sync::RwLock<Vec<Vec<u8>>> = std::sync::RwLock::new(Vec::new());

pub fn add_server_key(pk: &[u8]) {
    let mut k = SERVER_KEYS.write().unwrap_or_else(|e| e.into_inner());
    if !k.iter().any(|x| x == pk) {
        k.push(pk.to_vec());
    }
}

/// `script` with every 65-byte push that is a server key's signature for input `idx` blanked.
fn blank_server_sigs(script: &[u8], tx: &Transaction, entries: &[UtxoEntry], idx: usize) -> Vec<u8> {
    let keys = SERVER_KEYS.read().unwrap_or_else(|e| e.into_inner()).clone();
    let mut out = script.to_vec();
    if keys.is_empty() || script.len() < 66 {
        return out;
    }
    let msg = secp256k1::Message::from_digest_slice(&sighash(tx, entries, idx).as_bytes()).ok();
    let Some(msg) = msg else { return out };
    let pks: Vec<secp256k1::XOnlyPublicKey> = keys.iter().filter_map(|k| secp256k1::XOnlyPublicKey::from_slice(k).ok()).collect();
    let mut p = 0;
    while p + 66 <= script.len() {
        if script[p] == 0x41 {
            let body = &script[p + 1..p + 66];
            if body.iter().any(|&b| b != 0) {
                if let Ok(sig) = secp256k1::schnorr::Signature::from_slice(&body[..64]) {
                    if pks.iter().any(|pk| secp256k1::SECP256K1.verify_schnorr(&sig, &msg, pk).is_ok()) {
                        out[p + 1..p + 66].fill(0);
                        p += 66;
                        continue;
                    }
                }
            }
        }
        p += 1;
    }
    out
}

/// The transaction as a wallet may see it: server signatures blanked.
fn for_wallet(tx: &Transaction, entries: &[UtxoEntry]) -> Transaction {
    let mut t = tx.clone();
    for i in 0..t.inputs.len() {
        let s = blank_server_sigs(&tx.inputs[i].signature_script, tx, entries, i);
        t.inputs[i].signature_script = s;
    }
    t
}

/// The request for `window.kaspire.request({ method: "signPskt", params })`.
pub fn sign_request(tx: &Transaction, entries: &[UtxoEntry], sender: &str, unsigned: &[usize]) -> Value {
    sign_request_with_scripts(tx, entries, sender, unsigned, &[])
}

/// A covenant input whose own script carries the wallet's signature: the complete script
/// with a blank (all-zero) 65-byte slot at `offset`, which the wallet fills in.
pub struct ScriptSlot {
    pub input: usize,
    pub prebuilt: Vec<u8>,
    pub offset: usize,
}

pub fn sign_request_with_scripts(tx: &Transaction, entries: &[UtxoEntry], sender: &str, unsigned: &[usize], slots: &[ScriptSlot]) -> Value {
    let all: Vec<usize> = unsigned.iter().copied().chain(slots.iter().map(|s| s.input)).collect();
    let shown = for_wallet(tx, entries);
    json!({
        "sender": sender,
        "psktTransactionJson": safe_json(&shown, entries, &all).to_string(),
        "submitTransaction": false,
        "signInputs": all.iter().map(|i| json!({
            "index": i,
            "sighashType": SIG_HASH_ALL.to_u8(),
            "expectedSighash": sighash(tx, entries, *i).to_string(),
        })).collect::<Vec<_>>(),
        "scripts": slots.iter().map(|s| json!({
            "inputIndex": s.input,
            "scriptHex": "",
            "signType": SIG_HASH_ALL.to_u8(),
            "prebuiltSignatureScript": hex(&blank_server_sigs(&s.prebuilt, tx, entries, s.input)),
            "signatureOffsets": [s.offset],
            "expectedSighash": sighash(tx, entries, s.input).to_string(),
        })).collect::<Vec<_>>(),
    })
}

/// A prebuilt covenant script with the wallet's signature: only the blank 65-byte slots in our
/// copy are taken from the wallet's; everything else, including any server signature, stays ours.
fn merge_slots(ours: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    if ours.len() != theirs.len() {
        return None;
    }
    let mut out = ours.to_vec();
    let mut p = 0;
    let mut filled = false;
    while p + 66 <= ours.len() {
        if ours[p] == 0x41 && ours[p + 1..p + 66].iter().all(|&b| b == 0) && theirs[p] == 0x41 {
            out[p + 1..p + 66].copy_from_slice(&theirs[p + 1..p + 66]);
            filled |= theirs[p + 1..p + 66].iter().any(|&b| b != 0);
            p += 66;
        } else {
            p += 1;
        }
    }
    filled.then_some(out)
}

/// Copies the wallet's signatures for `indexes` into our own copy of the transaction.
/// Nothing else is taken from the wallet's JSON: if it changed anything, the signatures
/// won't match our transaction and the final check refuses it.
pub fn apply_signatures(tx: &mut Transaction, signed_json: &str, indexes: &[usize]) -> Result<()> {
    let v: Value = serde_json::from_str(signed_json).map_err(|_| anyhow!("wallet returned something that isn't a transaction"))?;
    let inputs = v.get("inputs").and_then(Value::as_array).ok_or_else(|| anyhow!("signed transaction has no inputs"))?;
    if inputs.len() != tx.inputs.len() {
        bail!("wallet returned a transaction with a different number of inputs");
    }
    for &i in indexes {
        let s = inputs[i].get("signatureScript").and_then(Value::as_str).unwrap_or_default();
        if s.is_empty() {
            bail!("wallet did not sign input {i}");
        }
        let mut b = vec![0u8; s.len() / 2];
        faster_hex::hex_decode(s.as_bytes(), &mut b).map_err(|_| anyhow!("bad signature script on input {i}"))?;
        let ours = &tx.inputs[i].signature_script;
        tx.inputs[i].signature_script = if ours.is_empty() { b } else { merge_slots(ours, &b).ok_or_else(|| anyhow!("the wallet changed input {i}'s contract script"))? };
    }
    tx.finalize();
    Ok(())
}

#[cfg(test)]
mod slot_tests {
    use super::merge_slots;

    fn script(a: u8, slot: [u8; 65], b: u8) -> Vec<u8> {
        let mut v = vec![0x01, a, 0x41];
        v.extend_from_slice(&slot);
        v.extend_from_slice(&[0x01, b]);
        v
    }

    #[test]
    fn only_the_blank_slot_comes_from_the_wallet() {
        let ours = script(7, [0u8; 65], 9);
        let theirs = script(7, [5u8; 65], 9);
        assert_eq!(merge_slots(&ours, &theirs).unwrap(), theirs);
        // A wallet that also changed an argument gets our argument back.
        let sneaky = script(8, [5u8; 65], 9);
        assert_eq!(merge_slots(&ours, &sneaky).unwrap(), script(7, [5u8; 65], 9));
        // A server signature (not blank in our copy) stays ours even if the wallet blanked it.
        let signed = script(7, [3u8; 65], 9);
        assert!(merge_slots(&signed, &script(7, [0u8; 65], 9)).is_none());
        // Nothing signed, or a different length: refused.
        assert!(merge_slots(&ours, &ours).is_none());
        assert!(merge_slots(&ours, &theirs[..theirs.len() - 1]).is_none());
    }
}
