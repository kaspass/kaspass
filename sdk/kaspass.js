// KasPass SDK: ask a KasPass indexer whether an address holds an active pass.
//
//   import { hasActivePass } from "./kaspass.js";
//   if (await hasActivePass("http://127.0.0.1:8787", "kaspatest:qp...")) unlock();
//
// The indexer proves every pass against the chain before reporting it, and
// anyone can run their own indexer from a tier card. Point this at one you trust,
// ideally your own.

/**
 * Full answer: whether the address is active, plus its passes.
 * @param {string} apiUrl  Base URL of a KasPass indexer, e.g. "http://127.0.0.1:8787"
 * @param {string} address A Kaspa address (or a member's separate pass-key address)
 * @returns {Promise<{address: string, active: boolean, passes: Array<object>}>}
 */
export async function checkPass(apiUrl, address) {
  const url = `${apiUrl.replace(/\/$/, "")}/v1/check?address=${encodeURIComponent(address)}`;
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`KasPass indexer error ${res.status}: ${await res.text()}`);
  }
  return res.json();
}

/**
 * The one-line check: true if the address holds at least one active pass.
 * @param {string} apiUrl
 * @param {string} address
 * @returns {Promise<boolean>}
 */
export async function hasActivePass(apiUrl, address) {
  return (await checkPass(apiUrl, address)).active;
}

/**
 * The tier this indexer serves (covenant ID, creator, permanent settings).
 * @param {string} apiUrl
 */
export async function getTier(apiUrl) {
  const res = await fetch(`${apiUrl.replace(/\/$/, "")}/v1/tier`);
  if (!res.ok) throw new Error(`KasPass indexer error ${res.status}`);
  return res.json();
}

/**
 * Every live, verified pass in the tier.
 * @param {string} apiUrl
 */
export async function listPasses(apiUrl) {
  const res = await fetch(`${apiUrl.replace(/\/$/, "")}/v1/passes`);
  if (!res.ok) throw new Error(`KasPass indexer error ${res.status}`);
  return res.json();
}

// ---------------------------------------------------------------------------
// Checking a site's owner from the chain, without trusting the site.
//
// GET /v1/owner on a KasPass site gives `owner`, `record_id`, `vault` and
// `record_state: { owner_hash, recovery_hash, record_address }`. Anyone can host
// a copy of a site, so a careful partner checks the chain itself:
//
//   1. ownerHash(signedInAddress) === record_state.owner_hash
//   2. recordAddress(owner_hash, recovery_hash, network) gives the address the
//      owner record coin sits at; check (with a node that reports covenant ids)
//      that a live coin there has covenant id `record_id`.
//   3. vaultAddress(record_id, network) === the vault you count income for.
//
// No network calls and no dependencies. The script templates below are the
// compiled OwnerRecord and Vault contracts with their 32-byte constants zeroed;
// a test in membership-cli (src/sdk_tests.rs) fails if they ever drift from
// the contracts, and sdk/test-vectors.json holds values to check against.
// Hashes are 64 hex characters, the bytes in order.
// ---------------------------------------------------------------------------

/** OwnerRecord(owner, recovery), compiled, with both 32-byte hashes zeroed. */
export const RECORD_TEMPLATE = 
  "6b2000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000" +
  "000000000000000000000000000000000000006c7604d69149d2876375527982599f69755279bfaa52798769b9cb519c" +
  "69b900cc52795279012052797e012052797e7eb976c902280194765193bc7c7eb976c97602e580937cbc7eaa02000001" +
  "aa7e01207e7c7e01877e5379c3885279c2b9be9c69757575757575516776043e011f63876375547982599f6975537982" +
  "01209d7552798201209d755479bfaa788769b9cb519c69b900cc54795479012052797e012052797e7eb976c902280194" +
  "765193bc7c7eb976c97602e580937cbc7eaa02000001aa7e01207e7c7e01877e5379c3885279c2b9be9c697575757575" +
  "75757551676a6868";
