#!/bin/sh
# Rebuilds ../walletconnect.js from the pinned packages in package-lock.json.
set -e
cd "$(dirname "$0")"
npm ci --ignore-scripts
npx esbuild entry.js --bundle --minify --format=iife --legal-comments=eof \
  --banner:js="/*! @walletconnect/sign-client 2.21.8 (WalletConnect, Apache-2.0), bundled for KasPass with esbuild. Licenses of the bundled packages are listed at the end of this file. */" \
  --outfile=../walletconnect.js
sha256sum ../walletconnect.js ../qrcode.js
