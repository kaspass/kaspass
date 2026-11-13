// Sessions travel in a header, never in a link: a "session=" in this site's own API URLs moves there.
(() => {
  const plain = window.fetch.bind(window);
  window.fetch = (u, o = {}) => {
    if (typeof u === "string" && (u.startsWith("/v1/") || u.startsWith(location.origin + "/v1/")) && /[?&]session=/.test(u)) {
      const url = new URL(u, location.origin);
      const t = url.searchParams.get("session");
      url.searchParams.delete("session");
      const h = new Headers(o.headers || {});
      if (t && !h.has("x-kaspass-session")) h.set("x-kaspass-session", t);
      u = url.pathname + url.search;
      o = { ...o, headers: h };
    }
    return plain(u, o);
  };
})();
// An example creator on KasPass's demo (kaspass.app/demo/<id>): every call to this site's API
// says which one, so names, descriptions and the look match the example.
(() => {
  const m = document.querySelector('meta[name="kp-example"]');
  if (!m) return;
  const ex = m.content, plain = window.fetch.bind(window);
  window.fetch = (u, o = {}) => {
    const url = typeof u === "string" ? u : u && u.url ? u.url : String(u);
    if (url.startsWith("/v1/") || url.startsWith(location.origin + "/v1/")) {
      const h = new Headers(o.headers || (typeof u === "object" && u.headers) || {});
      h.set("x-kp-example", ex);
      o = { ...o, headers: h };
    }
    return plain(u, o);
  };
})();
const $ = (id) => document.getElementById(id);
const show = (id, on = true) => $(id).classList.toggle("hidden", !on);
const KAS = 100000000;
const kas = (s) => (Number(s) / KAS).toLocaleString(undefined, { maximumFractionDigits: 2 }) + " KAS";
// "About N KAS" lines: whole KAS only. Exact amounts stay where they're what you pay.
const kasRound = (s) => Math.round(Number(s) / KAS).toLocaleString() + " KAS";
const mins = (ms) => Math.round(ms / 60000) + " min";
// "12 months" on a 30-day membership, "4 weeks", "7 days", otherwise "12 periods". A test
// membership with short periods still talks in months (the page says how long one really is).
const testing = () => !!card && card.network !== "mainnet" && card.settings.period_ms < 86400000;
const unitOf = (ms) => { if (testing()) return "month"; const d = ms / 86400000; return d >= 360 && d <= 366 ? "year" : d >= 28 && d <= 31 ? "month" : d === 7 ? "week" : d === 1 ? "day" : "period"; };
const perWord = () => ({ year: "Yearly", month: "Monthly", week: "Weekly", day: "Daily", period: "Per period" })[unitOf(card ? card.settings.period_ms : 0)];
const periods = (n) => { const u = unitOf(card ? card.settings.period_ms : 0); return `${n} ${u}${n === 1 ? "" : "s"}`; };
const span = (ms) => { const m = Math.round(ms / 60000); return m >= 2880 && m % 1440 === 0 ? `${m / 1440} days` : m >= 120 && m % 60 === 0 ? `${m / 60} hours` : `${m} min`; };
// v5 plans are priced in a currency; their renewals are at today's price, not a locked KAS rate.
const isV5 = () => !!(offer && offer.price);
const rateWord = () => (isV5() ? "today's price" : "your locked rate");
const disc = (sompi, bps) => Math.floor(Number(sompi) * (10000 - bps) / 10000);
let address = null;
let session = null;
// The signed-in session goes with this site's own API calls in a header, never in a link.
(() => {
  const plain = window.fetch.bind(window);
  window.fetch = (u, o = {}) => {
    const url = typeof u === "string" ? u : u && u.url ? u.url : String(u);
    if (session && (url.startsWith("/v1/") || url.startsWith(location.origin + "/v1/"))) {
      const h = new Headers(o.headers || (typeof u === "object" && u.headers) || {});
      if (!h.has("x-kaspass-session")) h.set("x-kaspass-session", session);
      o = { ...o, headers: h };
    }
    return plain(u, o);
  };
})();
let card = null;
let offer = null;
// Chain time minus this computer's time, learned from the server.
let clockOffset = 0;
// The signed-in wallet already has a pass (active or expired): offer Renew, not Buy.
let holdsPass = false;
// Several paid levels: the one being looked at (covenant id; null = the site's only or lowest level),
// every level on sale, and which ones this wallet holds right now.
let LEVEL = null, levels = [], held = [], site = { club_name: "", level_word: { one: "plan", many: "plans" } };
// The creator's word for what's on sale ("plan", "tier"), in every sentence that names one.
const lw = (many) => (many ? site.level_word.many : site.level_word.one);
function applyLevelWord() { document.querySelectorAll(".lw1").forEach((el) => (el.textContent = lw())); }
const withLevel = (u, lv) => (lv ? u + (u.includes("?") ? "&" : "?") + "level=" + encodeURIComponent(lv) : u);
const L = (u) => withLevel(u, LEVEL);
const levelOf = (id) => levels.find((l) => l.level === id);
// The creator's word for what members hold ("membership", "pass", "subscription").
let PW = { one: "membership", many: "memberships" };
const pw = (o = {}) => {
  let t = o.many ? PW.many : PW.one;
  if (o.a) t = (/^[aeiou]/i.test(t) ? "an " : "a ") + t;
  return o.cap ? t[0].toUpperCase() + t.slice(1) : t;
};
function applyWord() {
  document.querySelectorAll("[data-pw]").forEach((el) => {
    const f = el.dataset.pw;
    el.textContent = pw({ cap: f.includes("C"), many: f.includes("s"), a: f.includes("a") });
  });
}
const wordReady = fetch(L("/v1/offer")).then((r) => (r.ok ? r.json() : null)).then((o) => { if (o && o.pass_word) PW = o.pass_word; applyWord(); }).catch(() => {});
// v4: the holder discount this wallet gets right now (basis points).
let holder = { bps: 0, enabled: false };
const chainNow = () => Date.now() + clockOffset;

function fmtExpiry(p) {
  if (p.lifetime) return "Never expires";
  const d = p.expiry_ms - chainNow();
  const abs = Math.abs(d);
  const days = Math.floor(abs / 86400000), h = Math.floor((abs % 86400000) / 3600000);
  const m = Math.floor((abs % 3600000) / 60000), s = Math.floor((abs % 60000) / 1000);
  const span = days > 0 ? `${days}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m ${s}s`;
  return d >= 0 ? `Expires in ${span}` : `Expired ${span} ago`;
}

function setError(msg) {
  $("walletErr").textContent = msg || "";
  show("walletErr", !!msg);
}

