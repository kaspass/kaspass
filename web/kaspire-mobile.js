// Kaspire mobile over WalletConnect v2, for pages that otherwise talk to the Kaspire extension
// (the creator dashboard). connect() returns an object that answers the same requests as
// window.kaspire (getAccounts, signMessage, signPskt), so the page's sign-in and approvals work
// unchanged. The WalletConnect library (and the QR maker) load only when someone presses the button.
(function () {
  const PROJECT = "5dff3b7c95fb1abd4ffcd556328280ec"; // public Reown project id (allowed domains only)
  const LINK = "https://kaspire.kaslab.space/kaspire/wc";
  const isAndroid = /Android/i.test(navigator.userAgent);
  const isPhone = /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
  const WAKE = isAndroid ? `intent://dapp#Intent;scheme=kaspire;package=space.kaspire.wallet;S.browser_fallback_url=${encodeURIComponent(LINK)};end` : LINK;
  const pairLink = (uri) => `${LINK}?uri=${encodeURIComponent(uri)}`;
  const androidPair = (uri) => `intent://wc?uri=${encodeURIComponent(uri)}#Intent;scheme=kaspire;package=space.kaspire.wallet;S.browser_fallback_url=${encodeURIComponent(pairLink(uri))};end`;
  const CHANGED = "Your active Kaspire wallet changed. Press Reconnect to pair the wallet you want to use.";

  let client = null, pending = 0, cancel = null, changed = null;

  function load(src) {
    return new Promise((res, rej) => {
      const el = document.createElement("script");
      el.src = src; el.onload = res;
      el.onerror = () => rej(new Error("couldn't load the connection library"));
      document.head.append(el);
    });
  }

  // ---- A small box of its own (styles use the page's colors when it has them) ----
  const css = `
  .kpm-back{position:fixed;inset:0;background:rgba(0,0,0,.55);display:flex;align-items:center;justify-content:center;z-index:1000;padding:16px}
  .kpm-box{background:var(--panel,#0d1414);color:var(--text,#e8f1f0);border:1px solid var(--line,#1c2a29);border-radius:16px;padding:22px;max-width:380px;width:100%;font:inherit;text-align:center}
  .kpm-box h3{margin:0 0 6px;font-size:18px}
  .kpm-box p{margin:6px 0;color:var(--muted,#8aa09d);font-size:14px}
  .kpm-qr{background:#fff;border-radius:12px;padding:10px;width:220px;height:220px;margin:12px auto}
  .kpm-qr svg{width:100%;height:100%}
  .kpm-box a.kpm-go,.kpm-bar a.kpm-go{display:inline-flex;align-items:center;justify-content:center;min-height:46px;padding:0 18px;border-radius:10px;background:var(--teal,#49d9c8);color:#031312;font-weight:600;text-decoration:none;margin:8px 0}
  .kpm-box button,.kpm-bar button{background:transparent;color:var(--text,#e8f1f0);border:1px solid var(--line,#2c4442);border-radius:10px;min-height:40px;padding:0 14px;font:inherit;cursor:pointer;margin:4px}
  .kpm-bar{position:fixed;left:12px;right:12px;bottom:12px;z-index:999;background:var(--panel,#0d1414);color:var(--text,#e8f1f0);border:1px solid var(--teal,#49d9c8);border-radius:14px;padding:12px 14px;box-shadow:0 10px 30px rgba(0,0,0,.4);display:flex;flex-wrap:wrap;gap:6px 10px;align-items:center;font-size:14px}
  .kpm-bar span{flex:1 1 220px}
  .kpm-hidden{display:none!important}`;
  let ui = null;
  function build() {
    if (ui) return ui;
    const st = document.createElement("style"); st.textContent = css; document.head.append(st);
    const back = document.createElement("div");
    back.className = "kpm-back kpm-hidden";
    back.innerHTML = `<div class="kpm-box" role="dialog" aria-modal="true" aria-labelledby="kpmT">
      <h3 id="kpmT">Connect Kaspire mobile</h3>
      <p class="kpm-net"></p>
      <div class="kpm-desk"><div class="kpm-qr" aria-label="QR code to scan with Kaspire mobile"></div><p>Scan it with your phone's camera, then approve the connection in the Kaspire app.</p><p><a class="kpm-link" target="_blank" rel="noopener">Kaspire is on this device</a></p></div>
      <div class="kpm-phone"><a class="kpm-go kpm-open" target="_blank" rel="noopener">Open Kaspire app</a><p>Approve the connection, then come back to this tab. Don't reload this page.</p></div>
      <button type="button" class="kpm-cancel">Cancel</button></div>`;
    const bar = document.createElement("div");
    bar.className = "kpm-bar kpm-hidden";
    bar.setAttribute("role", "status");
    bar.innerHTML = `<span>Approve this in the Kaspire app, then come back to this tab. Don't reload this page.</span>${isPhone ? `<a class="kpm-go" target="_blank" rel="noopener" href="${WAKE}">Open Kaspire app</a>` : ""}<button type="button" class="kpm-check">Approved? Check again</button>`;
    document.body.append(back, bar);
    back.querySelector(".kpm-cancel").onclick = () => { if (cancel) cancel(); back.classList.add("kpm-hidden"); };
    bar.querySelector(".kpm-check").onclick = (e) => { revive(); e.target.textContent = "Checking…"; setTimeout(() => (e.target.textContent = "Approved? Check again"), 3000); };
    ui = { back, bar };
    return ui;
  }

  // A phone pauses this tab while the app is in front, and the connection can drop. Coming back,
  // reconnect so the app's answer (kept by the relay) arrives.
  function revive() {
    if (!client || !pending) return;
    try { client.core.relayer.restartTransport().catch(() => {}); } catch {}
  }
  document.addEventListener("visibilitychange", () => { if (!document.hidden) setTimeout(revive, 300); });
  window.addEventListener("pageshow", () => setTimeout(revive, 300));
  window.addEventListener("focus", () => setTimeout(revive, 300));

  // What the wallet shows on its approval screens: the site's name and logo, with KasPass.
  async function metadata() {
    let name = "KasPass", icons = [];
    try { const o = await (await fetch("/v1/offer")).json(); if (o.creator_name) name = `${o.creator_name} · KasPass`; } catch {}
    try { const b = await (await fetch("/v1/brand")).json(); if (b.logo) icons = [new URL(b.logo, location.origin).href]; } catch {}
    return { name, description: "Memberships on Kaspa", url: location.origin, icons, redirect: { universal: location.origin + location.pathname } };
  }

  async function init() {
    if (client) return client;
    if (!window.KasPassWC) await load("/assets/walletconnect.js?v=63");
    client = await window.KasPassWC.SignClient.init({ projectId: PROJECT, telemetryEnabled: false, metadata: await metadata() });
    client.on("session_delete", () => {});
    // The active wallet in the app changed: this connection no longer speaks for it.
    client.on("session_event", ({ params }) => {
      if (params && params.event && params.event.name === "accountsChanged") { changed = CHANGED; forget(); }
    });
    return client;
  }

  const chainOf = (network) => (network === "testnet-10" ? "kaspa:testnet-10" : "kaspa:mainnet");
  // "kaspa:mainnet:qq…" -> "kaspa:qq…", "kaspa:testnet-10:qq…" -> "kaspatest:qq…"
  function addressOf(sess, chain) {
    const acc = ((sess.namespaces.kaspa && sess.namespaces.kaspa.accounts) || []).find((a) => a.startsWith(chain + ":"));
    if (!acc) return null;
    const parts = acc.split(":");
    return (parts[1] === "testnet-10" ? "kaspatest:" : "kaspa:") + parts.slice(2).join(":");
  }

  // Drops every connection and its pairing in this browser, so the next connect pairs again.
  async function forget() {
    if (!client) return;
    for (const s of client.session.getAll()) {
      try { await client.disconnect({ topic: s.topic, reason: { code: 6000, message: "Reconnecting" } }); } catch {}
    }
    try { for (const p of client.core.pairing.getPairings()) await client.core.pairing.disconnect({ topic: p.topic }); } catch {}
  }

  function adapter(sess, chain) {
    const a = addressOf(sess, chain);
    const ask = async (method, params) => {
      if (changed) throw Object.assign(new Error(changed), { code: "WALLET_CHANGED" });
      const p = client.request({ topic: sess.topic, chainId: chain, request: { method, params } });
      pending++;
      build().bar.classList.remove("kpm-hidden");
      try {
        return await p;
      } catch (e) {
        const msg = (e && e.message) || String(e);
        if (/no longer the active wallet|reconnect the dapp/i.test(msg)) {
          changed = CHANGED;
          forget();
          throw Object.assign(new Error(CHANGED), { code: "WALLET_CHANGED" });
        }
        const err = new Error(msg);
        if ((e && e.code === 4001) || /reject/i.test(msg)) err.code = 4001;
        throw err;
      } finally {
        pending--;
        if (!pending) build().bar.classList.add("kpm-hidden");
      }
    };
    return {
      isKaspire: true,
      mobile: true,
      address: a,
      async request({ method, params }) {
        if (method === "getAccounts" || method === "requestAccounts") return [a];
        if (method === "signMessage") return { signature: await ask("kaspa_signPersonal", { message: params.message, address: a }) };
        if (method === "signPskt") {
          const { sender, ...rest } = params; // the app accepts only the transaction fields
          return ask("kaspa_signPskt", rest);
        }
        throw new Error("Kaspire mobile can't do that here yet.");
      },
      on() {},
    };
  }

  // An existing connection in this browser, without asking anything (null if there's none).
  async function known(network) {
    const c = await init();
    const chain = chainOf(network);
    const now = Math.floor(Date.now() / 1000);
    const s = c.session.getAll().find((x) => x.expiry > now + 60 && addressOf(x, chain));
    return s ? adapter(s, chain) : null;
  }

  // Pairs with the app (a QR code on a computer, a button on a phone) unless already connected.
  async function connect(network, { fresh = false } = {}) {
    if (fresh || changed) { await init(); await forget(); changed = null; }
    const have = await known(network);
    if (have) return have;
    const c = client, chain = chainOf(network);
    const { uri, approval } = await c.connect({ requiredNamespaces: { kaspa: { chains: [chain], methods: ["kaspa_getAccounts", "kaspa_signPersonal", "kaspa_signPskt"], events: ["accountsChanged"] } } });
    const { back } = build();
    if (uri) {
      back.querySelector(".kpm-net").textContent = "Network: " + (network === "testnet-10" ? "Testnet-10" : "mainnet");
      back.querySelector(".kpm-desk").classList.toggle("kpm-hidden", isPhone);
      back.querySelector(".kpm-phone").classList.toggle("kpm-hidden", !isPhone);
      back.querySelector(".kpm-open").href = isAndroid ? androidPair(uri) : pairLink(uri);
      back.querySelector(".kpm-link").href = isAndroid ? androidPair(uri) : pairLink(uri);
      if (!isPhone) {
        if (typeof window.qrcode !== "function") await load("/assets/qrcode.js?v=31");
        const q = window.qrcode(0, "M"); q.addData(pairLink(uri)); q.make();
        back.querySelector(".kpm-qr").innerHTML = q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
      }
      back.classList.remove("kpm-hidden");
    }
    const cancelled = new Promise((_, rej) => { cancel = () => rej(Object.assign(new Error("Cancelled."), { code: 4001 })); });
    try {
      const sess = await Promise.race([approval(), cancelled]);
      const ad = adapter(sess, chain);
      if (!ad.address) throw new Error(`the app didn't share a wallet for ${network === "testnet-10" ? "Testnet-10" : "mainnet"}. Switch Kaspire to that network and try again.`);
      return ad;
    } finally {
      cancel = null;
      back.classList.add("kpm-hidden");
    }
  }

  window.KaspireMobile = { isPhone, connect, known, forget: async () => { await init(); await forget(); } };
})();
