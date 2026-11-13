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

## API

| Endpoint | Returns |
|---|---|
| `GET /v1/health` | Sync status and counts |
| `GET /v1/tier` | The tier card |
| `GET /v1/passes` | Every live, verified pass |
| `GET /v1/check?address=...` | `{ address, active, passes }` |