// Errors in plain words: what happened and what to do next. Known messages from the server or
// the wallet get a rewrite; other plain sentences are shown as they are (with a capital letter);
// anything technical becomes `fallback`, with the original behind "Details".
const PLAIN = [
  [/phones don't run the Kaspire extension/i, "On a phone, sign in with the Kaspire app. Tap the button below."],
  [/extension not detected/i, "We couldn't find the Kaspire wallet in this browser. Install it (free) from kaspire.kaslab.space, then reload this page. Or use the Kaspire app on your phone."],
  [/no account was approved/i, "Kaspire didn't share a wallet. Open Kaspire, approve the connection, then try again."],
  [/indexer is still catching up|still reading the chain|catching up/i, "We're still loading the latest payments. Try again in a minute."],
  [/only regular \(P2PK\) addresses|regular \(P2PK\) address/i, "This kind of wallet address can't be used here. Use your main Kaspire address."],
  [/no sales lane|isn't on sale right now/i, "This plan isn't on sale right now."],
  [/too early to renew/i, "It's too early to renew. Renewing opens closer to the end date, and the card shows when."],
  [/purchase expired|please start again|quote (has )?expired/i, "That price was held for 10 minutes and has run out. Press the button again for today's price."],
  [/didn't pass the final check/i, "Something didn't match, so nothing was sent and nothing was paid. Please try again."],
  [/network refused/i, "The payment didn't go through and nothing was paid. Please try again."],
  [/must come from this site's own pages/i, "Please reload this page and try again."],
  [/couldn't load the connection library/i, "We couldn't start the link to the Kaspire app. Check your internet and try again."],
  [/^sold out/i, "Sorry, this plan is sold out."],
  [/recipient isn't a valid Kaspa address|not a valid Kaspa address/i, "That isn't a valid Kaspa address. Check it and try again."],
  [/recipient address is for a different network/i, "That address is for a different Kaspa network. Check it and try again."],
  [/session (expired|not found)|sign in again|not signed in/i, "You've been signed out. Sign in again, then try once more."],
];
const TECHY = /[0-9a-f]{16,}|\btx\b|script|covenant|rpc|panic|error:|\{|\bsompi\b|outpoint|utxo/i;
function plainError(raw, fallback) {
  const t = String(raw || "").replace(/^(Failed|Error|Sign-in failed|Kaspire mobile):\s*/i, "").trim();
  for (const [re, nice] of PLAIN) if (re.test(t)) return { text: nice };
  if (!t || TECHY.test(t) || t.length > 220) return { text: fallback, raw: t };
  // A capital letter, unless the message starts with a name or address ("nobody.kas isn't…").
  const c = /^[a-z]+[\s',]/.test(t) ? t[0].toUpperCase() + t.slice(1) : t;
  return { text: /[.!?]$/.test(c) ? c : c + "." };
}
function oops(id, e, fallback = "That didn't work and nothing was paid. Please try again.") {
  const { text, raw } = plainError(e && e.message !== undefined ? e.message : e, fallback);
  const el = $(id);
  el.textContent = text;
  if (raw) {
    const d = document.createElement("details");
    d.className = "errdet";
    d.innerHTML = "<summary>Details</summary>";
    d.append(raw);
    el.append(d);
  }
  show(id, true);
}

function dl(el, rows) {
  el.replaceChildren();
  for (const [k, v, mono] of rows) {
    const dt = document.createElement("dt"); dt.textContent = k;
    const dd = document.createElement("dd"); dd.textContent = v;
    if (mono) dd.className = "mono";
    el.append(dt, dd);
  }
}

async function loadTier() {
  await wordReady;
  try {
    card = await (await fetch(L("/v1/tier"))).json();
    const t = card.settings;
    const u = unitOf(t.period_ms);
    dl($("tier"), [
      ["Creator", card.creator_address, true],
      ...(testing() ? [[`Length of a ${u}`, `${mins(t.period_ms)} (shortened for testing)`]] : u === "period" ? [["Renews every", mins(t.period_ms)]] : []),
      ...(t.trial_ms > 0 ? [["Free trial", mins(t.trial_ms)]] : []),
      ...(t.v4 ? [["Auto-renew", `Optional. Pay a few renewals ahead and your ${pw()} renews itself before it runs out, so you never lose access. Cancel any time and get your unused balance back.`]] : []),
      ...(t.v4 && t.v4.max_discount_bps > 0 && !t.v4.record_id ? [["Holder discount", `NFT holders can pay up to ${t.v4.max_discount_bps / 100}% less.`]] : []),
      ["Refundable deposit", `${kas(t.bond)}, held in your ${pw()} and returned when you end it`],
      ["Passing it on", t.resellable ? `You can sell or give your ${pw()} to someone else${t.royalty > 0 ? `; the creator gets a ${kas(t.royalty)} royalty` : ""}` : `${pw({ cap: true, many: true })} stay with the wallet that bought them`],
      ["KasPass fee", "None for you. The price shown is what you pay."],
    ]);
    dl($("techDl"), [
      ["Membership ID", card.covenant_id, true],
      ["Network", card.network === "testnet-10" ? "Testnet-10 (test KAS only)" : card.network === "mainnet" ? "Kaspa mainnet" : card.network],
      ["Contract version", t.v4 ? "v4" : t.fee ? "v3" : "v2"],
      ...(t.v4 ? [["Auto-renew runs", `in the last ${span(t.v4.auto_window_ms)} of each ${u}`]] : []),
      ["Renew early", `up to ${mins(t.max_ahead_ms)} before the end`],
    ]);
    $("net").textContent = card.network === "testnet-10" ? "Testnet-10" : card.network === "mainnet" ? "Mainnet" : card.network;
    if (card.network === "mainnet") {
      $("netNote").textContent = "New software. The code is public and has not had an outside security review.";
      $("lookup").placeholder = "kaspa:...";
      $("transferTo").placeholder = "kaspa:… address, or a name like alice.kas or alice.k";
    }
    show("noFee", !card.placeholder && !!t.fee && t.fee.bps === 0 && card.network === "mainnet");
    show("comingSoon", !!card.placeholder);
    if (card.placeholder) { for (const id of ["noShop", "tierBox", "wallet", "welcome", "join"]) show(id, false); }
    renderCreatorRow();
    renderTierAbout();
  } catch {
    dl($("tier"), [["Error", "This page couldn't load. Check your connection and refresh."]]);
  }
}

async function detectKaspire(timeoutMs = 3000) {
  if (WC) return WC; // signed in with Kaspire mobile: it signs instead of the extension
  if (window.kaspire?.isKaspire) return window.kaspire;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(/Android|iPhone|iPad|iPod/i.test(navigator.userAgent) ? "On a phone, sign in with the Kaspire app. Tap the button below." : "We couldn't find the Kaspire wallet in this browser. Install it (free) from kaspire.kaslab.space, then reload this page. Or use the Kaspire app on your phone.")), timeoutMs);
    window.addEventListener("kaspire#initialized", () => { clearTimeout(t); resolve(window.kaspire); }, { once: true });
  });
}

function wantedPrefix() {
  return card && card.network === "testnet-10" ? "kaspatest:" : "kaspa:";
}

// .kas and .k names for an address (public records), cached per page.
const nameCache = {};
async function namesOf(a) {
  if (!a) return "";
  if (!(a in nameCache)) {
    nameCache[a] = fetch("/v1/names?address=" + encodeURIComponent(a)).then((r) => (r.ok ? r.json() : {})).then((n) => [n.k, n.kas].filter(Boolean).join(" · ")).catch(() => "");
  }
  return nameCache[a];
}

function setAddress(a) {
  address = a;
  const label = (n) => {
    $("addr").innerHTML = "";
    $("addr").append(session ? "Signed in as " : "Kaspire wallet found ");
    if (n) { const b = document.createElement("b"); b.textContent = n + " "; $("addr").append(b); }
    const m = document.createElement("span"); m.className = "a"; m.textContent = session && !n ? `the wallet ending in ${a.slice(-4)}` : `(ends in ${a.slice(-4)})`; m.title = a;
    $("addr").append(m);
    // Signed in with Kaspire mobile: a way to pair a different wallet.
    if (session && WC) {
      const x = document.createElement("a");
      x.href = "#"; x.className = "small"; x.style.marginLeft = "8px"; x.textContent = "Not you? Use another wallet";
      x.onclick = (e) => { e.preventDefault(); wcSwitch(); };
      $("addr").append(x);
    }
  };
  if (a) label("");
  show("addr", !!a);
  show("signin", !session && !(isPhone && !window.kaspire));
  show("wcGo", !session && !noApp());
  show("logout", !!session);
  signinLayout();
  $("walletTitle").textContent = session ? "Your wallet" : "Sign in with your wallet";
  show("walletHint", !session);
  if (!session) show("yourPlan", false);
  if (a && session) namesOf(a).then((n) => { if (n && address === a) label(n.split(" · ")[0]); });
}

// One line under the wallet: the plan it holds and until when (the highest one, on several levels).
const fmtDate = (ms) => new Date(ms).toLocaleDateString(undefined, { dateStyle: "medium" });
function renderWalletLine(passes) {
  if (!session) { show("yourPlan", false); return; }
  const act = (passes || []).filter((p) => p.active);
  const lv = (p) => (p.level ? levelOf(p.level) : null);
  act.sort((a, b) => ((lv(b) || {}).rank || 0) - ((lv(a) || {}).rank || 0) || b.expiry_ms - a.expiry_ms);
  const p = act[0];
  const el = $("yourPlan");
  if (!p) {
    const any = (passes || [])[0];
    el.innerHTML = any ? `Your ${escH(pw())} has ended. <span class="muted">Renew it below to get access back.</span>` : `<span class="muted">No ${escH(lw())} yet: pick one below.</span>`;
  } else {
    const name = (lv(p) && lv(p).name) || (offer && offer.name) || pw({ cap: true });
    el.innerHTML = `Your ${escH(lw())}: ${escH(name)}${p.lifetime ? ", for life" : `, until ${escH(fmtDate(p.expiry_ms))}`}`;
  }
  show("yourPlan", true);
}

let listening = false;
function watchAccounts(kaspire) {
  if (listening || typeof kaspire.on !== "function" || kaspire.mobile) return;
  listening = true;
  kaspire.on("accountsChanged", (acc) => {
    const now = (acc && acc[0]) || null;
    if (linking) { linkCandidate = now; $("linkSeen").textContent = linkCandidate ? "Kaspire now shows: " + linkCandidate : ""; return; }
    // Once signed in, switching wallets or networks in Kaspire never signs you out.
    // Kaspire reports every step of a switch (other network, other wallet), so we only
    // note where it is now; paying checks it's back on the signed-in wallet first.
    if (session) { kaspireNow = now; walletNote(); return; }
    setAddress(now); hideResults();
  });
}

let kaspireNow = null;
const shortAddr = (a) => (a ? a.slice(0, 12) + "…" + a.slice(-4) : "");
function walletNote() {
  const off = !!(session && kaspireNow !== address);
  $("walletNote").textContent = off
    ? `Kaspire is now on ${kaspireNow ? shortAddr(kaspireNow) : "another wallet or network"}. You're still signed in as ${shortAddr(address)}. Switch back to it before buying or renewing.`
    : "";
  show("walletNote", off);
}

// Paying signs with whatever wallet Kaspire is on, so make sure it's the signed-in one.
async function ensureSigningWallet(kaspire) {
  let a = null;
  try { a = (await kaspire.request({ method: "getAccounts" }))?.[0] || null; } catch {}
  kaspireNow = a; walletNote();
  if (a !== address) {
    throw new Error(`Kaspire is on ${a ? shortAddr(a) : "another wallet or network"}. Switch Kaspire back to ${shortAddr(address)} on ${card && card.network === "testnet-10" ? "Testnet-10" : "mainnet"}, then try again.`);
  }
}

function checkNetwork(a) {
  if (!a.startsWith(wantedPrefix())) {
    throw new Error(`This membership is on ${card ? card.network : "another network"}. Switch Kaspire to that network and try again (your address should start with "${wantedPrefix()}").`);
  }
}

// One button: connect if this site isn't approved yet, then go straight to signing.
$("signin").onclick = async () => {
  setError("");
  $("signin").disabled = true;
  $("signin").textContent = "Check Kaspire…";
  try {
    const kaspire = await detectKaspire();
    let a = null;
    try { a = (await kaspire.request({ method: "getAccounts" }))?.[0] || null; } catch {}
    if (!a) {
      a = (await kaspire.request({ method: "requestAccounts" }))?.[0] || null;
      if (!a) throw new Error("Kaspire didn't share a wallet. Open Kaspire, approve the connection, then try again.");
    }
    checkNetwork(a);
    setAddress(a);
    watchAccounts(kaspire);

    const { nonce } = await (await fetch("/v1/nonce")).json();
    const message = [
      "Sign in to KasPass",
      "Domain: " + location.host,
      "Address: " + address,
      "Nonce: " + nonce,
      "Issued At: " + new Date().toISOString(),
    ].join("\n");
    const signed = await kaspire.request({ method: "signMessage", params: { address, message } });
    const res = await fetch(L("/v1/login"), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ address, message, signature: signed.signature }),
    });
    if (!res.ok) throw new Error(await res.text());
    const out = await res.json();
    session = out.session;
    kaspireNow = address;
    setAddress(address);
    walletNote();
    await loadHolder();
    show("welcome", false);
    await refreshHeld();
    await loadQuotes();
    const top = held.filter((h) => h.active).sort((a, b) => b.rank - a.rank)[0];
    if (levels.length > 1 && top && top.level !== (LEVEL || levels[0].level) && !PAY) {
      await selectLevel(top.level, { quiet: true });
    } else {
      loadOffer();
      renderResult(out);
    }
    renderLevels();
    renderAccess();
  } catch (e) {
    if (e.code === 4001) setError("Cancelled in Kaspire. Press the button to try again.");
    else oops("walletErr", e, "That didn't work. Please try again.");
  } finally {
    $("signin").disabled = false;
    $("signin").textContent = "Sign in with Kaspire";
  }
};

// ---------------------------------------------------------------------------
// Kaspire mobile, over WalletConnect v2. The app signs the same sign-in message and the same
// transactions as the extension; the library loads only when someone presses the button.
const WC_PROJECT = "5dff3b7c95fb1abd4ffcd556328280ec"; // public Reown project id (allowed domains only)
const KASPIRE_LINK = "https://kaspire.kaslab.space/kaspire/wc";
const isAndroid = /Android/i.test(navigator.userAgent);
const isPhone = /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
// The Kaspire app is on Android (from kaspire.kaslab.space, not an app store yet). It isn't on
// iPhone yet: set this to true when it is, and iPhones get the same app button as Android.
const KASPIRE_IOS = false;
const KASPIRE_SITE = "https://kaspire.kaslab.space";
const appLink = () => KASPIRE_SITE;
const isIOS = /iPhone|iPad|iPod/i.test(navigator.userAgent);
// An iPhone with no way to sign in here yet: the page says so and points to a computer.
const noApp = () => isIOS && !KASPIRE_IOS && !window.kaspire;
const WC_LABEL = () => (isPhone && !window.kaspire ? "Sign in with the Kaspire app" : "Use the Kaspire app on my phone instead");
const WAKE = isAndroid ? `intent://dapp#Intent;scheme=kaspire;package=space.kaspire.wallet;S.browser_fallback_url=${encodeURIComponent(KASPIRE_LINK)};end` : KASPIRE_LINK;
let WC = null;
let wcClient = null;
// The creator's logo, for the app's approval screens (set when the brand loads).
let BRAND_ICON = null;
fetch("/v1/brand").then((r) => r.json()).then((b) => { BRAND_ICON = b.logo || null; }).catch(() => {});
let wcPending = 0;
// A phone pauses this tab while the Kaspire app is in front, and the connection can drop. Coming
// back, reconnect so the app's answer (kept by the relay) arrives.
function wcRevive() {
  if (!wcClient || !wcPending) return;
  try { wcClient.core.relayer.restartTransport().catch(() => {}); } catch {}
}
document.addEventListener("visibilitychange", () => { if (!document.hidden) setTimeout(wcRevive, 300); });
window.addEventListener("pageshow", () => setTimeout(wcRevive, 300));
window.addEventListener("focus", () => setTimeout(wcRevive, 300));
let wcCancel = null;
const wcChain = () => (card && card.network === "testnet-10" ? "kaspa:testnet-10" : "kaspa:mainnet");
const pairLink = (uri) => `${KASPIRE_LINK}?uri=${encodeURIComponent(uri)}`;
const androidPair = (uri) => `intent://wc?uri=${encodeURIComponent(uri)}#Intent;scheme=kaspire;package=space.kaspire.wallet;S.browser_fallback_url=${encodeURIComponent(pairLink(uri))};end`;
function loadScript(src) {
  return new Promise((res, rej) => { const el = document.createElement("script"); el.src = src; el.onload = res; el.onerror = () => rej(new Error("We couldn't start the link to the Kaspire app. Check your internet and try again.")); document.head.append(el); });
}
async function wcInit() {
  if (wcClient) return wcClient;
  if (!window.KasPassWC) await loadScript("/assets/walletconnect.js?v=63");
  wcClient = await window.KasPassWC.SignClient.init({
    projectId: WC_PROJECT,
    telemetryEnabled: false,
    // `redirect`: where a wallet that supports it sends you back after approving.
    // The site's name and logo with KasPass, shown on the app's approval screens.
    metadata: { name: offer && offer.creator_name ? `${offer.creator_name} · KasPass` : "KasPass", description: "Memberships on Kaspa", url: location.origin, icons: BRAND_ICON ? [new URL(BRAND_ICON, location.origin).href] : [], redirect: { universal: location.origin + location.pathname } },
  });
  wcClient.on("session_delete", () => { WC = null; });
  // The active wallet in the app changed: this connection no longer speaks for it.
  wcClient.on("session_event", ({ params }) => {
    if (params && params.event && params.event.name === "accountsChanged") wcChanged();
  });
  return wcClient;
}
const WC_CHANGED = "Your active Kaspire wallet changed. Press Reconnect the Kaspire app to pair the wallet you want to use.";
// Drops every connection and pairing in this browser, so the next press pairs again.
async function wcForget() {
  if (!wcClient) return;
  for (const s of wcClient.session.getAll()) { try { await wcClient.disconnect({ topic: s.topic, reason: { code: 6000, message: "Reconnecting" } }); } catch {} }
  try { for (const p of wcClient.core.pairing.getPairings()) await wcClient.core.pairing.disconnect({ topic: p.topic }); } catch {}
}
function wcChanged() {
  wcForget();
  const wasIn = !!WC;
  WC = null;
  $("wcGo").textContent = "Reconnect the Kaspire app";
  if (wasIn) { $("logout").onclick(); setError(WC_CHANGED); }
}
async function wcSwitch() {
  $("logout").onclick();
  await wcForget();
  $("wcGo").onclick();
}
// "kaspa:testnet-10:qq…" -> "kaspatest:qq…"
function wcAddress(sess) {
  const acc = ((sess.namespaces.kaspa && sess.namespaces.kaspa.accounts) || []).find((a) => a.startsWith(wcChain() + ":"));
  if (!acc) return null;
  const parts = acc.split(":");
  return (parts[1] === "testnet-10" ? "kaspatest:" : "kaspa:") + parts.slice(2).join(":");
}
// Looks like the extension to the rest of the page: sign-in, paying and checks work unchanged.
function wcAdapter(sess) {
  const a = wcAddress(sess);
  const ask = async (method, params) => {
    const p = wcClient.request({ topic: sess.topic, chainId: wcChain(), request: { method, params } });
    // On a phone: a button to bring the app forward (a tap, so the browser allows it; a new tab, so
    // this page can never be replaced by Kaspire's fallback page).
    wcPending++;
    if (isPhone) show("wcWake", true);
    try {
      return await p;
    } catch (e) {
      const err = new Error((e && e.message) || String(e));
      if (/no longer the active wallet|reconnect the dapp/i.test(err.message)) {
        wcChanged();
        throw Object.assign(new Error(WC_CHANGED), { code: "WALLET_CHANGED" });
      }
      if ((e && e.code === 4001) || /reject/i.test(err.message)) err.code = 4001;
      throw err;
    } finally {
      wcPending--;
      if (!wcPending) show("wcWake", false);
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
async function wcConnect() {
  const c = await wcInit();
  const now = Math.floor(Date.now() / 1000);
  const old = c.session.getAll().find((x) => x.expiry > now + 60 && wcAddress(x));
  if (old) return old; // connected before in this browser: no QR, just the sign-in approval
  const chain = wcChain();
  const { uri, approval } = await c.connect({ requiredNamespaces: { kaspa: { chains: [chain], methods: ["kaspa_getAccounts", "kaspa_signPersonal", "kaspa_signPskt"], events: ["accountsChanged"] } } });
  if (uri) {
    $("wcNet").textContent = chain === "kaspa:testnet-10" ? "Testnet-10" : "mainnet";
    for (const el of document.querySelectorAll(".wcNet2")) el.textContent = $("wcNet").textContent;
    $("wcImg").innerHTML = qrSvg(pairLink(uri));
    $("wcOpen").href = isAndroid ? androidPair(uri) : pairLink(uri);
    $("wcOpenBig").href = $("wcOpen").href;
    // On a phone, the main thing is a button that opens the app; the QR is for a computer.
    show("wcPhone", isPhone);
    show("wcDesk", !isPhone);
    show("wcBox", true);
    busy("wcStatus", "Waiting for the Kaspire app…", isPhone ? "Tap Open Kaspire app, approve the connection, then come back to this tab." : "Approve the connection in the app.");
    if (isPhone) $("wcBox").scrollIntoView({ behavior: "smooth", block: "center" });
  }
  const cancelled = new Promise((_, rej) => { wcCancel = () => rej(Object.assign(new Error("Cancelled."), { code: 4001 })); });
  try {
    return await Promise.race([approval(), cancelled]);
  } finally {
    wcCancel = null;
    show("wcBox", false);
    say("wcStatus", "");
  }
}
$("wcCancel").onclick = () => { if (wcCancel) wcCancel(); show("wcBox", false); };
$("wcWakeGo").href = WAKE;
$("wcWakeCheck").onclick = () => { wcRevive(); $("wcWakeCheck").textContent = "Checking…"; setTimeout(() => ($("wcWakeCheck").textContent = "Approved? Check again"), 3000); };
$("wcGo").onclick = async () => {
  setError("");
  $("wcGo").disabled = true;
  try {
    if (typeof qrcode !== "function") throw new Error("The Kaspire app can't connect from this page. Please reload it.");
    if ($("wcGo").textContent.startsWith("Reconnect")) await wcForget();
    const sess = await wcConnect();
    WC = wcAdapter(sess);
    $("wcGo").textContent = WC_LABEL();
    if (!WC.address) throw new Error("The app didn't share a wallet for this network. Switch Kaspire to " + (card && card.network === "testnet-10" ? "Testnet-10" : "mainnet") + " and try again.");
    await $("signin").onclick();
    if (!session) WC = null;
  } catch (e) {
    WC = null;
    if (e.code === 4001) setError("Cancelled in the Kaspire app. Press the button to try again.");
    else if (e.code === "WALLET_CHANGED") setError(WC_CHANGED);
    else oops("walletErr", e, "That didn't work. Please try again.");
  } finally {
    $("wcGo").disabled = false;
  }
};
// One clear way in for each device. A phone can't run browser extensions, so there the Kaspire
// app is the only button; on a computer it's the extension, with the phone as a small link.
function signinLayout() {
  const phone = isPhone && !window.kaspire;
  if (!$("wcGo").textContent.startsWith("Reconnect")) $("wcGo").textContent = WC_LABEL();
  $("wcGo").classList.toggle("linkish", !phone);
  $("wcGo").classList.remove("ghost");
  show("signin", !session && !phone);
  if (noApp()) show("wcGo", false);
  const g = $("getWallet");
  if (session) { g.innerHTML = ""; return; }
  if (noApp()) {
    g.innerHTML = `The Kaspire app isn't on iPhone yet. To join, open this page on a computer and use the free <a href="${KASPIRE_SITE}" target="_blank" rel="noopener">Kaspire browser extension</a>. <button class="linkish" id="copyLink" type="button">Copy this page's link</button>`;
    $("copyLink").onclick = () => navigator.clipboard.writeText(location.href).then(() => ($("copyLink").textContent = "Copied")).catch(() => {});
    return;
  }
  g.innerHTML = phone
    ? `Don't have it yet? <a href="${appLink()}" target="_blank" rel="noopener">Get the free Kaspire app</a>, add some KAS, then come back to this page.`
    : address ? "" : `No wallet yet? <a href="${KASPIRE_SITE}" target="_blank" rel="noopener">Get Kaspire for your browser</a> (free), add some KAS, then reload this page.`;
}
signinLayout();
// Inside the Kaspire app's own browser the wallet can arrive a moment after the page loads: as soon
// as it does, sign-in goes through it directly (one tap), like the extension on a computer.
window.addEventListener("kaspire#initialized", () => { show("signin", !session && !(isPhone && !window.kaspire)); show("wcGo", !session && !noApp()); signinLayout(); });

// Point holders to the club page, if this site runs one.
fetch("/v1/club?for=page").then((r) => r.json()).then((c) => { if (c.enabled) { $("clubLinkA").textContent = "Join " + c.club; show("clubLink"); show("navClub"); } }).catch(() => {});

// If this site is already approved in Kaspire, show which wallet will sign in (no pop-up).
(async () => {
  try {
    const kaspire = await detectKaspire(1500);
    const a = (await kaspire.request({ method: "getAccounts" }))?.[0];
    if (a) { setAddress(a); watchAccounts(kaspire); }
  } catch {}
})();

const endSession = (t) => { if (t) fetch("/v1/logout", { method: "POST", headers: { "x-kaspass-session": t } }).catch(() => {}); };
$("logout").onclick = () => { endSession(session); WC = null; session = null; PROMO_ME = null; held = []; heldPasses = []; QUOTES = {}; PAY = null; holder = { bps: 0, enabled: holder.enabled }; setAddress(kaspireNow || address); walletNote(); hideResults(); hideDone(); loadOffer(); renderLevels(); };

// v4: does this wallet (or one linked to it) qualify for the holder price right now?
async function loadHolder() {
  holder = { bps: 0, enabled: false };
  if (!card || !card.settings.v4 || !address) return;
  try {
    const r = await fetch(L("/v1/holder?address=" + encodeURIComponent(address)));
    if (r.ok) { const h = await r.json(); holder = { bps: h.bps || 0, enabled: !!h.enabled, setting: h.holder_bps || 0 }; }
  } catch {}
}

function hideResults() { holdsPass = false; ["result", "members", "join", "access"].forEach((id) => show(id, false)); }

async function renderResult(out, focusTxid) {
  if (out.now_ms) clockOffset = out.now_ms - Date.now();
  show("result");
  const badge = document.createElement("span");
  badge.className = "badge " + (out.active ? "ok" : "no");
  badge.innerHTML = '<span class="dot"></span>';
  const lname = levels.length > 1 ? (levelOf(LEVEL || levels[0].level) || {}).name : "";
  const others = held.filter((h) => h.active && h.level !== (LEVEL || (levels[0] || {}).level));
  badge.append(out.active ? (lname ? (v5Site() ? `Your ${lw()}: ${lname}` : `${lname} member`) : "Active member") : lname ? `Not a ${lname} member${others.length ? ` (you're ${others.map((h) => h.name).join(" and ")})` : ""}` : `No active ${pw()}`);
  $("statusBadge").replaceChildren(badge);
  if (levels.length < 2) renderWalletLine(out.passes);
  // The pass just changed, else a lifetime pass, else the active one, else the most recent.
  const newest = [...out.passes].sort((a, b) => b.expiry_ms - a.expiry_ms)[0];
  const p = out.passes.find((x) => x.txid === focusTxid) || out.passes.find((x) => x.active && x.lifetime) || out.passes.find((x) => x.active) || newest;
  dl($("passInfo"), p ? [
    [lw() === "plan" ? "Plan" : cap(lw()), p.lifetime ? (lname ? `${lname}, lifetime` : "Lifetime") : p.kind === "monthly" ? (lname ? `${lname}, ${perWord().toLowerCase()}` : perWord()) : p.kind],
    ["Status", fmtExpiry(p)],
    ...(p.lifetime ? [] : isV5()
      ? [["Renewal price", (() => {
          const hb = offer.v4 && offer.v4.discounts ? holder.bps : 0, keep = 1 - hb / 10000;
          return `${money(offer.price.currency, offer.price.amount * keep)} a ${unitOf(card.settings.period_ms)}${hb ? ` (holder price, ${hb / 100}% off)` : ""}${offer.monthly_sompi && offer.price.currency !== "KAS" ? `, about ${kasRound(disc(offer.monthly_sompi, hb))} today` : ""}`;
        })()]]
      : [["Your locked rate", `${kas(p.rate_sompi)} a ${unitOf(card.settings.period_ms)}`]]),
    ["Paid so far", kas(p.paid_sompi)],
    ...(card.settings.v4 && !p.lifetime ? [["Prepaid balance", balanceText(p)]] : []),
    ["On-chain", p.txid + ":" + p.index, true],
  ] : [["Wallet", `This wallet has no ${pw()} here.`]]);
  if (p && card.settings.v4 && !p.lifetime) {
    const dd = [...$("passInfo").children].find((el, i, a) => el.tagName === "DD" && a[i - 1].textContent === "Prepaid balance");
    if (dd) dd.className = "bal" + (p.balance_sompi >= renewPrice(p) ? " on" : "");
  }
  renderPassActions(p);
  holdsPass = out.passes.length > 0;
  show("buyBox", !!offer && !holdsPass);
  if (p && !out.active) {
    $("joinLead").textContent = `Your ${pw()} has expired.`;
    say("renewHint", isV5() ? `Renew it above to reactivate it at today's price.` : `Renew it above to reactivate it at your locked rate of ${kas(p.rate_sompi)}${p.lifetime_price_sompi > 0 && p.credit ? ", and keep your credit toward lifetime" : ""}.`);
  } else if (lname && others.length) {
    const o = others[others.length - 1];
    $("joinLead").textContent = o.rank < (levelOf(LEVEL || levels[0].level) || {}).rank
      ? `You're a ${o.name} member. ${lname} includes everything in ${o.name}; after you join, you can end ${o.name} and get its deposit back.`
      : `You're a ${o.name} member, which already includes everything in ${lname}.`;
    say("renewHint", "");
  } else {
    $("joinLead").textContent = lname ? `This wallet isn't a ${lname} member yet.` : `This wallet doesn't have an active ${pw()} yet.`;
    say("renewHint", "");
  }
  show("join", !out.active);
  // v5 sites with several plans: the cards carry the prices and actions. A plan this wallet
  // doesn't hold has no pass to show, and the pay box only opens under a card.
  if (v5Site()) {
    show("result", !!p);
    show("join", !!PAY);
    if (PAY) renderPay();
  }
  show("members", false);
  if (DO) setTimeout(runDo, 0);
  renderSwitch();
  renderReturn(out.active || held.some((h) => h.active));
  if (out.active || held.some((h) => h.active)) {
    const r = await fetch("/v1/members?session=" + encodeURIComponent(session));
    if (r.ok) {
      const m = await r.json();
      $("membersBody").innerHTML = m.html;
      show("members");
    }
  }
}

async function showTelegram(hasAccess) {
  try {
    const t = await (await fetch("/v1/telegram")).json();
    show("tgBox", !!(t.enabled && t.group_connected && hasAccess));
  } catch (e) { show("tgBox", false); }
  try {
    const d = await (await fetch("/v1/discord")).json();
    $("dcServer").textContent = d.server ? "in " + d.server : "";
    show("dcBox", !!(d.enabled && d.ready && hasAccess));
  } catch (e) { show("dcBox", false); }
}

$("dcBtn").onclick = async () => {
  show("dcErr", false);
  try {
    const r = await fetch("/v1/discord/link", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    window.location.href = (await r.json()).url;
  } catch (e) {
    oops("dcErr", e, "That didn't work. Please try again.");
  }
};

$("tgBtn").onclick = async () => {
  show("tgErr", false);
  try {
    const r = await fetch("/v1/telegram/link", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    const { url } = await r.json();
    window.open(url, "_blank", "noopener");
    $("tgNote").textContent = "Telegram should open now. Press Start in the chat with the bot. The link works for 10 minutes; click again for a new one.";
  } catch (e) {
    oops("tgErr", e, "That didn't work. Please try again.");
  }
};


// ---------------------------------------------------------------------------
// Holder club, linked wallets and group access
let linking = false, linkCandidate = null, clubInfo = null;
const escH = (t) => String(t ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

async function renderAccess() {
  if (!session) return;
  show("clubErr", false);
  try {
    clubInfo = clubInfo || (await (await fetch("/v1/club?for=page")).json());
    const r = await fetch("/v1/club/me?session=" + encodeURIComponent(session));
    if (!r.ok) throw new Error(await r.text());
    const me = await r.json();
    show("clubBox", !!clubInfo.enabled);
    if (clubInfo.enabled) {
      $("clubName").innerHTML = `<b>${escH(clubInfo.club)}</b> · ${escH(clubInfo.collection.tick)} on ${escH(clubInfo.collection.network)}`;
      const st = me.standing;
      if (st) {
        const bands = st.bands.filter((b) => b[1] > 0).map((b) => `${b[1]} ${escH(b[0])}`).join(", ");
        $("clubStats").innerHTML = `<div class="v" style="font-size:22px;font-weight:700">${st.credits} credits</div><div class="small">${st.tokens} NFT${st.tokens === 1 ? "" : "s"}${bands ? " (" + bands + ")" : ""}${st.listed ? `, plus ${st.listed} listed for sale (${clubInfo.count_listed ? "counted" : "not counted while listed"})` : ""}</div>`;
        $("clubTiers").innerHTML = clubInfo.tiers.map((t) => {
          const ok = st.tiers.includes(t.name);
          return `<div class="small">${ok ? "✅" : "⬜"} <b>${escH(t.name)}</b>: ${escH(t.rule)}</div>`;
        }).join("");
      }
      $("walletList").innerHTML = me.wallets.map((w, i) => {
        const h = st && st.wallets.find((x) => x.address === w);
        return `<div class="small mono" style="margin:4px 0">${escH(w)} ${h ? `· ${h.tokens} NFT${h.tokens === 1 ? "" : "s"}` : ""} ${i === 0 ? "(signed in)" : `<a href="#" data-unlink="${escH(w)}">remove</a>`}</div>`;
      }).join("");
      document.querySelectorAll("[data-unlink]").forEach((a) => (a.onclick = async (e) => {
        e.preventDefault();
        await fetch("/v1/wallets/unlink", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, address: a.dataset.unlink }) });
        renderAccess();
      }));
    }
    show("noAccess", !me.has_access && !!clubInfo.enabled);
    await showTelegram(me.has_access);
    const here = (id) => $(id).parentElement && $(id).parentElement.closest("#access") && !$(id).classList.contains("hidden");
    show("access", !!clubInfo.enabled || here("tgBox") || here("dcBox"));
  } catch (e) {
    oops("clubErr", e, "This part of the page couldn't load. Refresh to try again.");
    show("access");
  }
}

$("addWallet").onclick = () => { linking = true; linkCandidate = null; $("linkSeen").textContent = ""; show("linkBox"); show("addWallet", false); };
$("linkCancel").onclick = () => { linking = false; if (linkCandidate) { kaspireNow = linkCandidate; walletNote(); } show("linkBox", false); show("addWallet"); };
$("linkSign").onclick = async () => {
  show("clubErr", false);
  try {
    const kaspire = await detectKaspire();
    let a = linkCandidate;
    if (!a || a === address) { try { a = (await kaspire.request({ method: "getAccounts" }))?.[0] || null; } catch { a = null; } }
    // A wallet (or network) this site hasn't been connected to yet: ask Kaspire to connect it.
    if (!a || a === address) a = (await kaspire.request({ method: "requestAccounts" }))?.[0] || null;
    if (!a) throw new Error("Kaspire didn't share a wallet. Approve the connection in Kaspire and try again.");
    if (a === address) throw new Error("Kaspire still shows the wallet you signed in with. Switch to the other wallet in Kaspire first.");
    const { nonce } = await (await fetch("/v1/nonce")).json();
    const message = ["Link wallet to KasPass", "Domain: " + location.host, "Address: " + a, "Link to: " + address, "Nonce: " + nonce, "Issued At: " + new Date().toISOString()].join("\n");
    const signed = await kaspire.request({ method: "signMessage", params: { address: a, message } });
    const r = await fetch("/v1/wallets/link", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, address: a, message, signature: signed.signature }) });
    if (!r.ok) throw new Error(await r.text());
    linking = false;
    kaspireNow = a; walletNote();
    show("linkBox", false);
    show("addWallet");
    $("linkHelp").textContent = "";
    await renderAccess();
    $("clubErr").textContent = "Linked. You stay signed in while you switch Kaspire back to the wallet you signed in with.";
    show("clubErr");
  } catch (e) {
    $("clubErr").textContent = e.code === 4001 ? "Cancelled in Kaspire." : "Couldn't link: " + (e.message || e);
    show("clubErr");
  }
};

$("lookupBtn").onclick = async () => {
  const a = $("lookup").value.trim();
  const out = $("lookupOut");
  out.textContent = "Checking…";
  try {
    const r = await fetch("/v1/check?address=" + encodeURIComponent(a));
    if (!r.ok) throw new Error(await r.text());
    const j = await r.json();
    if (j.now_ms) clockOffset = j.now_ms - Date.now();
    const p = j.passes.find((x) => x.active) || j.passes[0];
    out.textContent = j.active ? `Active: ${p.kind} ${pw()}, ${fmtExpiry(p).toLowerCase()}.` : (p ? `Not active: ${p.kind} ${pw()}, ${fmtExpiry(p).toLowerCase()}.` : `This wallet has no ${pw()} here.`);
  } catch (e) {
    out.textContent = e.message;
  }
};

// ---- Buying -------------------------------------------------------------

// v5: what the signed-in wallet would pay for each level right now (/v1/quote), by level id.
let QUOTES = {};
// A holder promotion ("NACHO holders: your first month is free") and where this wallet stands.
let PROMO = null, PROMO_ME = null, claiming = false;
// The kind chosen in each level's pay box, and quotes for lifetime and commitment purchases.
const PAYKIND = {}, KQUOTES = {}, KFETCH = new Set();
const v5Site = () => levels.length > 1 && levels.every((l) => l.price);
async function loadPromo() {
  try { const r = await fetch("/v1/promo"); PROMO = r.ok ? await r.json() : null; } catch { PROMO = null; }
  if (PROMO && !PROMO.active) PROMO = null;
}
async function loadPromoMe() {
  PROMO_ME = null;
  if (!PROMO || !session) return;
  try {
    const r = await fetch("/v1/promo/check", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session }) });
    PROMO_ME = r.ok ? await r.json() : { error: await r.text() };
  } catch {}
}
// Self-serve free trials, per level: offered, how long, the first month's price, and (signed in)
// whether this wallet can start one.
let TRIALS = {};
async function loadTrials() {
  if (!v5Site()) return;
  const next = {};
  await Promise.all(levels.map(async (l) => {
    try {
      const r = await fetch("/v1/trial?level=" + encodeURIComponent(l.id) + (session && address ? "&address=" + encodeURIComponent(address) : ""));
      if (r.ok) { const t = await r.json(); if (t.offered) next[l.level] = t; }
    } catch {}
  }));
  TRIALS = next;
}
const trialDays = (t) => Math.max(1, Math.round(t.trial_ms / 86400000));
const trialWords = (t) => { const m = Math.max(1, Math.round(t.trial_ms / 60000)); return t.trial_ms >= 86400000 ? `${trialDays(t)} day${trialDays(t) === 1 ? "" : "s"}` : `${m} minute${m === 1 ? "" : "s"}`; };
const trialLen = (t) => (t.trial_ms >= 86400000 ? `${trialDays(t)}-day` : `${Math.max(1, Math.round(t.trial_ms / 60000))}-minute`);
// A pass that is a self-serve trial: nothing paid yet, on a level that offers them.
const isTrialPass = (p) => p && p.active && !p.lifetime && p.paid_sompi === 0 && !!TRIALS[p.level];

