//! Wallet sign-in: verify KIP-5 personal-message signatures, with one-time nonces.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use kaspa_hashes::{HasherBase, PersonalMessageSigningHash};
use secp256k1::{Message, XOnlyPublicKey, schnorr::Signature};

/// KIP-5: schnorr signature over PersonalMessageSigningHash(message).
pub fn verify_kip5(message: &str, signature: &[u8], pubkey: &[u8]) -> Result<()> {
    let mut h = PersonalMessageSigningHash::new();
    h.update(message.as_bytes());
    let digest = h.finalize();
    let msg = Message::from_digest_slice(digest.as_bytes().as_slice())?;
    let sig = Signature::from_slice(signature).map_err(|_| anyhow!("signature is not a valid schnorr signature"))?;
    let pk = XOnlyPublicKey::from_slice(pubkey).map_err(|_| anyhow!("address does not hold a valid public key"))?;
    secp256k1::SECP256K1.verify_schnorr(&sig, &msg, &pk).map_err(|_| anyhow!("signature does not match this address"))
}

/// Accepts the encodings wallets commonly use: hex, or base64.
pub fn decode_signature(s: &str) -> Result<Vec<u8>> {
    let s = s.trim().trim_start_matches("0x");
    let mut out = vec![0u8; s.len() / 2];
    let bytes = if s.len() % 2 == 0 && faster_hex::hex_decode(s.as_bytes(), &mut out).is_ok() {
        out
    } else {
        base64_decode(s).ok_or_else(|| anyhow!("signature is neither hex nor base64"))?
    };
    match bytes.len() {
        64 => Ok(bytes),
        65 => Ok(bytes[..64].to_vec()), // some wallets append a sighash or recovery byte
        n => bail!("signature has {n} bytes, expected 64"),
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let table = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let clean: Vec<u8> = s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()).collect();
    let mut out = Vec::new();
    for chunk in clean.chunks(4) {
        let mut acc = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            acc |= table(*c)? << (18 - 6 * i);
        }
        let n = chunk.len() * 6 / 8;
        for i in 0..n {
            out.push((acc >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

pub fn random_token() -> String {
    let b: [u8; 32] = secp256k1::rand::random();
    faster_hex::hex_string(&b)
}

/// One-time sign-in nonces and short-lived sessions, kept in memory.
#[derive(Default)]
pub struct Sessions {
    nonces: HashMap<String, Instant>,
    sessions: HashMap<String, (String, Instant)>,
}

const NONCE_TTL: Duration = Duration::from_secs(300);
const SESSION_TTL: Duration = Duration::from_secs(3600);
/// Upper limits, so a flood of requests can't use up the server's memory. The oldest go first.
const MAX_NONCES: usize = 20_000;
const MAX_SESSIONS: usize = 20_000;

fn evict_oldest<V>(m: &mut HashMap<String, V>, max: usize, at: impl Fn(&V) -> Instant) {
    while m.len() >= max {
        let Some(k) = m.iter().min_by_key(|(_, v)| at(v)).map(|(k, _)| k.clone()) else { break };
        m.remove(&k);
    }
}

impl Sessions {
    pub fn new_nonce(&mut self) -> String {
        let now = Instant::now();
        self.nonces.retain(|_, t| now.duration_since(*t) < NONCE_TTL);
        evict_oldest(&mut self.nonces, MAX_NONCES, |t| *t);
        let n = random_token()[..32].to_string();
        self.nonces.insert(n.clone(), now);
        n
    }

    /// A nonce works once, within five minutes.
    pub fn take_nonce(&mut self, n: &str) -> bool {
        match self.nonces.remove(n) {
            Some(t) => t.elapsed() < NONCE_TTL,
            None => false,
        }
    }

    pub fn open(&mut self, address: &str) -> String {
        let now = Instant::now();
        self.sessions.retain(|_, (_, t)| now.duration_since(*t) < SESSION_TTL);
        evict_oldest(&mut self.sessions, MAX_SESSIONS, |(_, t)| *t);
        let token = random_token();
        self.sessions.insert(token.clone(), (address.to_string(), now));
        token
    }

    /// Signing out: the session stops working at once.
    pub fn close(&mut self, token: &str) -> bool {
        self.sessions.remove(token).is_some()
    }

    pub fn address_for(&self, token: &str) -> Option<String> {
        self.sessions.get(token).filter(|(_, t)| t.elapsed() < SESSION_TTL).map(|(a, _)| a.clone())
    }

    /// Open sessions with the seconds each has left, for a planned restart (see `handoff`).
    pub fn export(&self) -> Vec<(String, String, u64)> {
        self.sessions
            .iter()
            .filter_map(|(k, (a, t))| SESSION_TTL.checked_sub(t.elapsed()).map(|left| (k.clone(), a.clone(), left.as_secs())))
            .collect()
    }

    pub fn import(&mut self, list: Vec<(String, String, u64)>) {
        let now = Instant::now();
        for (k, a, left) in list.into_iter().take(MAX_SESSIONS) {
            let left = Duration::from_secs(left.min(SESSION_TTL.as_secs()));
            if let Some(start) = now.checked_sub(SESSION_TTL - left) {
                self.sessions.insert(k, (a, start));
            }
        }
    }
}

/// When the site restarts itself (after the creator changes what's on sale), people stay signed
/// in: open sessions go into a private file just before it stops, and the next start reads it
/// back and deletes it. A file older than five minutes is ignored.
pub const HANDOFF_FILE: &str = "sessions-handoff.json";

pub fn save_handoff(dir: &std::path::Path, sessions: &Sessions) {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let body = serde_json::json!({ "saved_s": at, "sessions": sessions.export() });
    let path = dir.join(HANDOFF_FILE);
    // Live session tokens: private from the moment the file exists.
    let _ = std::fs::remove_file(&path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    if let Ok(mut f) = o.open(&path) {
        use std::io::Write;
        let _ = f.write_all(body.to_string().as_bytes());
    }
}

pub fn load_handoff(dir: &std::path::Path, sessions: &mut Sessions) -> usize {
    let path = dir.join(HANDOFF_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else { return 0 };
    let _ = std::fs::remove_file(&path);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return 0 };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if now.saturating_sub(v["saved_s"].as_u64().unwrap_or(0)) > 300 {
        return 0;
    }
    let list: Vec<(String, String, u64)> = serde_json::from_value(v["sessions"].clone()).unwrap_or_default();
    let n = list.len();
    sessions.import(list);
    n
}

/// Checks the signed sign-in message: right site, right address, a fresh nonce.
pub fn check_message(message: &str, address: &str, host: &str, sessions: &mut Sessions) -> Result<()> {
    let field = |name: &str| message.lines().find_map(|l| l.strip_prefix(&format!("{name}: ")).map(str::trim));
    if field("Domain") != Some(host) {
        bail!("this sign-in message was made for a different site");
    }
    if field("Address") != Some(address) {
        bail!("this sign-in message is for a different address");
    }
    let nonce = field("Nonce").ok_or_else(|| anyhow!("sign-in message has no nonce"))?;
    if !sessions.take_nonce(nonce) {
        bail!("sign-in request expired or already used; please try again");
    }
    Ok(())
}

/// Self-test against the official KIP-5 test vector ("Hello Kaspa!", key 3, zero aux).
pub fn self_test() -> Result<()> {
    let sig = faster_hex::hex_string(&[
        0x40, 0xB9, 0xBB, 0x2B, 0xE0, 0xAE, 0x02, 0x60, 0x72, 0x79, 0xED, 0xA6, 0x40, 0x15, 0xA8, 0xD8, 0x6E, 0x37, 0x63, 0x27, 0x91,
        0x70, 0x34, 0x0B, 0x82, 0x43, 0xF7, 0xCE, 0x53, 0x44, 0xD7, 0x7A, 0xFF, 0x11, 0x91, 0x59, 0x8B, 0xAF, 0x2F, 0xD2, 0x61, 0x49,
        0xCA, 0xC3, 0xB4, 0xB1, 0x2C, 0x2C, 0x43, 0x32, 0x61, 0xC0, 0x08, 0x34, 0xDB, 0x60, 0x98, 0xCB, 0x17, 0x2A, 0xA4, 0x8E, 0xF5,
        0x22,
    ]);
    let pk = [
        0xF9, 0x30, 0x8A, 0x01, 0x92, 0x58, 0xC3, 0x10, 0x49, 0x34, 0x4F, 0x85, 0xF8, 0x9D, 0x52, 0x29, 0xB5, 0x31, 0xC8, 0x45, 0x83,
        0x6F, 0x99, 0xB0, 0x86, 0x01, 0xF1, 0x13, 0xBC, 0xE0, 0x36, 0xF9,
    ];
    verify_kip5("Hello Kaspa!", &decode_signature(&sig)?, &pk)?;
    if verify_kip5("Not Hello Kaspa!", &decode_signature(&sig)?, &pk).is_ok() {
        bail!("verifier accepted a wrong message");
    }
    Ok(())
}

#[cfg(test)]
mod handoff_test {
    use super::*;

    #[test]
    fn sessions_survive_a_planned_restart() {
        let dir = std::env::temp_dir().join(format!("kaspass-handoff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Sessions::default();
        let t = a.open("kaspatest:abc");
        save_handoff(&dir, &a);
        let mut b = Sessions::default();
        assert_eq!(load_handoff(&dir, &mut b), 1);
        assert_eq!(b.address_for(&t).as_deref(), Some("kaspatest:abc"));
        // Read once: the file is gone, so a later start doesn't bring them back.
        assert!(!dir.join(HANDOFF_FILE).exists());
        let mut c = Sessions::default();
        assert_eq!(load_handoff(&dir, &mut c), 0);
        // A stale file is ignored.
        std::fs::write(dir.join(HANDOFF_FILE), r#"{"saved_s":1,"sessions":[["x","kaspatest:abc",3000]]}"#).unwrap();
        assert_eq!(load_handoff(&dir, &mut c), 0);
        assert!(c.address_for("x").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
