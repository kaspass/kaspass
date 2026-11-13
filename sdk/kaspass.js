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