async function startTrial(l, b) {
  if (!session || !address) {
    say("trialHint", "Sign in with your wallet first (just below), then press Start again.");
    $("wallet").scrollIntoView({ behavior: "smooth", block: "start" });
    return;
  }
  await runPaid({ url: "/v1/trial/prepare", level: l.level, body: { address }, statusId: "buyStatus", errId: "buyErr", buttons: [b], doneScreen: true, action: "trial" });
  await loadTrials();
  renderLevels();
}

// The one box a member on a trial needs: when it ends, what happens then, and how to cancel.
function renderTrialBox() {
  const box = $("trialBox");
  if (!box) return;
  const p = (heldPasses || []).filter(isTrialPass).sort((a, b) => a.expiry_ms - b.expiry_ms)[0];
  if (!p) { show("trialBox", false); return; }
  const t = TRIALS[p.level];
  const name = levelName(p.level) || pw({ cap: true });
  const left = p.expiry_ms - chainNow();
  const days = Math.floor(left / 86400000), hours = Math.floor(left / 3600000);
  const inWords = days >= 1 ? `${days} day${days === 1 ? "" : "s"}` : hours >= 1 ? `${hours} hour${hours === 1 ? "" : "s"}` : "less than an hour";
  const fp = t.first_period;
  const u = unitOf(levelOf(p.level).period_ms);
  const price = fp ? (fp.currency === "KAS" ? kas(fp.sompi) : money(fp.currency, fp.amount)) : `your first ${u}`;
  const chargeAt = p.expiry_ms - (t.charge_window_ms || 0);
  const when = (ms) => new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  if (p.balance_sompi > 0) {
    const held = kas(p.balance_sompi);
    box.innerHTML = `<h2>Your free trial of ${escH(name)} ends in ${escH(inWords)}</h2>
      <p>You set aside <b>${escH(held)}</b> when you started. It is still yours. On <b>${escH(fmtDate(p.expiry_ms))}</b> your first ${escH(u)} (${escH(price)}) is taken from it and you stay a member. Nothing is charged before then. Anything left over stays in your ${escH(pw())}, and you can take it back any time from "Manage your ${escH(pw())}".</p>
      <p class="muted small">Changed your mind? Cancel before ${escH(when(chargeAt))} and all ${escH(held)} goes straight back to your wallet. You keep access until the trial ends.</p>
      <button id="trialCancel" class="warn">Cancel my trial</button>
      <div id="trialStatus" class="small" role="status"></div><div id="trialErr" class="err small" role="alert"></div>`;
    $("trialCancel").onclick = () => runPaid({
      url: "/v1/pass/prepare", level: p.level, body: { address, txid: p.txid, index: p.index, action: "withdraw" },
      statusId: "trialStatus", errId: "trialErr", buttons: [$("trialCancel")],
      done: (j) => j.passes.some((x) => x.owner_hash === p.owner_hash && x.balance_sompi === 0 && x.active),
    });
  } else {
    const dep = kas(p.value_sompi || (card ? card.settings.bond : KAS));
    box.innerHTML = `<h2>Your trial is cancelled</h2>
      <p>Nothing was charged, and the money you set aside is back in your wallet. You still have ${escH(name)} until <b>${escH(fmtDate(p.expiry_ms))}</b>.</p>
      <p class="muted small">You can join again once the trial ends on ${escH(fmtDate(p.expiry_ms))}. Your ${escH(dep)} deposit stays in the ${escH(pw())} until then; after that, "Manage your ${escH(pw())}" gives it back.</p>`;
  }
  show("trialBox", true);
}

async function loadQuotes() {
  await loadTrials();
  QUOTES = {};
  for (const k of Object.keys(KQUOTES)) delete KQUOTES[k];
  if (!session || !address || !v5Site()) return;
  loadPromoMe().then(() => { if (!claiming) renderLevels(); });
  await Promise.all(levels.map(async (l) => {
    try {
      const r = await fetch("/v1/quote?level=" + encodeURIComponent(l.id) + "&address=" + encodeURIComponent(address));
      if (r.ok) QUOTES[l.level] = await r.json();
    } catch {}
  }));
}
const kasOf = (x) => kas(Math.round(Number(x) * KAS));
const kasAbout = (x) => kasRound(Math.round(Number(x) * KAS));
const levelName = (id) => (levelOf(id) || {}).name || "";