/** Byte offset of the owner hash in RECORD_TEMPLATE. */
export const RECORD_OWNER_AT = 2;
/** Byte offset of the recovery hash in RECORD_TEMPLATE. */
export const RECORD_RECOVERY_AT = 35;
/** Vault(record_id), compiled, with record_id zeroed. */
export const VAULT_TEMPLATE = "7604d55d1cec8763757682599f697576cf20000000000000000000000000000000000000000000000000000000000000000087697551676a68";
/** Byte offset of record_id in VAULT_TEMPLATE. */
export const VAULT_RECORD_ID_AT = 18;

/**
 * The hash a KasPass contract keeps for a wallet: blake2b-256 of its
 * scriptPubKey, written as version (2 bytes, big-endian) then script.
 * @param {string} address "kaspa:..." or "kaspatest:..."
 * @returns {string} 64 hex characters
 */
export function ownerHash(address) {
  const { version, payload } = decodeAddress(address);
  let script;
  if (version === 0 && payload.length === 32) script = [0x20, ...payload, 0xac]; // Schnorr key, OP_CHECKSIG
  else if (version === 1 && payload.length === 33) script = [0x21, ...payload, 0xab]; // ECDSA key, OP_CHECKSIGECDSA
  else if (version === 8 && payload.length === 32) script = [0xaa, 0x20, ...payload, 0x87]; // script hash
  else throw new Error("unsupported address kind");
  return toHex(blake2b256(Uint8Array.from([0, 0, ...script])));
}

/**
 * The address an owner record coin sits at, in the state (owner_hash, recovery_hash).
 * @param {string} ownerHashHex
 * @param {string} recoveryHashHex
 * @param {string} network "mainnet" or "testnet-10"
 */
export function recordAddress(ownerHashHex, recoveryHashHex, network) {
  const script = fromHex(RECORD_TEMPLATE);
  script.set(hash32(ownerHashHex, "owner hash"), RECORD_OWNER_AT);
  script.set(hash32(recoveryHashHex, "recovery hash"), RECORD_RECOVERY_AT);
  return scriptHashAddress(script, network);
}

/**
 * A creator's payout vault address, from their owner record's covenant id.
 * @param {string} recordIdHex
 * @param {string} network "mainnet" or "testnet-10"
 */
export function vaultAddress(recordIdHex, network) {
  const script = fromHex(VAULT_TEMPLATE);
  script.set(hash32(recordIdHex, "record id"), VAULT_RECORD_ID_AT);
  return scriptHashAddress(script, network);
}

function scriptHashAddress(script, network) {
  return encodeAddress(prefixOf(network), 8, blake2b256(script));
}

function prefixOf(network) {
  if (network === "mainnet" || network === "kaspa") return "kaspa";
  if (network === "testnet-10" || network === "testnet" || network === "kaspatest") return "kaspatest";
  throw new Error(`unknown network "${network}" (use mainnet or testnet-10)`);
}

function hash32(hex, what) {
  if (typeof hex !== "string" || !/^[0-9a-fA-F]{64}$/.test(hex)) throw new Error(`${what} must be 64 hex characters`);
  return fromHex(hex);
}

