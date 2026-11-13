# Bundled scripts

Two files in `web/` aren't written by hand. This is where they come from, so anyone can check them.

| File | What it is | SHA-256 |
|---|---|---|
| `web/qrcode.js` | qrcode-generator 1.4.4 by Kazuhiko Arase (MIT), the package's `qrcode.js` unchanged | `18ae399f81182bc9de916e9c77b195df20cc58d6f2d55a62b085a299f1bf1780` |
| `web/walletconnect.js` | @walletconnect/sign-client 2.21.8 (Apache-2.0) and its dependencies, bundled with esbuild 0.25.10 | `aff9f35b331137567673038f365df166b97f242b9dd2d120f1d4387699ac32e4` |

## Check them

- `qrcode.js`: `npm pack qrcode-generator@1.4.4`, unpack it, and compare `package/qrcode.js` with `web/qrcode.js`.
- `walletconnect.js`: run `sh web/vendor/build.sh`. It installs the exact versions in `package-lock.json` (with `--ignore-scripts`), bundles `entry.js`, overwrites `web/walletconnect.js` and prints both checksums. They should match the table.

`entry.js` only exposes the WalletConnect sign client to the page as `window.KasPassWC`. The page loads `walletconnect.js` only when someone presses the Kaspire app button.