async function loadOffer() {
  try {
    const r = await fetch(L("/v1/offer"));
    offer = r.ok ? await r.json() : null;
  } catch { offer = null; }
  const box = $("buyButtons");
  box.replaceChildren();
  show("buyBox", !!offer && !holdsPass);
  show("noShop", !offer && !(card && card.placeholder));
  if (!offer) return;
  if (offer.pass_word) { PW = offer.pass_word; applyWord(); }
  if (offer.name) { $("tierTitle").textContent = offer.name; syncTierSum(); if (document.title === "KasPass") document.title = `${offer.name} · Membership`; }
  const sale = offer.sale || null;
  const regular = (kind) => (sale && kind !== "term" ? (kind === "lifetime" ? (sale.lifetime_pct ? sale.regular_lifetime_sompi : 0) : (sale.monthly_pct ? sale.regular_monthly_sompi : 0)) : 0);
  const pctOf = (kind) => (sale ? (kind === "lifetime" ? sale.lifetime_pct : sale.monthly_pct) : 0);
  // A holder discount comes off the on-chain price; a sale is shown against the regular price.
  const hb = offer.v4 && offer.v4.discounts ? holder.bps : 0;
  const priceHtml = (kind, price) => {
    // v5: the holder discount and any sale (percent off new purchases), shown off today's price.
    const sp = offer.price && offer.sale && offer.sale.pct ? offer.sale.pct : 0;
    const off5 = offer.price ? Math.round(10000 - (10000 - hb) * (100 - sp) / 100) : 0;
    if (offer.price && off5) return v5HolderHtml(offer, kind, price, off5);
    if (offer.price) return kind === "monthly" ? v5PriceHtml(offer, true) : v5KindHtml(offer, kind);
    const final = disc(price, hb);
    if (regular(kind)) return `<s>${kas(regular(kind))}</s>${kas(final)}<span class="off">${hb ? Math.round(100 - final * 100 / regular(kind)) : pctOf(kind)}% off</span>`;
    if (hb) return `<s>${kas(price)}</s>${kas(final)}<span class="off">${hb / 100}% off</span>`;
    return kas(price);
  };
  if (sale) {
    const best = Math.max(sale.monthly_pct || 0, sale.lifetime_pct || 0);
    const ends = sale.ends_ms ? new Date(sale.ends_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "";
    const after = offer.price ? "The sale price is for joining; renewals are at the regular price." : "Your rate stays locked in when you renew.";
    $("saleBanner").innerHTML = `${escH(sale.label || "Sale")}: ${sale.pct ? `${sale.pct}% off` : `up to ${best}% off`}` + `<div class="sub">${ends ? `Ends ${escH(ends)}. ` : ""}${after}</div>`;
  }
  show("saleBanner", !!sale);
  const v4 = offer.v4 || null;
  const termN = v4 && v4.term_sompi > 0 ? Math.round(v4.term_ms / offer.period_ms) : 0;
  const u = unitOf(offer.period_ms);
  // What each choice is for, so the price makes sense next to the others.
  const monthly = disc(offer.monthly_sompi, hb);
  const termSave = termN ? monthly * termN - disc(v4.term_sompi, hb) : 0;
  show("autoLifeNote", offer.lifetime_sompi > 0);
  const lifeMonths = offer.lifetime_sompi > 0 ? Math.round(offer.lifetime_sompi / Math.max(1, offer.monthly_sompi)) : 0;
  const card1 = (kind, title, price, per, desc, save, cls) => {
    const d = document.createElement("div");
    d.className = "opt" + (cls ? " " + cls : "");
    d.innerHTML = `<div class="ot">${escH(title)}</div><div class="op">${priceHtml(kind, price)} <span>${escH(per)}</span></div>${save ? `<div class="save">${escH(save)}</div>` : ""}<div class="od">${escH(desc)}</div>`;
    const b = document.createElement("button");
    b.textContent = `Choose ${title.toLowerCase()}`;
    if (kind !== "monthly") b.className = "ghost";
    b.disabled = !offer.available || !!offer.price_unavailable;
    b.dataset.off = offer.available ? "0" : "1";
    b.dataset.kind = kind;
    b.onclick = () => buy(kind);
    d.append(b);
    box.append(d);
  };
  card1("monthly", perWord(), offer.monthly_sompi, `a ${u}`, `Pay as you go. Renew each ${u}, or stop any time.`, "", "pick");
  if (termN) {
    const pct = termSave > 0 ? Math.round(termSave * 100 / (monthly * termN)) : 0;
    card1("term", `Commitment`, v4.term_sompi, `for ${periods(termN)}`,
      `Paid once up front for ${periods(termN)}, not refundable. After that it continues ${perWord().toLowerCase()}.`,
      termSave > 0 ? `Save ${kas(termSave)} (${pct}%) compared with paying each ${u}` : "");
  }
  if (offer.lifetime_sompi > 0) {
    card1("lifetime", "Lifetime", offer.lifetime_sompi, "once",
      `Pay once and never renew.${offer.credit && upgradeOffered() ? ` If you start ${perWord().toLowerCase()}, what you've paid counts toward it.` : ""}`,
      lifeMonths > 1 ? `Costs the same as ${periods(lifeMonths)}` : "");
  }
  // Testing note: a "month" here is minutes long.
  if (testing()) {
    $("testNote").innerHTML = `<b>Testing:</b> a ${u} here lasts ${mins(offer.period_ms)}, so you can watch renewals and auto-renew happen. This is Testnet-10, so nothing costs real money.`;
  }
  show("testNote", testing());
  // Holder price banner.
  if (v4 && v4.discounts && v4.holder_bps > 0) {
    $("holderBanner").innerHTML = hb
      ? `Holder price: ${hb / 100}% off<div class="sub">Your NFTs qualify. Your wallet is checked at every renewal, so the discount lasts as long as you hold.</div>`
      : `NFT holders pay ${v4.holder_bps / 100}% less<div class="sub">${session ? "This wallet doesn't qualify yet. If your NFTs are in another wallet, link it in the holder club section below." : "Sign in to see your price."}</div>`;
  }
  show("holderBanner", !!(v4 && v4.discounts && v4.holder_bps > 0));
  // Auto-renew toggle.
  show("autoBox", !!(v4 && v4.auto_renew));
  const autoAmt = () => {
    const n = Number($("autoN").value);
    $("autoAmt").textContent = `= ${kas(n * offer.monthly_sompi)} more, held in your ${pw()}. It's still yours: whatever isn't used comes back if you cancel.`;
  };
  $("autoOn").onchange = () => {};
  $("autoN").onchange = () => { $("autoOn").checked = true; autoAmt(); };
  autoAmt();
  const left = offer.remaining >= 0 ? (offer.remaining === 0 ? " Sold out." : ` ${offer.remaining} left.`) : "";
  $("buyNote").textContent = `Every ${pw()} holds a ${kas(offer.bond_sompi)} deposit that you get back when you end it, and each payment has a tiny network fee. Kaspire shows every amount before you approve.` + left;
  // Prices at the top of the membership box, following the holder price after sign-in.
  const priceRows = [
    [perWord(), `${priceHtml("monthly", offer.monthly_sompi)} a ${u}`],
    ...(termN ? [["Commitment", `${priceHtml("term", v4.term_sompi)} for ${periods(termN)}${termSave > 0 ? ` <span class="small muted">(save ${kas(termSave)})</span>` : ""}`]] : []),
    ...(offer.lifetime_sompi > 0 ? [["Lifetime", `${priceHtml("lifetime", offer.lifetime_sompi)} once`]] : []),
  ];
  const tier = $("tier");
  tier.querySelectorAll("[data-price]").forEach((el) => el.remove());
  for (const [k, html] of priceRows.reverse()) {
    const dt = document.createElement("dt"); dt.textContent = k; dt.dataset.price = "1";
    const dd = document.createElement("dd"); dd.innerHTML = html; dd.dataset.price = "1";
    tier.prepend(dt, dd);
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const say = (id, msg) => { $(id).textContent = msg || ""; show(id, !!msg); };
// A transaction in progress: an animated hourglass, and a warning if you try to leave.
const HG = `<svg class="hg" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 3h12M6 21h12M7.5 3c0 5.5 4 6 4 9s-4 3.5-4 9M16.5 3c0 5.5-4 6-4 9s4 3.5 4 9" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/><path class="top" d="M9 6.5h6c-.7 2-2.2 3-3 4.6-.8-1.6-2.3-2.6-3-4.6z" fill="currentColor"/><path class="bot" d="M8.6 19.5c.7-2.2 2.3-3.3 3.4-4.6 1.1 1.3 2.7 2.4 3.4 4.6z" fill="currentColor"/></svg>`;
let txBusy = false;
addEventListener("beforeunload", (e) => { if (txBusy) { e.preventDefault(); e.returnValue = ""; } });
function busy(id, msg, sub = "Keep this page open until it's done.") {
  const el = $(id);
  el.innerHTML = `<div class="busy" role="status">${HG}<div><b></b><div class="bsub"></div></div></div>`;
  el.querySelector("b").textContent = msg;
  el.querySelector(".bsub").textContent = sub;
  show(id, true);
}

// Every paid action works the same way: the server builds it, Kaspire signs only
// your own coins, the server checks and broadcasts, then we wait for the indexer.
async function runPaid({ url, body, statusId, errId, buttons, done, level, doneScreen, action }) {
  if (doneScreen === undefined) doneScreen = ["renew", "upgrade"].includes(body && body.action);
  const lv = level || LEVEL;
  // `done(check, txid)` says when the change has shown up; by default, when the new pass appears.
  done = done || ((j, txid) => j.passes.some((p) => p.txid === txid));
  say(errId, "");
  buttons.forEach((b) => (b.disabled = true));
  try {
    if (!address || !session) throw new Error("Sign in with Kaspire first.");
    const kaspire = await detectKaspire();
    await ensureSigningWallet(kaspire);
    busy(statusId, "Getting your payment ready…", "This takes a few seconds.");
    const prep = await fetch(withLevel(url, lv), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    if (!prep.ok) throw new Error(await prep.text());
    const { quote, request, summary, price } = await prep.json();
    // v5: what this costs in the plan's currency today (with founding or upgrade credit).
    const v5note = price ? `${money(price.currency, price.amount)} today${price.founding ? " (founding price)" : ""}${price.credit > 0 ? `, after a ${money(price.currency, price.credit)} credit for your unused time` : ""}: ` : "";
    say(statusId, `${v5note}${summary.text}. Total ${kas(summary.total_sompi)}. ${WC ? "The Kaspire app on your phone" : "Kaspire"} will show these same lines. Check they match, then approve.`);
    if (summary.outputs) {
      const ul = document.createElement("ul");
      ul.className = "outs";
      for (const o of summary.outputs) {
        const li = document.createElement("li");
        const n = document.createElement("span"); n.className = "n"; n.textContent = "Output " + o.index;
        const w = document.createElement("span"); w.className = "what" + (o.label.startsWith("KasPass fee") ? " fee" : ""); w.textContent = `${o.kas} · ${o.label}`;
        const a = document.createElement("span"); a.className = "addr"; a.textContent = "to the address ending in " + String(o.address).slice(-4); a.title = o.address;
        li.append(n, w, a);
        ul.append(li);
      }
      $(statusId).append(ul);
    }
    const signed = await kaspire.request({ method: "signPskt", params: request });
    const signedJson = typeof signed === "string" ? signed : signed.psktTransactionJson;
    txBusy = true;
    busy(statusId, "Sending to the network…");
    const sub = await fetch("/v1/submit", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ quote, psktTransactionJson: signedJson }) });
    if (!sub.ok) throw new Error(await sub.text());
    const { txid } = await sub.json();
    busy(statusId, "Sent. Waiting for the network to confirm…", `Keep this page open until it's done. Transaction ${txid.slice(0, 12)}…`);
    for (let i = 0; i < 40; i++) {
      await sleep(2500);
      const r = await fetch(withLevel("/v1/check?address=" + encodeURIComponent(address), lv));
      if (!r.ok) continue;
      const j = await r.json();
      if (done(j, txid)) {
        say(statusId, "");
        await refreshHeld();
        await loadQuotes();
        if (doneScreen) PAY = null;
        if (lv === LEVEL) await renderResult(j, txid);
        else await showLevelResult();
        await loadOffer();
        renderLevels();
        renderPay();
        if (doneScreen) showDone(j, txid, lv, action || (body && body.action));
        return;
      }
    }
    say(statusId, "Your payment was sent and is still being confirmed. Please don't pay again. Refresh this page in a minute.");
  } catch (e) {
    say(statusId, "");
    if (e.code === 4001) say(errId, "Cancelled in Kaspire. Nothing was sent.");
    else oops(errId, e);
  } finally {
    txBusy = false;
    buttons.forEach((b) => (b.disabled = b.dataset.off === "1"));
  }
}

async function buy(kind, buttons) {
  const auto = offer && offer.v4 && $("autoOn").checked && kind !== "lifetime";
  const body = { address, kind, ...(auto ? { prepaid_periods: Number($("autoN").value) } : {}) };
  await runPaid({ url: "/v1/buy/prepare", body, statusId: "buyStatus", errId: "buyErr", buttons: buttons || [...$("buyButtons").querySelectorAll("button")], doneScreen: true });
}

// What one renewal of this pass costs this wallet (its locked rate, at the holder price if it qualifies).
// v5 plans renew at today's price; older ones at the pass's locked rate (less any holder discount).
const renewPrice = (p) => (offer && offer.price && offer.monthly_sompi ? disc(offer.monthly_sompi, offer.v4 && offer.v4.discounts ? holder.bps : 0) : disc(p.rate_sompi, holder.bps));

function balanceText(p) {
  const n = Math.floor(p.balance_sompi / Math.max(1, renewPrice(p)));
  if (p.balance_sompi === 0) return "None. Auto-renew is off";
  if (n === 0) return `${kas(p.balance_sompi)}, not enough for a renewal yet (${kas(renewPrice(p))})`;
  const wait = p.auto_renew === "waiting_for_price" ? ". Auto-renew is waiting for a price and tries again every few minutes; nothing is charged until it has one" : "";
  // Plans priced in a currency renew at today's KAS price, so the count is approximate.
  const v5 = !!(offer && offer.price && offer.price.currency !== "KAS");
  return `${kas(p.balance_sompi)}. Auto-renew is on: covers ${v5 ? "about " : ""}${n} renewal${n === 1 ? "" : "s"}${v5 ? " at today's price" : ""}${wait}`;
}

function upgradeCost(p) {
  return Math.max(0, p.lifetime_price_sompi - (p.credit ? p.paid_sompi : 0));
}

// Renew and upgrade buttons for the pass shown. Anyone can pay for a pass,
// but here it's always the signed-in wallet paying for its own pass.
function renderPassActions(p) {
  const box = $("passActions"), menu = $("passMenu");
  box.replaceChildren(); menu.replaceChildren();
  say("passNote", ""); say("passErr", "");
  show("transferBox", false); show("topupBox", false); show("passMenu", false);
  if (!p || !offer) { show("passActions", false); return; }
  const v4 = card.settings.v4;
  const auto = !!(v4 && !p.lifetime && p.balance_sompi >= renewPrice(p));
  const tooEarly = !p.lifetime && Math.max(p.expiry_ms, chainNow()) - chainNow() > card.settings.max_ahead_ms;
  const all = () => [...box.querySelectorAll("button"), ...menu.querySelectorAll("button")];
  const run = (body, extra = {}) => runPaid({ url: "/v1/pass/prepare", body: { address, txid: p.txid, index: p.index, ...body }, statusId: "passStatus", errId: "passErr", buttons: all(), ...extra });
  const gone = (j) => !j.passes.some((x) => x.txid === p.txid && x.index === p.index);
  // Two clicks for anything that ends something, so nobody does it by accident.
  const twoStep = (b, again, go) => () => {
    if (b.dataset.armed !== "1") {
      const was = b.textContent;
      b.dataset.armed = "1"; b.textContent = again;
      setTimeout(() => { b.dataset.armed = "0"; b.textContent = was; }, 6000);
      return;
    }
    go();
  };
  const btn = (label, cls, onclick, off = false) => {
    const b = document.createElement("button");
    b.textContent = label;
    if (cls) b.className = cls;
    b.dataset.off = off ? "1" : "0";
    b.disabled = off;
    b.onclick = onclick;
    return b;
  };
  const item = (title, desc, b) => {
    const row = document.createElement("div"); row.className = "item";
    const txt = document.createElement("div");
    const t = document.createElement("div"); t.className = "t"; t.textContent = title;
    const d = document.createElement("div"); d.className = "d"; d.textContent = desc;
    txt.append(t, d); row.append(txt, b); menu.append(row);
  };
  const renewLabel = `Renew · ${kas(renewPrice(p))}`;
  const renew = () => run({ action: "renew" });
  const topup = () => { show("topupBox", $("topupBox").classList.contains("hidden")); };
  $("topupGo").onclick = () => run({ action: "deposit", periods: Number($("topupN").value) }, { buttons: [...all(), $("topupGo")] });

  // The one thing most people want right now.
  // Renew leads only when it's due: expired, or in the last third of the period (when the
  // reminder goes out). Before that, on a v4 tier, the useful next step is auto-renew.
  const nearEnd = !p.lifetime && p.expiry_ms - chainNow() <= card.settings.period_ms / 3;
  let primary = null;
  if (!p.lifetime && !tooEarly && (!p.active || (!auto && (nearEnd || !v4)))) primary = "renew";
  else if (v4 && !p.lifetime && !auto) primary = "auto";
  if (primary === "renew") box.append(btn(renewLabel, "", renew));
  if (primary === "auto") box.append(btn("Turn on auto-renew…", "ghost", topup));
  const manage = btn(`Manage ${pw()} ▾`, "ghost", () => {
    const open = menu.classList.contains("hidden");
    show("passMenu", open);
    manage.textContent = `Manage ${pw()} ${open ? "▴" : "▾"}`;
  });
  box.append(manage);

  // People open this to leave: say first that there's nothing to do.
  if (!p.lifetime && p.active && !auto && !isTrialPass(p)) {
    const lead = document.createElement("div");
    lead.className = "lead";
    lead.innerHTML = `<b>Want to cancel?</b> You don't need to do anything. Your ${escH(pw())} ends on ${escH(fmtDate(p.expiry_ms))} unless you renew.`;
    menu.append(lead);
  }

  // Everything else, each with what it does.
  if (!p.lifetime && primary !== "renew") {
    item("Renew now", tooEarly ? `Opens when ${mins(card.settings.max_ahead_ms)} or less remain.` : `Adds a ${unitOf(card.settings.period_ms)}${testing() ? ` (${mins(card.settings.period_ms)} here)` : ""} at ${rateWord()}${holder.bps && v4 && !isV5() ? `, ${holder.bps / 100}% off as a holder` : ""}.`, btn(renewLabel, "ghost", renew, tooEarly));
  }
  if (v4 && !p.lifetime && primary !== "auto") {
    item(p.balance_sompi > 0 ? "Top up balance" : "Turn on auto-renew", `Pay a few renewals ahead and your ${pw()} renews itself before it runs out. Cancel any time and get your unused balance back.`, btn(p.balance_sompi > 0 ? "Top up…" : "Turn on…", "ghost", topup));
  }
  if (v4 && p.balance_sompi > 0) {
    const w = btn(`Cancel auto-renew · get ${kas(p.balance_sompi)} back`, "ghost warn", null);
    w.onclick = twoStep(w, "Press again to confirm", () => run({ action: "withdraw" }));
    item("Cancel auto-renew", `Your unused balance comes back to you. Your access stays until the current ${unitOf(card.settings.period_ms)} ends.`, w);
  }
  if (!p.lifetime && p.lifetime_price_sompi > 0 && upgradeOffered()) {
    const credit = p.credit && p.paid_sompi > 0 ? ` Includes ${kas(Math.min(p.paid_sompi, p.lifetime_price_sompi))} credit for what you've paid.` : "";
    item("Upgrade to lifetime", "Never renew again." + credit, btn(`Upgrade · ${kas(upgradeCost(p))}`, "ghost", () => run({ action: "upgrade" })));
  }
  if (card.settings.resellable) {
    item("Give it to someone else", `They get the rest of your time and you lose access.${card.settings.royalty > 0 ? ` You pay the creator ${kas(card.settings.royalty)} when it changes hands.` : ""}`, btn("Give away…", "ghost", () => { show("transferBox", $("transferBox").classList.contains("hidden")); $("transferTo").focus(); }));
    $("transferGo").onclick = () => {
      const to = $("transferTo").value.trim();
      if (!to) { say("passErr", "Paste their Kaspa address (or .kas or .k name) first."); return; }
      run({ action: "transfer", to }, { buttons: [...all(), $("transferGo")], done: gone });
    };
  }
  const dep = kas(card.settings.bond);
  const timeLeft = !p.lifetime && p.active;
  const c = btn(timeLeft || p.lifetime ? "End now and lose my remaining time" : `Get my ${dep} deposit back`, "ghost warn", null);
  c.onclick = twoStep(c, "Press again to end it for good", () => run({ action: "close" }, { done: gone }));
  const bal = v4 && p.balance_sompi > 0 ? ` Your ${kas(p.balance_sompi)} auto-renew balance comes back too.` : "";
  if (p.lifetime) item(`End ${pw()} now`, `You lose access today, for good. Only your ${dep} deposit comes back.${bal}`, c);
  else if (timeLeft) item(`End ${pw()} now`, `You lose access today. The time you paid for (until ${fmtDate(p.expiry_ms)}) is not refunded. Only your ${dep} deposit comes back.${bal}`, c);
  else item("Get your deposit back", `This ${pw()} has ended. Ending it for good returns your ${dep} deposit.${bal}`, c);
  show("passActions", true);

  // One short line about where things stand.
  const note = p.lifetime ? `Lifetime ${pw()}: nothing to renew.`
    : auto ? `Auto-renew is on. Nothing to do: it renews itself shortly before each ${unitOf(card.settings.period_ms)} ends.`
    : !p.active ? `Renew to bring this ${pw()} back at ${rateWord()}.`
    : primary === "auto" ? `You're paid up for now. Turn on auto-renew and the ${pw()} renews itself, or renew by hand later.`
    : tooEarly ? `You're paid up. Renewals open when ${mins(card.settings.max_ahead_ms)} or less remain.`
    : `Renewing adds a ${unitOf(card.settings.period_ms)} at ${rateWord()}.`;
  say("passNote", note);
}

// After paying: the result, big and first, with the way back and anything left to tidy up.
// After a payment, the way back tells the partner site so (`paid=1`), so it can say "confirming your
// payment" instead of "welcome back". Only a hint: the partner still checks the pass itself.
function markPaidReturn() {
  if (!RETURN) return;
  try { const u = new URL(RETURN); u.searchParams.set("paid", "1"); $("returnGo").href = u.href; $("returnLink").href = u.href; } catch {}
}
function showDone(j, txid, lv, action) {
  const ps = (j.passes || []);
  const p = ps.find((x) => x.txid === txid) || ps.filter((x) => x.active).sort((a, b) => b.expiry_ms - a.expiry_ms)[0];
  if (!p) return;
  markPaidReturn();
  const name = levelName(lv) || (offer && offer.name) || pw({ cap: true });
  const m = pw();
  $("doneTitle").textContent = p.lifetime ? `You're in. You have ${name} for life.`
    : action === "renew" ? `Renewed. Your ${name} ${m} now runs until ${fmtDate(p.expiry_ms)}.`
    : action === "trial" ? `Your free trial of ${name} has started. It ends on ${fmtDate(p.expiry_ms)}.`
    : action === "promo" ? `You're in. Your free ${name} ${m} runs until ${fmtDate(p.expiry_ms)}.`
    : `You're in. Your ${name} ${m} runs until ${fmtDate(p.expiry_ms)}.`;
  show("doneTitle", true);
  // The next step, right here: the members' groups. The buttons move up from "Your access".
  $("doneNext").append($("tgBox"), $("dcBox"));
  const auto = p.balance_sompi > 0 && action !== "trial";
  say("doneLine", p.lifetime || action === "trial" ? "" : auto
    ? `Auto-renew is on: it renews itself from your balance before ${fmtDate(p.expiry_ms)}.`
    : `It won't renew by itself. Come back before ${fmtDate(p.expiry_ms)} to renew, or turn on auto-renew in "Manage your ${m}".`);
  $("switchSlot").append($("switchBox"));
  $("passDetails").open = false;
  FOLDLIST = true;
  renderLevels();
  renderAccess();
  renderDoneBox();
  $("doneBox").scrollIntoView({ behavior: "smooth", block: "start" });
  setTimeout(() => { try { $("doneTitle").focus({ preventScroll: true }); } catch {} }, 400);
}
function hideDone() {
  show("doneTitle", false);
  say("doneLine", "");
  FOLDLIST = false;
  if ($("tgBox").parentElement === $("doneNext")) $("noAccess").after($("tgBox"), $("dcBox"));
  const sw = $("switchBox");
  if (sw.parentElement === $("switchSlot")) $("passStatus").before(sw);
  renderDoneBox();
}
let FOLDLIST = false;
const renderDoneBox = () => show("doneBox", !$("doneTitle").classList.contains("hidden") || !$("returnDone").classList.contains("hidden") || ($("switchBox").parentElement === $("switchSlot") && !$("switchBox").classList.contains("hidden")));

// First-time visitors: a short how-to-join, until they sign in or hide it.
const welcomeHidden = () => { try { return localStorage.getItem("kaspass.welcome") === "hidden"; } catch { return false; } };
function renderWelcome() {
  show("welcome", !session && !welcomeHidden() && !(card && card.placeholder));
  const w = $("welcome");
  if (!w.dataset.fold && window.matchMedia && matchMedia("(max-width: 700px)").matches) {
    w.dataset.fold = "1";
    w.classList.add("folded");
    const t = document.createElement("button");
    t.type = "button";
    t.className = "more-t";
    t.setAttribute("aria-expanded", "false");
    t.innerHTML = 'How joining works <span aria-hidden="true">▾</span>';
    t.onclick = () => {
      const open = w.classList.toggle("folded") === false;
      t.setAttribute("aria-expanded", String(open));
      t.innerHTML = `${open ? "Hide the steps" : "How joining works"} <span aria-hidden="true">${open ? "▴" : "▾"}</span>`;
    };
    w.querySelector("h2").after(t);
  }
  if (!card) return;
  const t = card.settings;
  // The three steps, for the device in hand: the app on a phone, the extension on a computer.
  const test = card.network !== "mainnet";
  const phone = isPhone && !window.kaspire;
  const get = noApp()
    ? `It's free, but the app isn't on iPhone yet, so use a computer: add <a href="${KASPIRE_SITE}" target="_blank" rel="noopener">Kaspire</a> to your browser.`
    : phone
    ? `It's free. Get the <a href="${appLink()}" target="_blank" rel="noopener">Kaspire app</a>.`
    : `It's free. Add <a href="${KASPIRE_SITE}" target="_blank" rel="noopener">Kaspire</a> to your browser (on a phone, get the Kaspire app instead).`;
  $("wStep1").innerHTML = `${get}${test ? " Switch it to <b>Testnet-10</b>." : ""} Write down the recovery words it gives you and keep them safe. <a href="/help">Step-by-step help</a>.`;
  $("wStep2").innerHTML = test
    ? `This site uses test KAS, which is free: get some from the <a href="https://faucet-testnet.kaspanet.io" target="_blank" rel="noopener">test faucet</a>. Nothing here costs real money.`
    : `KAS is the Kaspa coin. Buy it on an exchange and send it to your Kaspire address. You need a little more than the ${escH(lw())}'s price. <a href="/help#kas">Where to buy KAS</a>.`;
  $("wStep3").textContent = `Signing in is free. Kaspire shows every amount before you approve${t.v4 ? ", and nothing renews by itself unless you turn on auto-renew" : ""}.`;
  applyLevelWord();
  renderHero();
  namesOf(card.creator_address).then((n) => { n = (offer && offer.creator_name) || (n || "").split(" · ")[0]; if (site.club_name) {
      $("pageTitle").textContent = site.club_name; document.title = site.club_name;
      if (n) { $("pageBy").textContent = "by " + n; show("pageBy"); }
    } else if (n) { $("pageTitle").textContent = n; document.title = `${n} · ${pw({ cap: true })}`; }
    if (n && !$("brandName").textContent) $("brandName").textContent = (site.club_name || n);
    if (n) $("wLead").innerHTML = `This is <b>${escH(n)}</b>'s ${escH(pw())}. When you join, it lives in your own Kaspa wallet: nobody can take it away, and there's no card to enter.`; });
  renderHolderRow();
  renderCreatorRow();
}

// Creator: the name they chose (if any), then their wallet's .k/.kas names, then the address.
async function renderCreatorRow() {
  const n = await namesOf(card.creator_address);
  const chosen = offer && offer.creator_name;
  if (!n && !chosen) return;
  const rows = [...$("tier").children];
  const i = rows.findIndex((el) => el.tagName === "DT" && el.textContent === "Creator");
  if (i < 0) return;
  rows[i + 1].className = "";
  rows[i + 1].innerHTML = (chosen ? `<b>${escH(chosen)}</b><br>` : "") + (n ? (chosen ? `<span class="small">${escH(n)}</span><br>` : `<b>${escH(n)}</b><br>`) : "") + `<span class="mono muted">${escH(card.creator_address)}</span>`;
}

// The holder offer lives with the prices, in the tier box.
async function renderHolderRow() {
  const v4 = offer && offer.v4;
  if (!v4 || !(v4.max_discount_bps > 0)) return;
  if (clubInfo === null) { try { clubInfo = await (await fetch("/v1/club?for=page")).json(); } catch { clubInfo = { enabled: false }; } }
  const rows = [...$("tier").children];
  const i = rows.findIndex((el) => el.tagName === "DT" && el.textContent === "Holder discount");
  if (i < 0) return;
  const club = clubInfo && clubInfo.enabled ? ` Holders also get free access to <a href="/club">${escH(clubInfo.club)}</a>.` : "";
  const tick = clubInfo && clubInfo.enabled ? escH(clubInfo.collection.tick) : "NFT";
  rows[i + 1].innerHTML = v4.discounts && v4.holder_bps > 0
    ? `Own a ${tick} NFT? You pay ${v4.holder_bps / 100}% less. Your wallet is checked each time you renew, so the discount lasts as long as you hold.${club ? `<br>${club.trim()}` : ""}`
    : `NFT holders can pay up to ${v4.max_discount_bps / 100}% less, checked each time they renew.${club ? `<br>${club.trim()}` : ""}`;
}
$("wClose").onclick = () => { try { localStorage.setItem("kaspass.welcome", "hidden"); } catch {} show("welcome", false); };

// ---- Several paid levels --------------------------------------------------

// Sent here from a partner site the server allows (`serve --allow-return`): remember where to go
// back to (for this tab, so it survives signing in), and offer the way back once access is active.
let RETURN = null;
function pickReturn(allowed) {
  let u = null;
  try { u = new URLSearchParams(location.search).get("return") || sessionStorage.getItem("kaspass.return"); } catch {}
  if (!u || !allowed.length) return;
  let url;
  try { url = new URL(u); } catch { return; }
  if (url.protocol !== "https:" || !allowed.includes(url.origin.toLowerCase())) return;
  RETURN = url.href;
  try { sessionStorage.setItem("kaspass.return", RETURN); } catch {}
  document.querySelectorAll(".rh").forEach((e) => (e.textContent = url.host));
  $("returnHost").textContent = url.host;
  $("returnLink").href = RETURN;
  $("returnGo").href = RETURN;
  show("returnBar");
}
let returnShown = false;
function renderReturn(active) {
  const on = !!RETURN && !!active;
  show("returnDone", on);
  renderDoneBox();
  // Going back is the next step: the other pass actions step down to plain buttons.
  $("result").classList.toggle("returning", on);
  if (!on) return;
  $("returnBar").querySelector("span").textContent = "Your access is active.";
  if (!returnShown) { returnShown = true; setTimeout(() => $("doneBox").scrollIntoView({ behavior: "smooth", block: "center" }), 300); }
}

async function loadLevels() {
  try {
    const r = await fetch("/v1/levels");
    const j = r.ok ? await r.json() : {};
    pickReturn(j.return_origins || []);
    levels = j.levels || [];
    FX = j.fx || null;
    PRICE_DISPLAY = j.price_display || "plan";
    site = { club_name: j.club_name || "", level_word: j.level_word || site.level_word };
  } catch { levels = []; }
  await loadPromo();
  const w = site.level_word;
  applyLevelWord();
  $("levelsTitle").textContent = `Choose your ${w.one}`;
  $("levelsLead").textContent = `Each ${w.one} includes everything in the cheaper ${w.many}.`;
  // Prices set in a currency: say once what KAS is, and that nothing is charged by itself.
  const curs = [...new Set(levels.map((l) => l.price && l.price.currency).filter((c) => c && c !== "KAS"))];
  const NAMES = { CAD: "Canadian dollars", USD: "US dollars", EUR: "euros" };
  $("levelsKas").textContent = (curs.length === 1 ? `KAS is the Kaspa coin. Prices are set in ${NAMES[curs[0]] || curs[0]} and you pay the same value in KAS. ` : "") + "Nothing is charged automatically unless you turn on auto-renew.";
  show("levelsKas", levels.length > 1);
  levelsTail();
}
// At the top of the page: the lowest price and a way straight to the plans.
function renderHero() {
  const w = site.level_word;
  const cheapest = levels.filter((l) => l.price && l.offer).sort((a, b) => (a.offer.monthly_sompi || 0) - (b.offer.monthly_sompi || 0))[0];
  if (levels.length > 1 && cheapest) {
    $("heroFrom").textContent = `From ${money(cheapest.price.currency, cheapest.price.amount)} a ${unitOf(cheapest.period_ms)}`;
    $("heroGo").textContent = `See ${w.many} and prices`;
    $("heroGo").onclick = () => $("levelsBox").scrollIntoView({ behavior: "smooth", block: "start" });
    show("hero", true);
  }
}
function levelsTail() {
  if (!LEVEL) {
    // A link may pick the level, e.g. an "Upgrade" button on a partner site: ?level=<id>.
    let want = null;
    try { want = new URLSearchParams(location.search).get("level"); } catch {}
    const hit = want && levels.find((l) => l.level === want || l.id === want);
    if (hit) LEVEL = hit.level;
  }
  if (levels.length > 1 && !LEVEL) LEVEL = levels[0].level;
  show("levelsBox", levels.length > 1);
}

// Which levels the signed-in wallet holds (from the chain), with their passes.
let heldPasses = [];
async function refreshHeld() {
  held = []; heldPasses = [];
  if (!address || levels.length < 2) return;
  try {
    const r = await fetch("/v1/check?address=" + encodeURIComponent(address));
    if (!r.ok) return;
    const j = await r.json();
    held = (j.levels || []).filter((l) => (j.passes || []).some((p) => p.level === l.level));
    heldPasses = j.passes || [];
    if (session) renderWalletLine(heldPasses);
  } catch {}
}

// A level's monthly price as shown: a sale against the regular price, a holder discount against the
// on-chain one, each with its % off.
// ---- v5 prices: plans priced in a currency, paid in KAS at today's rate --------
let FX = null, PRICE_DISPLAY = "plan";
const EURO = ["AT", "BE", "CY", "DE", "EE", "ES", "FI", "FR", "GR", "HR", "IE", "IT", "LT", "LU", "LV", "MT", "NL", "PT", "SI", "SK"];
// The visitor's currency, guessed from their browser's language and time zone.
function viewerCurrency() {
  let region = "";
  try { region = new Intl.Locale(navigator.language).maximize().region || ""; } catch {}
  let tz = "";
  try { tz = Intl.DateTimeFormat().resolvedOptions().timeZone || ""; } catch {}
  if (region === "CA" || /^America\/(Toronto|Vancouver|Edmonton|Winnipeg|Halifax|St_Johns|Regina|Montreal|Whitehorse|Yellowknife|Iqaluit|Moncton)/.test(tz)) return "CAD";
  if (EURO.includes(region)) return "EUR";
  return "USD";
}
function toCur(amount, from, to) {
  if (!FX || from === to) return from === to ? amount : null;
  const usd = from === "USD" ? amount : from === "CAD" ? amount / FX.usd_cad : from === "EUR" ? amount * FX.eur_usd : null;
  if (usd == null) return null;
  return to === "USD" ? usd : to === "CAD" ? usd * FX.usd_cad : to === "EUR" ? usd / FX.eur_usd : null;
}
function money(cur, amount) {
  // Plans priced in KAS: the exact amount, the way KAS is written everywhere else on the page.
  if (cur === "KAS") return `${Number(amount).toLocaleString(undefined, { maximumFractionDigits: 2 })} KAS`;
  // Whole amounts only (CA$10, CA$101): the exact KAS is on the pay button and in the wallet.
  const fmt = (loc) => new Intl.NumberFormat(loc, { style: "currency", currency: cur, minimumFractionDigits: 0, maximumFractionDigits: 0 }).format(amount);
  // Some browsers report a language tag Intl refuses (e.g. "en-US@posix"); fall back to the default.
  try { return fmt(navigator.language); } catch { try { return fmt(undefined); } catch { return `${amount} ${cur}`; } }
}
// A price as this site shows it: the plan's currency, or about the same in the visitor's.
function shown(cur, amount) {
  const v = viewerCurrency();
  if (PRICE_DISPLAY === "viewer" && v !== cur) {
    const x = toCur(amount, cur, v);
    if (x != null) return { main: "about " + money(v, x), plan: money(cur, amount) };
  }
  return { main: money(cur, amount), plan: "" };
}
// The price block for a v5 offer: founding first if there is one, then today's KAS amount.
function v5PriceHtml(o, big) {
  const p = o.price;
  const reg = shown(p.currency, p.amount);
  const f = o.founding;
  const kasLine = p.currency === "KAS" ? ""
    : o.price_unavailable || p.kas_today == null
    ? `<div class="kasline">Price unavailable right now; try again shortly</div>`
    : `<div class="kasline">${reg.plan ? escH(reg.plan) + " · " : ""}about ${kasRound(o.monthly_sompi)} today</div>`;
  const per = big === "card" ? ` <span class="per">a ${escH(unitOf(o.period_ms))}</span>` : "";
  if (f) {
    const fp = shown(p.currency, f.amount);
    const until = new Date(f.until_ms - 1).toLocaleDateString(undefined, { dateStyle: "medium" });
    return `<div class="was"><s>${escH(reg.main)}</s><span class="off">Founding</span></div>${escH(fp.main)}` +
      `<div class="kasline">Your first ${escH(unitOf(o.period_ms))} if you join by ${escH(until)}${f.kas_today != null ? ` · about ${kasAbout(f.kas_today)} today` : ""}</div>` + (big ? kasLine : "");
  }
  return `${escH(reg.main)}${per}${kasLine}`;
}

// A v5 lifetime or commitment price: the amount in the plan's currency, then today's KAS.
function v5KindHtml(o, kind) {
  const p = o.price, a = p[kind] && p[kind].amount;
  const sompi = kind === "lifetime" ? o.lifetime_sompi : (o.v4 && o.v4.term_sompi) || 0;
  if (a == null) return kasRound(sompi);
  if (p.currency === "KAS") return escH(kasRound(Math.round(a * KAS)));
  const reg = shown(p.currency, a);
  const line = sompi ? `${reg.plan ? escH(reg.plan) + " · " : ""}about ${kasRound(sompi)} today` : "Price unavailable right now; try again shortly";
  return `${escH(reg.main)}<div class="kasline">${line}</div>`;
}
// A v5 price with this wallet's NFT holder discount: the regular price struck through.
function v5HolderHtml(o, kind, sompi, hb) {
  const p = o.price;
  const amt = kind === "monthly" ? p.amount : p[kind] && p[kind].amount;
  const off = `<span class="off">${hb / 100}% off</span>`;
  if (p.currency === "KAS" || amt == null) return `<s>${kas(sompi)}</s>${kas(disc(sompi, hb))}${off}`;
  const keep = 1 - hb / 10000;
  return `<s>${escH(money(p.currency, amt))}</s>${escH(money(p.currency, amt * keep))}${off}<div class="kasline">about ${kasRound(disc(sompi, hb))} today</div>`;
}
// Upgrading a pass to lifetime pays the lifetime price locked into the pass. On a v5 plan priced
// in a currency that's the KAS ceiling, not today's price, so it's only offered on KAS plans.
const upgradeOffered = () => !isV5() || offer.price.currency === "KAS";

function levelPrice(l) {
  const o = l.offer;
  if (!o) return null;
  const hb = o.v4 && o.v4.discounts ? holder.bps : 0;
  const final = disc(o.monthly_sompi, hb);
  const reg = o.sale && o.sale.monthly_pct ? o.sale.regular_monthly_sompi : hb ? o.monthly_sompi : 0;
  const pct = reg ? Math.round(100 - (final * 100) / reg) : 0;
  return { final, reg: pct > 0 ? reg : 0, pct, u: unitOf(l.period_ms) };
}

// Every level below this one, by name: "Core and Plus".
function belowNames(l) {
  const below = levels.slice(0, l.rank).map((x) => escH(x.name));
  return below.length > 1 ? `${below.slice(0, -1).join(", ")} and ${below[below.length - 1]}` : below[0] || "";
}

// Cards side by side, or one table comparing what each level includes.
let VIEW = null;
function setView(v, remember) {
  VIEW = v === "compare" ? "compare" : "cards";
  if (remember) { try { localStorage.setItem("kaspass.view", VIEW); } catch {} }
  $("vCards").setAttribute("aria-pressed", String(VIEW === "cards"));
  $("vCompare").setAttribute("aria-pressed", String(VIEW === "compare"));
  renderLevels();
}
$("vCards").onclick = () => setView("cards", true);
$("seeOther").onclick = () => { FOLDLIST = false; renderLevels(); $("levelsBox").scrollIntoView({ behavior: "smooth", block: "start" }); };
$("vCompare").onclick = () => setView("compare", true);

function renderCompare() {
  const box = $("levelCompare");
  const check = '<svg class="yes" viewBox="0 0 24 24" aria-label="Included" role="img"><path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const cls = (l) => (l.level === LEVEL ? ' class="sel"' : "");
  let h = `<table class="cmp"><thead><tr><th scope="col">What's included</th>`;
  for (const l of levels) {
    const pr = levelPrice(l);
    const price = !pr ? "Not on sale" : l.offer.price ? v5PriceHtml(l.offer) + ` / ${pr.u}` : `${pr.reg ? `<s>${kas(pr.reg)}</s>` : ""}${kas(pr.final)} / ${pr.u}${pr.reg ? `<span class="off">${pr.pct}% off</span>` : ""}`;
    h += `<th scope="col"${cls(l)}><div class="cn">${escH(l.name)}</div><div class="cp">${price}</div></th>`;
  }
  h += "</tr></thead><tbody>";
  // Each perk is a row; it's included in its own level and every level above it.
  for (const l of levels) {
    for (const p of l.perks || []) {
      h += `<tr><th scope="row" style="font-weight:500">${escH(p)}${l.rank > 0 ? `<div class="from">from ${escH(l.name)}</div>` : ""}</th>`;
      for (const x of levels) h += `<td${cls(x)}>${x.rank >= l.rank ? check : '<span class="no" aria-label="Not included">&mdash;</span>'}</td>`;
      h += "</tr>";
    }
  }
  h += "</tbody><tfoot><tr><td></td>";
  for (const l of levels) {
    const mine = held.find((x) => x.level === l.level && x.active);
    const sel = l.level === LEVEL;
    h += `<td${cls(l)}><button data-pick="${escH(l.level)}" class="${sel ? "ghost" : ""}" ${sel ? "disabled" : ""}>${sel ? "Viewing" : "Choose"} ${escH(l.name)}</button>${mine ? `<div class="from">Your ${escH(lw())}</div>` : ""}</td>`;
  }
  box.innerHTML = h + "</tr></tfoot></table>";
  box.querySelectorAll("[data-pick]").forEach((b) => (b.onclick = () => selectLevel(b.dataset.pick)));
}

function renderLevels() {
  renderTrialBox();
  if (levels.length < 2) return;
  const anyPerks = levels.some((l) => l.perks && l.perks.length);
  // The visitor's own choice, else the creator's default.
  let stored = null; try { stored = localStorage.getItem("kaspass.view"); } catch {}
  const view = VIEW || stored || (page && page.view) || "cards";
  $("vCards").setAttribute("aria-pressed", String(view !== "compare"));
  $("vCompare").setAttribute("aria-pressed", String(view === "compare"));
  const narrow = window.matchMedia && matchMedia("(max-width: 700px)").matches;
  const compare = view === "compare" && anyPerks && !narrow;
  show("viewSw", anyPerks && !FOLDLIST);
  show("levelCards", !compare && !FOLDLIST);
  show("levelCompare", compare && !FOLDLIST);
  show("seeOther", FOLDLIST);
  $("seeOther").textContent = `See other ${lw(true)}`;
  show("levelsLead", !FOLDLIST);
  show("levelsKas", !FOLDLIST && !!$("levelsKas").textContent);
  $("levelsTitle").textContent = FOLDLIST ? `Other ${lw(true)}` : `Choose your ${lw()}`;
  if (compare) renderCompare();
  renderTierAbout();
  const box = $("levelCards");
  // The pay box may sit between the cards: keep it in the page while they're redrawn.
  if ($("join").parentElement === box) $("levelsBox").after($("join"));
  box.replaceChildren();
  for (const l of levels) {
    const o = l.offer;
    const d = document.createElement("div");
    const sel = l.level === LEVEL;
    d.className = "lvl" + (sel ? " sel" : "");
    const mine = held.find((h) => h.level === l.level && h.active);
    const pr = levelPrice(l);
    const price = !pr ? "Not on sale right now" : l.offer.price ? (l.offer.founding ? v5PriceHtml(l.offer) : v5PriceHtml(l.offer, "card")) : (pr.reg ? `<div class="was"><s>${kas(pr.reg)}</s><span class="off">${pr.pct}% off</span></div>${kas(pr.final)}` : kas(pr.final)) + `<div class="per">a ${pr.u}</div>`;
    const lower = belowNames(l);
    const about = richHtml(page && page.about ? page.about[l.level] : "");
    const hasMore = !!(about || lower || (l.perks && l.perks.length));
    d.innerHTML = `<div class="ln">${escH(l.name)}${mine ? `<span class="yours">Your ${escH(lw())}</span>` : ""}</div>` +
      `<div class="lp">${price}</div>` +
      `<div class="det"${OPEN.has(l.level) ? "" : " hidden"}>` +
      (about ? `<div class="rich about">${about}</div>` : "") +
      (lower ? `<div class="inc">Everything in ${lower}${l.perks && l.perks.length ? ", plus:" : ""}</div>` : "") +
      (l.perks && l.perks.length ? `<ul>${l.perks.map((x) => `<li>${escH(x)}</li>`).join("")}</ul>` : `<ul></ul>`) +
      `</div>`;
    if (v5Site()) {
      v5Card(d, l);
      if (hasMore) foldCard(d, l);
      box.append(d);
      if (PAY === l.level) box.append($("join"));
      continue;
    }
    if (hasMore) foldCard(d, l);
    const b = document.createElement("button");
    b.textContent = sel ? `Viewing ${l.name}` : `Choose ${l.name}`;
    b.className = sel ? "ghost" : "";
    b.disabled = sel;
    b.onclick = () => selectLevel(l.level);
    d.append(b);
    box.append(d);
  }
}

// What a plan includes stays folded until asked for, so a phone shows every plan at a glance:
// "What's included" (or a tap anywhere on the card that isn't a button) opens it.
const OPEN = new Set();
function foldCard(d, l) {
  const det = d.querySelector(".det");
  if (!det) return;
  const t = document.createElement("button");
  t.type = "button";
  t.className = "more-t";
  const set = (open) => {
    det.hidden = !open;
    d.classList.toggle("open", open);
    t.setAttribute("aria-expanded", String(open));
    t.innerHTML = `${open ? "Hide details" : "What's included"} <span aria-hidden="true">${open ? "▴" : "▾"}</span>`;
    if (open) OPEN.add(l.level); else OPEN.delete(l.level);
  };
  t.onclick = (e) => { e.stopPropagation(); set(det.hidden); };
  det.before(t);
  set(!det.hidden);
  d.addEventListener("click", (e) => {
    if (e.target.closest("button, a, input, select, label, .promo")) return;
    set(det.hidden);
  });
}

// On a plan you hold: the way to its renewals, auto-renew, transfer and close.
function manageBtn(d, id) {
  const b = document.createElement("button");
  b.className = "ghost manage";
  b.textContent = `Manage your ${pw()}`;
  b.onclick = () => goManage(id);
  d.append(b);
}
async function goManage(id, { menu = true } = {}) {
  if (LEVEL !== id) await selectLevel(id, { quiet: true });
  if (PAY) { PAY = null; show("join", false); renderLevels(); }
  if (menu && $("passMenu").children.length) {
    show("passMenu", true);
    const m = [...$("passActions").querySelectorAll("button")].find((b) => b.textContent.startsWith("Manage "));
    if (m) m.textContent = `Manage ${pw()} ▴`;
  }
  $("result").scrollIntoView({ behavior: "smooth", block: "start" });
}

// A link from a partner site can open one step for a plan, after sign-in:
// ?level=<id>&do=manage | renew | topup. Nothing happens without the member's own click and approval.
let DO = null;
try { DO = (new URLSearchParams(location.search).get("do") || "").toLowerCase() || null; } catch {}
if (DO && !["manage", "renew", "topup"].includes(DO)) DO = null;
async function runDo() {
  if (!DO || !session || !levels.length) return;
  let want = null;
  try { want = new URLSearchParams(location.search).get("level"); } catch {}
  const hit = want && levels.find((l) => l.level === want || l.id === want);
  const id = hit ? hit.level : LEVEL || levels[0].level;
  const holds = levels.length > 1 ? heldPasses.some((x) => x.level === id) : holdsPass;
  const what = DO;
  DO = null;
  // Not held here: the plan's own card is the step (buy or upgrade), so just show it.
  if (!holds) { if (LEVEL !== id) await selectLevel(id); return; }
  const q = QUOTES[id];
  if (what === "renew" && q && q.action === "renew") { await openPay(id); return; }
  await goManage(id, { menu: what !== "topup" });
  if (what === "topup" && $("topupBox").classList.contains("hidden")) {
    const t = [...$("passActions").querySelectorAll("button"), ...$("passMenu").querySelectorAll("button")].find((b) => /auto-renew|Top up/i.test(b.textContent) && !/Cancel/i.test(b.textContent));
    if (t) t.click();
    if (!$("topupBox").classList.contains("hidden")) $("topupBox").scrollIntoView({ behavior: "smooth", block: "center" });
  }
}

// v5 card: the price this wallet pays (credit and founding spelled out) and one button that
// names the action and the amount. Signed out, the plan's own price.
function v5Card(d, l) {
  const q = QUOTES[l.level];
  const o = l.offer || {};
  const lp = d.querySelector(".lp");
  d.querySelector(".ln .yours")?.remove();
  const btn = (label, cls) => { const b = document.createElement("button"); b.textContent = label; if (cls) b.className = cls; b.onclick = () => openPay(l.level); d.append(b); return b; };
  const also = kindsOf(l).filter((k) => k.kind !== "monthly").map((k) => `${k.label.toLowerCase()} ${money(l.price.currency, k.amount)}${k.kind === "term" ? ` for ${k.periods} ${unitOf(l.period_ms)}s` : ""}`);
  if (also.length && (!q || !["own", "renew"].includes(q.action))) lp.insertAdjacentHTML("afterend", `<div class="also">Also: ${escH(also.join(" · "))}</div>`);
  const promo = PROMO && PROMO.level === l.level && (!q || !["own", "renew"].includes(q.action)) ? promoBlock(d, l) : null;
  const tr = TRIALS[l.level];
  let trialOn = false;
  if (tr && (!q || !["own", "renew"].includes(q.action)) && !(promo && promo.claim)) {
    const fp = tr.first_period;
    const price = fp ? (fp.currency === "KAS" ? kas(fp.sompi) : money(fp.currency, fp.amount)) : "";
    if (tr.eligible === false) {
      if (tr.reason !== "member") d.insertAdjacentHTML("beforeend", `<div class="trialno small">${escH(tr.reason_text || "")}</div>`);
    } else {
      trialOn = true;
      const note = document.createElement("div");
      note.className = "trialnote small";
      const back = fp ? kasAbout(fp.sompi / KAS) : "";
      const ends = fmtDate(chainNow() + tr.trial_ms);
      const cushion = fp && fp.currency !== "KAS" ? ` (${price}, plus 10% in case the KAS price moves)` : price ? ` (${price})` : "";
      const bondKas = kas((l.offer && l.offer.bond_sompi) || (card && card.settings.bond) || KAS);
      note.textContent = `Try it free for ${trialWords(tr)}. To start, you set aside your first ${unitOf(l.period_ms)}${back ? `: about ${back}${cushion}` : ""}, and a ${bondKas} deposit. It stays yours. Cancel before ${ends} and all of it comes back. If you stay, ${price || `your first ${unitOf(l.period_ms)}`} is taken on ${ends}. One free trial per person${tr.min_age_days ? `, and your wallet must be at least ${tr.min_age_days} days old` : ""}.`;
      d.append(note);
      const tb = document.createElement("button");
      tb.className = "trialgo";
      tb.textContent = `Start my free ${trialLen(tr)} trial`;
      tb.onclick = () => startTrial(l, tb);
      d.append(tb);
      d.insertAdjacentHTML("beforeend", '<div id="trialHint" class="small muted" role="status"></div>');
    }
  }
  if (!q) {
    // Signed out (or no quote yet): founding or regular price, "Buy for ...".
    const sp = o.sale && o.sale.pct ? o.sale.pct : 0;
    const amt = (l.founding ? l.founding.amount : l.price.amount) * (100 - sp) / 100;
    if (sp) lp.innerHTML = `<div class="was"><s>${escH(money(l.price.currency, l.founding ? l.founding.amount : l.price.amount))}</s><span class="off">${sp}% off</span></div>${escH(money(l.price.currency, amt))} <span class="per">a ${escH(unitOf(l.period_ms))}</span>${l.price.currency === "KAS" ? "" : `<div class="kasline">about ${kasRound(Math.round(o.monthly_sompi * (100 - sp) / 100))} today</div>`}`;
    // The button says what the wallet actually pays: KAS. The currency price above is for reference.
    const kasNow = l.price.currency === "KAS" ? money("KAS", amt) : `about ${kasRound(Math.round(o.monthly_sompi * amt / Math.max(0.0001, l.price.amount)))}`;
    const b = btn(`Buy for ${kasNow}`, (promo && promo.claim) || trialOn ? "ghost" : "");
    b.disabled = !!o.price_unavailable || o.available === false;
    return;
  }
  const cur = q.currency;
  if (q.action === "own") {
    // Already held (or included in a higher plan): no price to pay, so none in big type.
    d.classList.add("owned");
    if (q.included_in) {
      lp.innerHTML = `<div class="have">Included in your ${escH(levelName(q.included_in))} ${escH(lw())}</div><div class="per">Nothing to buy here</div>`;
      return;
    }
    d.querySelector(".ln").insertAdjacentHTML("beforeend", `<span class="yours">Your ${escH(lw())}</span>`);
    const hp = heldPasses.filter((x) => x.level === l.level && x.active).sort((a, b) => b.expiry_ms - a.expiry_ms)[0];
    const onAuto = !!(hp && hp.balance_sompi > 0 && !isTrialPass(hp));
    lp.innerHTML = `<div class="have">You have this ${escH(lw())}</div>${q.until_ms ? `<div class="per">${onAuto ? "Renews by itself" : `Ends ${escH(fmtDate(q.until_ms))}. Nothing is charged automatically.`}</div>` : ""}`;
    const next = q.regular ? `Renewing costs ${escH(money(cur, q.regular))} a ${escH(unitOf(l.period_ms))}` : "";
    const opens = q.renew_opens_ms ? ` (you can renew from ${escH(fmtDate(q.renew_opens_ms))})` : "";
    if (next) d.insertAdjacentHTML("beforeend", `<div class="noact">${next}${opens}. Nothing to pay now.</div>`);
    manageBtn(d, l.level);
    return;
  }
  if (q.price_unavailable) {
    lp.insertAdjacentHTML("afterend", '<div class="until">Price unavailable right now; try again shortly</div>');
    btn(q.action === "renew" ? "Renew" : "Buy").disabled = true;
    return;
  }
  // Held and inside its renewal window: say so first; renewing is optional, not a purchase.
  if (q.action === "renew") {
    d.classList.add("owned");
    d.querySelector(".ln").insertAdjacentHTML("beforeend", `<span class="yours">Your ${escH(lw())}</span>`);
    const p = heldPasses.filter((x) => x.level === l.level).sort((a, b) => b.expiry_ms - a.expiry_ms)[0];
    const end = p && p.expiry_ms;
    const onAuto = !!(p && p.balance_sompi > 0 && !isTrialPass(p));
    lp.innerHTML = !p || p.active ? `<div class="have">You have this ${escH(lw())}</div>${end ? `<div class="per">${onAuto ? `Renews by itself before ${escH(fmtDate(end))}` : `Ends ${escH(fmtDate(end))}. Nothing is charged automatically.`}</div>` : ""}` : `<div class="have">Your ${escH(lw())} has ended</div>${end ? `<div class="per">Ended ${escH(fmtDate(end))}</div>` : ""}`;
    lp.insertAdjacentHTML("afterend", `<div class="noact">${!p || p.active ? "Renewing now adds" : "Renewing brings it back for"} a ${escH(unitOf(l.period_ms))} for ${escH(money(cur, q.amount))} (about ${kasAbout(q.kas_today)} today).</div>`);
    // Optional while it's still running: the quieter button.
    btn(`Renew for ${cur === "KAS" ? money(cur, q.amount) : "about " + kasAbout(q.kas_today)}`, !p || p.active || PAY === l.level ? "ghost" : "");
    manageBtn(d, l.level);
    return;
  }
  // Crossed out: what this wallet would pay without the credit (the founding or regular price).
  const base = q.credit > 0 ? q.amount + q.credit : q.regular;
  const was = base - q.amount > 0.004 || q.founding ? `<div class="was">${base - q.amount > 0.004 ? `<s>${escH(money(cur, base))}</s>` : ""}${q.founding ? '<span class="off">Founding</span>' : ""}</div>` : "";
  const plain = !(q.credit > 0) && !q.founding && q.action !== "renew";
  lp.innerHTML = plain
    ? `${was}${escH(money(cur, q.amount))} <span class="per">a ${escH(unitOf(l.period_ms))}</span>${cur === "KAS" ? "" : `<div class="kasline">about ${kasAbout(q.kas_today)} today</div>`}`
    : `${was}${escH(money(cur, q.amount))}${cur === "KAS" ? "" : `<div class="kasline">about ${kasAbout(q.kas_today)} today</div>`}<div class="per">${q.action === "renew" ? "for the next " + escH(unitOf(l.period_ms)) : q.credit > 0 ? "now, then " + escH(money(cur, q.regular)) + " a " + escH(unitOf(l.period_ms)) : "your first " + escH(unitOf(l.period_ms))}</div>`;
  if (q.credit > 0) lp.insertAdjacentHTML("afterend", `<div class="credit">${escH(money(cur, q.amount))} today because your unused ${escH(levelName(q.from_level))} time counts toward it (${escH(money(cur, q.credit))}).</div>`);
  if (q.action === "renew") d.querySelector(".ln").insertAdjacentHTML("beforeend", `<span class="yours">Your ${escH(lw())}</span>`);
  const verb = q.action === "upgrade" ? "Upgrade" : q.action === "renew" ? "Renew" : "Buy";
  btn(`${verb} for ${cur === "KAS" ? money(cur, q.amount) : "about " + kasAbout(q.kas_today)}`, PAY === l.level || (promo && promo.claim) || trialOn ? "ghost" : "");
}

// The promotion on its plan's card: the line, then (signed in) a claim button or why not.
function promoBlock(d, l) {
  const box = document.createElement("div");
  box.className = "promo";
  box.innerHTML = `<div class="pl"><span aria-hidden="true">🎁</span> ${escH(PROMO.label)}</div><div class="pr">${escH(cap(PROMO.requirement))}. Ends ${escH(fmtDate(PROMO.end_ms))}${PROMO.claims_left != null ? ` · ${PROMO.claims_left} left` : ""}.</div><div id="promoStatus" class="small" role="status"></div><div id="promoErr" class="err small" role="alert"></div>`;
  (d.querySelector(".also") || d.querySelector(".lp")).after(box);
  const me = PROMO_ME;
  const note = (t) => box.insertAdjacentHTML("beforeend", `<div class="pn">${escH(t)}</div>`);
  if (!session) { note("Sign in to check whether your wallet qualifies."); return { claim: false }; }
  if (!me) { note("Checking your wallet…"); return { claim: false }; }
  if (me.error) { note(me.error); return { claim: false }; }
  if (me.eligible) {
    const did = (me.rows || []).filter((r) => r.ok && (r.kind === "migrated" || r.kind === "burned")).map((r) => `${r.kind} ${fmtNum(r.have)} ${r.tick}`);
    if (did.length) note(`You ${did.join(" and ")}: your free ${unitOf(l.period_ms)} is ready.`);
    const b = document.createElement("button");
    b.className = "claim";
    b.textContent = `Claim my free ${unitOf(l.period_ms)}`;
    b.onclick = () => claimPromo(l, b);
    box.append(b);
    return { claim: true };
  }
  if (me.reason === "already_claimed") note("You've claimed this one. Enjoy!");
  else if (me.reason === "already_member") note("You already have this plan, so there's nothing to claim.");
  else if (me.closed) note("This promotion isn't taking claims right now.");
  else {
    const have = (me.rows || []).map((r) => r.kind === "migrated" || r.kind === "burned" ? `${fmtNum(r.have)} ${r.tick} ${r.kind}` : `${fmtNum(r.have)} ${r.tick} ${r.kind === "nft" ? "NFT" : "token"}${r.have === 1 ? "" : "s"}${r.kind === "kcc20" ? " (KCC-20)" : ""} held`).join(", ");
    note(`Not yet. So far, for your wallet${me.wallets_checked > 1 ? "s" : ""}: ${have}${PROMO.once_per_nft ? " (NFTs already used in this promotion don't count, nor ones listed for sale)" : ""}. Sign in with the wallet that holds them${clubInfo && clubInfo.enabled ? ", or link it in the holder club section below" : ""}.`);
  }
  return { claim: false };
}
const cap = (t) => (t ? t[0].toUpperCase() + t.slice(1) : t);
const fmtNum = (x) => Number(x).toLocaleString(undefined, { maximumFractionDigits: 4 });

async function claimPromo(l, b) {
  if (claiming) return;
  claiming = true;
  b.disabled = true;
  say("promoErr", "");
  busy("promoStatus", `Sending your free ${pw()}…`, "No wallet approval needed. Keep this page open until it's done.");
  try {
    const r = await fetch("/v1/promo/claim", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    const { txid } = await r.json();
    busy("promoStatus", "Sent. Waiting for the network to confirm…", `Transaction ${txid.slice(0, 12)}…`);
    for (let i = 0; i < 40; i++) {
      await sleep(2500);
      const c = await fetch(withLevel("/v1/check?address=" + encodeURIComponent(address), l.level));
      if (!c.ok) continue;
      const j = await c.json();
      if (j.passes.some((p) => p.txid === txid)) {
        claiming = false;
        await refreshHeld();
        await loadQuotes();
        if (l.level === LEVEL) await renderResult(j, txid); else await showLevelResult();
        await loadOffer();
        renderLevels();
        showDone(j, txid, l.level, "promo");
        return;
      }
    }
    say("promoStatus", "Your free membership was sent and is still being confirmed. Refresh this page in a minute.");
  } catch (e) {
    say("promoStatus", "");
    oops("promoErr", e, "That didn't work. Please try again.");
    b.disabled = false;
  } finally {
    claiming = false;
  }
}

// The ways to pay for a v5 level: monthly, and lifetime or a commitment where the plan offers them.
function kindsOf(l) {
  const o = l.offer || {}, p = l.price || {};
  const out = [{ kind: "monthly", label: perWord(), amount: p.amount }];
  if (o.v4 && o.v4.term_sompi > 0 && p.term) out.push({ kind: "term", label: "Commitment", amount: p.term.amount, periods: Math.round((o.v4.term_ms || 0) / Math.max(1, l.period_ms)) });
  if (o.lifetime_sompi > 0 && p.lifetime) out.push({ kind: "lifetime", label: "Lifetime", amount: p.lifetime.amount });
  return out;
}
const payKind = (id) => PAYKIND[id] || "monthly";
async function chooseKind(id, kind) {
  PAYKIND[id] = kind;
  const key = id + ":" + kind;
  if (kind !== "monthly" && session && address && !KQUOTES[key] && !KFETCH.has(key)) {
    KFETCH.add(key);
    renderPay();
    try {
      const r = await fetch("/v1/quote?level=" + encodeURIComponent(levelOf(id).id) + "&address=" + encodeURIComponent(address) + "&kind=" + kind);
      KQUOTES[id + ":" + kind] = r.ok ? await r.json() : { price_unavailable: true };
    } catch { KQUOTES[id + ":" + kind] = { price_unavailable: true }; }
    KFETCH.delete(key);
  }
  renderPay();
}

// The pay box, under the card that was pressed: what you get, what you pay now and next year.
let PAY = null;
async function openPay(id) {
  if (LEVEL !== id) await selectLevel(id, { quiet: true });
  PAY = id;
  hideDone();
  renderLevels();
  renderPay();
  $("join").scrollIntoView({ behavior: "smooth", block: "start" });
}
function renderPay() {
  const v5 = v5Site();
  for (const id of ["payInfo", "payGoBox"]) show(id, v5 && !!PAY);
  if (!v5) return;
  const l = levelOf(PAY);
  if (!PAY || !l) { show("join", false); return; }
  show("join", true);
  show("buyButtons", false); show("buyNote", false); show("noFee", false); show("testNote", false); show("renewHint", false);
  show("buyBox", true);
  const base = QUOTES[PAY];
  // Lifetime and commitment are choices when buying (not when renewing a plan you hold).
  const kinds = kindsOf(l);
  const choosing = kinds.length > 1 && (!base || base.action === "buy" || base.action === "upgrade");
  const kind = choosing ? payKind(PAY) : "monthly";
  const kb = $("payKinds");
  kb.replaceChildren();
  if (choosing) {
    for (const k of kinds) {
      const b = document.createElement("button");
      b.type = "button";
      b.setAttribute("aria-pressed", String(k.kind === kind));
      b.dataset.kind = k.kind;
      b.innerHTML = `<span class="k">${escH(k.label)}</span><span class="a">${escH(money(l.price.currency, k.amount))}${k.kind === "term" ? ` for ${k.periods} ${escH(unitOf(l.period_ms))}s` : k.kind === "lifetime" ? " once" : ` a ${escH(unitOf(l.period_ms))}`}</span>`;
      b.onclick = () => chooseKind(PAY, k.kind);
      kb.append(b);
    }
  }
  show("payKinds", choosing);
  const q = kind === "monthly" ? base : base && KQUOTES[PAY + ":" + kind];
  if (base && !q) {
    if (!KFETCH.has(PAY + ":" + kind)) chooseKind(PAY, kind);
    $("joinTitle").textContent = `Buy ${l.name}`;
    show("joinLead", false); show("payInfo", false); show("autoBox", false); show("qrGo", false);
    $("payGo").textContent = "Getting today's price…";
    $("payGo").disabled = true;
    return;
  }
  const bond = (card && card.settings.bond) || KAS;
  const until = (ms) => (ms ? fmtDate(ms) : "");
  if (!session || !q) {
    $("joinTitle").textContent = `Buy ${l.name}`;
    $("joinLead").textContent = "Sign in with Kaspire first: your price depends on the plans your wallet already holds.";
    show("joinLead", true); show("payInfo", false); show("autoBox", false);
    $("payGo").textContent = "Sign in with Kaspire to continue";
    $("payGo").disabled = false;
    $("payGo").onclick = () => (noApp() ? $("wallet").scrollIntoView({ behavior: "smooth", block: "start" }) : (isPhone && !window.kaspire ? $("wcGo") : $("signin")).click());
    show("qrGo", false);
    return;
  }
  const cur = q.currency;
  const verb = q.action === "upgrade" ? "Upgrade to" : q.action === "renew" ? "Renew" : "Buy";
  $("joinTitle").textContent = `${verb} ${l.name}`;
  show("joinLead", false);
  // Auto-renew: buying only; a renewal adds to the membership's own balance instead.
  const autoOk = q.action !== "renew" && kind !== "lifetime" && !!(offer && offer.v4 && offer.v4.auto_renew);
  show("autoBox", autoOk);
  const u = unitOf(l.period_ms);
  const ahead = () => (autoOk && $("autoOn").checked ? Number($("autoN").value) * Number(q.kas_next || 0) * 1.1 : 0);
  const total = () => Number(q.kas_today || 0) + (q.action === "renew" ? 0 : bond / KAS) + ahead();
  const heldP = heldPasses.filter((x) => x.level === PAY && x.active).sort((a, b) => b.expiry_ms - a.expiry_ms)[0];
  const passAuto = q.action === "renew" && heldP && heldP.balance_sompi > 0;
  // The rows, in the order money moves: what you get, the price, the deposit, the total, then what
  // happens at the end date (nothing, unless auto-renew is on).
  const paint = () => {
    const rows = [];
    rows.push(["You get", `<b>${escH(l.name)}</b>${kind === "lifetime" ? " <b>for life</b>" : q.until_ms ? ` until <b>${escH(until(q.until_ms))}</b>` : ""}${kind === "term" && q.term_periods ? ` (${q.term_periods} ${escH(u)}s paid up front, not refundable)` : ""}`]);
    if (q.price_unavailable) {
      rows.push(["Price today", "Price unavailable right now; try again in a minute."]);
    } else {
      const label = q.action === "renew" ? "renewal price" : q.founding ? "founding price" : kind === "lifetime" ? "lifetime price" : kind === "term" ? "commitment price" : "";
      const bits = [cur === "KAS" ? "" : escH(money(cur, q.amount)), label, q.holder_pct ? `${q.holder_pct}% holder discount` : ""].filter(Boolean).join(", ");
      rows.push(["Price today", `<b>${kasOf(q.kas_today)}</b>${bits ? ` (${bits})` : ""}` +
        (q.credit > 0 ? `<div class="sub">${escH(money(cur, q.founding ? q.amount + q.credit : q.regular))} minus ${escH(money(cur, q.credit))} for your unused ${escH(levelName(q.from_level))} time${q.from_until_ms ? ` (until ${escH(until(q.from_until_ms))})` : ""}</div>` : "")]);
      if (q.action !== "renew") rows.push(["Refundable deposit", `${kas(bond)}. You get it back when you end your ${escH(pw())}.`]);
      if (ahead()) rows.push(["Paid ahead", `about ${kasAbout(ahead())} for auto-renew. It stays yours until it's used.`]);
      rows.push(["Total now", `<b>${kasOf(total())}</b>, plus a tiny network fee`]);
      if (kind !== "lifetime" && q.until_ms) {
        const after = `After ${until(q.until_ms)}`;
        const price = `${escH(money(cur, q.regular))} a ${escH(u)}`;
        rows.push([after, ahead() || passAuto
          ? `Renews by itself each ${escH(u)} at ${price}, from the balance you add.`
          : `<b>Nothing is taken automatically.</b> To stay, renew for ${price}${q.kas_next && cur !== "KAS" ? ` <span class="sub">(about ${kasAbout(q.kas_next)} at today's price)</span>` : ""}.`]);
      }
    }
    if (q.action === "upgrade" && q.from_level) rows.push(["Afterwards", `You can end your ${escH(levelName(q.from_level))} ${escH(pw())} to get its ${kas(bond)} deposit back.`]);
    $("payInfo").innerHTML = rows.map(([k, v]) => `<dt>${escH(k)}</dt><dd>${v}</dd>`).join("");
    if (autoOk) $("autoAmt").textContent = `= about ${kasAbout(Number($("autoN").value) * Number(q.kas_next || 0) * 1.1)} more (today's price plus 10% in case KAS falls), held in your ${pw()}. Whatever isn't used comes back if you cancel.`;
    $("payGo").textContent = `Pay ${kasOf(total())} in Kaspire`;
  };
  show("payInfo", true);
  $("autoOn").onchange = paint;
  // Choosing how many renewals to pay ahead turns auto-renew on.
  $("autoN").onchange = () => { $("autoOn").checked = true; paint(); };
  paint();
  $("payGo").disabled = !!q.price_unavailable;
  $("payGo").dataset.off = q.price_unavailable ? "1" : "0";
  // Paying from another wallet: a one-time address and amount, shown as a QR code.
  show("qrGo", kind === "monthly" && !q.price_unavailable && typeof qrcode === "function" && !!(offer && offer.payin));
  $("qrGo").onclick = () => startQr(q, autoOk && $("autoOn").checked ? Number($("autoN").value) : 0);
  if (QR && QR.level === PAY) { show("payGo", false); show("payGoNote", false); show("qrGo", false); }
  $("payGo").onclick = async () => {
    if (LEVEL !== PAY) await selectLevel(PAY, { quiet: true });
    if (q.action === "renew") {
      const p = heldPasses.find((x) => x.level === PAY) ;
      if (!p) { say("buyErr", `Your ${pw()} isn't showing yet. Refresh the page, then try again.`); return; }
      runPaid({ url: "/v1/pass/prepare", body: { address, txid: p.txid, index: p.index, action: "renew" }, statusId: "buyStatus", errId: "buyErr", buttons: [$("payGo")], doneScreen: true });
    } else {
      buy(kind, [$("payGo")]);
    }
  };
}

// ---------------------------------------------------------------------------
// Paying from any wallet with a QR code. The server makes a one-time pay-in address for the
// signed-in wallet; a phone wallet sends the amount there and the server finishes the purchase.
let QR = null;
const clock = (ms) => new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
function qrSvg(text) {
  const q = qrcode(0, "M");
  q.addData(text);
  q.make();
  return q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
}
async function copyText(text, btn) {
  try { await navigator.clipboard.writeText(text); const t = btn.textContent; btn.textContent = "Copied"; setTimeout(() => (btn.textContent = t), 1500); } catch { /* the text is on screen to copy by hand */ }
}
function endQr(keepBox) {
  QR = null;
  if (!keepBox) show("qrBox", false);
  show("payGo", true); show("payGoNote", true);
  renderPay();
}
async function startQr(q, periods) {
  if (LEVEL !== PAY) await selectLevel(PAY, { quiet: true });
  say("buyErr", "");
  $("qrGo").disabled = true;
  try {
    const action = q.action === "renew" ? "renew" : "buy";
    const r = await fetch(withLevel("/v1/payin/order", PAY), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ address, action, periods }) });
    if (!r.ok) throw new Error(await r.text());
    const o = await r.json();
    QR = { id: o.id, level: PAY, action, amount: o.amount_kas };
    // The pay box above shows the price this order locked (an upgrade credit shrinks over time).
    loadQuotes().then(renderPay).catch(() => {});
    $("qrImg").innerHTML = qrSvg(o.uri);
    $("qrAmt").textContent = `${o.amount_kas} KAS`;
    $("qrAddr").textContent = o.address;
    $("qrOpen").href = o.uri;
    $("qrCopyAmt").onclick = () => copyText(o.amount_kas, $("qrCopyAmt"));
    $("qrCopyAddr").onclick = () => copyText(o.address, $("qrCopyAddr"));
    $("qrCancel").onclick = () => endQr();
    $("qrWho").innerHTML = `<b>The ${escH(pw())} goes to your signed-in wallet</b> (ending in ${escH(address.slice(-4))}), not to the wallet you pay from. Paying from a phone wallet, a hardware wallet or an exchange is fine. Any change or refund also goes to your signed-in wallet.`;
    $("qrNote").textContent = `Send the exact amount by ${clock(o.expires_ms)}. If anything goes wrong, your KAS comes back to your signed-in wallet automatically by ${clock(o.refund_after_ms)}.`;
    show("payGo", false); show("payGoNote", false); show("qrGo", false);
    show("qrBox", true);
    $("qrBox").scrollIntoView({ behavior: "smooth", block: "center" });
    pollQr(QR);
  } catch (e) {
    oops("buyErr", e);
  } finally {
    $("qrGo").disabled = false;
  }
}
async function pollQr(me) {
  const lv = me.level;
  while (QR === me) {
    let o = null;
    try { const r = await fetch("/v1/payin/order?id=" + encodeURIComponent(me.id)); if (r.ok) o = await r.json(); } catch {}
    if (QR !== me) return;
    if (o) {
      if (o.status === "completed") {
        busy("qrStatus", "Payment received. Finishing up…", `Your ${pw()} is being confirmed on the network.`);
        txBusy = true;
        try {
          for (let i = 0; i < 40; i++) {
            const r = await fetch(withLevel("/v1/check?address=" + encodeURIComponent(address), lv));
            if (r.ok) {
              const j = await r.json();
              if (j.passes.some((p) => p.txid === o.txid)) {
                say("qrStatus", "");
                QR = null;
                show("qrBox", false); show("payGo", true); show("payGoNote", true);
                await refreshHeld();
                await loadQuotes();
                PAY = null;
                if (lv === LEVEL) await renderResult(j, o.txid);
                else await showLevelResult();
                await loadOffer();
                renderLevels();
                renderPay();
                showDone(j, o.txid, lv, me.action);
                return;
              }
            }
            await sleep(2500);
          }
          say("qrStatus", "Your payment was received and is still being confirmed. Please don't pay again. Refresh this page in a minute.");
        } finally {
          txBusy = false;
        }
        return;
      }
      if (o.status === "refunded") { say("qrStatus", `Sent back to your signed-in wallet (${o.paid_kas} KAS minus a tiny network fee).`); return; }
      if (o.status === "expired") { say("qrStatus", "This QR payment ran out with nothing received. Start a new one if you still want to pay."); return; }
      if (o.short) say("qrStatus", `Received ${o.paid_kas} KAS, which is less than the ${o.amount_kas} KAS needed. It goes back to your signed-in wallet automatically after ${clock(o.refund_after_ms)}. Cancel and start a new QR payment for the full amount.`);
      else if (Number(o.paid_kas) > 0) busy("qrStatus", "Payment received. Finishing up…", "This takes a few seconds.");
      else if (Date.now() > o.expires_ms) say("qrStatus", `The quoted amount has run out. A payment now still goes through if today's price allows; otherwise it comes back to your signed-in wallet after ${clock(o.refund_after_ms)}.`);
      else busy("qrStatus", "Waiting for your payment…", "Scan the code with your phone wallet and send the exact amount. Keep this page open.");
    }
    await sleep(3000);
  }
}

// Look at one level: its membership box, prices, and (signed in) this wallet's pass on it.
async function selectLevel(id, { quiet } = {}) {
  LEVEL = id;
  await loadTier();
  await loadHolder();
  await loadOffer();
  renderLevels();
  await showLevelResult();
  if (!quiet) {
    const vis = (id) => !$(id).classList.contains("hidden");
    (session ? (vis("result") ? $("result") : vis("join") ? $("join") : $("levelsBox")) : $("wallet")).scrollIntoView({ behavior: "smooth", block: "start" });
  }
}

async function showLevelResult() {
  if (!session || !address) return;
  try {
    const r = await fetch(L("/v1/check?address=" + encodeURIComponent(address)));
    if (r.ok) await renderResult(await r.json());
  } catch {}
}

// Holding two levels after moving up or down: offer to close the one that's no longer needed.
function renderSwitch() {
  const box = $("switchBox");
  box.replaceChildren();
  const act = held.filter((h) => h.active).sort((a, b) => b.rank - a.rank);
  if (levels.length < 2 || act.length < 2) { show("switchBox", false); return; }
  const top = act[0];
  for (const h of act.slice(1)) {
    const p = heldPasses.find((x) => x.level === h.level && x.active);
    if (!p) continue;
    const back = kas(p.value_sompi || (card ? card.settings.bond : 0));
    const div = document.createElement("div");
    div.className = "switchbox";
    div.innerHTML = v5Site()
      ? `<p>You also still have <b>${escH(h.name)}</b>. ${escH(top.name)} includes everything in it, so end it to get its ${p.balance_sompi > 0 ? `deposit and prepaid balance (${escH(back)})` : `${escH(back)} deposit`} back.</p>`
      : `<p>You're also a <b>${escH(h.name)}</b> member (${escH(fmtExpiry(p).toLowerCase())}). ${escH(top.name)} includes everything in ${escH(h.name)}, so you can end ${escH(h.name)} now and get its ${p.balance_sompi > 0 ? "deposit and prepaid balance" : "deposit"} back.</p>`;
    const b = document.createElement("button");
    b.className = "ghost warn";
    b.textContent = `End ${h.name} · get ${back} back`;
    b.onclick = () => {
      if (b.dataset.armed !== "1") {
        const was = b.textContent;
        b.dataset.armed = "1"; b.textContent = "Press again to end it for good";
        setTimeout(() => { b.dataset.armed = "0"; b.textContent = was; }, 6000);
        return;
      }
      runPaid({ url: "/v1/pass/prepare", level: h.level, body: { address, txid: p.txid, index: p.index, action: "close" }, statusId: "passStatus", errId: "passErr", buttons: [b],
        done: (j) => !j.passes.some((x) => x.txid === p.txid && x.index === p.index) });
    };
    div.append(b);
    box.append(div);
  }
  show("switchBox", box.children.length > 0);
  renderDoneBox();
}

// ---- The creator's profile and each level's description -----------------------

// Formatted text from the creator, cleaned by the server; checked again here so only plain
// formatting (and safe links) ever reaches the page.
const RICH_TAGS = new Set(["P", "BR", "B", "STRONG", "I", "EM", "U", "S", "H3", "H4", "UL", "OL", "LI", "BLOCKQUOTE", "A"]);
function richHtml(html) {
  const doc = new DOMParser().parseFromString(`<div>${html || ""}</div>`, "text/html");
  const walk = (el) => {
    for (const n of [...el.childNodes]) {
      if (n.nodeType === 3) continue;
      if (n.nodeType !== 1) { n.remove(); continue; }
      walk(n);
      if (!RICH_TAGS.has(n.tagName)) {
        if (["SCRIPT", "STYLE", "IFRAME", "OBJECT", "EMBED", "TEMPLATE", "SVG", "MATH"].includes(n.tagName)) n.remove(); else n.replaceWith(...n.childNodes);
        continue;
      }
      const href = n.tagName === "A" ? n.getAttribute("href") || "" : "";
      for (const at of [...n.attributes]) n.removeAttribute(at.name);
      if (n.tagName === "A" && /^(https?:|mailto:)/i.test(href)) { n.setAttribute("href", href); n.setAttribute("target", "_blank"); n.setAttribute("rel", "noopener noreferrer nofollow"); }
    }
  };
  const root = doc.body.firstChild;
  walk(root);
  return root.innerHTML;
}

let page = null;
const LINK_NAMES = { website: "Website", x: "X", tiktok: "TikTok", youtube: "YouTube", instagram: "Instagram", telegram: "Telegram", discord: "Discord" };
async function loadPage() {
  try { const r = await fetch("/v1/page"); page = r.ok ? await r.json() : null; } catch { page = null; }
  if (!page) return;
  $("pageTag").textContent = page.tagline || "";
  show("pageTag", !!page.tagline);
  const arrow = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 3h7v7M13 3L4 12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const links = Object.keys(LINK_NAMES).filter((k) => page.links && /^https?:\/\//i.test(page.links[k] || ""))
    .map((k) => `<a href="${escH(page.links[k])}" target="_blank" rel="noopener noreferrer nofollow">${LINK_NAMES[k]}${arrow}</a>`);
  if (links.length) links.unshift('<span class="follow">Follow</span>');
  if (page.show_count && page.members != null) links.push(`<span class="count"><b>${page.members}</b> member${page.members === 1 ? "" : "s"}</span>`);
  $("pageMeta").innerHTML = links.join("");
  show("pageMeta", links.length > 0);
  const intro = richHtml(page.intro_html);
  $("introText").innerHTML = intro;
  show("introBox", !!intro);
  if (intro) {
    // Long intros start folded, with "See more".
    $("introText").classList.add("clamp");
    const long = $("introText").scrollHeight > $("introText").clientHeight + 8;
    if (!long) $("introText").classList.remove("clamp");
    show("introMore", long);
    $("introMore").onclick = () => {
      const open = $("introText").classList.toggle("clamp") === false;
      $("introMore").textContent = open ? "See less" : "See more";
      $("introMore").setAttribute("aria-expanded", String(open));
    };
  }
}

// The selected level's description, above its prices.
function renderTierAbout() {
  const cov = LEVEL || (card && card.covenant_id);
  // With several plans shown as cards, each card has its own description, so it isn't repeated
  // here; in the comparison table it is (for the plan picked).
  const cardsView = levels.length > 1 && $("levelCompare").classList.contains("hidden");
  const html = cardsView ? "" : richHtml(page && page.about ? page.about[cov] : "");
  $("tierAbout").innerHTML = html;
  show("tierAbout", !!html);
  foldTierBox();
}

// With several plans, the plan facts (length, deposit, fees) fold under one line.
function foldTierBox() {
  const box = $("tierBox");
  if (levels.length < 2 || box.dataset.folded) return;
  box.dataset.folded = "1";
  const det = document.createElement("details");
  det.className = "tierfold";
  const sum = document.createElement("summary");
  sum.id = "tierSum";
  det.append(sum, $("tier"), $("techBox"));
  $("tierAbout").after(det);
  $("tierTitle").classList.add("hidden");
  syncTierSum();
}
function syncTierSum() {
  const s = document.getElementById("tierSum");
  if (s) s.textContent = `Plan details: ${$("tierTitle").textContent}`;
}

loadLevels().then(loadPage).then(loadTier).then(loadOffer).then(renderWelcome).then(loadTrials).then(renderLevels);

// Light/dark switch (shown when the creator set both): the visitor's choice is remembered.
(function themeSwitch() {
  const b = document.getElementById("themeToggle");
  if (!b || b.classList.contains("hidden")) return;
  const start = b.dataset.start || "light";
  const current = () => document.documentElement.dataset.theme || (start === "device" ? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light") : start);
  const paint = () => { const d = current() === "dark"; b.classList.toggle("isdark", d); b.setAttribute("aria-label", d ? "Switch to light" : "Switch to dark"); };
  b.onclick = () => {
    const next = current() === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem("kp-theme", next); } catch {}
    paint();
  };
  paint();
})();

