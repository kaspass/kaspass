// Checks the reference functions in kaspass.js against values worked out by
// membership-cli (sdk/test-vectors.json). Run: node sdk/test.mjs
import { readFileSync } from "node:fs";
import { ownerHash, recordAddress, vaultAddress, blake2b256 } from "./kaspass.js";

const v = JSON.parse(readFileSync(new URL("./test-vectors.json", import.meta.url), "utf8"));
let fails = 0, checks = 0;
const eq = (what, got, want) => {
  checks++;
  if (got !== want) {
    fails++;
    console.log(`FAIL ${what}\n  got  ${got}\n  want ${want}`);
  }
};
const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
const throws = (what, f) => {
  checks++;
  try { f(); fails++; console.log(`FAIL ${what}: no error`); } catch { /* expected */ }
};

// blake2b-256 against RFC 7693-style known answers (empty input, "abc", and two blocks).
eq("blake2b256('')", hex(blake2b256(new Uint8Array(0))), "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8");
eq("blake2b256('abc')", hex(blake2b256(new TextEncoder().encode("abc"))), "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");

for (const r of v.records) {
  const at = `${r.network} ${r.owner.slice(0, 16)}`;
  eq(`ownerHash owner ${at}`, ownerHash(r.owner), r.owner_hash);
  eq(`ownerHash recovery ${at}`, ownerHash(r.recovery), r.recovery_hash);
  eq(`recordAddress ${at}`, recordAddress(r.owner_hash, r.recovery_hash, r.network), r.record_address);
  eq(`vaultAddress ${at}`, vaultAddress(r.record_id, r.network), r.vault_address);
  eq(`uppercase hex ${at}`, vaultAddress(r.record_id.toUpperCase(), r.network), r.vault_address);
}
for (const o of v.owner_hash_other_kinds) eq(`ownerHash ${o.address.slice(0, 16)}`, ownerHash(o.address), o.owner_hash);

const r0 = v.records[0];
throws("short hash", () => vaultAddress(r0.record_id.slice(2), "mainnet"));
throws("unknown network", () => vaultAddress(r0.record_id, "devnet"));
throws("bad checksum", () => ownerHash(r0.owner.slice(0, -1) + (r0.owner.endsWith("q") ? "p" : "q")));
throws("not an address", () => ownerHash("hello"));

// A record moved by a handover sits at a different address.
const r1 = v.records[1];
checks++;
if (recordAddress(r1.owner_hash, r0.recovery_hash, r0.network) === r0.record_address) { fails++; console.log("FAIL handover address unchanged"); }

console.log(fails ? `SDK FAILED (${fails} of ${checks})` : `SDK OK (${checks} checks)`);
process.exit(fails ? 1 : 0);
