# Checks before a release

`dev/check.sh` runs, in order:

1. the unit tests (`cargo test`);
2. every contract and the website's flows on a simulated chain (`membership-cli simulate`, which must end with "SIMULATION PASSED");
3. the website's pages in a real browser (`dev/pages.js`), signed out, on a phone and a computer, in light and dark mode, against a local simulated site with example creators and a 7-day free trial;
4. the creator dashboard, signed in with the simulator's creator test key (`dev/dashboard.js`): every tab with stream mode on, with and without sample members, payments and claims, must show nothing private (addresses, amounts, dates, ids), plus the dashboard fixes of update 92, and the Get started guide and locked-in list of update 101 (`dev/leaks.js` is the shared check for private text);
5. a brand-new creator-only site (`dev/empty.js`, with `KASPASS_SIM_EMPTY=1 KASPASS_SIM_CREATOR_ONLY=1`): the Get started guide opens by itself at step 1, then the states after payouts and after creating a plan that isn't on sale yet (stubbed, since the test wallet can't approve transactions), in stream mode too, at 1280 by 720.

It needs Rust, Node 18 or newer, Playwright with Chromium, and the two signing libraries the dashboard test uses (`npm install playwright @noble/curves @noble/hashes && npx playwright install chromium`). To use a Chrome that's already installed instead, set `KASPASS_CHROME` to its path. `KASPASS_SHOTS=<folder>` saves screenshots of each dashboard tab.

```
dev/check.sh
```

Nothing here touches a real network or a real wallet: the simulated site uses made-up keys, made-up exchange rates and free test KAS.

# Before every push

`dev/privacy-scan.sh` checks the files and every commit that isn't on GitHub yet for secret-looking files, keys and tokens, public IP addresses, user folder paths, a list of private words and the commit authors. It prints `CLEAN` only when there's nothing to fix, so push with:

```
./dev/privacy-scan.sh && git push origin HEAD
```

The list of private words is kept outside the repository (`~/.kaspass-private-words`, or the file named in `KASPASS_PRIVATE_WORDS`), one word or phrase per line. The scan refuses to run without it.
