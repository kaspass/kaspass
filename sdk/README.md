# KasPass SDK (JavaScript)

One check for any app: *does this address hold an active KasPass?*

```js
import { hasActivePass } from "./kaspass.js";

const API = "http://127.0.0.1:8787"; // a KasPass indexer

if (await hasActivePass(API, userAddress)) {
  showMembersArea();
} else {
  showJoinButton();
}
```

Also available: `checkPass(api, address)` for the full answer with pass details, `getTier(api)`, and `listPasses(api)`.

## Where answers come from

The SDK asks a KasPass indexer (`membership-cli serve`). The indexer only reports a pass after confirming that exact coin is unspent on-chain and that its fingerprint matches the pass terms, so it cannot invent or alter passes. Anyone can run their own indexer from a tier's public `tier-card.json`.

## Proving the address is really the user's

`hasActivePass` tells you whether an address holds a pass. Before unlocking anything, your app should also confirm the visitor controls that address, typically by asking their wallet to sign a short message (for example with Kaspire's message signing). The KasPass website already does this: `GET /v1/nonce`, then `POST /v1/login` with the wallet's signature, which returns a session (see the main README).

## Checking a site's owner from the chain

`GET /v1/owner` on a KasPass site says who owns it. Anyone can host a copy of a site, so an app that relies on that answer (for example one that counts a creator's KasPass income) can check it against the chain instead of trusting the JSON. Three functions in `kaspass.js` do the address work, with no network calls and no dependencies:

| Function | Gives |
|---|---|
| `ownerHash(address)` | The hash a KasPass contract keeps for a wallet: blake2b-256 of its scriptPubKey (2-byte big-endian version, then script). Works for Schnorr, ECDSA and script-hash addresses. |
| `recordAddress(ownerHash, recoveryHash, network)` | The address the owner record coin sits at, in that state. |
| `vaultAddress(recordId, network)` | The creator's payout vault: a pure function of the owner record's covenant id. |

`network` is `"mainnet"` or `"testnet-10"`. Hashes are 64 hex characters, the bytes in order.

The check, given `record_id`, `vault` and `record_state: { owner_hash, recovery_hash, record_address }` from `/v1/owner`:

1. `ownerHash(signedInAddress) === record_state.owner_hash`: the wallet the user signed in with is the owner the record holds.
2. `recordAddress(owner_hash, recovery_hash, network)`: a live coin at this address must carry covenant id `record_id`. This step needs a node or API that reports covenant ids on coins; it's the one that proves the hashes are the record's real, current state.
3. `vaultAddress(record_id, network) === vault`: the vault you count income for belongs to that record.

`recovery_hash` is the backup wallet's hash, never its address; it isn't secret (every spend of the record shows it on chain). After a handover the record moves to a new address with the same covenant id, so step 2 fails for the old state.

The script templates are exported too (`RECORD_TEMPLATE`, `RECORD_OWNER_AT`, `RECORD_RECOVERY_AT`, `VAULT_TEMPLATE`, `VAULT_RECORD_ID_AT`): the compiled contracts as hex with their 32-byte constants zeroed, and each constant's byte offset. To rebuild an address in another language, put the hashes at those offsets, take blake2b-256 of the script, and encode it as a version 8 (script hash) Kaspa address. `test-vectors.json` holds values worked out by `membership-cli` to check against, and `node test.mjs` checks this file against them. A test in `membership-cli` fails if the templates ever drift from the contracts; sites already set up keep their addresses for good.

## API

| Endpoint | Returns |
|---|---|
| `GET /v1/health` | Sync status and counts |
| `GET /v1/tier` | The tier card |
| `GET /v1/passes` | Every live, verified pass |
| `GET /v1/check?address=...` | `{ address, active, passes }` |
