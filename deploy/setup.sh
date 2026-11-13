#!/usr/bin/env bash
# KasPass server setup for a fresh Ubuntu 24.04 machine (Oracle Cloud Always Free or any small VPS).
#
# Copy this script and your tier card to the server, then run:
#   bash setup.sh tier-card-XXXXXXXXXXXX.json
#
# What it does (safe to run again, for example to switch to another tier card):
#   1. Gets the KasPass program: uses ./membership-cli-linux if you copied one here (x86_64 only),
#      otherwise builds it from GitHub (Arm or x86_64; takes 10 to 20 minutes the first time).
#   2. Runs it as a background service that starts on boot and restarts if it stops.
#   3. Puts Caddy in front for HTTPS, on a free <your-ip>.sslip.io address (or your own domain:
#      KASPASS_DOMAIN=members.example.com bash setup.sh card.json).
#   4. Opens ports 80 and 443 in the server's own firewall.
#
# Every payment is signed in the payer's own Kaspa wallet. The server does keep two keys of its own in
# /var/lib/kaspass/keys: the voucher key (approves today's price) and the keeper wallet (pays auto-renewal
# fees). Keep that folder private and backed up.
set -euo pipefail

CARD="${1:-}"
if [[ -z "$CARD" || ! -f "$CARD" ]]; then
  echo "Usage: bash setup.sh tier-card-XXXXXXXXXXXX.json   (the tier card must be in this folder)"
  exit 1
fi
SUDO=""; [[ $EUID -ne 0 ]] && SUDO="sudo"
DATA=/var/lib/kaspass
BIN=/usr/local/bin/kaspass
# Which KasPass version to build: a release tag or commit (KASPASS_REF=v1.0 bash setup.sh ...).
KASPASS_REF="${KASPASS_REF:-main}"
step() { echo; echo "==> $*"; }

step "Installing system packages"
$SUDO apt-get update -qq
$SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy curl git ca-certificates >/dev/null

step "Getting the KasPass program"
if [[ -f ./membership-cli-linux && "$(uname -m)" == "x86_64" ]]; then
  $SUDO install -m 755 ./membership-cli-linux "$BIN.new"
  echo "Using the program you copied (membership-cli-linux)."
else
  $SUDO DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential pkg-config >/dev/null
  # Small machines need swap to compile.
  if [[ $(awk '/MemTotal/ {print $2}' /proc/meminfo) -lt 3000000 && ! -f /swapfile ]]; then
    echo "Adding 4 GB of swap for the build."
    $SUDO fallocate -l 4G /swapfile && $SUDO chmod 600 /swapfile && $SUDO mkswap /swapfile >/dev/null && $SUDO swapon /swapfile
    echo "/swapfile none swap sw 0 0" | $SUDO tee -a /etc/fstab >/dev/null
  fi
  if ! command -v cargo >/dev/null && [[ ! -x "$HOME/.cargo/bin/cargo" ]]; then
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
  fi
  export PATH="$HOME/.cargo/bin:$PATH"
  SRC="$HOME/kaspass-src"
  mkdir -p "$SRC"
  if [[ ! -d "$SRC/kaspass/.git" ]]; then git clone -q https://github.com/kaspass/kaspass "$SRC/kaspass"; fi
  git -C "$SRC/kaspass" fetch -q --tags origin && git -C "$SRC/kaspass" checkout -q "$KASPASS_REF"
  [[ "$KASPASS_REF" == "main" ]] && git -C "$SRC/kaspass" pull -q --ff-only
  # The SilverScript compiler is pinned to one commit in Cargo.toml and Cargo.lock.
  echo "Building (10 to 20 minutes the first time, much faster after)..."
  (cd "$SRC/kaspass" && cargo build --release --locked -q)
  $SUDO install -m 755 "$SRC/kaspass/target/release/membership-cli" "$BIN.new"
fi
$SUDO mv "$BIN.new" "$BIN"

step "Setting up the service"
id kaspass >/dev/null 2>&1 || $SUDO useradd --system --home-dir "$DATA" --create-home --shell /usr/sbin/nologin kaspass
$SUDO mkdir -p "$DATA"
CARD_NAME="$(basename "$CARD")"
$SUDO cp "$CARD" "$DATA/$CARD_NAME"
# Saved scan progress, if you copied it, saves re-reading the chain from launch.
for f in scan-cache-*.json; do [[ -f "$f" ]] && $SUDO cp -n "$f" "$DATA/" || true; done
$SUDO chown -R kaspass:kaspass "$DATA"
# Files copied from Windows can arrive read-only; the service must be able to save its progress.
$SUDO chmod -R u+rwX "$DATA"
# Member addresses, notes and chat links live here: only the service can read them.
$SUDO chmod 700 "$DATA"
$SUDO tee /etc/systemd/system/kaspass.service >/dev/null <<EOF
[Unit]
Description=KasPass website
After=network-online.target
Wants=network-online.target

[Service]
User=kaspass
WorkingDirectory=$DATA
ExecStart=$BIN serve --card $CARD_NAME --port 8787
Restart=always
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
UMask=0077
ReadWritePaths=$DATA

[Install]
WantedBy=multi-user.target
EOF
$SUDO systemctl daemon-reload
$SUDO systemctl enable -q kaspass
$SUDO systemctl restart kaspass

step "Opening ports 80 and 443 in the server's firewall"
for p in 80 443; do
  $SUDO iptables -C INPUT -p tcp --dport $p -m conntrack --ctstate NEW -j ACCEPT 2>/dev/null \
    || $SUDO iptables -I INPUT 1 -p tcp --dport $p -m conntrack --ctstate NEW -j ACCEPT
done
if command -v netfilter-persistent >/dev/null; then $SUDO netfilter-persistent save >/dev/null 2>&1 || true; fi

step "Setting up HTTPS"
if [[ -n "${KASPASS_DOMAIN:-}" ]]; then
  DOMAIN="$KASPASS_DOMAIN"
else
  IP="$(curl -fsS https://api.ipify.org || curl -fsS https://ifconfig.me)"
  DOMAIN="${IP//./-}.sslip.io"
fi
$SUDO tee /etc/caddy/Caddyfile >/dev/null <<EOF
$DOMAIN {
	reverse_proxy 127.0.0.1:8787
}
EOF
$SUDO systemctl enable -q caddy
$SUDO systemctl restart caddy

step "Waiting for the website to start"
for i in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:8787/v1/health >/dev/null 2>&1; then break; fi
  sleep 2
done
if curl -fsS http://127.0.0.1:8787/v1/health >/dev/null 2>&1; then
  echo "KasPass is running."
else
  echo "KasPass hasn't answered yet. See what it says with:  sudo journalctl -u kaspass -n 30"
fi

cat <<EOF

Your site:       https://$DOMAIN/
Your dashboard:  https://$DOMAIN/dashboard

(The first visit can take up to a minute while Caddy gets the HTTPS certificate.
 If it never loads, check that ports 80 and 443 are open in Oracle's Security List.)

Handy commands:
  sudo journalctl -u kaspass -f     watch the website's log (Ctrl+C to stop watching)
  sudo systemctl restart kaspass    restart it
  sudo systemctl status kaspass     is it running?
EOF
