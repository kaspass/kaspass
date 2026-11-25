//! The reference functions in `sdk/kaspass.js` (`ownerHash`, `recordAddress`, `vaultAddress`)
//! rebuild the owner record and vault scripts from hex templates. These tests keep the templates
//! in that file equal to what the contracts compile to, and keep `sdk/test-vectors.json` equal to
//! what this program works out, so the JS checks in `sdk/test.mjs` test against the real thing.
//!
//! After a contract change: `KASPASS_WRITE_VECTORS=1 cargo test --release sdk_` rewrites the
//! vectors and prints the new templates to paste into `sdk/kaspass.js`.

use super::*;

const SDK_JS: &str = include_str!("../sdk/kaspass.js");
const VECTORS: &str = include_str!("../sdk/test-vectors.json");

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Where each marker sits in the bytecode (each must appear exactly once).
fn find_once(code: &[u8], marker: &[u8; 32]) -> usize {
    let hits: Vec<usize> = code.windows(32).enumerate().filter(|(_, w)| *w == marker).map(|(i, _)| i).collect();
    assert_eq!(hits.len(), 1, "marker {:02x} found {} times", marker[0], hits.len());
    hits[0]
}

struct Templates {
    record: Vec<u8>,
    owner_at: usize,
    recovery_at: usize,
    vault: Vec<u8>,
    record_id_at: usize,
}

/// Compile both contracts with marker constants and blank them out.
fn templates() -> Templates {
    let (a, b, c) = ([0xa1u8; 32], [0xb2u8; 32], [0xc3u8; 32]);
    let mut record = owner_record(Hash::from_bytes(a), Hash::from_bytes(b)).unwrap().bytecode;
    let owner_at = find_once(&record, &a);
    let recovery_at = find_once(&record, &b);
    record[owner_at..owner_at + 32].fill(0);
    record[recovery_at..recovery_at + 32].fill(0);
    let mut vault = vault(Hash::from_bytes(c)).unwrap().bytecode;
    let record_id_at = find_once(&vault, &c);
    vault[record_id_at..record_id_at + 32].fill(0);
    Templates { record, owner_at, recovery_at, vault, record_id_at }
}

fn splice(t: &[u8], parts: &[(usize, &[u8; 32])]) -> Vec<u8> {
    let mut v = t.to_vec();
    for (at, h) in parts {
        v[*at..*at + 32].copy_from_slice(*h);
    }
    v
}

/// The value of `export const NAME = ...;` in kaspass.js, as a string with quotes and
/// concatenation removed.
fn js_const(name: &str) -> String {
    let start = SDK_JS.find(&format!("export const {name} =")).unwrap_or_else(|| panic!("{name} missing from sdk/kaspass.js"));
    let rest = &SDK_JS[start + name.len() + 15..];
    let end = rest.find(';').unwrap();
    rest[..end].chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

fn addr_of(spk: &ScriptPublicKey, prefix: Prefix) -> String {
    kaspa_txscript::extract_script_pub_key_address(spk, prefix).unwrap().to_string()
}

fn vectors() -> serde_json::Value {
    let mut records = Vec::new();
    // The simulator's funded test keys, as owners and recovery wallets, on both networks.
    let pairs = [(0x11u8, 0x22u8), (0x22, 0x33), (0x55, 0x66), (0x66, 0x11)];
    for (net, prefix) in [("testnet-10", Prefix::Testnet), ("mainnet", Prefix::Mainnet)] {
        for (i, (o, r)) in pairs.iter().enumerate() {
            let key = |s: u8| {
                let kp = Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[s; 32]).unwrap());
                Address::new(prefix, Version::PubKey, &kp.x_only_public_key().0.serialize())
            };
            let (owner, recovery) = (key(*o), key(*r));
            let oh = spk_hash(&pay_to_address_script(&owner));
            let rh = spk_hash(&pay_to_address_script(&recovery));
            let rec = owner_record(oh, rh).unwrap();
            let record_id = blake(format!("kaspass test record {i}").as_bytes());
            records.push(serde_json::json!({
                "network": net,
                "owner": owner.to_string(),
                "recovery": recovery.to_string(),
                "owner_hash": hex(oh.as_bytes().as_slice()),
                "recovery_hash": hex(rh.as_bytes().as_slice()),
                "record_address": addr_of(&rec.spk(), prefix),
                "record_id": hex(record_id.as_bytes().as_slice()),
                "vault_address": addr_of(&vault(record_id).unwrap().spk(), prefix),
            }));
        }
    }
    // The other two address kinds, for ownerHash().
    let kp = Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[0x33; 32]).unwrap());
    let ecdsa = Address::new(Prefix::Mainnet, Version::PubKeyECDSA, &kp.public_key().serialize());
    let p2sh = Address::new(Prefix::Testnet, Version::ScriptHash, blake(b"kaspass test script").as_bytes().as_slice());
    let others: Vec<serde_json::Value> = [ecdsa, p2sh]
        .iter()
        .map(|a| serde_json::json!({ "address": a.to_string(), "owner_hash": hex(spk_hash(&pay_to_address_script(a)).as_bytes().as_slice()) }))
        .collect();
    serde_json::json!({
        "about": "Test values for ownerHash(), recordAddress() and vaultAddress() in kaspass.js, worked out by membership-cli (src/sdk_tests.rs). Owners and recovery wallets are the simulator's test keys 0x11, 0x22, 0x33, 0x55 and 0x66 repeated; record ids are made up.",
        "records": records,
        "owner_hash_other_kinds": others,
    })
}

