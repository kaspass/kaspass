# The KasPass guide {#top}

Everything about KasPass in one place: how to run a membership from the dashboard, how members join and manage theirs, how the contracts work, and what they can and can't do. Each section stands on its own, so you can jump straight to the one you need.

- **Words used here:** a **plan** is what the site sells (a creator can rename it "tier", "level" and so on). A **membership** or **pass** is what a member holds. The **creator** runs the site. **KAS** is the Kaspa coin.

## Contents {#contents}

1. [What KasPass is](#what-is-kaspass)
2. [How it works: the pieces](#how-it-works)
3. [What's fixed and what can change](#fixed-vs-changeable)
4. [Money: prices, fees, deposits and refunds](#money)
5. [For members: joining and managing a membership](#members)
6. [For creators: the dashboard, tab by tab](#dashboard)
7. [Promotions and invite links](#promotions)
8. [Telegram, Discord and the holder club](#groups)
9. [Linking from another website](#partner-sites)
10. [Common messages and what to do](#messages)
11. [Safety, privacy and limits](#safety)
12. [Quick answers](#faq)

**About this guide:**

- **For AI assistants:** the same text is at [/guide.md](/guide.md) as plain Markdown. Each section has a stable anchor (for example `/guide#refunds`). When you answer from this guide, link the section you used.
- **Last reviewed:** update 85, November 17, 2026 (accuracy checked against the code by Fable). Updated for update 99.

## What KasPass is {#what-is-kaspass}

KasPass lets a creator sell memberships that live on the Kaspa blockchain instead of inside a company's database.

- **A member's pass sits in their own wallet.** It's a small contract on Kaspa with an end date, created when they buy.
- **Payments go straight to the creator,** into a payout vault only the creator can withdraw from. KasPass never holds anyone's money.
- **The contracts enforce the core rules on Kaspa itself:** where the money goes, how long a pass lasts, the most KAS it can ever be charged (its ceiling), the KasPass fee, and that only the member can take back their deposit. A pass can't be forged, because Kaspa refuses any pass the plan didn't issue.
- **The site's server handles the rest:** it signs each payment at today's price (within the ceiling), and applies sales, trial limits and the 30-day notice on price rises. See [what the server decides](#server-rules).
- **Websites and apps check one thing:** does this wallet hold an active pass for this plan? If yes, they unlock the content, the Telegram group or the Discord role.
- **Wallet:** members and creators use **Kaspire**, as a Chrome extension on a computer or as the Android app on a phone. There's no iPhone app yet; iPhone users join from a computer.
- **Status:** the code is public at github.com/kaspass/kaspass. It's new software and hasn't had an outside security review yet.

[Back to contents](#contents)

## How it works: the pieces {#how-it-works}

Each plan is a few small contracts that belong together.

| Piece | What it is | Who controls it |
|---|---|---|
| **Owner record** | One per creator. Names the creator's wallet and a separate **backup wallet**. Price changes, giving passes and other creator actions go through it. | The creator. The backup wallet can hand it to new wallets. |
| **Payout vault** | Where every payment lands. | Only the owner record's current owner can withdraw. |
| **Sales lane** (minter) | The only way into a plan. Holds the plan's prices (as KAS ceilings) and any supply limit. | Anyone can buy; only the creator can change prices. |
| **Pass** (membership) | One per member. Holds its owner, end date, the rates it was bought at, a refundable deposit and any prepaid balance. | The member. |
| **Fees wallet** (keeper) | A small wallet on the site's server. Pays network fees for auto-renewals and pays for free promotion and invite claims. | The server. Its key never leaves it. |
| **Server key** (voucher) | Signs each purchase and renewal at today's price. | The server. The creator can replace it (see [Payouts](#tab-payouts)). |

**Prices in dollars, paid in KAS.** A plan can be priced in CAD, USD, EUR or KAS. For a currency plan, the contract holds a KAS **ceiling** (by default 3 times today's amount), and the server signs each payment down to today's price, using Kraken exchange rates refreshed every minute. The server signs every payment at today's price, so a buyer pays today's price, not the ceiling. With no fresh exchange rate, nothing is sold and auto-renewals wait.

### What the contracts guarantee and what the server decides {#server-rules}

- **The contracts guarantee:** payments go to the creator's vault (minus the fee), a pass lasts exactly the period paid for, no payment can exceed the pass's KAS ceiling, the deposit and any prepaid balance can only be taken back by the member, and the creator can't end a paid pass early.
- **The server decides, using its key:** today's price within the ceiling, sales and holder discounts, the 30-day notice before a price rise reaches renewals, free-trial limits, and who may claim a promotion. A server that misbehaved (or whose key was stolen) could charge up to a pass's ceiling, including from a prepaid balance, but never more. That's why the creator can [replace the server key](#tab-payouts), and why members should only prepay what they need.

**Network:** each site runs on Kaspa mainnet (real KAS) or Testnet-10 (test KAS, for trying things). The strip at the top of every page says which.

[Back to contents](#contents)

## What's fixed and what can change {#fixed-vs-changeable}

This is the most important part to understand before launching a plan.

### Fixed forever when a plan is created {#fixed-at-launch}

These are written into the plan's contracts. To offer something different, create a new plan.

- **Billing period** (for example 30 days or 1 year).
- **How it's priced:** in KAS, or in a currency (CAD, USD, EUR).
- **Whether it has a lifetime price or a commitment price at all.** Their amounts can change later, but they can't be added to a plan that was created without them.
- **Free trial**: whether it has one, how long, and whether members can start it themselves.
- **Refundable deposit** (the bond each pass holds, usually 1 KAS).
- **Supply limit** (for example 100 founding members), or no limit.
- **Resale**: whether passes can be given or sold to someone else, and the creator's royalty on each transfer.
- **Prepay cap** and the **auto-renew window**.
- **Largest holder discount** the plan allows.
- **The KasPass fee** (2%) and where it goes.

The dashboard lists these for each plan under **Plans › Set when it was created**.

### Can change any time {#changeable}

- **Prices**, including lifetime and commitment prices. Saving a price needs no wallet approval. See [prices and existing members](#price-changes).
- **The KAS ceiling** (the most KAS a member can be charged): one approval in Kaspire, never below today's price, for new members only.
- **Sales**: a percent off for new purchases, with an optional end date.
- **The founding (early) price** and its end date.
- **Names and wording**: the plan's name, description, what it includes, the creator's name, the club name, the word for plans.
- **The page**: tagline, intro, links, pictures, colours, card style, highlighted plan, how prices are shown.
- **Which plans are on sale**, and their order.
- **Promotions, invite links, Telegram groups, Discord roles, holder club rules.**
- **The server key** (replaceable if it may have been exposed).
- **Who owns the plans**: the backup wallet can hand the owner record to new wallets. Members do nothing and keep paying the same vault.

### What a creator can never do {#creator-cant}

- **Take back a paid membership early.** Once a member has paid for a period, it's theirs until it ends.
- **Take a member's deposit or prepaid balance.** Only the member can withdraw those.
- **Charge more than a pass's KAS ceiling.** (The 30-day notice before a price rise reaches renewals is applied by the server; see [what the server decides](#server-rules).)
- **Change a fixed setting** on an existing plan (see the list above).
- **Refund through KasPass.** Payments are final on the blockchain; a creator who wants to refund someone sends them KAS from their own wallet.

### Prices and existing members {#price-changes}

- **Plans priced in KAS:** every pass keeps the rate it was bought at.
- **Plans priced in a currency:** renewals are charged at that day's price in the plan's currency, converted to KAS, but never above the pass's own KAS ceiling. A price cut reaches renewals at once; a rise only after 30 days' notice.
- **Switching a plan's currency** (say CAD to USD) is only possible while the plan has no members, because members' renewals are promised in the plan's currency. With members, change the amount instead (a rise still waits 30 days), or launch a new level in the other currency.
- **The ceiling:** each pass keeps the ceiling it was bought with, and changing the ceiling only affects new members. If KAS falls so far that today's price is above a pass's ceiling, that pass renews at its ceiling, and the creator receives less than the plan's price.
- **Sales** apply to new purchases only; renewals stay at the regular price.

[Back to contents](#contents)

## Money: prices, fees, deposits and refunds {#money}

### What a member pays {#what-members-pay}

- **The plan's price**, in KAS at today's rate. A price shown on the page is held for 10 minutes.
- **A refundable deposit**, usually 1 KAS, held inside their own pass. They get it back when they end their membership.
- **A tiny network fee** for each payment.
- **Optional prepaid months** for auto-renew, held inside their own pass (see [auto-renew](#auto-renew)).

### The KasPass fee {#fee}

- **2% of each payment**: sales, renewals, upgrades and resale royalties. Free trials, promotions' gifts and ending a membership pay nothing.
- **It comes out of the creator's share**, not on top: a 200 KAS payment sends 196 KAS to the creator and 4 KAS to KasPass. The buyer pays the same either way.
- **Skipped on small payments:** fees under 0.5 KAS aren't charged (payments under 25 KAS), because tiny outputs cost more than they're worth on Kaspa.
- **Enforced by the contracts**, so it can't be skipped or changed after launch. There's no setup fee and no monthly fee. (A site can be set up fee-free; its plans then pay no fee at all.)

### Other payment options {#payment-options}

- **Lifetime price:** one payment, never renews.
- **Commitment price:** several periods paid up front (2 to 120), often at a discount. Not refunded if the member leaves early; afterwards the pass renews at the regular price.
- **Founding (early) price:** a lower price for purchases made before a set date (it ends at the end of that day in your time zone when you saved it). On the cards it shows as "Early price until …", with that same date for every visitor, wherever they are.
- **Holder discount:** members who qualify in the [holder club](#holder-club) pay less, on purchases and at every renewal.
- **Upgrade credit:** moving to a higher plan credits the unused part of the current one, at what was paid for it (at most one period at that plan's regular price).
- **Sales and discounts together** never take a purchase more than 95% off.

### Refunds {#refunds}

Payments go straight to the creator, so KasPass can't refund them. A member who wants money back should contact the creator, whose links are on the membership page. These always come back to the member:

- their **deposit**, when they end their membership;
- any **prepaid KAS** (auto-renew) not yet used;
- a **free trial's money**, if they cancel before the trial ends.

### Prices shown in other currencies {#currency-display}

- Every amount shows its currency: CA$10, US$7, €6, never a bare "$".
- **On the plan cards, the big number is today's KAS,** with the Kaspa mark, because that's what you actually pay. The plan's own price sits small underneath, for example "Set at CA$100 a year, so the KAS amount follows today's rate": the price in the plan's currency is the one that's fixed, and the KAS figure follows the exchange rate (the same words as on kaspa.vision).
- A creator can add a small line under each card's price for visitors outside the plan's country: "about US$7 in your currency" (US visitors), "about €6 in your currency" (euro countries), or "about US$7" (everyone else). It's worked out on the visitor's device from its language and time zone.

[Back to contents](#contents)

## For members: joining and managing a membership {#members}

### Get a wallet {#get-wallet}

- **On a computer:** add the free Kaspire extension to Chrome (or another browser that runs Chrome extensions) from kaspire.kaslab.space.
- **On an Android phone:** install the Kaspire app from kaspire.kaslab.space.
- **On an iPhone:** there's no Kaspire app yet. Open the page on a computer instead.
- **Write down the recovery words** and keep them offline. They're the only way back into the wallet, and the membership lives there.

### Get KAS {#get-kas}

Buy KAS on a crypto exchange, then send it to your Kaspire address (Receive in Kaspire; always copy and paste the address). Send the price plus about 2 KAS: 1 KAS for the refundable deposit and a little for network fees.

### Join a plan {#join}

1. On the membership page, press **Choose this plan** on the plan you want, or arrive from the creator's website with the plan already chosen.
2. **Sign in with Kaspire.** It's free: Kaspire asks you to approve a short message, and nothing is paid. On a computer you can also use **Use the Kaspire app on my phone instead** and scan the code.
3. The pay box opens beside your plan. It shows what you get, the price, the refundable deposit and the total. Tick **Prepay future … (auto-renew, optional)** (months, years and so on, following the plan) if you'd like it to renew itself.
4. Press **Pay … KAS in Kaspire**. Right under the button the page says "You're approving … KAS"; the outputs to match with Kaspire are under **Match these with Kaspire**, and the button itself shows each step (Waiting for Kaspire, Sending, Sent, confirming). Check the amounts match, then approve.
5. Your membership appears within a few seconds. If you came from the creator's site, a button takes you back; otherwise, if the creator has a website, **Open <their site>** appears.

**Paying from another wallet or an exchange (test networks only for now):** on plans priced in a currency, sign in, pick the plan, and choose **Pay from another wallet (QR code)**. Send the exact amount shown. The membership goes to the wallet you're signed in with; if the payment can't become a membership (too little or too late), it comes back automatically after an hour. It's switched off on mainnet until its new contract has been reviewed.

### Free trials {#trials}

Some plans let you try them free.

- **To start:** press **Start my free trial**. You set aside your first period inside your own pass (plus 10% in case the KAS price moves) and the deposit. Nothing is paid to the creator yet.
- **Changed your mind?** Press **Cancel my trial** before it ends. Everything you set aside comes straight back, and you keep access until the trial ends.
- **Staying?** Do nothing. At the end of the trial your first period is taken from what you set aside.
- **Limits:** one free trial per wallet, only for wallets that have never held a pass on the site, on mainnet only for wallets that sent a payment at least 30 days ago, and at most 20 new trials per plan per day.

### Renewing {#renewing}

- **No subscription.** Nothing is taken automatically unless you choose to prepay. When a membership ends, it simply stops.
- **Renew by hand:** near the end date a **Renew** button appears on your plan. If your membership has already ended, renewing brings it back.

### Auto-renew (prepaid months) {#auto-renew}

- Set some KAS aside and your membership renews itself, one period at a time, near the end of each period.
- The KAS stays in **your own pass's contract**, not with KasPass or the creator. It can only pay your renewals, one at a time, and you can take the rest back any time.
- On plans priced in a currency, choose a cushion in case KAS drops: none, 10% (recommended) or 25%.
- If the balance doesn't cover a renewal when it's due, nothing is charged and the membership ends normally.
- Only keep enough there for your renewals. It isn't a bank account.
- In the pay box the options (how many renewals, the cushion) only open once you tick **Prepay future months**; choosing a number doesn't tick it for you.
- A membership with a prepaid balance shows "Renews by itself before <date>" and no Renew button on its card; renewing early is still under **Manage membership**.

### Moving to a higher plan {#upgrading}

Buy the higher plan. The unused part of your current one counts toward its price. Afterwards the page offers to end the lower one and give its deposit back. On plans priced in KAS there may also be **Upgrade to lifetime**.

### Cancel or end a membership {#cancel}

- **You don't need to do anything to cancel.** A membership ends on its end date unless you renew.
- **Stop auto-renew:** Manage your membership › **Cancel prepaid months**. Unused prepaid KAS comes back; access stays until the end date.
- **End it early:** Manage your membership › **End membership now**. Access stops at once; the time paid for isn't refunded. The deposit (and any prepaid KAS) comes back. You press twice, so it can't happen by accident.
- **After it has ended:** the same place offers **Get my deposit back**.

### Give your membership to someone else {#give}

Only if the plan allows resale: Manage your membership › **Give it to someone else**, then paste their Kaspa address or a .kas or .k name. They get the rest of your time and you lose access. It can't be undone. Some creators receive a small royalty when a membership changes hands; the page shows it first.

### Linked wallets {#linked-wallets}

On the holder club page you can add more wallets you own (**Add another wallet**, then switch Kaspire to that wallet and sign). Their NFTs and tokens count together for the holder club and promotions, and a wallet belongs to one member at a time.

### New phone or computer {#new-device}

Install Kaspire and restore your wallet with its recovery words. Your membership is still in it.

[Back to contents](#contents)

## For creators: the dashboard, tab by tab {#dashboard}

The dashboard is at `/dashboard` on the site. **Sign in with your wallet**: the one that created the plans (Kaspire on a computer, or the Kaspire app on a phone). If you see **This membership belongs to another wallet**, Kaspire is on a different wallet: switch to the creator's wallet in Kaspire and sign in again. Every action that changes something on the blockchain is approved in Kaspire; settings like names and the page's look save straight away. Every setting has a "?" tip.

On a test network a **Testnet-10** badge sits beside the KasPass name; mainnet sites show no badge.

The plain words the dashboard uses: the keeper is the **fees wallet**, the recovery wallet is the **backup wallet**, setting up the owner record is **Turn on payouts**, and closing a pass is **ending a membership**.

### First-time setup {#setup}

The Overview's **Get set up** list walks through it (about 10 minutes):

1. **Turn on payouts** (Payouts tab): choose a backup wallet, a separate wallet (even a paper one) kept somewhere safe. This creates the owner record and the payout vault, and costs about 1 KAS.
2. **Create your first plan** (Plans tab): name, price, billing period, and the settings that are [fixed at creation](#fixed-at-launch). Each plan locks about 1 KAS in its contract. Putting it on sale restarts the site to load it; the page is unavailable for about a minute.
3. **Connect your group and share your link**: Telegram or Discord (Groups tab), then send people your page.

Also worth doing: **top up the fees wallet** (needed for auto-renewals and free claims, see [Fees wallet](#fees-wallet)), write your page, and turn on sale alerts.

**Keep the backup wallet safe.** Whoever holds it can take over ownership of your plans and your vault, a handover can't be undone, and the backup wallet itself can't be changed from the dashboard.

### Overview {#tab-overview}

- **The payout vault:** what's waiting to be withdrawn, with a **Withdraw** button (only your wallet can take it out; Kaspire asks first).
- **The numbers:** earned so far, members now, renewals, upgrades, members on auto-renew. All read from the blockchain.
- **Still to set up:** what's left, with a button for each. If you don't run a Telegram group, **I don't have one** takes that step off the list.
- **Messages from your website:** notes from the "Get your own page" form (on KasPass's own site).
- **Latest activity:** the most recent payments across every plan, newest first, each with its date, what went to you and the KasPass fee. "Sale" is a regular purchase of one period, whatever its length. A free month paid by your fees wallet shows as **Free month (promotion)** or **Free month (invite link)**.
- **Members by plan:** one bar split by plan. Each plan's part has its own fill (solid, stripes, pale, dots), so they're easy to tell apart in any theme.

### Your page {#tab-page}

**Words and links:**

- **How members see you:** your display name, the club name (for a site with several plans), the word for what members hold ("membership", "pass", "subscription") and the word for plans ("plan", "tier", "level").
- **Top of your page:** a one-line tagline (shown as the page's heading), an intro (with headings, lists, links and quotes), and links to your website, X, TikTok, YouTube, Instagram, Telegram and Discord.
- **Plans on your page:**
  - **Show them as:** cards side by side, or a comparison table. Visitors can switch.
  - **Card style:** **KasPass** (compact cards whose details fold away), **Open** (every plan's details shown, in your look) or **Kaspa Vision** (Open, in Kaspa Vision's exact colours and fonts). The quick help, this guide and the holder club page use the same style and colours. Kaspa Vision's style has a day and a night look: visitors start in their device's own and can switch with the sun and moon button.
  - **Highlight a plan:** a thicker outline and the main button, like "best value" (Open and Kaspa Vision styles).
  - **How visitors see prices:** **My currency only**, or **Mine, plus the visitor's** (adds "about … in your currency" for visitors outside your country).
  - **Show how many members have joined**, and **Show a "Join the holder club" link on this page** (only when you have a [holder club](#holder-club); greyed out until then).

**Look and brand:** logo, profile photo, banner, colours (your own, or matched to your website), fonts, and a preview of your page.

### Plans {#tab-plans}

- **What you sell:** every plan on the site. Switch on the plans you want to sell, then **Save changes**; they're sold side by side, cheapest first. Each plan includes everything in the plans below it.
- **This membership:** pick a plan, then edit its **name**, **what it includes** (one line per feature, up to 8, shown on its card), its **badges** (see below) and **about it** (the short description under its name). Writing `{tax_year}` in a name or feature shows the current tax year (it moves on every November 1, Pacific time); `{next_tax_year}` shows the next one.
- **Badges on its card:** small labels above the price. Until you change them, a plan shows the automatic ones, worked out from your prices: **Save N% vs monthly** on a longer plan that costs at least 5% less than paying for the shorter one, and **Early price until …** while an [early price](#tab-prices) runs. **Add a badge** starts your own list from the ones showing; then each plan can have up to 3, each **Your text** (up to 24 characters, for example "Most popular"), **Savings vs a plan** you pick, or **Early price**, in one of six colours (your accent, gold, green, red, purple, grey) that read in both the day and night looks, in the order you set. Each row shows a preview of the badge in its colour as you edit it (dimmed when it wouldn't show right now, for example **Early price** with no early price running). **Save** shows them. An empty list hides every badge on that plan; **Back to automatic** returns to the automatic ones.
- **"This is the same plan as …, just paid a different way":** tick it when a plan gives the same access as another and is only billed differently (for example yearly instead of monthly).
- **Set when it was created:** the plan's [fixed settings](#fixed-at-launch).
- **Add a plan:** name it, write what it includes, set its prices and fixed settings, and create it. Self-serve free trials ("Members can start the free trial themselves") are chosen here and can't be combined with a supply limit.

### Members {#tab-members}

- **Members:** every live pass, with the member's .k or .kas name and wallet, time left, who paid, and **your note** (a private note per member, saved as you type, never shown to anyone else). A wallet shows "Wallet not seen yet" until the member signs in or buys on the site.
- **Activity:** every sale, renewal, upgrade, trial and royalty, with your share and the fee. On a phone each payment is two lines: the date and what it was, then what was paid and your share.
- **All plans or one:** with several plans, Members opens on **All plans** (every plan's members and activity together, with a Plan column), the same count as the side menu. Pick a plan in the picker at the top to see only that one.
- **Give a membership:** for giveaways, staff, or someone who paid another way. Type an address or a .kas or .k name, pick the **Plan** (shown when you have several; it's the same as the picker at the top), then what to give: a free trial (you fund only its refundable deposit), one paid period named from the plan (**One month**, **One year**), or lifetime when the plan has a lifetime price. A paid one comes from your own wallet at today's price and goes back to your vault, less the KasPass fee, so you need the full price in your wallet to start.

### Prices & sales {#tab-prices}

- **Price:** change the price, plus lifetime, commitment and founding prices, with **Save price**. No wallet approval is needed. How it reaches existing members: [prices and existing members](#price-changes).
- **Advanced: the most KAS a member can be charged:** the ceiling, changed with **Change ceiling** (one approval in Kaspire, never below today's price, new members only).
- **Run a sale:** a percent off (1% to 90%), with an optional name and end date, for new purchases only. Nothing to approve. The page shows the regular price crossed out. A sale is for the plan picked at the top; tick **Run it on all … plans** to start the same sale on every plan at once.
- **Holder discount:** a lasting percent off this paid plan for members who qualify in your [holder club](#holder-club), on joining and at every renewal (0 = off). It needs a holder club first. The most it can be is the plan's largest allowed discount, set when the plan was created (usually 99.99%: never 100%, because Kaspa needs every payment to be at least a little KAS). It isn't the free month: that's a [holder promotion](#promotions).
- Warnings appear before you sign a price that doesn't make sense for buyers (for example a commitment that costs more than lifetime).

### Promotions {#tab-promotions}

- **Fees wallet:** its address and balance (see [Fees wallet](#fees-wallet)).
- **Holder promotion** and **Invite links:** see [Promotions and invite links](#promotions).

### Groups {#tab-groups}

Telegram group, Discord server and holder club: see [Telegram, Discord and the holder club](#groups).

### Payouts {#tab-payouts}

- **Payouts:** turn on payouts (owner record and backup wallet) and withdraw from the vault. Under **Advanced: server key**, **Replace server key** if it may have been exposed; then **Move to the new key** moves existing plans and passes over, several per approval. Members do nothing.
- **Payments for your taxes:** **Download payments (spreadsheet)**: one row per member payment with the date, plan, kind (purchase, upgrade, renewal, auto-renewal, gift, promo), KAS paid and the price in your currency. No member addresses. It's a record for your books, not tax or legal advice: ask an accountant how to report it where you live.
- **Hand over ownership:** the backup wallet can give the owner record to new wallets. Existing passes keep paying the same vault.
- If a payment ever went through below the server's price, you get a message and it's listed in this tab.

### Settings {#tab-settings}

- **Who can see your page:** everyone, or only people with a private link.
- **Sale alerts on Telegram:** a message from the site's bot when someone joins, and optionally on renewals and upgrades. **Connect my Telegram** links your chat.
- **Display:** whether the Overview shows exact KAS amounts, with decimals, and [stream mode](#stream-mode).
- **Dashboard theme** and **Effects:** colours for the dashboard itself (only you see them), day and night, and optional glow and background pattern.
- **Technical details** and **Launch a new membership.**

### Stream mode {#stream-mode}

For sharing your screen on a live stream or a call, or recording a video.

- **Turn it on:** the **Stream mode** switch at the bottom of the side menu (also in Settings › Display). A badge at the bottom right says it's on. Other open tabs of the same site turn on too.
- **What it hides:** wallet addresses, .kas and .k names, KAS and money amounts, dates, times and time left, transaction ids, the big numbers and counts on the Overview and in Promotions, members' notes, Telegram and Discord names, holder club figures, QR codes and payment links. Each becomes a grey bar of the same width, so its length gives nothing away. Boxes with something in them are covered, and so are tooltips and link addresses that hold private details.
- **Where:** the dashboard, and your membership page and holder club page in the same browser. It's set per site address (pass.example.com and www.pass.example.com are separate). A link ending in `?stream=1` turns it on; no link can turn it off.
- **Showing one thing:** click its grey bar, or **Reveal** beside an address. It hides again by itself after 10 seconds. A box you click into shows while you type.
- **Turning it off** takes two presses, a moment apart (the badge's **Turn off**, then **Press again to show everything**), so a slip or a double-click can't show everything.
- **What it can't hide:** anything outside the page, like your wallet's own windows, browser tabs and bookmarks, the address bar, notifications, or other apps.
- **On the day of a stream:**
  1. Sign in to Kaspire and the dashboard before you go live.
  2. Share one browser tab, not the whole screen.
  3. Turn stream mode on at each site address you'll open, and check each tab shows the badge.
  4. Don't open links from the chat.
  5. Don't use Upload buttons or "Connect my Telegram" on air.
  6. Keep the browser in English: dates in other languages are only partly recognised.

### Fees wallet {#fees-wallet}

- A small wallet on the site's server. It pays the network fees for auto-renewals (without it, auto-renewals can't run), and pays for each free claim from a promotion or invite link (usually about 2.5 KAS per claim: about 1 KAS goes into your own vault, 1 KAS is the member's refundable deposit, plus fees; more when the plan's largest discount can't bring its price down to about 1 KAS).
- **Send it a little KAS now and then.** Its address and balance are in the Promotions tab.
- **When it runs low:** free claims pause by themselves and you get one message. Paid purchases are not affected; they're paid by the buyer. Top it up and claims restart on their own.

[Back to contents](#contents)

## Promotions and invite links {#promotions}

### Holder promotion {#holder-promotion}

A free period for holders of an NFT or token, for example "Migrated NACHO to KCC-20? Claim a free month on us."

- **Set it up** (Promotions › Holder promotion): pick the plan, write the line shown on its card, choose who qualifies, and set the start, the end (up to a year) and the most free passes to give.
- **Who qualifies:** at least N NFTs of a KRC-721 collection, N tokens of a KRC-20 token, N tokens of a KCC-20 token (by its covenant id, so a look-alike never counts), tokens migrated to KCC-20 through the KAT Bridge (optionally since a date), or tokens burned. Combine rules with "all of these" or "any one of these".
- **On the page:** the plan's card shows the offer. Signed in, a member who qualifies sees **Claim my free month**; others see what their wallet holds. The offer box glows while that wallet can claim, and the plan's pay box shows the free month as a minus line, so the total is nothing (or only what they choose to prepay).
- **The claim:** the fees wallet buys the member a normal pass on that plan. You approve nothing. The pass doesn't auto-renew; members renew at the regular price if they want to stay.
- **Limits:** one claim per member (their wallet plus linked wallets) per promotion; not for anyone who already has that plan or a higher one; each NFT claims once; listed NFTs don't count. For token rules a claim limit is required, because tokens can be moved to new wallets. Migration history can't be moved, so the KAT Bridge rule is much harder to farm.
- **Picture or emoji:** upload a picture (for example the token's logo) or pick an emoji to show beside the offer.
- **Claims so far:** listed under the promotion with when, the wallet, what counted (for example "1 KAT Bridge migration") and the transaction.
- **One holder promotion at a time.** "Start a new campaign" starts the next one fresh. "End it now" stops it; claims already made stay.

### Invite links {#invite-links}

A free period for people you choose, without being there when they claim.

- **Set it up** (Promotions › Invite links): pick the plan, write the line, set the last day the links work (up to a year), and either list who they're for or just say how many. Each person gets their own link.
- **The claim:** the person signs in and presses **Claim my free month**. Their wallet can be empty; the fees wallet pays. The pass doesn't auto-renew unless they prepay: in the pay box they can tick **Prepay future months**, and the button becomes **Claim my free month, then prepay X KAS**. The free month is sent first (no approval), then Kaspire asks them to approve only the prepay. If they decline, they keep the free month.
- **Each link works once.** It's used at the claim, not at sign-in. A wallet that already has the plan is refused and the link stays unused. You can cancel unused links; cancelled and expired links fold into a closed bar under the others.
- **With a partner site:** "After claiming, send them to" sets where the done screen's button goes.
- Invite links run alongside a holder promotion and don't count toward its limit.

[Back to contents](#contents)

## Telegram, Discord and the holder club {#groups}

### Telegram group {#telegram}

- The site's bot runs a members-only Telegram group: members with an active pass get in, and anyone whose pass ends is removed (not banned) automatically.
- **Set up:** add the bot to your private group as an admin with **Invite users** and **Ban users**, and switch off **Add members** for members so people only get in through the bot. For several plans, send `/kaspass <plan>` in each group to choose which plan (and higher) it's for. Then approve the group in **Groups**: until you do, the bot invites nobody to it, because anyone can add a bot to a group they own. Every group in the list has **Remove the bot**.
- **Members:** press **Join the Telegram group** on the membership page, then **Start** in the bot. After renewing, send `/status` to get back in.
- **Reminders:** members who linked Telegram get one message a little before a period ends. `/quiet` stops them, `/remind` turns them back on.

### Discord server {#discord}

- Members with an active pass get a role (one per plan, plus Pass Holder), and lose it after their pass has ended.
- **Set up:** add the bot to your server with the link in the Groups tab (it needs **Manage Roles** and **Create Invite**), keep the bot's role above the roles it gives, and make members-only channels visible to those roles only.
- **Members:** press **Connect Discord** and approve on Discord's own screen. After renewing, the role comes back by itself.

### Holder club {#holder-club}

- Access by which NFTs or tokens someone holds, with no paid pass needed. Members sign in at `/club` (a signed message, never a payment), see their standing and join the matching Telegram groups and Discord roles.
- **Rules:** uploaded under **Advanced: upload holder club rules** (Groups tab), then **Check and apply**, with levels based on the number of NFTs, ranks and credits, specific token ids, other collections, KRC-20 or KCC-20 tokens, or holding a paid pass. **Check a wallet** previews any address.
- **Linked wallets** add up across a member's wallets. NFTs listed for sale don't count unless the rules say so.
- **Holder discount:** members who qualify can pay less for paid plans (Prices & sales tab).
- Holdings are re-checked every minute; anyone who stops qualifying loses that group or role.

### Writing a holder club rules file {#holder-club-rules}

The rules file is a small JSON text file. Upload it under **Groups › Holder club › Advanced: upload holder club rules**, then press **Check and apply**. If anything is wrong, nothing changes and the message says what to fix. **Download an example** in that panel gives you a working file to start from.

**Ask an AI to write it.** Give any AI assistant this page's plain-text version, `https://<your site>/guide.md`, and describe your club in your own words: the collection, who should get in, and any levels. For example: "Write a KasPass holder club rules file for my BITCOIN NFT collection on mainnet. Anyone with 1 NFT is a Turtle Holder. Anyone who owns a rank 1 to 4 NFT, or has 200 credits, is a Whale." Then upload the file and use **Check a wallet** on a few wallets you know before relying on it.

**The fields:**

| Field | Needed | What it is |
|---|---|---|
| `club` | yes | The club's name, shown to members. |
| `collection` | yes | Your NFT collection: `{"network": "mainnet", "tick": "BITCOIN"}`. Use `"testnet-10"` while testing. |
| `tiers` | yes | The levels, at least one. Each is `{"name": "Whale", "rule": {...}}`, with an optional `"discord_role"` if the Discord role should have another name. Each level becomes a Discord role and can have its own Telegram group. Names must be different, and `pass` is taken. |
| `bands` | no | Rank ranges that give credits: `{"name": "Gold", "ranks": [5, 21], "credits": 25}`. Ranges can't overlap, and need the ranks list. |
| `count_listed` | no | `true` to count NFTs listed for sale. Default `false`: a listed NFT doesn't count. |

**Rules.** Each `rule` has exactly one of these. To combine several, put them in `any_of` (any one is enough) or `all_of` (all are needed).

| Rule | Example | Means |
|---|---|---|
| `min_tokens` | `{"min_tokens": 3}` | Holds at least 3 NFTs of your collection. |
| `min_credits` | `{"min_credits": 200}` | Has at least 200 credits from the bands. |
| `owns_band` | `{"owns_band": "Gold"}` | Owns at least one NFT in that band. |
| `owns_token` | `{"owns_token": [7, 42]}` | Owns one of these specific NFTs (token ids). |
| `has_pass` | `{"has_pass": true}` | Has an active paid membership on this site. |
| `holds_nft` | `{"holds_nft": {"tick": "NACHO", "min": 1}}` | Holds NFTs of another KRC-721 collection (whole number, 1 to 1000). |
| `holds_token` | `{"holds_token": {"tick": "NACHO", "min": 100000}}` | Holds a KRC-20 token amount. |
| `holds_kcc20` | `{"holds_kcc20": {"id": "<64-character covenant id>", "tick": "NACHO", "min": 1000}}` | Holds a KCC-20 token amount. The id comes from the token's official page. |
| `any_of` / `all_of` | `{"any_of": [{"owns_band": "Legacy"}, {"min_credits": 200}]}` | Combines rules. |

Holdings add up across all the wallets a member has linked.

**The ranks list** (optional second file, only if you use bands): each NFT's rank, either `{"1": 950, "2": 14}` (token id, then rank) or a list of rows like `[{"id": 1, "rank": 950}]`, which is what a spreadsheet export usually gives. The ranks come from your collection's own rarity list.

**A full example:**

```json
{
  "club": "Bitcoin the Turtle",
  "collection": { "network": "mainnet", "tick": "BITCOIN" },
  "count_listed": false,
  "bands": [
    { "name": "Legacy", "ranks": [1, 4], "credits": 0 },
    { "name": "Gold", "ranks": [5, 21], "credits": 25 },
    { "name": "Silver", "ranks": [22, 210], "credits": 10 },
    { "name": "Bronze", "ranks": [211, 630], "credits": 5 },
    { "name": "Normal", "ranks": [631, 2100], "credits": 2 }
  ],
  "tiers": [
    { "name": "Turtle Holder", "rule": { "min_tokens": 1 } },
    { "name": "Whale", "rule": { "any_of": [ { "owns_band": "Legacy" }, { "min_credits": 200 } ] } }
  ]
}
```

**Common messages:**
- "isn't valid JSON": a missing comma, quote or bracket. Paste the file into an AI or a JSON checker to find it.
- "each rule needs exactly one key": a rule has two things in it; wrap them in `any_of` or `all_of`.
- "owns_band "Gold" isn't one of the bands": the band name doesn't match one in `bands` exactly (capitals count).
- "bands "Gold" and "Silver" overlap": two rank ranges share a number.
- "the bands use ranks, so the club needs a ranks list": choose your ranks file next to the rules file (the example uses bands, so it needs one too), or remove `bands` and any `owns_band` or `min_credits` rules.
- "unknown rule": a spelling mistake in the rule name; the message lists the right names.

[Back to contents](#contents)

## Linking from another website {#partner-sites}

- **Open a plan directly:** `https://<site>/?level=<plan id>`. The page shows that plan alone, with the sign-in beside it, and "Change plan" for the others.
- **Come back after paying:** add `&return=https://your-site/page`. The return site must be on the server's allowed list. The page then says whose payment page it is, and after paying (or claiming) a button goes back to that address with `paid=1` added. Always confirm the membership with the API rather than trusting `paid=1`.
- **Match the look:** `&style=kaspass`, `&style=open` or `&style=kaspa-vision`, and `&theme=day` or `&theme=night`.
- **Open one step:** `&do=manage`, `&do=renew` or `&do=topup` (after sign-in). It never pays or ends anything by itself.
- **Read-only API for partner sites:** `GET /v1/levels` (plans and prices; each plan has its `name`, `perks`, `about_html`, `badges` (top first, each `{kind, text, tone}`: `kind` is `text`, `save` or `early`, `tone` is one of the six colours or empty for an automatic badge, and an early price adds `until_ms` and `tz_min`, the site's time zone in minutes east of UTC, so the date can be written the way the creator picked it), `badges_custom` (the creator's own list rather than the automatic one), `badge` (the first text badge, empty when none) and `founding` (with `until_ms` and `tz_min`)), `GET /v1/page` (`highlight`: the highlighted plan's `level`, empty when none), `GET /v1/promo` (the running promotion), `GET /v1/quote?address=…&level=…` (what a wallet would pay), `GET /v1/check?address=…` (does it hold an active pass). `GET /v1/owner` (who owns the site now, its owner record, payout vault and plans, for an app that counts the creator's KasPass income: accept the site only when `owner` is a wallet the user has signed in with). `/v1/check` doesn't prove the visitor controls that address: have them sign in (a signed message) first, and gate access on `access_rank` so a higher plan counts for a lower one.
- **Kaspa Vision's KasPass tab:** a creator who uses Kaspa Vision can add their KasPass site in Kaspa Vision's account settings, so payments into their vault show up in their tax records there. Kaspa Vision accepts a site only on mainnet, only when the site's owner (from `GET /v1/owner`) is one of the wallets the creator has signed in to Kaspa Vision with, and one site per vault. Money arriving in the vault is counted as income in CA$ when it arrives; withdrawals are shown but not counted again. Kaspa Vision checks the site again once a day: if it stops answering or the owner changes, the creator is told, and income already recorded stays.
- The pages can't be shown inside another site's frame, so visitors always see the real address.

[Back to contents](#contents)

## Common messages and what to do {#messages}

| Message | What to do |
|---|---|
| "We couldn't find the Kaspire wallet in this browser" | Check the extension is installed and turned on, then reload. On a phone, use the Kaspire app. |
| "You need about … KAS and this wallet has … KAS" | Add KAS to the wallet, then try again. |
| "That price was held for 10 minutes and has run out" | Press the button again for today's price. |
| "Price unavailable right now" | Exchange rates couldn't be read just now. Try again in a minute. |
| "We're still loading the latest payments" | Wait a minute and try again. |
| "You've been signed out" | Sign in again. |
| "Your payment was sent and is still being confirmed" | Don't pay again. Refresh in a minute. |
| "Free passes are paused for a moment" | The site's fees wallet is low. The creator tops it up, and claims restart on their own. |
| "Couldn't read what your wallet holds right now" | The token or NFT index didn't answer. Try again in a minute; it never counts as holding nothing. |
| "This invite has already been used" | Each invite link works once. Ask the creator for a new one. |
| "This membership belongs to another wallet" (dashboard) | You signed in with a wallet that isn't the creator's. Switch wallets in Kaspire and sign in again. |
| "This page isn't public" | The creator only shares the page by private link. |

Still stuck? Contact the creator (links on their page) and say which message you saw.

[Back to contents](#contents)

## Safety, privacy and limits {#safety}

- **Nobody can take your membership early,** and only you can withdraw your deposit and prepaid KAS.
- **The site never asks for recovery words or passwords.** Signing in is a signed message; Kaspire shows every amount before any payment.
- **Check the address** before approving: you should be on the creator's own payment site, the address their website links to.
- **What the site keeps:** which wallets signed in or bought (to show members to the creator), linked Telegram and Discord accounts, and the creator's settings. A pass names its owner only by a hash of their key. The creator's member notes are private.
- **The server's keys** (the server key and the fees wallet) are created on the server and never leave it. Creator actions need the creator's own signature, so nobody else can use the dashboard even if they reach it.
- **Pages load nothing from other sites,** and every page carries a strict security policy.
- **Not yet independently audited.** The code is public; review it before trusting large amounts.

[Back to contents](#contents)

## Quick answers {#faq}

- **Is it a subscription?** No. Nothing is charged automatically unless the member prepays for auto-renew, and they can take that back any time.
- **Can I get a refund?** Payments go straight to the creator, so ask them. The deposit, unused prepaid KAS and a cancelled trial's money always come back. See [Refunds](#refunds).
- **Why do I pay a deposit?** Each pass is a small contract on Kaspa, which needs a little KAS to exist. It's yours, and it comes back when you end the membership.
- **Why did the KAS price change?** Plans are priced in dollars or euros, and KAS's value moves. You always pay today's price; a shown price is held for 10 minutes.
- **Can a creator change my price?** For new periods only, and on currency plans only after 30 days' notice and never above your pass's KAS ceiling. What you've paid for is yours.
- **Can a creator give someone a free month?** Yes: **Members › Give a membership**, an invite link, or a holder promotion.
- **Can I change a plan's billing period or trial?** No, those are fixed when it's created. Create a new plan and put it on sale instead.
- **Why did a promotion stop showing?** It ended, reached its limit, or the fees wallet ran low (claims pause until it's topped up).
- **Where's my money as a creator?** In your payout vault. Withdraw it from the Overview or Payouts tab.
- **What if I lose my wallet as a creator?** The backup wallet can hand the owner record to a new wallet. Members do nothing.
- **What does KasPass cost?** 2% of each payment, taken from the creator's share. No setup or monthly fee.

[Back to contents](#contents)
