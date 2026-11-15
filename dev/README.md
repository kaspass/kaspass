# Checks before a release

`dev/check.sh` runs, in order:

1. the unit tests (`cargo test`);
2. every contract and the website's flows on a simulated chain (`membership-cli simulate`, which must end with "SIMULATION PASSED");
3. the website's pages in a real browser (`dev/pages.js`), signed out, on a phone and a computer, in light and dark mode, against a local simulated site with example creators and a 7-day free trial.

It needs Rust, Node 18 or newer, and Playwright with Chromium (`npm install playwright && npx playwright install chromium`).

```
dev/check.sh
```

Nothing here touches a real network or a real wallet: the simulated site uses made-up keys, made-up exchange rates and free test KAS.
