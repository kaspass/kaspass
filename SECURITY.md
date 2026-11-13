# Security

KasPass handles real payments on Kaspa mainnet, so problems are taken seriously.

## Reporting a problem

Please report security problems privately, not in a public issue:

- On GitHub, open the **Security** tab of this repository and choose **Report a vulnerability**.

Include what you found, how to reproduce it, and what an attacker could do with it. You'll get an answer as soon as possible, and credit when it's fixed, if you'd like it.

## What's in scope

- The contracts in `contracts/` (membership, minter, owner record, vault, pay-in).
- The server: sign-in, purchases, price quotes (the voucher key), auto-renewal and promotions (the keeper wallet), the creator dashboard and the member check API.
- The website and the SDK in `sdk/`.

## What you have to trust (known limits)

KasPass is not audited. These are the limits of the current design, stated plainly:

- **The voucher key.** It approves today's price as a discount off each plan's on-chain ceiling. The contracts cap that discount at the plan's maximum, which is 99.99% on current plans, so whoever holds the key (the server, or a thief) could approve passes and renewals far below the real price until the creator replaces the key. It can also renew a member's prepaid pass at the full ceiling (up to 3 times the price) once per period; that money still only goes to the creator's vault. The server never shows the key's signatures to a wallet: each one is made when a payment is prepared, kept in the server's own copy (blanked in the copy the wallet signs), and only leaves the server when the payment is sent. Quotes work once and expire after 10 minutes. If a payment on chain carries an approval this server never sent, the owner gets an alert (purchases, lifetime and commitment passes, renewals, auto-renewals and free trials, including anything that landed while the server was down).
- **Prices come from exchange rates.** The server reads Kraken (KAS/USD, USD/CAD and EUR/USD) and checks KAS/USD against a second source (CoinGecko, then api.kaspa.org). Quotes wait (purchases and renewals don't go through) when the two disagree by more than 5%, when no second source has answered for 15 minutes, after a restart until a second source or the last saved rate confirms the first reading, and when USD/CAD or EUR/USD is outside a plausible range. A move of more than 20% in KAS (5% in a currency rate) is used only after it has held for 5 minutes, so a wrong rate that holds that long still gets through until the feed recovers.
- **The recovery wallet has the owner's powers.** Anything that can spend the owner record (the owner wallet or the recovery wallet) can withdraw the vault, change prices, give trials and change the server key. Keep both wallets for KasPass only, and never sign anything else with them using a partial signature type.
- **Pay from any wallet (QR code).** Version 1 of the pay-in contract doesn't check what a completed order bought, so someone watching the chain could take a QR payment before the server completes it. It's off on mainnet, and a fixed version is being built. Test networks only until then.
- **Self-serve free trials.** The server decides who may start one: one per wallet (counted from its records and from the chain), a wallet that has never held a pass here, on mainnet a wallet that sent a payment at least 30 days ago, and at most a set number of new trials per plan per day (20 unless the operator changes it; the owner is told when it's reached). The contract only checks that the server approved it. The first period a member sets aside stays theirs and can be taken back at any time, so a trial costs them only network fees, and someone patient with many old wallets can still collect several trials. Plans with self-serve trials can't have a limited number of passes. When a trial is charged rests on the server too: the contract lets the voucher key charge it once it's inside the auto-renew window, and the server only does so in the trial's last 15 minutes, never after the member pressed "Cancel my trial".
- **Sign-in is a signed message.** A fake site can ask you to sign KasPass's real sign-in message. Always check the "Domain:" line in your wallet before you sign.
- **Anyone can add to a pass's prepaid balance.** That's by design (gifts), but it also lets anyone move a pass coin, which can make a pending action fail and need a retry.
- **The private page link hides the page, not the data.** Plan names, prices and pictures stay readable through the public API.

## How KasPass is built to limit damage

- Members and creators sign every payment in their own wallet. Creator earnings go to the creator's vault, which only the creator's owner record can spend.
- The server holds two keys of its own: the voucher key, which approves today's price within a maximum fixed in the contract, and the keeper wallet, which pays network fees for auto-renewal and promotion passes. A creator can replace the voucher key at any time from the owner record. Keep only a small amount of KAS in the keeper.