#[test]
fn sdk_templates_match_the_contracts() {
    let t = templates();
    if std::env::var("KASPASS_WRITE_VECTORS").is_ok() {
        println!("RECORD_TEMPLATE {}\nowner at {} recovery at {}", hex(&t.record), t.owner_at, t.recovery_at);
        println!("VAULT_TEMPLATE {}\nrecord_id at {}", hex(&t.vault), t.record_id_at);
    }
    assert_eq!(js_const("RECORD_TEMPLATE"), hex(&t.record), "sdk/kaspass.js RECORD_TEMPLATE is out of date");
    assert_eq!(js_const("VAULT_TEMPLATE"), hex(&t.vault), "sdk/kaspass.js VAULT_TEMPLATE is out of date");
    let at = js_const("RECORD_OWNER_AT");
    assert_eq!(at, t.owner_at.to_string());
    assert_eq!(js_const("RECORD_RECOVERY_AT"), t.recovery_at.to_string());
    assert_eq!(js_const("VAULT_RECORD_ID_AT"), t.record_id_at.to_string());
}

#[test]
fn sdk_templates_rebuild_any_state() {
    // Splicing values into the template gives exactly what the compiler gives, for any values
    // (so the template holds no value-dependent bytes).
    let t = templates();
    for i in 0..8u8 {
        let (o, r) = (blake(&[i, 1]), blake(&[i, 2]));
        let want = owner_record(o, r).unwrap().bytecode;
        assert_eq!(splice(&t.record, &[(t.owner_at, &o.as_bytes()), (t.recovery_at, &r.as_bytes())]), want);
        let id = blake(&[i, 3]);
        assert_eq!(splice(&t.vault, &[(t.record_id_at, &id.as_bytes())]), vault(id).unwrap().bytecode);
    }
    // All zeros and all ones too.
    for v in [[0u8; 32], [0xff; 32]] {
        let h = Hash::from_bytes(v);
        assert_eq!(splice(&t.record, &[(t.owner_at, &v), (t.recovery_at, &v)]), owner_record(h, h).unwrap().bytecode);
    }
}

#[test]
fn sdk_vectors_are_current() {
    let now = vectors();
    if std::env::var("KASPASS_WRITE_VECTORS").is_ok() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/test-vectors.json"), serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
        return;
    }
    let saved: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(saved, now, "sdk/test-vectors.json is out of date (KASPASS_WRITE_VECTORS=1 cargo test --release sdk_)");
}

#[test]
fn sdk_hash_text_is_plain_hex() {
    // /v1/owner writes record_id with Hash's own Display and the state hashes as plain hex:
    // both must be the bytes in order, which is what the JS reads.
    let h = blake(b"x");
    assert_eq!(h.to_string(), hex(h.as_bytes().as_slice()));
}

#[test]
fn record_state_matches_record_info() {
    let kp = |s: u8| Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[s; 32]).unwrap());
    let a = |s: u8| Address::new(net_prefix(), Version::PubKey, &kp(s).x_only_public_key().0.serialize()).to_string();
    let r = RecordInfo { record_id: blake(b"r").to_string(), owner: a(0x11), recovery: a(0x22) };
    let st = r.state().unwrap();
    let (o, rc) = r.hashes().unwrap();
    assert_eq!(st["owner_hash"], hex(o.as_bytes().as_slice()));
    assert_eq!(st["recovery_hash"], hex(rc.as_bytes().as_slice()));
    assert_eq!(st["record_address"], addr_of(&owner_record(o, rc).unwrap().spk(), net_prefix()));
    assert_ne!(st["owner_hash"], st["recovery_hash"]);
}
