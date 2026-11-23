#!/usr/bin/env bash
# Quick checks before a release: unit tests, every contract on a simulated chain, then the
# website's pages in a real browser against a local simulated site. See dev/README.md.
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${PORT:-8790}"
SITE="$(node -e 'console.log(require("os").tmpdir())')/kaspass-sim-telegram"

echo "== unit tests"
cargo test --release --quiet

echo "== contracts on a simulated chain"
cargo build --release --quiet
./target/release/membership-cli simulate > /tmp/kaspass-simulate.log 2>&1 || { tail -40 /tmp/kaspass-simulate.log; exit 1; }
grep -q "SIMULATION PASSED" /tmp/kaspass-simulate.log && echo "SIMULATION PASSED"

echo "== pages in a browser"
pkill -f "membership-cli simulate" 2>/dev/null || true
rm -rf "$SITE" && mkdir -p "$SITE"
# Made-up exchange rates for dollar prices, and a contact for the help page.
echo '{"kas_usd":0.2,"usd_cad":1.38,"eur_usd":1.08}' > "$SITE/sim-rates.json"
echo '{"telegram":"example_handle","email":"hello@example.com"}' > "$SITE/contact.json"
KASPASS_SIM_SERVE="$PORT" KASPASS_SIM_V5=1 KASPASS_SIM_TRIAL=1 KASPASS_SIM_TRIAL_MIN=10080 KASPASS_HOME_HOSTS=127.0.0.1 \
  ./target/release/membership-cli simulate > /tmp/kaspass-site.log 2>&1 &
SIM=$!
trap 'kill $SIM 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do curl -sf -o /dev/null "http://127.0.0.1:$PORT/" && break; sleep 3; done
sleep 8
node dev/pages.js "http://127.0.0.1:$PORT"

echo "== the dashboard, signed in, and stream mode"
node dev/dashboard.js "http://127.0.0.1:$PORT"