function fromHex(hex) {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

function toHex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

// Kaspa addresses: prefix, ":", then version byte + payload in base32 with an
// 8-character checksum (the CashAddr scheme).
const CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

function polymod(values) {
  const gen = [0x98f2bc8e61n, 0x79b76d99e2n, 0xf33e5fb3c4n, 0xae2eabe2a8n, 0x1e4f43e470n];
  let c = 1n;
  for (const v of values) {
    const c0 = c >> 35n;
    c = ((c & 0x07ffffffffn) << 5n) ^ BigInt(v);
    for (let i = 0; i < 5; i++) if ((c0 >> BigInt(i)) & 1n) c ^= gen[i];
  }
  return c ^ 1n;
}

function convertBits(data, from, to, pad) {
  let acc = 0, bits = 0;
  const out = [], max = (1 << to) - 1;
  for (const v of data) {
    acc = (acc << from) | v;
    bits += from;
    while (bits >= to) {
      bits -= to;
      out.push((acc >> bits) & max);
    }
  }
  if (pad && bits > 0) out.push((acc << (to - bits)) & max);
  else if (!pad && (bits >= from || ((acc << (to - bits)) & max))) throw new Error("bad address padding");
  return out;
}

function prefixBits(prefix) {
  return [...prefix].map((c) => c.charCodeAt(0) & 31);
}

function encodeAddress(prefix, version, payload) {
  const data = convertBits([version, ...payload], 8, 5, true);
  const sum = polymod([...prefixBits(prefix), 0, ...data, 0, 0, 0, 0, 0, 0, 0, 0]);
  const check = [];
  for (let i = 0; i < 8; i++) check.push(Number((sum >> BigInt(5 * (7 - i))) & 31n));
  return `${prefix}:${[...data, ...check].map((d) => CHARSET[d]).join("")}`;
}

function decodeAddress(address) {
  const s = String(address).trim().toLowerCase();
  const i = s.indexOf(":");
  if (i < 1) throw new Error("not a Kaspa address");
  const prefix = s.slice(0, i);
  prefixOf(prefix);
  const words = [...s.slice(i + 1)].map((c) => {
    const d = CHARSET.indexOf(c);
    if (d < 0) throw new Error("not a Kaspa address");
    return d;
  });
  if (words.length < 9 || polymod([...prefixBits(prefix), 0, ...words]) !== 0n) throw new Error("bad address checksum");
  const bytes = convertBits(words.slice(0, -8), 5, 8, false);
  return { prefix, version: bytes[0], payload: bytes.slice(1) };
}

// blake2b with a 32-byte digest and no key (RFC 7693), on 64-bit BigInts:
// slow, but these functions hash a few hundred bytes at most.
const M64 = 0xffffffffffffffffn;
const IV = [
  0x6a09e667f3bcc908n, 0xbb67ae8584caa73bn, 0x3c6ef372fe94f82bn, 0xa54ff53a5f1d36f1n,
  0x510e527fade682d1n, 0x9b05688c2b3e6c1fn, 0x1f83d9abfb41bd6bn, 0x5be0cd19137e2179n,
];
const SIGMA = [
  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
  [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
  [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
  [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
  [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
  [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
  [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
  [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
  [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
  [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

function rotr(x, n) {
  return ((x >> n) | (x << (64n - n))) & M64;
}

export function blake2b256(input) {
  const h = IV.slice();
  h[0] ^= 0x01010000n ^ 32n; // digest length 32, no key, fanout 1, depth 1
  const blocks = Math.max(1, Math.ceil(input.length / 128));
  for (let b = 0; b < blocks; b++) {
    const block = new Uint8Array(128);
    block.set(input.subarray(b * 128, b * 128 + 128));
    const m = [];
    for (let i = 0; i < 16; i++) {
      let w = 0n;
      for (let j = 7; j >= 0; j--) w = (w << 8n) | BigInt(block[i * 8 + j]);
      m.push(w);
    }
    const last = b === blocks - 1;
    const t = BigInt(last ? input.length : (b + 1) * 128);
    const v = [...h, ...IV];
    v[12] ^= t & M64;
    v[13] ^= t >> 64n;
    if (last) v[14] ^= M64;
    const g = (a, bb, c, d, x, y) => {
      v[a] = (v[a] + v[bb] + x) & M64; v[d] = rotr(v[d] ^ v[a], 32n);
      v[c] = (v[c] + v[d]) & M64; v[bb] = rotr(v[bb] ^ v[c], 24n);
      v[a] = (v[a] + v[bb] + y) & M64; v[d] = rotr(v[d] ^ v[a], 16n);
      v[c] = (v[c] + v[d]) & M64; v[bb] = rotr(v[bb] ^ v[c], 63n);
    };
    for (let r = 0; r < 12; r++) {
      const s = SIGMA[r % 10];
      g(0, 4, 8, 12, m[s[0]], m[s[1]]); g(1, 5, 9, 13, m[s[2]], m[s[3]]);
      g(2, 6, 10, 14, m[s[4]], m[s[5]]); g(3, 7, 11, 15, m[s[6]], m[s[7]]);
      g(0, 5, 10, 15, m[s[8]], m[s[9]]); g(1, 6, 11, 12, m[s[10]], m[s[11]]);
      g(2, 7, 8, 13, m[s[12]], m[s[13]]); g(3, 4, 9, 14, m[s[14]], m[s[15]]);
    }
    for (let i = 0; i < 8; i++) h[i] ^= v[i] ^ v[i + 8];
  }
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++) out[i] = Number((h[i >> 3] >> BigInt(8 * (i & 7))) & 0xffn);
  return out;
}
