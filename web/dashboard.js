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
const $ = (id) => document.getElementById(id);
const show = (id, on = true) => $(id).classList.toggle("hidden", !on);
fetch("/v1/club").then((r) => r.json()).then((c) => { if (c.enabled) { show("navClub"); } }).catch(() => {});
const say = (id, msg) => { $(id).textContent = msg || ""; show(id, !!msg); };
// A transaction or restart in progress: an animated hourglass, and a warning if you try to leave.
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
const KAS = 100000000;
const kas = (s) => (Number(s) / KAS).toLocaleString(undefined, { maximumFractionDigits: 2 }) + " KAS";
// Errors in plain words: no "Failed:" prefix, a capital letter, and technical text behind "Details".
function plainSay(id, e, fallback = "That didn't work. Nothing was sent. Please try again.") {
  let t = String((e && e.message !== undefined ? e.message : e) || "").replace(/^(Failed|Error|Sign-in failed|Kaspire mobile):\s*/i, "").trim();
  const el = $(id);
  if (!t || /[0-9a-f]{16,}|script|covenant|rpc|panic|\{|outpoint|utxo/i.test(t) || t.length > 260) {
    el.textContent = fallback;
    if (t) { const d = document.createElement("details"); d.innerHTML = "<summary>Details</summary>"; d.append(t); d.style.fontSize = "12px"; el.append(d); }
  } else {
    if (/recovery wallet isn't a valid Kaspa address/i.test(t)) t = "That backup wallet address isn't a valid Kaspa address. Copy it again from Kaspire.";
    el.textContent = (/^[a-z]+[\s',]/.test(t) ? t[0].toUpperCase() + t.slice(1) : t) + (/[.!?]$/.test(t) ? "" : ".");
  }
  show(id, true);
}
const mins = (ms) => { const m = Math.round(ms / 60000); return m % 1440 === 0 ? `${m / 1440} days` : `${m} min`; };
// Addresses only ever hold letters, digits and ":", so anything else is dropped: the result is safe
// in HTML as well as in text.
const short = (a) => { const b = String((a || "").split(":")[1] || a || "").replace(/[^a-z0-9]/gi, ""); return b.length > 12 ? `${b.slice(0, 6)}…${b.slice(-4)}` : b; };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// A length typed as a number and a unit (minutes, hours, days, weeks, months), in whole minutes.
const minutesOf = (id) => Math.round(Number($(id).value || 0) * Number(($(id + "U") || {}).value || 1));
let address = null, session = null, card = null, data = null, clockOffset = 0;
const periodsOf = (ms) => Math.round(ms / card.settings.period_ms);
const chainNow = () => Date.now() + clockOffset;

function setError(m) { say("walletErr", m); }

// Kaspire mobile (WalletConnect), when the creator signed in with it. It answers the same requests
// as the extension, so sign-in and every approval below work unchanged.
let MOBILE = null;
const usedMobile = () => { try { return sessionStorage.getItem("kaspass.mobile") === "1"; } catch { return false; } };
const netOf = () => (card && card.network) || "mainnet";
async function detectKaspire(timeoutMs = 3000) {
  if (MOBILE) return MOBILE;
  if (usedMobile() && window.KaspireMobile) {
    // Signed in with the app earlier in this tab (the page was reloaded): its connection is still
    // here, so approvals keep going to the app.
    MOBILE = await window.KaspireMobile.known(netOf());
    if (MOBILE) return MOBILE;
    throw Object.assign(new Error("The Kaspire app isn't connected any more. Press Reconnect the Kaspire app."), { code: "WALLET_CHANGED" });
  }
  if (window.kaspire?.isKaspire) return window.kaspire;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(window.KaspireMobile && window.KaspireMobile.isPhone ? "On a phone, sign in with the Kaspire app. Tap the button below." : "We couldn't find the Kaspire wallet in this browser. Install it (free) from kaspire.kaslab.space, then reload this page. Or use the Kaspire app on your phone.")), timeoutMs);
    window.addEventListener("kaspire#initialized", () => { clearTimeout(t); resolve(window.kaspire); }, { once: true });
  });
}

function setAddress(a) {
  address = a;
  $("addr").textContent = a ? (session ? `Signed in as the wallet ending in ${a.slice(-4)}` : `Kaspire wallet found (ends in ${a.slice(-4)})`) : "";
  $("addr").title = a || "";
  show("addr", !!a);
  show("signin", !session && !(window.KaspireMobile && window.KaspireMobile.isPhone));
  const phone = !!(window.KaspireMobile && window.KaspireMobile.isPhone);
  const ios = phone && /iPhone|iPad|iPod/i.test(navigator.userAgent);
  show("wcGo", !session && !!window.KaspireMobile && !ios);
  if (ios && !session) $("signHint").textContent = "The Kaspire app isn't on iPhone yet. Sign in on a computer with the free Kaspire browser extension (kaspire.kaslab.space).";
  if (!$("wcGo").textContent.startsWith("Reconnect")) $("wcGo").textContent = phone ? "Sign in with the Kaspire app" : "Use the Kaspire app on my phone instead";
  $("wcGo").classList.toggle("ghost", !phone);
  show("wcOtherLine", !!session && !!MOBILE);
  show("logout", !!session);
  // Signed in: the sign-in panel shrinks to one line.
  $("signPanel").classList.toggle("in", !!session);
  show("signTitle", !session);
  show("signHint", !session);
}

// ---- Tabs ------------------------------------------------------------------
// One job per screen. "#all" shows everything on one page.
const TABS = ["overview", "page", "levels", "members", "prices", "promos", "groups", "payouts", "settings"];
function showTab(name) {
  if (name !== "all" && !TABS.includes(name)) name = "overview";
  document.querySelectorAll(".pane").forEach((p) => p.classList.toggle("hidden", name !== "all" && p.dataset.pane !== name));
  document.querySelectorAll("#tabs button").forEach((b) => b.classList.toggle("on", b.dataset.tab === name));
  // The tier picker only matters on tabs about one tier.
  $("levelPick").classList.toggle("offtab", !["levels", "members", "prices", "all"].includes(name));
  if (name === "all") showLaunch(true);
  try { history.replaceState(null, "", "#" + name); } catch {}
}
document.querySelectorAll("#tabs button").forEach((b) => (b.onclick = () => showTab(b.dataset.tab)));
document.addEventListener("click", (e) => {
  const a = e.target.closest("[data-go]");
  if (a) { e.preventDefault(); showTab(a.dataset.go); window.scrollTo({ top: 0 }); }
});
const startTab = (location.hash || "").slice(1);
let tabShown = false;

// What the creator calls levels ("level", "tier", "plan").
const levelWord = () => (data && data.level_word) || { one: "plan", many: "plans" };
const cap = (t) => t.charAt(0).toUpperCase() + t.slice(1);
function applyLevelWord() {
  const w = levelWord();
  const tab = document.querySelector('#tabs [data-tab="levels"]'); if (tab) tab.textContent = cap(w.many);
  $("lvAdd").textContent = `Add a ${w.one}`;
  $("levelPick").querySelector("span").textContent = cap(w.one);
  document.querySelectorAll(".lw1").forEach((e) => (e.textContent = w.one));
  document.querySelectorAll(".lwN").forEach((e) => (e.textContent = w.many));
  const cl = $("lCopy").closest("label");
  if (cl && cl.firstChild && cl.firstChild.nodeType === 3) cl.firstChild.textContent = `Start from one of your ${w.many} (optional)`;
}

// The launch form: for a creator it's "Add a level" in the Levels tab; for anyone else it's on its own.
let LAUNCH_OPEN = true;
function showLaunch(open) {
  const creator = !!(card && address === card.creator_address && session);
  // Once opened it stays open (switching levels refreshes the page's data, not the form or its result).
  const wasOpen = creator && $("launch").parentElement === $("launchSlot") && !$("launch").classList.contains("hidden");
  (creator ? $("launchSlot") : $("launchHome")).append($("launch"));
  const w = levelWord();
  $("launchTitle").textContent = creator ? `Add a ${w.one}` : "Launch a new membership";
  $("lGo").textContent = creator ? `Create ${w.one}` : "Launch membership";
  $("lGoNote").textContent = creator ? "Kaspire will ask you to approve." : "";
  show("lNameBox", creator); show("lPerksBox", creator); show("lAboutBox", creator);
  // Only the site's owner launches levels once it has an owner record (launch_open is false then).
  show("launch", creator ? open || wasOpen : LAUNCH_OPEN);
}
$("lvAdd").onclick = () => { showLaunch(true); fillCopyList(); $("launch").scrollIntoView({ behavior: "smooth", block: "start" }); };

// "Start from one of your levels": copies a level's details into the form, so a similar level
// (another currency, a free trial, a new price) only needs the parts that differ.
async function fillCopyList() {
  try {
    const j = await (await fetch("/v1/levels")).json();
    const lv = (j.levels || []).filter((l) => !j.empty);
    COPY_FROM = lv;
    $("lCopy").innerHTML = '<option value="">Start from scratch</option>' + lv.map((l) => `<option value="${escHtml(l.level)}">${escHtml(l.name)}</option>`).join("");
    show("lCopyBox", lv.length > 0);
  } catch {}
}
let COPY_FROM = [];
// "No free trial" in the unit list: the number box and the self-serve trial switch step aside.
function syncTrial() {
  const none = $("lTrialU").value === "0";
  $("lTrial").disabled = none;
  if (none) $("lSelfTrial").checked = false;
  show("lSelfTrialBox", !none);
  // Each pass uses one from a limit and trials can't give theirs back, so the two don't mix.
  const self = $("lSelfTrial").checked;
  $("lSupply").disabled = self;
  if (self) $("lSupply").value = "";
  $("lSupply").placeholder = self ? "No limit (needed for free trials)" : "No limit";
}
$("lSelfTrial").addEventListener("change", syncTrial);
$("lTrialU").addEventListener("change", () => { if ($("lTrialU").value !== "0" && !(Number($("lTrial").value) > 0)) $("lTrial").value = 7; syncTrial(); });
function setDur(id, ms) {
  const min = Math.round((ms || 0) / 60000);
  if (id === "lTrial" && min === 0) { $("lTrialU").value = "0"; syncTrial(); return; }
  const units = [43200, 10080, 1440, 60, 1];
  const u = min === 0 ? 1440 : units.find((x) => min % x === 0) || 1;
  $(id).value = min / u;
  if ($(id + "U")) $(id + "U").value = String(u);
}
$("lCopy").onchange = async () => {
  const l = COPY_FROM.find((x) => x.level === $("lCopy").value);
  if (!l) return;
  try {
    const t = await (await fetch("/v1/tier?level=" + encodeURIComponent(l.level))).json();
    const st = t.settings || {};
    $("lName").value = `${l.name} (2)`.slice(0, 40);
    syncTrial();
    $("lPerks").value = (l.perks || []).join("\n");
    lAbout.set(l.about_html || "");
    setDur("lPeriod", st.period_ms); setDur("lTrial", st.trial_ms); setDur("lAhead", st.max_ahead_ms); syncTrial();
    $("lBond").value = (st.bond || 0) / 1e8;
    $("lResell").checked = !!st.resellable;
    $("lSelfTrial").checked = !!(st.v4 && st.v4.self_trial);
    $("lRoyalty").value = (st.royalty || 0) / 1e8;
    $("lSupply").value = "";
    if (l.price) {
      $("lCur").value = l.price.currency; $("lCur").dispatchEvent(new Event("change"));
      $("lAmount").value = l.price.amount;
      $("lV5Life").value = l.price.lifetime ? l.price.lifetime.amount : "";
      $("lV5Term").value = l.price.term ? l.price.term.amount : "";
      const o = l.offer || {};
      if (l.price.term && o.v4 && o.v4.term_ms) $("lV5TermN").value = Math.round(o.v4.term_ms / Math.max(1, st.period_ms || 1));
    }
    say("lStatus", `Filled in from ${l.name}. Check the name and anything you want different, then create it.`);
  } catch (e) { say("lErr", "Couldn't copy that plan. Please try again."); }
};

// What's left to set up, each with a button to the tab that does it.
async function renderSetup() {
  const items = [];
  if (!data.creator_name) items.push(["Choose the name members see for you", "page"]);
  if (!(data.level && data.level.perks && data.level.perks.length)) items.push([`Say what ${data.level && data.level.name ? data.level.name : "your plan"} includes`, "levels"]);
  try {
    const t = await (await fetch("/v1/creator/telegram?session=" + encodeURIComponent(session))).json();
    if (t.enabled && !(t.groups || []).some((g) => g.assigned)) items.push(["Connect your Telegram group", "groups"]);
  } catch {}
  if (pageCfg && !pageCfg.intro_html) items.push(["Write an intro for your page", "page"]);
  if (data.v4 && data.v4.keeper && data.v4.keeper.balance_sompi < KAS) items.push([`Turn on auto-renew: send 2 ${card && card.network === "mainnet" ? "KAS" : "test KAS"} to your fees wallet. It pays the tiny network fee each time a member's plan renews itself (2 KAS covers about 60 renewals).`, "promos"]);
  if (!(data.totals && data.totals.active > 0)) items.push(["Share your page link to get your first member", "page"]);
  $("setupList").innerHTML = items.map(([t, tab]) => `<div class="setup"><span>${escHtml(t)}</span><button class="ghost" data-go="${tab}">Open</button></div>`).join("");
  show("setupPanel", items.length > 0);
}

$("signin").onclick = async () => {
  setError("");
  $("signin").disabled = true;
  try {
    const kaspire = await detectKaspire();
    let a = null;
    try { a = (await kaspire.request({ method: "getAccounts" }))?.[0] || null; } catch {}
    if (!a) a = (await kaspire.request({ method: "requestAccounts" }))?.[0] || null;
    if (!a) throw new Error("Kaspire didn't share a wallet. Open Kaspire, approve the connection, then try again.");
    const want = card && card.network === "testnet-10" ? "kaspatest:" : "kaspa:";
    if (!a.startsWith(want)) throw new Error(`This site is on ${card.network}. Switch Kaspire to that network and try again.`);
    setAddress(a);
    const { nonce } = await (await fetch("/v1/nonce")).json();
    const message = ["Sign in to KasPass", "Domain: " + location.host, "Address: " + a, "Nonce: " + nonce, "Issued At: " + new Date().toISOString()].join("\n");
    const signed = await kaspire.request({ method: "signMessage", params: { address: a, message } });
    const res = await fetch("/v1/login", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ address: a, message, signature: signed.signature }) });
    if (!res.ok) throw new Error(await res.text());
    session = (await res.json()).session;
    // Remembered for this tab only, so a reload (after the site restarts) keeps you signed in.
    try { sessionStorage.setItem("kaspass.dash", JSON.stringify({ session, address: a })); } catch {}
    setAddress(a);
    await refresh();
  } catch (e) {
    if (e.code === 4001) setError("Cancelled in Kaspire. Press the button to try again.");
    else if (e.code === "WALLET_CHANGED") setError(e.message);
    else plainSay("walletErr", e, "That didn't work. Please try again.");
    if (e.code === "WALLET_CHANGED") wcReconnectMode();
    throw e;
  } finally {
    $("signin").disabled = false;
  }
};
// Without a rejected promise reaching the button (the error is shown above).
const signinBtn = $("signin").onclick;
$("signin").onclick = () => signinBtn().catch(() => {});
const endSession = (t) => { if (t) fetch("/v1/logout", { method: "POST", headers: { "x-kaspass-session": t } }).catch(() => {}); };
$("logout").onclick = () => {
  endSession(session);
  session = null; MOBILE = null;
  try { sessionStorage.removeItem("kaspass.dash"); sessionStorage.removeItem("kaspass.mobile"); } catch {}
  setAddress(address); ["creatorArea", "notCreator", "launch"].forEach((id) => show(id, false));
};

// Kaspire mobile: pair (QR on a computer, a button on a phone), then the same sign-in as above.
function wcReconnectMode() {
  MOBILE = null;
  show("wcGo", true);
  $("wcGo").textContent = "Reconnect the Kaspire app";
}
async function wcSignIn(fresh) {
  setError("");
  $("wcGo").disabled = true;
  try {
    MOBILE = await window.KaspireMobile.connect(netOf(), { fresh });
    try { sessionStorage.setItem("kaspass.mobile", "1"); } catch {}
    await signinBtn();
    $("wcGo").textContent = "Use the Kaspire app on my phone instead";
  } catch (e) {
    if (!session) { MOBILE = null; try { sessionStorage.removeItem("kaspass.mobile"); } catch {} }
    if (e.code === "WALLET_CHANGED") wcReconnectMode();
    if (e.code === 4001) setError("Cancelled in the Kaspire app. Press the button to try again.");
    else if (e.code === "WALLET_CHANGED") setError(e.message);
    // (A sign-in step that failed has already explained itself.)
    else if ($("walletErr").classList.contains("hidden")) plainSay("walletErr", e, "That didn't work. Please try again.");
  } finally {
    $("wcGo").disabled = false;
    setAddress(address);
  }
}
$("wcGo").onclick = () => wcSignIn($("wcGo").textContent.startsWith("Reconnect"));
$("wcOther").onclick = async (e) => {
  e.preventDefault();
  $("logout").onclick();
  await window.KaspireMobile.forget();
  wcSignIn(true);
};
// No extensions on phones: the Kaspire app is the only button there.
if (window.KaspireMobile && window.KaspireMobile.isPhone) { $("wcGo").className = ""; $("wcGo").textContent = "Sign in with the Kaspire app"; show("signin", false); }

// Several levels: which one the dashboard is managing (covenant id; null = the only or lowest one).
let LEVEL = null;
const L = (u) => (LEVEL ? u + (u.includes("?") ? "&" : "?") + "level=" + encodeURIComponent(LEVEL) : u);

function renderLevelPick() {
  const lv = (data && data.levels) || [];
  show("levelPick", lv.length > 1);
  show("lvEditHint", lv.length > 1);
  const box = $("levelBtns");
  box.replaceChildren();
  for (const l of lv) {
    const b = document.createElement("button");
    const sel = l.covenant_id === (LEVEL || lv[0].covenant_id);
    b.textContent = l.name;
    b.className = sel ? "on" : "";
    b.onclick = () => pickLevel(l.covenant_id);
    box.append(b);
  }
  // The same choice as a dropdown at the top of the level editor.
  const sel = $("lvSelect");
  sel.innerHTML = lv.map((l) => `<option value="${escHtml(l.covenant_id)}">${escHtml(l.name)}</option>`).join("");
  sel.value = LEVEL || (lv[0] && lv[0].covenant_id) || "";
  sel.onchange = () => pickLevel(sel.value);
}

// Manage one level: every panel follows.
async function pickLevel(id, { scroll } = {}) {
  const lv = (data && data.levels) || [];
  if (id === (LEVEL || (lv[0] && lv[0].covenant_id))) { if (scroll) $("tierTitle").scrollIntoView({ behavior: "smooth", block: "start" }); return; }
  LEVEL = id;
  card = await (await fetch(L("/v1/tier"))).json();
  namesLoaded = false;
  await refresh();
  if (scroll) $("tierTitle").scrollIntoView({ behavior: "smooth", block: "start" });
}

// "No KasPass fee" sites (serve --no-fee) say so on the launch form.
// v5 sites (with an owner record): plans priced in a currency, paid into a vault.
let SITE_V5 = false;
function v5Launch() {
  const until = $("lFoundUntil").value;
  // The end of that day in the creator's own time zone.
  const end = until ? new Date(Number(until.slice(0, 4)), Number(until.slice(5, 7)) - 1, Number(until.slice(8, 10)) + 1).getTime() : null;
  const found = Number($("lFound").value) || 0;
  const ceil = Number($("lCeil").value) || 3;
  const life = Number($("lV5Life").value) || 0, term = Number($("lV5Term").value) || 0;
  return {
    v4: true, currency: $("lCur").value, amount: Number($("lAmount").value), ceiling_times: ceil,
    ...(found > 0 && end ? { founding_amount: found, founding_until_ms: end } : {}),
    ...(life > 0 ? { lifetime_amount: life } : {}),
    ...(term > 0 ? { term_amount: term, term_periods: Number($("lV5TermN").value) || 0 } : { term_kas: 0 }),
    lifetime_kas: 0, max_discount_pct: 0,
  };
}
// A plan priced in KAS has no ceiling above its price.
$("lCur").addEventListener("change", () => show("lCeilBox", $("lCur").value !== "KAS"));
function applyV5(on) {
  SITE_V5 = on;
  // Every membership is v5: it needs the owner record (Payouts tab) before the first launch.
  show("lV5Box", on); show("tabPayouts", true); show("lNeedRecord", !on); show("lBody", on); $("lGo").disabled = !on;
  renderSteps3();
}
$("lNeedGo").onclick = () => {
  if (card && card.placeholder) { const s = $("recSetup"); if (s) s.scrollIntoView({ behavior: "smooth", block: "start" }); setTimeout(() => $("recRecovery").focus(), 400); }
  else { showTab("payouts"); window.scrollTo({ top: 0 }); }
};
// A brand-new site: three numbered steps at the top until the first plan is on sale.
let HAS_RECORD = false;
function renderSteps3() {
  const isNew = !!(card && card.placeholder && session && address === card.creator_address);
  show("steps3", isNew);
  if (!isNew) return;
  const done = [HAS_RECORD || SITE_V5, false, false];
  const now = done.indexOf(false);
  document.querySelectorAll("#s3List li").forEach((li, i) => { li.classList.toggle("done", done[i]); li.classList.toggle("now", i === now); });
  $("payTitle").textContent = HAS_RECORD || SITE_V5 ? "Step 1 done: payouts are on" : "Step 1. Turn on payouts";
  $("launchTitle").textContent = "Step 2. Create your first plan";
}
fetch("/v1/levels").then((r) => r.json()).then((j) => {
  applyV5(!!j.v5);
  if (j.launch_no_fee) $("lFeeNote").textContent = "No KasPass fee: every payment goes to you in full.";
  if (j.launch_open === false) { $("signHint").textContent = "Sign in with the wallet that created this membership to manage it."; LAUNCH_OPEN = false; if (card && !card.placeholder) showLaunch(false); }
}).catch(() => {});

// ---- v5 price (in a currency) ----------------------------------------------
async function renderV5Price() {
  const v5 = !!(card.settings.v4 && card.settings.v4.record_id);
  show("v5PriceBox", v5);
  if (!v5) return;
  try {
    const j = await (await fetch("/v1/levels")).json();
    const l = (j.levels || []).find((x) => x.level === card.covenant_id);
    if (!l || !l.price || document.activeElement === $("vpAmount")) return;
    $("vpCur").value = l.price.currency; $("vpAmount").value = l.price.amount;
    // A plan priced in KAS stays in KAS (its on-chain price is the real price); one priced in
    // CAD, USD or EUR can move between those three.
    const inKas = l.price.currency === "KAS";
    $("vpCur").disabled = inKas;
    for (const o of $("vpCur").options) o.disabled = !inKas && o.value === "KAS";
    show("vpLifeBox", !!l.price.lifetime); show("vpTermBox", !!l.price.term);
    if (l.price.lifetime) $("vpLife").value = l.price.lifetime.amount;
    if (l.price.term) $("vpTerm").value = l.price.term.amount;
    $("vpFound").value = l.founding ? l.founding.amount : "";
    if (l.founding) { const d = new Date(l.founding.until_ms - 1); $("vpFoundUntil").value = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`; }
    VP_RATE = l.offer && l.offer.monthly_sompi && l.price.amount ? { cur: l.price.currency, kasPer: l.offer.monthly_sompi / KAS / l.price.amount } : null;
    vpPreview();
  } catch {}
}
$("vpGo").onclick = async () => {
  say("vpErr", ""); say("vpMsg", "");
  const until = $("vpFoundUntil").value;
  const end = until ? new Date(Number(until.slice(0, 4)), Number(until.slice(5, 7)) - 1, Number(until.slice(8, 10)) + 1).getTime() : null;
  const found = Number($("vpFound").value) || 0;
  const r = await fetch(L("/v1/creator/level-price"), { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ session, currency: $("vpCur").value, amount: Number($("vpAmount").value), ...(found > 0 && end ? { founding_amount: found, founding_until_ms: end } : {}),
      ...(!$("vpLifeBox").classList.contains("hidden") && Number($("vpLife").value) > 0 ? { lifetime_amount: Number($("vpLife").value) } : {}),
      ...(!$("vpTermBox").classList.contains("hidden") && Number($("vpTerm").value) > 0 ? { term_amount: Number($("vpTerm").value) } : {}) }) });
  if (!r.ok) { plainSay("vpErr", await r.text()); return; }
  const j = await r.json();
  say("vpMsg", j.warning ? "Saved. " + j.warning : "Saved. Members see the new price now.");
};
// What members will see: "CA$160 a month, about 580 KAS today".
let VP_RATE = null;
function vpPreview() {
  const a = Number($("vpAmount").value), cur = $("vpCur").value;
  if (!(a > 0)) { $("vpPreview").textContent = ""; return; }
  const per = card ? dur(card.settings.period_ms).replace(/^1 /, "") : "period";
  const amt = cur === "KAS" ? `${a.toLocaleString()} KAS` : new Intl.NumberFormat(undefined, { style: "currency", currency: cur, maximumFractionDigits: 0 }).format(a);
  const kasToday = cur !== "KAS" && VP_RATE && VP_RATE.cur === cur ? ` , about ${Math.round(a * VP_RATE.kasPer).toLocaleString()} KAS today` : "";
  $("vpPreview").textContent = `Members will see: ${amt} a ${card && card.settings.period_ms < 86400000 ? "month" : per}${kasToday.replace(" ,", ",")}.`;
}
$("vpAmount").addEventListener("input", vpPreview);
$("vpCur").addEventListener("change", vpPreview);

// ---- Messages from the website's "Get your own page" form ----------------------
async function loadNotes() {
  try {
    const r = await fetch("/v1/creator/contact?session=" + encodeURIComponent(session));
    if (!r.ok) { show("notesPanel", false); return; }
    const j = await r.json();
    show("notesPanel", j.notes.length > 0);
    const box = $("notesList");
    box.replaceChildren();
    for (const n of j.notes) {
      const d = document.createElement("div");
      d.className = "note-item";
      const when = new Date(n.at_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
      d.innerHTML = `<div><span class="who"></span> <span class="muted small"></span></div><div class="small reach"></div><div class="msgtext"></div>`;
      d.querySelector(".who").textContent = n.name;
      d.querySelector(".muted").textContent = when;
      d.querySelector(".reach").textContent = "Reach them at: " + n.reach;
      d.querySelector(".msgtext").textContent = n.message;
      const del = document.createElement("button");
      del.className = "ghost"; del.style.padding = "4px 10px"; del.textContent = "Delete";
      del.onclick = async () => {
        if (del.dataset.armed !== "1") { del.dataset.armed = "1"; del.textContent = "Click again to delete"; setTimeout(() => { del.dataset.armed = "0"; del.textContent = "Delete"; }, 5000); return; }
        const r2 = await fetch("/v1/creator/contact/delete", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, id: n.id }) });
        if (!r2.ok) { say("notesErr", await r2.text()); return; }
        loadNotes();
      };
      d.append(del);
      box.append(d);
    }
  } catch { show("notesPanel", false); }
}

// ---- Payouts: owner record and vault (v5) ---------------------------------
async function loadRecord() {
  try {
    const r = await fetch("/v1/creator/record?session=" + encodeURIComponent(session));
    if (!r.ok) return null;
    const j = await r.json();
    show("recSetup", !j.record);
    show("recInfo", !!j.record);
    HAS_RECORD = !!j.record;
    VAULT = j.record ? Number(j.vault_balance_sompi || 0) : null;
    renderSteps3();
    if (data && card) render();
    if (j.record) {
      $("vaultBal").textContent = kas(j.vault_balance_sompi);
      $("vaultCount").textContent = j.vault_payments ? `${j.vault_payments} payment${j.vault_payments === 1 ? "" : "s"} ready to withdraw` : "Nothing to withdraw yet";
      $("recOwner").textContent = j.record.owner;
      $("recRec").textContent = j.record.recovery;
      $("vaultAddr").textContent = j.vault_address;
      $("vaultGo").disabled = !j.vault_balance_sompi;
    }
    return j;
  } catch { return null; }
}
let VAULT = null;
$("recGo").onclick = () => {
  if (!$("recRecovery").value.trim()) { say("recErr", "Paste your backup wallet's address first."); $("recRecovery").focus(); return; }
  if ($("recRecovery").value.trim() === address) { say("recErr", "That's the wallet you're signed in with. The backup must be a second wallet of yours."); return; }
  runPaid({
    url: "/v1/creator/record", body: { recovery: $("recRecovery").value.trim() }, statusId: "recStatus", errId: "recErr", button: $("recGo"),
    after: async () => {
      busy("recStatus", "Sent. Waiting for the network to confirm…");
      for (let i = 0; i < 30; i++) { await sleep(2000); const j = await loadRecord(); if (j && j.record) break; }
      say("recStatus", card && card.placeholder ? "Payouts are on. Next: create your first plan, just below." : "Payouts are on. New plans you create pay into your vault.");
      applyV5(true);
    },
  });
};
$("vaultGo").onclick = () => runPaid({
  url: "/v1/creator/withdraw", body: {}, statusId: "recStatus", errId: "recErr", button: $("vaultGo"),
  after: async (out) => {
    busy("recStatus", "Sent. Waiting for the network to confirm…");
    for (let i = 0; i < 30; i++) { await sleep(2000); const j = await loadRecord(); if (j && !j.vault_balance_sompi) break; }
    say("recStatus", "Done: withdrawn to your wallet.");
  },
});
// ---- Payments export (v5): one CSV for the year, for taxes ----------------
(() => {
  const sel = $("payYear");
  const y = new Date().getUTCFullYear();
  sel.append(new Option("All years", ""));
  for (let k = y; k >= 2026; k--) sel.append(new Option(String(k), String(k)));
  sel.value = String(y);
})();
$("payCsv").onclick = async () => {
  say("payErr", "");
  try {
    const yr = $("payYear").value;
    const r = await fetch("/v1/creator/payments.csv?session=" + encodeURIComponent(session) + (yr ? "&year=" + yr : ""));
    if (!r.ok) throw new Error(await r.text());
    const url = URL.createObjectURL(await r.blob());
    const a = document.createElement("a");
    a.href = url; a.download = yr ? `kaspass-payments-${yr}.csv` : "kaspass-payments.csv";
    document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 5000);
  } catch (e) { plainSay("payErr", e, "The download didn't work. Please try again."); }
};

// ---- Server key swap (v5) --------------------------------------------------
const plural = (n, w) => `${n} ${w}${n === 1 ? "" : (w.endsWith("s") ? "es" : "s")}`;
async function loadKey() {
  try {
    const r = await fetch("/v1/creator/server-key?session=" + encodeURIComponent(session));
    if (!r.ok) return null;
    const j = await r.json();
    show("keyBox", !!j.v5);
    const al = j.alerts || [];
    say("keyAlert", al.length ? `Security warning: someone else may be able to set prices on your plans (seen ${new Date(al[al.length - 1].at_ms).toLocaleString()}). Your money is safe: payments still only reach your vault. Open "Advanced: server key" below, press Replace server key and follow the steps, or message KasPass.` : "");
    if (al.length) $("keyAdv").open = true;
    const left = j.lanes_left + j.passes_left;
    keyLeft = left;
    $("keyState").textContent = left
      ? `Still on the old key: ${plural(j.lanes_left, "plan")} and ${plural(j.passes_left, "membership")}. Sales and renewals keep working meanwhile.`
      : `Server key ${j.key}…${j.replaced ? " (replaced)" : ""}. All your plans and memberships trust it.`;
    show("keyMove", left > 0);
    show("keyNew", left === 0);
    return j;
  } catch { return null; }
}
let keyArmed = false, keyLeft = 0;
$("keyNew").onclick = async () => {
  say("keyErr", "");
  if (!keyArmed) {
    keyArmed = true;
    $("keyNew").textContent = "Press again to replace the key";
    say("keyStatus", "Only do this if you think the server key was exposed. You'll then approve a few transactions to move your plans and passes.");
    setTimeout(() => { keyArmed = false; $("keyNew").textContent = "Replace server key"; }, 8000);
    return;
  }
  keyArmed = false;
  $("keyNew").textContent = "Replace server key";
  const r = await fetch("/v1/creator/server-key", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session }) });
  if (!r.ok) { plainSay("keyErr", await r.text()); return; }
  await loadKey();
  say("keyStatus", "New key made. New sales use it now. Next: move your plans and memberships over (Move to the new key).");
};
$("keyMove").onclick = () => { const was = keyLeft; runPaid({
  url: "/v1/creator/server-key/prepare", body: {}, statusId: "keyStatus", errId: "keyErr", button: $("keyMove"),
  after: async (out, summary) => {
    busy("keyStatus", "Sent. Waiting for the network to confirm…");
    // Each plan's reader catches up on its own, so wait for the whole batch.
    for (let i = 0; i < 30 && summary.moved; i++) { await sleep(2000); const j = await loadKey(); if (!j || j.lanes_left + j.passes_left <= Math.max(0, was - summary.moved)) break; }
    const j = await loadKey();
    const left = j ? j.lanes_left + j.passes_left : 0;
    if (!summary.moved) await sleep(4000);
    say("keyStatus", !summary.moved ? "Done. Now press Move to the new key." : left ? `Done. ${left} left: press Move to the new key again.` : "Done. Everything trusts the new key; the old one can't approve anything any more.");
  },
}); };
$("hoGo").onclick = () => {
  // Permanent, so two presses like everything else that can't be undone.
  if ($("hoGo").dataset.armed !== "1") {
    $("hoGo").dataset.armed = "1"; $("hoGo").textContent = "Press again: this can't be undone";
    setTimeout(() => { $("hoGo").dataset.armed = "0"; $("hoGo").textContent = "Hand over ownership"; }, 6000);
    return;
  }
  $("hoGo").dataset.armed = "0"; $("hoGo").textContent = "Hand over ownership";
  runPaid({
    url: "/v1/handover/prepare", body: { new_owner: $("hoOwner").value.trim(), new_recovery: $("hoRecovery").value.trim() }, statusId: "hoStatus", errId: "hoErr", button: $("hoGo"),
    after: async () => { say("hoStatus", "Done: ownership handed over. The new owner can sign in here now."); },
  });
};

async function refresh() {
  const isCreator = address === card.creator_address;
  // The recovery wallet only gets the handover form.
  if (!isCreator) {
    const j = await loadRecord();
    const rec = !!(j && j.role === "recovery");
    show("recoveryArea", rec);
    if (rec) { show("notCreator", false); show("creatorArea", false); show("launch", false); return; }
  } else {
    show("recoveryArea", false);
    loadRecord();
    loadKey();
    loadNotes();
    loadPromo();
    loadAlerts();
    loadVis();
  }
  // A new site with nothing on sale yet: its creator launches the first membership, nothing else.
  if (card.placeholder) {
    show("notCreator", !isCreator);
    $("creatorAddr").textContent = card.creator_address;
    show("creatorArea", false);
    $("launchHome").append($("launch"));
    show("launch", isCreator);
    $("launchTitle").textContent = "Step 2. Create your first plan";
    $("lGo").textContent = "Create my first plan";
    $("lGoNote").textContent = "Kaspire will ask you to approve.";
    show("lNameBox", false); show("lPerksBox", false); show("lAboutBox", false);
    // Payouts first: with an owner record, plans are priced in CAD, USD or EUR and pay into a vault.
    if (isCreator) {
      const pay = document.querySelector('[data-pane="payouts"] section');
      if (pay && pay.parentElement !== $("launchHome")) $("launchHome").prepend(pay);
    }
    renderSteps3();
    return;
  }
  show("notCreator", !isCreator);
  $("creatorAddr").textContent = card.creator_address;
  show("creatorArea", isCreator);
  showLaunch(startTab === "all");
  if (!isCreator) return;
  // The first time in, open the tab the link asked for (dashboard#prices), else Overview.
  if (!tabShown) { tabShown = true; showTab(startTab || "overview"); }
  const r = await fetch(L("/v1/creator?session=" + encodeURIComponent(session)));
  if (!r.ok) { setError(await r.text()); return; }
  data = await r.json();
  renderLevelPick();
  if (document.activeElement !== $("perks")) $("perks").value = ((data.level && data.level.perks) || []).join("\n");
  if (document.activeElement !== $("clubName")) $("clubName").value = data.club_name || "";
  if (document.activeElement !== $("lvWord")) $("lvWord").value = data.level_word && data.level_word.one !== "plan" ? data.level_word.one : "";
  applyLevelWord();
  await loadPageCfg();
  loadBrand();
  loadNames();
  clockOffset = data.now_ms - Date.now();
  render();
  renderV5Price();
  renderSetup();
  renderTelegram();
  renderDiscord();
  renderClub();
  renderCards();
  loadAllTotals();
  renderSteps3();
}

// ---- Overview across every plan --------------------------------------------
let ALL = null;
async function loadAllTotals() {
  const lv = (data && data.levels) || [];
  if (lv.length < 2) { ALL = null; render(); return; }
  try {
    const each = await Promise.all(lv.map((l) => fetch("/v1/creator?level=" + encodeURIComponent(l.covenant_id) + "&session=" + encodeURIComponent(session)).then((r) => (r.ok ? r.json() : null)).catch(() => null)));
    const t = { earned_sompi: 0, active: 0, renewals: 0, upgrades: 0, autorenewals: 0, sales: 0, trials: 0, auto_renewing: 0 }, ev = [];
    for (const d of each) {
      if (!d) continue;
      for (const k of ["earned_sompi", "active", "renewals", "upgrades", "autorenewals", "sales", "trials"]) t[k] += Number(d.totals[k] || 0);
      t.auto_renewing += d.v4 ? Number(d.v4.auto_renewing || 0) : 0;
      for (const e of d.events || []) ev.push({ ...e, plan: d.name || "" });
    }
    ALL = { totals: t, events: ev.sort((a, b) => (a.at_ms || 0) - (b.at_ms || 0)) };
  } catch { ALL = null; }
  render();
}

// ---- Tiers on this site ----------------------------------------------------
async function renderCards() {
  try {
    const r = await fetch("/v1/creator/cards?session=" + encodeURIComponent(session));
    if (!r.ok) throw new Error(await r.text());
    const { cards } = await r.json();
    try {
      const lv = (await (await fetch("/v1/levels")).json()).levels || [];
      levelPrices = Object.fromEntries(lv.filter((l) => l.offer).map((l) => [l.level, l.offer.monthly_sompi]));
      // A plan shares access with the one right above it when their access ranks match.
      if (!sameAccess) sameAccess = new Set(lv.filter((l, i) => lv[i + 1] && lv[i + 1].access_rank === l.access_rank).map((l) => l.level));
      savedSame = new Set(lv.filter((l, i) => lv[i + 1] && lv[i + 1].access_rank === l.access_rank).map((l) => l.level));
    } catch { levelPrices = {}; }
    // On sale first, lowest level first; then the rest, newest first.
    if (!cardOrder) cardOrder = cards.slice().sort((a, b) => (a.rank ?? 99) - (b.rank ?? 99)).map((c) => c.file);
    cardList = cards;
    drawCards();
  } catch (e) { $("cardsT").innerHTML = `<tr><td class="muted">${escHtml(e.message || e)}</td></tr>`; }
}

let cardOrder = null, cardList = [], onSale = null, levelPrices = {}, priceFilled = "", sameAccess = null, savedSame = new Set();
// The plans whose "same access as the level above" box is ticked, as covenant ids (only below the top).
const sameList = (ticked) => ticked.slice(0, -1).filter((c) => sameAccess && sameAccess.has(c.covenant_id)).map((c) => c.covenant_id);
function drawCards() {
  if (!onSale) onSale = new Set(cardList.filter((c) => c.on_sale).map((c) => c.file));
  const all = cardOrder.map((f) => cardList.find((c) => c.file === f)).filter(Boolean);
  // Only your own memberships can be levels; anything else on sale stays listed so it can be unticked.
  const sellable = (c) => c.yours || c.on_sale || onSale.has(c.file);
  const cards = all.filter(sellable), older = all.filter((c) => !sellable(c));
  const ticked = cards.filter((c) => onSale.has(c.file));
  const when = (ms) => (ms ? new Date(ms).toLocaleDateString(undefined, { dateStyle: "medium" }) : "");
  const w = levelWord();
  // One card per plan: name and price, an "On sale" switch, Edit, and arrows for the order.
  $("cardsT").innerHTML = cards.map((c) => {
    const ti = ticked.indexOf(c);
    const lvl = onSale.has(c.file) && ticked.length > 1 ? `<span class="pill ok">${escHtml(cap(w.one))} ${ti + 1}</span> ` : "";
    // Switched on but not saved yet: it isn't live, so it can't be edited until Save.
    const pending = onSale.has(c.file) && !c.on_sale ? '<div class="pw"><span class="pill warn">Not on sale yet: press "Save changes" below</span></div>' : !onSale.has(c.file) && c.on_sale ? '<div class="pw"><span class="pill warn">Comes off sale when you press Save</span></div>' : "";
    const pr = levelPrices[c.covenant_id];
    const priceLine = pr ? `about ${Math.round(pr / KAS).toLocaleString()} KAS today` : `Created ${escHtml(when(c.saved_ms))}`;
    return `<div class="pcard"><label class="sw"><input type="checkbox" aria-label="Sell ${escHtml(c.name || "this plan")}" data-sell="${escHtml(c.file)}" ${onSale.has(c.file) ? "checked" : ""} ${c.yours ? "" : "disabled"}> On sale</label>` +
      `<div>${lvl}<span class="pn">${escHtml(c.name || "Unnamed plan")}</span><div class="pp">${priceLine}</div></div>` +
      `<div class="pa">${c.on_sale && c.yours ? `<button class="ghost" data-edit="${escHtml(c.covenant_id)}">${c.name ? "Edit" : "Name it"}</button>` : ""}${onSale.has(c.file) && ticked.length > 1 ? `<button class="ghost" data-up="${escHtml(c.file)}" title="Move earlier (cheaper)" aria-label="Move earlier">↑</button><button class="ghost" data-down="${escHtml(c.file)}" title="Move later (pricier)" aria-label="Move later">↓</button>` : ""}</div>${pending}</div>`;
  }).join("");
  // "Same plan, another way to pay" lives in Edit, for the plan being edited.
  const me = ticked.findIndex((c) => card && c.covenant_id === card.covenant_id);
  const above = me >= 0 && me < ticked.length - 1 ? ticked[me + 1] : null;
  show("lvSameBox", !!above);
  if (above) {
    $("lvSameText").textContent = `This is the same ${w.one} as ${above.name || "the next one"}, just paid a different way (for example yearly instead of monthly).`;
    $("lvSame").dataset.same = ticked[me].covenant_id;
    $("lvSame").checked = !!(sameAccess && sameAccess.has(ticked[me].covenant_id));
    $("lvSame").onchange = () => { if (!sameAccess) sameAccess = new Set(); $("lvSame").checked ? sameAccess.add($("lvSame").dataset.same) : sameAccess.delete($("lvSame").dataset.same); drawCards(); $("lvChanged").scrollIntoView({ behavior: "smooth", block: "center" }); };
  }
  $("olderT").innerHTML = "<tr><th>Membership</th><th>Launched</th></tr>" + older.map((c) =>
    `<tr><td>${c.name ? `<b>${escHtml(c.name)}</b>` : "Unnamed"}${c.yours ? "" : `<br><span class="muted small">by ${short(c.creator)}</span>`}</td><td>${escHtml(when(c.saved_ms))}</td></tr>`).join("");
  // Only offer to save when something changed from what's on sale now.
  const savedNow = cardList.filter((c) => c.on_sale).sort((a, b) => a.rank - b.rank).map((c) => c.file);
  const sameNow = JSON.stringify(sameList(ticked).sort()) !== JSON.stringify([...savedSame].filter((id) => ticked.some((c) => c.covenant_id === id)).sort());
  show("lvChanged", JSON.stringify(ticked.map((c) => c.file)) !== JSON.stringify(savedNow) || sameNow);
  // A higher level should cost more than the one below it.
  // Today's price per period (only known for levels that are live; a new plan priced in a currency
  // only has its KAS ceiling until it's on sale, which would be misleading here).
  const price = (c) => levelPrices[c.covenant_id] ?? (c.version >= 5 ? null : c.launch_price_sompi);
  const warn = [];
  for (let i = 1; i < ticked.length; i++) {
    const lo = ticked[i - 1], hi = ticked[i], pl = price(lo), ph = price(hi);
    if (pl != null && ph != null && ph < pl) warn.push(`${escHtml(hi.name || "Plan " + (i + 1))} comes after ${escHtml(lo.name || "Plan " + i)} but costs less (${kas(ph)} vs ${kas(pl)} a period). Members can get the bigger plan for less. Change their prices in Prices &amp; sales, or swap their order with the arrows.`);
  }
  $("lvWarn").innerHTML = warn.join("<br>");
  show("lvWarn", warn.length > 0);
  $("olderSum").textContent = `Older plans (${older.length})`;
  show("olderBox", older.length > 0);
  document.querySelectorAll("[data-edit]").forEach((b) => (b.onclick = () => pickLevel(b.dataset.edit, { scroll: true })));
  document.querySelectorAll("[data-sell]").forEach((b) => (b.onchange = () => { b.checked ? onSale.add(b.dataset.sell) : onSale.delete(b.dataset.sell); drawCards(); }));

  const move = (f, d) => {
    const sold = cardOrder.filter((x) => onSale.has(x));
    const i = sold.indexOf(f), j = i + d;
    if (j < 0 || j >= sold.length) return;
    [sold[i], sold[j]] = [sold[j], sold[i]];
    cardOrder = [...sold, ...cardOrder.filter((x) => !onSale.has(x))];
    drawCards();
  };
  document.querySelectorAll("[data-up]").forEach((b) => (b.onclick = () => move(b.dataset.up, -1)));
  document.querySelectorAll("[data-down]").forEach((b) => (b.onclick = () => move(b.dataset.down, 1)));
}

$("lvUndo").onclick = () => { cardOrder = null; onSale = null; sameAccess = null; renderCards(); };
$("lvSave").onclick = () => saveLevels(cardOrder.filter((f) => onSale.has(f)), $("lvSave"), "swStatus", "swErr");

// Two clicks, then the site restarts with the chosen levels; we wait for it to come back.
async function saveLevels(files, button, statusId, errId) {
  if (!files.length) { say(errId, "Switch on at least one plan to sell."); return; }
  if (button.dataset.armed !== "1") {
    button.dataset.armed = "1";
    const was = button.textContent;
    button.textContent = "Press again to save";
    setTimeout(() => { button.dataset.armed = "0"; button.textContent = was; }, 6000);
    return;
  }
  say(errId, "");
  button.disabled = true;
  try {
    const ticked = files.map((f) => cardList.find((c) => c.file === f)).filter(Boolean);
    const same_access = sameList(ticked);
    const r = await fetch("/v1/creator/levels", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, files, same_access }) });
    if (!r.ok) throw new Error(await r.text());
    const out = await r.json();
    if (!out.restarting) { say(statusId, "Saved. It shows on your page once your site restarts; message KasPass if it doesn't within an hour."); button.disabled = false; return; }
    // Show the new choice straight away (ticks, order, the level picker); the reload confirms it.
    onSale = new Set(files);
    cardOrder = [...files, ...(cardOrder || []).filter((f) => !onSale.has(f))];
    cardList = cardList.map((c) => ({ ...c, on_sale: onSale.has(c.file), rank: onSale.has(c.file) ? files.indexOf(c.file) : null }));
    drawCards();
    if (data) {
      data.levels = files.map((f) => {
        const c = cardList.find((x) => x.file === f) || {};
        return (data.levels || []).find((l) => l.covenant_id === c.covenant_id) || { covenant_id: c.covenant_id, name: c.name || `New ${levelWord().one}` };
      });
      renderLevelPick();
    }
    busy(statusId, "Saved. Your site is restarting…", "This page reloads by itself when it's back, usually in about a minute.");
    const want = files.map((f) => (cardList.find((c) => c.file === f) || {}).covenant_id);
    await sleep(4000);
    for (let i = 0; i < 90; i++) {
      try {
        const lv = await (await fetch("/v1/levels", { cache: "no-store" })).json();
        const h = await (await fetch("/v1/health", { cache: "no-store" })).json();
        if (h.synced && JSON.stringify(lv.levels.map((l) => l.level)) === JSON.stringify(want)) { location.reload(); return; }
      } catch {}
      await sleep(2000);
    }
    say(statusId, "The site hasn't come back yet. Give it another minute and reload this page.");
  } catch (e) {
    say(errId, e.message || String(e));
    button.disabled = false;
  }
}

// Two clicks, then the site restarts on the chosen tier; we wait for it to come back.
async function switchTier(file, button, statusId, errId) {
  if (button.dataset.armed !== "1") {
    button.dataset.armed = "1";
    const was = button.textContent;
    button.textContent = "Press again to sell only this one";
    setTimeout(() => { button.dataset.armed = "0"; button.textContent = was; }, 6000);
    return;
  }
  say(errId, "");
  button.disabled = true;
  try {
    const r = await fetch("/v1/creator/switch", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, file }) });
    if (!r.ok) throw new Error(await r.text());
    const out = await r.json();
    if (!out.restarting) { say(statusId, "Saved. It shows on your page once your site restarts; message KasPass if it doesn't within an hour."); return; }
    busy(statusId, "Saved. Your site is restarting…", "This page reloads by itself when it's back, usually in about a minute.");
    await sleep(4000);
    for (let i = 0; i < 90; i++) {
      try {
        const t = await (await fetch("/v1/tier", { cache: "no-store" })).json();
        const h = await (await fetch("/v1/health", { cache: "no-store" })).json();
        if (t.covenant_id === out.covenant_id && h.synced) { location.reload(); return; }
      } catch {}
      await sleep(2000);
    }
    say(statusId, "The site hasn't come back yet. Give it another minute and reload this page.");
  } catch (e) {
    say(errId, e.message || String(e));
    button.disabled = false;
  }
}

async function renderClub() {
  const el = $("clubStatus");
  try {
    const c = await (await fetch("/v1/creator/club?session=" + encodeURIComponent(session))).json();
    if (!c.enabled) {
      el.textContent = "No holder club yet. A holder club gives free access to people who own your NFTs. Message KasPass to set one up.";
      $("clubBands").innerHTML = ""; $("clubTiersT").innerHTML = "";
      return;
    }
    el.innerHTML = `<b>${escHtml(c.club)}</b>: ${escHtml(c.collection.tick)} on ${escHtml(c.collection.network)}, ${c.ranked} ranked NFTs. Listed NFTs ${c.count_listed ? "count" : "don't count while listed"}.<br>Share this page with holders: <a href="/club" target="_blank" rel="noopener">${escHtml(location.origin)}/club</a>`;
    $("clubBands").innerHTML = c.bands.length ? "<tr><th>Band</th><th>Ranks</th><th>NFTs</th><th>Credits each</th></tr>" + c.bands.map((b) => `<tr><td>${escHtml(b.name)}</td><td>${b.ranks[0]}–${b.ranks[1]}</td><td>${b.nfts}</td><td>${b.credits}</td></tr>`).join("") : "";
    $("clubTiersT").innerHTML = "<tr><th>Level</th><th>Rule</th><th>Discord role</th></tr>" + c.tiers.map((t) => `<tr><td>${escHtml(t.name)}</td><td>${escHtml(t.rule)}</td><td>${escHtml(t.discord_role)}</td></tr>`).join("");
  } catch (e) { el.textContent = "Couldn't load the club."; }
}

const readJson = (input) => new Promise((resolve, reject) => {
  const f = input.files && input.files[0];
  if (!f) return resolve(null);
  const r = new FileReader();
  r.onload = () => { try { resolve(JSON.parse(r.result)); } catch (e) { reject(new Error(f.name + " isn't valid JSON: " + e.message)); } };
  r.onerror = () => reject(new Error("couldn't read " + f.name));
  r.readAsText(f);
});

$("clubUpload").onclick = async () => {
  const msg = $("clubUploadMsg");
  msg.textContent = "Checking…";
  try {
    const rules = await readJson($("clubRules"));
    if (!rules) throw new Error("Choose a rules file first.");
    const ranks = await readJson($("clubRanks"));
    const r = await fetch("/v1/creator/club", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, rules, ranks }) });
    if (!r.ok) throw new Error(await r.text());
    const c = await r.json();
    msg.textContent = `Applied: ${c.tiers.length} level${c.tiers.length === 1 ? "" : "s"}, ${c.ranked} ranked NFTs. Discord roles are created within a minute; assign Telegram groups with /kaspass <level> in each group.`;
    renderClub(); renderDiscord(); renderTelegram();
  } catch (e) { msg.textContent = "Not applied. " + String(e.message || e).replace(/^./, (c) => c.toUpperCase()); }
};

$("clubCheckBtn").onclick = async () => {
  const out = $("clubCheckOut");
  const a = $("clubCheckAddr").value.trim();
  out.textContent = "Checking…";
  try {
    const r = await fetch("/v1/club/check?address=" + encodeURIComponent(a));
    if (!r.ok) throw new Error(await r.text());
    const st = await r.json();
    const bands = st.bands.filter((b) => b[1] > 0).map((b) => `${b[1]} ${escHtml(b[0])}`).join(", ");
    out.innerHTML = `<b>${st.credits} credits</b> from ${st.tokens} NFT${st.tokens === 1 ? "" : "s"}${bands ? " (" + bands + ")" : ""}${st.listed ? `, ${st.listed} listed` : ""}. Qualifies for: <b>${st.tiers.length ? st.tiers.map(escHtml).join(", ") : "nothing yet"}</b>.`;
  } catch (e) { out.textContent = e.message || String(e); }
};

async function renderDiscord() {
  const el = $("dcStatus");
  try {
    const t = await (await fetch("/v1/creator/discord?session=" + encodeURIComponent(session))).json();
    if (!t.enabled) {
      el.textContent = "Discord isn't switched on for your site yet. Message KasPass and we'll turn it on.";
      $("dcLinks").innerHTML = "";
      pill("dcPill", false, "Not available yet");
      return;
    }
    pill("dcPill", !!t.ready, t.ready ? `Connected: ${t.server}` : "Not connected");
    el.innerHTML = t.ready
      ? `<ol class="small"><li>The bot <b>${escHtml(t.bot)}</b> manages these roles in <b>${escHtml(t.server)}</b>: ${t.roles.map((r) => "<b>" + escHtml(r) + "</b>").join(", ")}.</li><li>Make your members-only channels visible to those roles.</li><li>In Discord, open Server Settings, then Roles, and drag the <b>${escHtml(t.bot)}</b> role above the member roles.</li></ol>Members connect from your page and get their role; anyone whose membership ends loses it.`
      : `<ol class="small"><li><a href="${escHtml(t.invite_bot_url)}" target="_blank" rel="noopener">Add the bot to your Discord server</a>.</li><li>Come back here: this box shows "Connected" once it's in.</li></ol>`;
    $("dcLinks").innerHTML = t.links.length
      ? "<tr><th>Discord</th><th>Wallet</th><th>Qualifies for</th><th>Has roles</th></tr>" +
        t.links.map((l) => `<tr><td>${escHtml(l.name)}</td><td>${short(l.address)}</td><td>${l.qualifies ? (l.qualifies.map(escHtml).join(", ") || "None") : "checking…"}</td><td>${l.roles.map(escHtml).join(", ") || "None"}</td></tr>`).join("")
      : t.ready ? "<tr><td class=small>No members have connected Discord yet.</td></tr>" : "";
    $("dcLinks").classList.add("stack");
  } catch (e) {
    el.textContent = "Couldn't load the Discord status.";
  }
}

function pill(id, ok, text) { const p = $(id); if (!p) return; p.className = "pill " + (ok ? "ok" : "no"); p.textContent = text; }
const escHtml = (t) => String(t ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

async function renderTelegram() {
  const el = $("tgStatus");
  try {
    const r = await fetch("/v1/creator/telegram?session=" + encodeURIComponent(session));
    const t = await r.json();
    if (!t.enabled) {
      el.textContent = "Telegram isn't switched on for your site yet. Message KasPass and we'll turn it on.";
      $("tgLinks").innerHTML = "";
      pill("tgPill", false, "Not available yet");
      return;
    }
    const on = t.groups.filter((g) => g.assigned);
    pill("tgPill", on.length > 0, on.length ? `Connected: ${on.map((g) => g.title).join(", ")}` : "Not connected");
    const groups = t.groups.map((g) => `<b>${escHtml(g.title)}</b> → ${g.assigned ? escHtml(g.for) : "<i>not linked to a plan yet: send /kaspass in the group</i>"}`).join("<br>");
    el.innerHTML = `<ol class="small"><li>Open your private group in Telegram and add <b>@${escHtml(t.bot)}</b> as a member.</li><li>Make it an admin, with "Invite users" and "Ban users" switched on.</li><li>In the group, send <b>/kaspass</b>. The bot lists your ${escHtml(levelWord().many)}. Send the one this group is for.</li></ol>` +
      (t.groups.length ? `<div style="margin-top:8px">Your groups:<br>${groups}</div>` : "") +
      `<p class="muted small" style="margin:8px 0 0">Members can then join from your page, and anyone whose membership ends is removed for you.</p>`;
    if (t.unmanaged && t.unmanaged.length) {
      el.innerHTML += `<div style="margin-top:10px">The bot is also in ${t.unmanaged.length === 1 ? "a group" : "these groups"} without admin rights, so it isn't managing ${t.unmanaged.length === 1 ? "it" : "them"}:<br>` +
        t.unmanaged.map((g) => `<b>${escHtml(g.title)}</b> <button class="ghost" style="padding:4px 10px;font-size:13px" data-leave="${g.chat_id}">Remove the bot</button>`).join("<br>") + "</div>";
      el.querySelectorAll("[data-leave]").forEach((b) => (b.onclick = async () => {
        if (b.dataset.armed !== "1") { b.dataset.armed = "1"; b.textContent = "Press again to remove"; return; }
        b.disabled = true;
        const r = await fetch("/v1/creator/telegram/leave", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, chat_id: Number(b.dataset.leave) }) });
        b.textContent = r.ok ? "Removed" : "Couldn't remove: " + (await r.text());
        if (r.ok) setTimeout(renderTelegram, 1200);
      }));
    }
    $("tgLinks").innerHTML = t.links.length
      ? "<tr><th>Telegram</th><th>Wallet</th><th>Qualifies for</th><th>In groups</th></tr>" +
        t.links.map((l) => `<tr><td>${escHtml(l.name)}</td><td>${short(l.address)}</td><td>${l.qualifies ? (l.qualifies.map(escHtml).join(", ") || "None") : "checking…"}</td><td>${l.in.map(escHtml).join(", ") || "None"}</td></tr>`).join("")
      : "";
    $("tgLinks").classList.add("stack");
  } catch (e) {
    el.textContent = "Couldn't load the Telegram status.";
  }
}

// .kas and .k names for members (from their linked mainnet wallets), filled in when they arrive.
let memberNames = {};
let namesLoaded = false;
async function loadNames() {
  if (namesLoaded || !session) return;
  namesLoaded = true;
  try {
    const r = await fetch(L("/v1/creator/names?session=" + encodeURIComponent(session)));
    if (r.ok) { memberNames = await r.json(); if (data && !(document.activeElement && document.activeElement.classList.contains("mnote"))) render(); }
  } catch {}
}

// Time left, rounded to the unit that reads best: "30 days", "5 hours", "12 minutes".
function timeLeft(ms) {
  const n = (x, u) => `${x} ${u}${x === 1 ? "" : "s"}`;
  if (ms >= 2 * 86400000) return n(Math.round(ms / 86400000), "day");
  if (ms >= 2 * 3600000) return n(Math.round(ms / 3600000), "hour");
  if (ms >= 60000) return n(Math.ceil(ms / 60000), "minute");
  return "under a minute";
}

// A length in words: "10 minutes", "1 hour", "30 days", "2 weeks".
function dur(ms) {
  const m = Math.round(ms / 60000);
  const say = (n, u) => `${n} ${u}${n === 1 ? "" : "s"}`;
  if (m === 0) return "none";
  if (m % 10080 === 0) return say(m / 10080, "week");
  if (m % 1440 === 0) return say(m / 1440, "day");
  if (m % 60 === 0) return say(m / 60, "hour");
  return say(m, "minute");
}

// The level's permanent settings, in plain words.
function renderFixed(lane) {
  const s = card.settings, w = levelWord();
  $("fixedTitle").textContent = `Set when ${data.name || "it"} was created`;
  $("fixedHow").textContent = `Want different ones? Add a new ${w.one} with the settings you want, sell it, and stop selling this one. Current members keep what they bought.`;
  const rows = [
    ["Billing period", `${dur(s.period_ms)}${s.period_ms < 86400000 ? " (shortened for testing; members see it as a month)" : ""}`],
    ["Free trial", s.trial_ms > 0 ? dur(s.trial_ms) : "none"],
    ["Renew early", `up to ${dur(s.max_ahead_ms)} before a membership ends`],
    ["Deposit per member", `${kas(s.bond)}, returned when a member ends their membership`],
    ["Supply", lane ? (lane.remaining < 0 ? "unlimited" : `${lane.remaining} left`) : "?"],
    ["Resale", s.resellable ? `allowed, ${kas(s.royalty)} to you each time` : "not allowed"],
  ];
  if (s.v4) {
    rows.push(["Auto-renew", `in the last ${dur(s.v4.auto_window_ms)} of each period`]);
  }
  if (s.fee) rows.push(["KasPass fee", `${s.fee.bps / 100}% of each payment, from your share`]);
  $("fixedList").innerHTML = rows.map(([k, v]) => `<dt>${escHtml(k)}</dt><dd>${escHtml(v)}</dd>`).join("");
}

function stat(v, l) { return `<div class="stat"><div class="v">${v}</div><div class="l">${l}</div></div>`; }
// Overview amounts: whole KAS unless the creator ticks "Show exact KAS" (remembered in this browser).
let statExact = false;
try { statExact = localStorage.getItem("kaspass.statExact") === "1"; } catch {}
const kasStat = (s) => (statExact ? kas(s) : Math.round(Number(s) / KAS).toLocaleString() + " KAS");
$("statExact").checked = statExact;
$("statExact").onchange = () => { statExact = $("statExact").checked; try { localStorage.setItem("kaspass.statExact", statExact ? "1" : "0"); } catch {} if (data && card) render(); };

function render() {
  const s = card.settings, v4 = data.v4;
  // Overview: every plan together, in plain tiles. Hidden until the first sale.
  const t = ALL ? ALL.totals : { ...data.totals, auto_renewing: v4 ? v4.auto_renewing : 0 };
  const any = t.sales + t.renewals + t.upgrades + t.trials + t.active > 0;
  $("stats").innerHTML = !any ? "" : [
    stat(kasStat(t.earned_sompi), "Earned so far"),
    ...(VAULT != null ? [stat(kasStat(VAULT), `Ready to withdraw${VAULT > 0 ? ' <a href="#payouts" data-go="payouts">Withdraw</a>' : ""}`)] : []),
    stat(`${t.active}`, "Members now"),
    stat(`${t.renewals}`, `Renewals${t.autorenewals ? ` (${t.autorenewals} by auto-renew)` : ""}`),
    stat(`${t.upgrades}`, "Upgrades"),
    ...(v4 ? [stat(`${t.auto_renewing}`, "On auto-renew")] : []),
    ...(t.trials ? [stat(`${t.trials}`, "Free trials")] : []),
  ].join("");
  show("stats", any);
  show("statsNone", !any);
  renderV4();
  $("tierTitle").textContent = data.name || "This membership";
  if (document.activeElement !== $("tName")) $("tName").value = data.name || "";
  // The tax-year tip is for names that already use {…}.
  show("taxTip", /[{}]/.test($("tName").value + " " + $("perks").value));
  if (document.activeElement !== $("cName")) $("cName").value = data.creator_name || "";
  if (document.activeElement !== $("pWord")) $("pWord").value = data.pass_word && data.pass_word.one !== "membership" ? data.pass_word.one : "";

  const lane = (data.lanes.find((l) => l.lane.remaining !== 0) || data.lanes[0] || {}).lane;

  renderSale(lane);
  $("pricesHint").textContent = data.levels && data.levels.length > 1 ? `These are for ${data.level.name || "this plan"}. Pick another plan at the top to change its price or sale.` : "";
  show("pricesHint", !!(data.levels && data.levels.length > 1));

  const kinds = [];
  if (s.trial_ms > 0) kinds.push(["trial", `Free trial for ${dur(s.trial_ms)} (you pay its ${kas(s.bond)} deposit)`]);
  // v5 lanes hold a ceiling; a gift is charged today's price under it. Say it in money: what
  // leaves the wallet, and what comes back to the vault after the KasPass fee.
  const v5card = !!(s.v4 && s.v4.record_id);
  const today = levelPrices[card.covenant_id];
  const back = (x) => (s.fee && s.fee.bps ? Math.round(x * (10000 - s.fee.bps) / 10000) : x);
  const gp = (x, word) => (v5card && today ? `${word} (you pay about ${Math.round(today / KAS).toLocaleString()} KAS, about ${Math.round(back(today) / KAS).toLocaleString()} KAS comes back to your vault)` : v5card ? `${word} (paid by you at today's price; it comes back to your vault, less the KasPass fee)` : `${word} (you pay ${kas(x)}, ${kas(back(x))} comes back to you)`);
  if (lane) kinds.push(["monthly", gp(lane.price, "One month")]);
  if (lane && lane.life > 0) kinds.push(["lifetime", v5card ? "Lifetime (paid by you; it comes back to your vault, less the KasPass fee)" : `Lifetime (you pay ${kas(lane.life)}, ${kas(back(lane.life))} comes back to you)`]);
  const sel = $("gKind"), keep = sel.value;
  sel.innerHTML = kinds.map(([v, l]) => `<option value="${v}">${l}</option>`).join("");
  if (keep) sel.value = keep;

  const rows = [...data.passes].sort((a, b) => Number(b.active) - Number(a.active) || b.expiry_ms - a.expiry_ms);
  const bal = (p) => (s.v4 ? `<td data-l="Prepaid">${p.lifetime ? "" : p.balance_sompi > 0 ? `${kas(p.balance_sompi)}${p.balance_sompi >= p.rate_sompi ? ` <span class="pill ok">auto</span>` : ""}` : "none"}</td>` : "");
  const nameOf = (a) => { const n = a && memberNames[a]; return n ? [n.k, n.kas].filter(Boolean) : []; };
  const whoHtml = (a, fallback) => {
    if (!a) return fallback;
    const names = nameOf(a);
    return (names.length ? names.map((x) => `<span class="name">${escHtml(x)}</span>`).join(" ") + "<br>" : "") + `<span class="mono ${names.length ? "muted small" : ""}" title="${escHtml(a)}">${short(a)}</span>`;
  };
  $("members").innerHTML = `<tr><th>Member</th><th>Your note</th><th>Kind</th><th>Status</th><th>Paid</th>${s.v4 ? "<th>Prepaid</th>" : ""}<th>Bought by</th></tr>` + (rows.length ? rows.map((p, i) => {
    const left = p.expiry_ms - chainNow();
    const status = p.lifetime ? "Never expires" : left > 0 ? `${timeLeft(left)} left` : "Expired";
    const who = whoHtml(p.owner_address, `<span class="muted small" title="This site hasn't seen this member's wallet yet. It fills in when they sign in here.">Wallet not seen yet</span><br><span class="mono muted small">${p.owner_hash.slice(0, 10)}…</span>`);
    const by = p.bought_by_keeper ? "Your fees wallet (free month)" : !p.bought_by ? `<span class="muted small">Not known</span>` : p.bought_by === p.owner_address ? "The member" : whoHtml(p.bought_by, "");
    return `<tr><td>${who}</td><td data-l="Your note"><input class="mnote" data-owner="${escHtml(p.owner_hash)}" maxlength="120" placeholder="Add a note" value="${escHtml(p.note || "")}" aria-label="Your note on this member" style="min-width:140px"></td><td data-l="Kind">${escHtml(p.kind)}</td><td data-l="Status"><span class="pill ${p.active ? "ok" : "no"}">${status}</span></td><td data-l="Paid">${kas(p.paid_sompi)}</td>${bal(p)}<td data-l="Bought by">${by}</td></tr>`;
  }).join("") : `<tr><td colspan="7" class="muted">No members yet. Share your page to get your first one. <button class="ghost" id="copyPage" style="padding:4px 10px;font-size:13px">Copy my page link</button></td></tr>`);
  if ($("copyPage")) $("copyPage").onclick = () => navigator.clipboard.writeText(location.origin + "/").then(() => { $("copyPage").textContent = "Copied"; setTimeout(() => ($("copyPage").textContent = "Copy my page link"), 1500); }).catch(() => {});
  document.querySelectorAll("#members .mnote").forEach((inp) => {
    const save = async () => {
      if (inp.value === (inp.dataset.saved ?? inp.defaultValue)) return;
      try {
        const r = await fetch("/v1/creator/member-note", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, owner_hash: inp.dataset.owner, note: inp.value }) });
        if (!r.ok) throw new Error(await r.text());
        inp.dataset.saved = inp.value;
        for (const p of data.passes) if (p.owner_hash === inp.dataset.owner) p.note = inp.value;
        inp.style.outline = "2px solid var(--ok, #3fb68b)"; setTimeout(() => (inp.style.outline = ""), 1200);
      } catch (e) { inp.style.outline = "2px solid var(--bad, #e5484d)"; inp.title = String(e.message || e); }
    };
    inp.addEventListener("change", save);
    inp.addEventListener("keydown", (e) => { if (e.key === "Enter") inp.blur(); });
  });

  const label = { monthly: "Monthly sale", lifetime: "Lifetime sale", term: "Commitment sale", trial: "Free trial", self_trial: "Free trial", renew: "Renewal", autorenew: "Auto-renewal", upgrade: "Upgrade to lifetime", transfer: "Resale royalty" };
  const row = (e, plan) => `<tr><td>${label[e.kind] || escHtml(e.kind)}${plan ? ` <span class="muted small">${escHtml(plan)}</span>` : ""}</td><td data-l="Paid">${kas(e.amount_sompi)}</td><td data-l="To you">${kas(e.creator_sompi)}</td><td data-l="KasPass fee">${e.fee_sompi ? kas(e.fee_sompi) : "none"}</td></tr>`;
  const head = "<tr><th>What</th><th>Paid</th><th>To you</th><th>KasPass fee</th></tr>";
  const ev = [...data.events].reverse().slice(0, 50);
  $("activity").innerHTML = head + (ev.length ? ev.map((e) => row(e)).join("") : `<tr><td colspan="4" class="muted">No payments yet.</td></tr>`);
  // The overview shows the latest few, from every plan.
  const evAll = ALL ? [...ALL.events].reverse().slice(0, 5) : ev.slice(0, 5);
  $("recent").innerHTML = head + (evAll.length ? evAll.map((e) => row(e, ALL ? e.plan : "")).join("") : `<tr><td colspan="4" class="muted">No payments yet.</td></tr>`);
  $("recent").classList.add("stack");

  renderFixed(lane);
  $("tierInfo").innerHTML = `
    <p class="mono" style="margin-top:0">${escHtml(card.covenant_id)}</p>
    <p class="muted" style="margin:0">Period ${mins(s.period_ms)} · trials ${s.trial_ms > 0 ? mins(s.trial_ms) : "off"} · ${s.resellable ? `resellable, ${kas(s.royalty)} royalty` : "not resellable"} · deposit ${kas(s.bond)} · prepay limit ${mins(s.max_ahead_ms)} · ${s.v4 ? `KasPass fee ${s.fee.bps / 100}% · auto-renew window ${mins(s.v4.auto_window_ms)} · holder discounts up to ${s.v4.max_discount_bps / 100}% (v4)` : s.fee ? `KasPass fee ${s.fee.bps / 100}% (v3)` : "no KasPass fee (v2)"}</p>`;
}

// v4: the keeper wallet that pays network fees for auto-renewals, and the holder discount.
// Both Save buttons (Levels: name and perks; Settings: your name and the word) save everything.
async function saveNames(msgId) {
  ["tNameMsg", "lvNameMsg"].forEach((id) => say(id, ""));
  const post = (url, name) => fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, name }) });
  const word = fetch("/v1/creator/pass-word", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, one: $("pWord").value }) });
  const site = fetch("/v1/creator/site-names", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, club_name: $("clubName").value, level_word: $("lvWord").value }) });
  const perks = fetch(L("/v1/creator/perks"), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, perks: $("perks").value.split("\n") }) });
  const [a, b, c, d, e] = await Promise.all([post("/v1/creator/display-name", $("cName").value), post(L("/v1/creator/name"), $("tName").value), word, perks, site]);
  const bad = [a, b, c, d, e].find((r) => !r.ok);
  if (bad) { say(msgId, await bad.text()); return; }
  const [ja, jb, jc, jd, je] = [await a.json(), await b.json(), await c.json(), await d.json(), await e.json()];
  data.creator_name = ja.name; data.name = jb.name; data.pass_word = jc.pass_word; data.club_name = je.club_name; data.level_word = je.level_word;
  $("lvWord").value = je.level_word.one === "plan" ? "" : je.level_word.one;
  applyLevelWord(); showLaunch(!$("launch").classList.contains("hidden"));
  if (data.level) {
    data.level.perks = jd.perks;
    const mine = (data.levels || []).find((l) => l.covenant_id === data.level.covenant_id);
    if (mine && jb.name) { mine.name = jb.name; data.level.name = jb.name; }
  }
  $("tierTitle").textContent = jb.name || "This membership";
  $("pWord").value = jc.pass_word.one === "membership" ? "" : jc.pass_word.one;
  renderLevelPick();
  renderSetup();
  if (msgId === "lvNameMsg") {
    const ra = await fetch(L("/v1/creator/about"), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, html: lvAbout.get() }) });
    if (!ra.ok) { say(msgId, await ra.text()); return; }
    const ja = await ra.json();
    lvAbout.set(ja.html);
    if (pageCfg) { pageCfg.about = pageCfg.about || {}; pageCfg.about[card.covenant_id] = ja.html; pageCfg.about_raw = pageCfg.about_raw || {}; pageCfg.about_raw[card.covenant_id] = ja.html; }
    say(msgId, "Saved. Members see it on the membership page now."); renderCards(); return;
  }
  say(msgId, "Saved. Members see " + (ja.name ? `"${ja.name}"` : "your wallet name") + " as the creator, " + (jb.name ? `"${jb.name}"` : "\"This membership\"") + " as the membership's name, and hold " + (/^[aeiou]/i.test(jc.pass_word.one) ? "an" : "a") + ` "${jc.pass_word.one}".`);
  renderCards();
}

$("lvNameGo").onclick = () => saveNames("lvNameMsg");

function renderV4() {
  const v4 = data.v4, s = card.settings;
  show("v4Panel", !!v4);
  document.querySelectorAll(".v4only").forEach((el) => el.classList.toggle("hidden", !s.v4));
  if (!v4) return;
  const k = v4.keeper;
  const low = k && k.balance_sompi < KAS;
  const unit = card && card.network === "mainnet" ? "KAS" : "test KAS";
  $("keeperInfo").innerHTML = (k
    ? `<b>Fees wallet</b><br>When a member's ${escHtml(data.pass_word ? data.pass_word.one : "membership")} renews itself, nobody is there to approve it, so this small wallet sends the renewal and pays its tiny network fee (about 0.03 ${unit} each). The renewal itself is paid from the member's own balance and can only go to you; this wallet can't move anyone's money.` +
      `<br><span class="mono">${escHtml(k.address)}</span> <button class="ghost" id="keeperCopy" style="padding:4px 10px;font-size:12px">Copy</button>` +
      `<br>Balance <b>${kas(k.balance_sompi)}</b>, enough for about ${Math.floor(Number(k.balance_sompi) / 3000000).toLocaleString()} renewals.` +
      (low ? ` <span style="color:var(--bad)">Send it 2 ${unit} from any wallet (that covers about 60 renewals) so auto-renew can run.</span>` : "")
    : "Auto-renew isn't switched on for your site yet. Message KasPass and we'll turn it on.") +
    `<br>${v4.auto_renewing} member${v4.auto_renewing === 1 ? "" : "s"} on auto-renew, ${kas(v4.prepaid_sompi)} paid ahead (theirs until it's used).`;
  if ($("keeperCopy")) $("keeperCopy").onclick = () => { navigator.clipboard.writeText(k.address).then(() => { $("keeperCopy").textContent = "Copied"; setTimeout(() => ($("keeperCopy").textContent = "Copy"), 1500); }).catch(() => {}); };
  show("discountBox", v4.max_discount_bps > 0);
  if (document.activeElement !== $("dPct")) $("dPct").value = v4.holder_bps / 100;
  $("dPct").max = v4.max_discount_bps / 100;
  $("dNote").textContent = v4.discounts
    ? `Members who qualify in your holder club pay this much less, checked again at every renewal. The most you can set is ${v4.max_discount_bps / 100}%.`
    : "Holder discounts can't be turned on for this plan. Message KasPass.";
  $("dGo").disabled = !v4.discounts;
}

$("dGo").onclick = async () => {
  say("dErr", ""); say("dStatus", "");
  try {
    const r = await fetch(L("/v1/creator/discount"), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, holder_pct: Number($("dPct").value) }) });
    if (!r.ok) throw new Error(await r.text());
    const j = await r.json();
    data.v4.holder_bps = j.holder_bps;
    say("dStatus", j.holder_bps ? `Saved: holders pay ${j.holder_bps / 100}% less from now on.` : "Saved: holder discount is off.");
  } catch (e) { plainSay("dErr", e); }
};

function renderOutputs(statusId, outputs) {
  if (!outputs) return;
  const ul = document.createElement("ul");
  ul.className = "outs";
  for (const o of outputs) {
    const li = document.createElement("li");
    const n = document.createElement("span"); n.className = "n"; n.textContent = "Output " + o.index;
    const w = document.createElement("span"); w.className = "what" + (o.label.startsWith("KasPass fee") ? " fee" : ""); w.textContent = `${o.kas} · ${o.label}`;
    const a = document.createElement("span"); a.className = "addr"; a.textContent = "to the address ending in " + String(o.address).slice(-4); a.title = o.address;
    li.append(n, w, a);
    ul.append(li);
  }
  $(statusId).append(ul);
}

// Same flow as the members site: the server builds it, Kaspire signs, the server checks and sends.
async function runPaid({ url, body, statusId, errId, button, after }) {
  say(errId, "");
  button.disabled = true;
  try {
    if (!session) throw new Error("Sign in with Kaspire first.");
    const kaspire = await detectKaspire();
    busy(statusId, "Getting it ready…", "This takes a few seconds.");
    const prep = await fetch(L(url), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, ...body }) });
    if (!prep.ok) throw new Error(await prep.text());
    const { quote, request, summary } = await prep.json();
    say(statusId, `${summary.text}. Kaspire will show these same lines. Check they match, then approve.`);
    renderOutputs(statusId, summary.outputs);
    const signed = await kaspire.request({ method: "signPskt", params: request });
    const signedJson = typeof signed === "string" ? signed : signed.psktTransactionJson;
    txBusy = true;
    busy(statusId, "Sending to the network…");
    const sub = await fetch("/v1/submit", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ quote, psktTransactionJson: signedJson }) });
    if (!sub.ok) throw new Error(await sub.text());
    const out = await sub.json();
    await after(out, summary);
  } catch (e) {
    say(statusId, "");
    if (e.code === 4001) say(errId, "Cancelled in Kaspire. Nothing was sent.");
    else if (e.code === "WALLET_CHANGED") say(errId, e.message + " Nothing was sent.");
    else plainSay(errId, e);
    if (e.code === "WALLET_CHANGED") { wcReconnectMode(); $("signPanel").scrollIntoView({ behavior: "smooth", block: "center" }); }
  } finally {
    txBusy = false;
    button.disabled = false;
  }
}

async function waitAndRefresh(statusId, txid, test) {
  busy(statusId, "Sent. Waiting for the network to confirm…", "Keep this page open until it's confirmed.");
  for (let i = 0; i < 40; i++) {
    await sleep(2500);
    const r = await fetch(L("/v1/creator?session=" + encodeURIComponent(session)));
    if (!r.ok) continue;
    const d = await r.json();
    if (test(d)) { data = d; render(); say(statusId, "Done."); return; }
  }
  say(statusId, "It was sent and is still being confirmed. Refresh this page in a minute.");
}

// ---- Sales ---------------------------------------------------------------
const toKas = (sompi) => Math.round((sompi / KAS) * 10000) / 10000;
let curLane = null;

// A sale is a percent off new purchases (monthly, commitment and lifetime), applied by the server
// to today's price. Nothing is signed; renewals stay at the regular price.
function salePct() {
  const pct = Number($("sPct").value);
  return pct >= 1 && pct <= 90 ? pct : null;
}

function updatePreview() {
  const pct = salePct();
  $("sPreview").textContent = pct ? `Buyers will see the regular price crossed out and ${pct}% off when they join. Renewals stay at the regular price.` : "Pick a discount between 1% and 90%.";
}
$("sPct").addEventListener("input", updatePreview);

function renderCeiling(lane) {
  if (!lane || !(card.settings.v4 && card.settings.v4.record_id)) return;
  $("vcNow").textContent = `Ceiling now: ${kas(lane.price)}.`;
  if (document.activeElement !== $("vcKas") && !$("vcKas").value) $("vcKas").value = lane.price / 1e8;
}
$("vcGo").onclick = () => runPaid({
  url: "/v1/creator/prepare",
  body: { action: "prices", price_kas: Number($("vcKas").value), lifetime_kas: 0, credit: false, term_kas: 0, term_periods: 0 },
  statusId: "vcStatus", errId: "vcErr", button: $("vcGo"),
  after: (out) => waitAndRefresh("vcStatus", out.txid, (d) => d.lanes.some((l) => l.txid === out.txid)),
});

function renderSale(lane) {
  curLane = lane || null;
  renderCeiling(lane);
  const sale = data.sale;
  show("saleOn", !!sale); show("saleForm", !sale);
  if (sale) {
    const ends = sale.ends_ms ? new Date(sale.ends_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : null;
    $("saleNow").innerHTML = `${escHtml(sale.label || "Sale")} is on` +
      `<div class="sub">${sale.pct ? `${sale.pct}% off new purchases` : "Lower prices for new buyers"}${ends ? ` · ends ${escHtml(ends)}` : ""}</div>`;
    say("saleEnded", sale.ended ? "The end date has passed, so the sale is over. End it here to tidy up." : "");
  } else {
    updatePreview();
  }
}

const postSale = async (body) => {
  const r = await fetch(L("/v1/creator/sale"), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, ...body }) });
  if (!r.ok) throw new Error(await r.text());
};
$("sGo").onclick = async () => {
  const pct = salePct();
  if (!pct) { say("sErr", "Pick a discount between 1% and 90%."); return; }
  say("sErr", ""); say("sStatus", "");
  const endsVal = $("sEnds").value;
  const ends_ms = endsVal ? new Date(endsVal).getTime() : null;
  if (ends_ms && ends_ms < Date.now()) { say("sErr", "The end date is in the past."); return; }
  try {
    await postSale({ action: "start", pct, label: $("sLabel").value, ends_ms });
    say("sStatus", "Done: the sale is on.");
    await refresh();
  } catch (e) { plainSay("sErr", e); }
};

$("sEnd").onclick = async () => {
  say("sErr", ""); say("sStatus", "");
  try {
    await postSale({ action: "end" });
    say("sStatus", "Done: the sale has ended.");
    await refresh();
  } catch (e) { plainSay("sErr", e); }
};

$("gGo").onclick = () => {
  const to = $("gTo").value.trim();
  if (!to) { say("gErr", "Paste their Kaspa address first."); return; }
  const kind = $("gKind").value;
  runPaid({
    url: "/v1/creator/prepare",
    body: { action: kind === "trial" ? "trial" : "gift", kind, to },
    statusId: "gStatus", errId: "gErr", button: $("gGo"),
    after: (out) => waitAndRefresh("gStatus", out.txid, (d) => d.passes.some((p) => p.txid === out.txid)),
  });
};

$("lGo").onclick = () => runPaid({
  url: "/v1/launch/prepare",
  body: {
    period_minutes: minutesOf("lPeriod"), trial_minutes: minutesOf("lTrial"), max_ahead_minutes: minutesOf("lAhead"),
    bond_kas: Number($("lBond").value), supply: Number($("lSupply").value) || 0, resellable: $("lResell").checked, royalty_kas: $("lResell").checked ? Number($("lRoyalty").value) : 0,
    price_kas: 0, lifetime_kas: 0, credit: false,
    self_trial: $("lSelfTrial").checked && minutesOf("lTrial") > 0,
    ...v5Launch(),
  },
  statusId: "lStatus", errId: "lErr", button: $("lGo"),
  after: async (out) => {
    say("lStatus", "");
    const creator = address === card.creator_address;
    const nm = $("lName").value.trim();
    // A creator names the new level and lists its perks right away.
    if (creator && (nm || $("lPerks").value.trim())) {
      const post = (url, body) => fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, covenant_id: out.card.covenant_id, ...body }) });
      await Promise.all([nm ? post("/v1/creator/name", { name: nm }) : null, post("/v1/creator/perks", { perks: $("lPerks").value.split("\n") })]);
    }
    if (creator && lAbout.get()) await fetch("/v1/creator/about", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, covenant_id: out.card.covenant_id, html: lAbout.get() }) });
    const w = levelWord();
    const box = document.createElement("div");
    box.className = "ok-box";
    const tech = `<details class="adv"><summary>Technical details</summary><p class="small">Plan ID: <span class="mono">${escHtml(out.card.covenant_id)}</span><br>Transaction: <span class="mono">${escHtml(out.txid)}</span><br>Saved on this site as <code>${escHtml(out.card_file)}</code>.</p>${address === card.creator_address ? "" : `<p class="small">To run your own site: <code>membership-cli serve --card ${escHtml(out.card_file)}</code>.</p>`}</details>`;
    box.innerHTML = (creator ? `<b>Your new ${escHtml(w.one)}${nm ? `, ${escHtml(nm)},` : ""} is created.</b>` : `<b>Your new ${escHtml(w.one)} is created.</b>`) +
      (address === card.creator_address
        ? `<p class="small">Put it on sale next to your other ${escHtml(w.many)}. Saving takes about a minute, and your page doesn't load during that minute.</p><div class="row"><button id="lAdd">Put it on sale</button><button id="lSwitch" class="ghost">Sell only this one</button></div><div id="lSwStatus" class="small hidden" style="margin-top:10px"></div><div class="err hidden" id="lSwErr"></div>`
        : `<p class="small">This site sells another creator's plans. To sell yours, ask the site's owner to switch to it.</p>`) + tech;
    $("lStatus").append(box);
    show("lStatus", true);
    if (creator) { $("lName").value = ""; $("lPerks").value = ""; lAbout.set(""); }
    if ($("lSwitch")) {
      $("lSwitch").onclick = () => switchTier(out.card_file, $("lSwitch"), "lSwStatus", "lSwErr");
      $("lAdd").onclick = () => {
        // What's on sale now, from the server (not unsaved ticks in the table).
        const now = cardList.filter((c) => c.on_sale).sort((a, b) => a.rank - b.rank).map((c) => c.file);
        saveLevels([...now.filter((f) => f !== out.card_file), out.card_file], $("lAdd"), "lSwStatus", "lSwErr");
      };
      cardOrder = null; onSale = null;
      renderCards();
    }
  },
});


// ---- A small formatting editor: bold, italic, headings, lists, quotes, links ----
// What it makes is cleaned again by the server, which keeps only these.
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
try { document.execCommand("defaultParagraphSeparator", false, "p"); } catch {}
const editors = [];
function makeEditor(mountId, labelId, placeholder) {
  const wrap = document.createElement("div"); wrap.className = "rte";
  const bar = document.createElement("div"); bar.className = "rte-bar"; bar.setAttribute("role", "toolbar"); bar.setAttribute("aria-label", "Formatting");
  const linkRow = document.createElement("div"); linkRow.className = "rte-link hidden";
  linkRow.innerHTML = '<input type="url" placeholder="https://…" aria-label="Link address"><button type="button">Add link</button><button type="button" class="ghost">Cancel</button>';
  const body = document.createElement("div");
  body.className = "rte-body rich"; body.contentEditable = "true"; body.id = mountId.replace(/Ed$/, "");
  body.setAttribute("role", "textbox"); body.setAttribute("aria-multiline", "true"); body.setAttribute("aria-labelledby", labelId); body.dataset.ph = placeholder;
  const BLOCKS = ["h3", "h4", "p", "blockquote"];
  const tools = [["bold", "<b>B</b>", "Bold"], ["italic", "<i>I</i>", "Italic"], ["underline", "<u>U</u>", "Underline"], "|",
    ["h3", "Heading", "Heading"], ["h4", "Subheading", "Subheading"], ["p", "Text", "Normal text"], "|",
    ["insertUnorderedList", "• List", "Bulleted list"], ["insertOrderedList", "1. List", "Numbered list"], ["blockquote", "Quote", "Quote"], "|",
    ["link", "Link", "Add a link"], ["removeFormat", "Clear", "Clear formatting"]];
  let saved = null;
  const keepSel = () => { const sel = getSelection(); if (sel.rangeCount && body.contains(sel.anchorNode)) saved = sel.getRangeAt(0).cloneRange(); };
  const restore = () => { body.focus(); if (!saved) return; const sel = getSelection(); sel.removeAllRanges(); sel.addRange(saved); };
  const esc = (t) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const run = (cmd) => {
    body.focus();
    if (BLOCKS.includes(cmd)) document.execCommand("formatBlock", false, cmd);
    else if (cmd === "link") { keepSel(); linkRow.classList.remove("hidden"); linkRow.querySelector("input").focus(); return; }
    else if (cmd === "removeFormat") { document.execCommand("removeFormat"); document.execCommand("unlink"); document.execCommand("formatBlock", false, "p"); }
    else document.execCommand(cmd);
    states();
  };
  for (const t of tools) {
    if (t === "|") { const sep = document.createElement("span"); sep.className = "sep"; bar.append(sep); continue; }
    const [cmd, label, title] = t;
    const b = document.createElement("button");
    b.type = "button"; b.innerHTML = label; b.title = title; b.setAttribute("aria-label", title); b.dataset.cmd = cmd;
    if (cmd !== "link" && cmd !== "removeFormat") b.setAttribute("aria-pressed", "false");
    b.onmousedown = (e) => e.preventDefault();
    b.onclick = () => run(cmd);
    bar.append(b);
  }
  const [lin, ladd, lcancel] = linkRow.children;
  const addLink = () => {
    const u = lin.value.trim();
    linkRow.classList.add("hidden"); lin.value = "";
    restore();
    if (!u) return;
    const url = /^(https?:|mailto:)/i.test(u) ? u : "https://" + u;
    const sel = getSelection();
    if (!sel.rangeCount || sel.isCollapsed) document.execCommand("insertHTML", false, `<a href="${esc(url)}">${esc(u)}</a>`);
    else document.execCommand("createLink", false, url);
  };
  ladd.onclick = addLink;
  lcancel.onclick = () => { linkRow.classList.add("hidden"); lin.value = ""; restore(); };
  lin.onkeydown = (e) => { if (e.key === "Enter") { e.preventDefault(); addLink(); } if (e.key === "Escape") lcancel.onclick(); };
  // Pasted text comes in plain, as paragraphs, so the page keeps one look.
  body.addEventListener("paste", (e) => {
    e.preventDefault();
    const text = (e.clipboardData || window.clipboardData).getData("text/plain");
    if (!/\n/.test(text)) { document.execCommand("insertText", false, text); return; }
    document.execCommand("insertHTML", false, text.split(/\r?\n/).filter((l) => l.trim()).map((l) => `<p>${esc(l)}</p>`).join(""));
  });
  body.addEventListener("drop", (e) => e.preventDefault());
  const states = () => {
    for (const b of bar.querySelectorAll("[aria-pressed]")) {
      const c = b.dataset.cmd;
      let on = false;
      try { on = BLOCKS.includes(c) ? String(document.queryCommandValue("formatBlock")).toLowerCase() === c : document.queryCommandState(c); } catch {}
      b.setAttribute("aria-pressed", String(!!on && c !== "p"));
    }
  };
  document.addEventListener("selectionchange", () => { const sel = getSelection(); if (sel.rangeCount && body.contains(sel.anchorNode)) states(); });
  // Typing always starts inside a paragraph (so Heading, Quote and lists work on the first line);
  // an editor left empty shows its placeholder again.
  body.addEventListener("focus", () => {
    if (body.firstChild) return;
    body.innerHTML = "<p><br></p>";
    const r = document.createRange(); r.setStart(body.firstChild, 0); r.collapse(true);
    const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r);
  });
  body.addEventListener("blur", () => { if (!body.textContent.trim() && !body.querySelector("li")) body.innerHTML = ""; });
  // On a phone the toolbar folds behind one "Formatting" button.
  const fmt = document.createElement("button");
  fmt.type = "button"; fmt.className = "rte-fmt"; fmt.textContent = "Formatting ▾";
  fmt.onclick = () => { const on = wrap.classList.toggle("fmt"); fmt.textContent = on ? "Formatting ▴" : "Formatting ▾"; };
  wrap.append(fmt, bar, linkRow, body);
  $(mountId).replaceWith(wrap);
  wrap.id = mountId;
  // Empty paragraphs (from pressing Enter twice) are left out.
  const tidy = (h) => { const t = document.createElement("div"); t.innerHTML = richHtml(h); t.querySelectorAll("p, h3, h4, blockquote, li").forEach((e) => { if (!e.textContent.trim()) e.remove(); }); t.querySelectorAll("ul, ol").forEach((e) => { if (!e.children.length) e.remove(); }); return t.innerHTML; };
  const ed = { body, get: () => (body.textContent.trim() ? tidy(body.innerHTML) : ""), set: (h) => { body.innerHTML = richHtml(h); }, focused: () => body.contains(document.activeElement) || document.activeElement === body };
  editors.push(ed);
  return ed;
}
const pgIntro = makeEditor("pgIntroEd", "pgIntroL", "Who you are and what members get. For example: 7 years in crypto, no hype, just research. Use Heading for section titles and lists for what's included.");
const lvAbout = makeEditor("lvAboutEd", "lvAboutL", "What this tier is, who it's for and what members get, in as much detail as you like.");
const lAbout = makeEditor("lAboutEd", "lAboutL", "What this tier is, who it's for and what members get. You can also add this later.");

// ---- Your page: tagline, intro, links and how the tiers show ---------------------
const LINK_KINDS = ["website", "x", "tiktok", "youtube", "instagram", "telegram", "discord"];
let pageCfg = null, pageFilled = false;
async function loadPageCfg() {
  try { pageCfg = await (await fetch("/v1/page", { cache: "no-store" })).json(); } catch { pageCfg = null; }
  if (!pageCfg) return;
  if (!pageFilled) {
    pageFilled = true;
    $("pgTagline").value = pageCfg.tagline || "";
    pgIntro.set(pageCfg.intro_html || "");
    for (const k of LINK_KINDS) $("pgL_" + k).value = (pageCfg.links || {})[k] || "";
    $("pgView").value = pageCfg.view === "compare" ? "compare" : "cards";
    $("pgCount").checked = !!pageCfg.show_count;
    $("pgClub").checked = !pageCfg.hide_club;
    $("pgPriceDisplay").value = pageCfg.price_display === "viewer" ? "viewer" : "plan";
  }
  if (!lvAbout.focused()) lvAbout.set((pageCfg.about_raw || pageCfg.about || {})[card.covenant_id] || "");
}
async function savePage() {
  say("pgMsg", ""); say("pgErr", "");
  $("pgGo").disabled = true;
  try {
    const links = Object.fromEntries(LINK_KINDS.map((k) => [k, $("pgL_" + k).value.trim()]));
    const r = await fetch("/v1/creator/page", { method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ session, tagline: $("pgTagline").value, intro_html: pgIntro.get(), links, view: $("pgView").value, show_count: $("pgCount").checked, price_display: $("pgPriceDisplay").value, hide_club: !$("pgClub").checked }) });
    if (!r.ok) throw new Error(await r.text());
    const j = await r.json();
    await saveNames("tNameMsg");
    pgIntro.set(j.intro_html);
    for (const k of LINK_KINDS) $("pgL_" + k).value = (j.links || {})[k] || "";
    if (pageCfg) Object.assign(pageCfg, j);
    say("pgMsg", "Saved. Your page shows it now.");
    renderSetup();
  } catch (e) { say("pgErr", e.message || String(e)); }
  finally { $("pgGo").disabled = false; }
}
$("pgGo").onclick = savePage;

// ---- "?" tips beside each setting -----------------------------------------
const TIPS = {
  tName: "The name members see on this plan's card, for example Core or VIP.",
  perks: "What this plan gives members, one per line. Bigger plans automatically include everything in the cheaper ones, so only list what's new here.",
  gTo: "Who gets the membership. Paste their Kaspa address, or type a .kas or .k name and you'll see where it points before anything is signed.",
  gKind: "A free trial only costs you its refundable deposit. A month or lifetime is paid from your wallet as if they'd bought it, and the payment comes back to your vault, less the KasPass fee.",
  sPct: "How much cheaper joining is during the sale: monthly, commitment and lifetime. Renewals stay at the regular price.",
  vpAmount: "What a member pays each billing period, in the currency you choose. They pay it in KAS at today's rate.",
  vpLife: "One payment for a membership that never expires, in the same currency.",
  vpTerm: "A commitment: one payment up front that covers several months at once, usually for less than paying monthly. It isn't refunded if the member leaves early.",
  sLabel: "Shown on the sale banner, for example Launch sale or Black Friday.",
  sEnds: "Shown to buyers as a deadline, and the sale stops by itself at this time. Leave it empty to run the sale until you press End sale.",
  dPct: "Members who qualify in your holder club pay this much less, checked again at every renewal. It can't be more than the largest discount set when the plan was created.",
  cName: "Your name as members see it. Your wallet address and any .kas or .k names are always shown with it, so nobody can pretend to be you.",
  pWord: "What members hold. The site says this wherever it would say membership, for example pass or subscription.",
  clubName: "The name of the whole group, shown as your page's title, for example Executive Club.",
  lvWord: "What you call your plans: plan, tier, level. Used across the dashboard and your membership page.",
  lvSelect: "Which plan this section edits.",
  pgTagline: "One short line under your name at the top of your page, like a motto.",
  pgView: "How visitors on a computer first see your plans: cards side by side, or one table that compares what each includes. Phones always show cards.",
  pgCount: "Shows how many people have an active membership right now.",
  pgL_website: "Links show as buttons under your name. Leave any of them empty to hide it.",
  lName: "The name members see on this plan's card, for example VIP.",
  lPerks: "What this plan gives members, one per line. It also includes everything in the cheaper plans.",
  lPeriod: "How often members pay: one month, one year, and so on. Permanent: it can't be changed after you create the plan.",
  lTrial: "How long a free trial lasts, whether you give it or a member starts it. Permanent. Choose \"no free trial\" for none.",
  lAhead: "How early a member may renew before their membership ends, so nobody prepays for years by accident. Permanent.",
  lBond: "A small deposit held in each membership and returned when the member ends it. Permanent.",
  lSupply: "The most memberships this plan can ever sell. Leave it empty for no limit. Permanent: it can never be raised.",
  lResell: "Whether members can sell or give their membership to someone else. Permanent.",
  lRoyalty: "What you receive each time a membership changes hands, when that's allowed. Permanent.",
  lCeil: "The most KAS one payment can ever be, as a multiple of today's price. If KAS falls a lot, members pay this instead. You can change it later in Prices & sales.",
};
function addTips() {
  const box = document.createElement("div");
  box.id = "tipBox"; box.setAttribute("role", "tooltip"); box.className = "hidden";
  document.body.append(box);
  let openBtn = null;
  const hide = () => { box.classList.add("hidden"); if (openBtn) { openBtn.setAttribute("aria-expanded", "false"); openBtn.removeAttribute("aria-describedby"); } openBtn = null; };
  const open = (b) => {
    box.textContent = b.dataset.tip;
    box.classList.remove("hidden");
    const r = b.getBoundingClientRect(), w = box.offsetWidth, h = box.offsetHeight;
    const left = Math.max(16, Math.min(r.left - 12, innerWidth - 16 - w));
    const top = r.bottom + 8 + h > innerHeight ? r.top - 8 - h : r.bottom + 8;
    box.style.left = left + "px"; box.style.top = Math.max(8, top) + "px";
    openBtn = b; b.setAttribute("aria-expanded", "true"); b.setAttribute("aria-describedby", "tipBox");
  };
  for (const [id, text] of Object.entries(TIPS)) {
    const el = $(id), label = el && el.closest("label");
    if (!label || label.querySelector(".tip")) continue;
    const b = document.createElement("button");
    b.type = "button"; b.className = "tip"; b.textContent = "?"; b.dataset.tip = text;
    b.setAttribute("aria-label", "What's this?"); b.setAttribute("aria-expanded", "false");
    b.onmouseenter = () => open(b);
    b.onmouseleave = () => { if (document.activeElement !== b) hide(); };
    b.onfocus = () => open(b);
    b.onblur = hide;
    b.onclick = (e) => { e.preventDefault(); open(b); };
    // After the label's text, before its box (for a checkbox, at the end).
    if (label.classList.contains("check")) { label.append(b); continue; }
    const first = [...label.childNodes].find((n) => n.nodeType === 1 && n !== b && !n.classList.contains("perm")) || null;
    const head = document.createElement("span");
    head.className = "lt";
    while (label.firstChild && label.firstChild !== first) head.append(label.firstChild);
    head.append(b);
    label.insertBefore(head, first);
  }
  addEventListener("scroll", hide, { passive: true });
  addEventListener("keydown", (e) => { if (e.key === "Escape") hide(); });
}
addTips();

(async () => {
  card = await (await fetch("/v1/tier")).json();
  $("net").textContent = card.network === "testnet-10" ? "Testnet-10" : card.network === "mainnet" ? "Mainnet" : card.network;
  // The create form starts from sensible settings: a month, no trial, renew up to a week ahead on
  // mainnet; short test periods (minutes) on a test network so renewals can be watched.
  if (card.network === "mainnet") { setDur("lPeriod", 30 * 86400000); $("lTrialU").value = "0"; setDur("lAhead", 7 * 86400000); }
  else $("lTrialU").value = "1";
  syncTrial();
  if (card.network === "mainnet") {
    $("netNote").textContent = "New software. The code is public and has not had an outside security review.";
    $("gTo").placeholder = $("gTo").placeholder.replace("kaspatest:", "kaspa:");
  }
  // Still signed in from before a reload?
  let saved = null;
  try { saved = JSON.parse(sessionStorage.getItem("kaspass.dash") || "null"); } catch {}
  if (saved && saved.session && saved.address) {
    const ok = await fetch("/v1/creator/names?session=" + encodeURIComponent(saved.session)).then((r) => r.status !== 401).catch(() => false);
    if (ok) {
      session = saved.session;
      setAddress(saved.address);
      await refresh();
      return;
    }
    try { sessionStorage.removeItem("kaspass.dash"); } catch {}
  }
  try {
    const kaspire = await detectKaspire(1500);
    const a = (await kaspire.request({ method: "getAccounts" }))?.[0];
    if (a) setAddress(a);
  } catch {}
})();

// ---- Look and brand ---------------------------------------------------------
// The creator's own colors, fonts, pictures and website link for their membership page.
const KP_LOOK = { light: { bg: "#050808", text: "#e8f1f0", accent: "#49d9c8", card: "#0d1414" }, dark: { bg: "#050808", text: "#e8f1f0", accent: "#49d9c8", card: "#0d1414" } };
let brand = null, brandFilled = false;
const hexOk = (c) => /^#[0-9a-fA-F]{6}$/.test(c || "");
function lum(c) {
  const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
  const n = parseInt(c.slice(1), 16);
  return 0.2126 * f((n >> 16) & 255) + 0.7152 * f((n >> 8) & 255) + 0.0722 * f(n & 255);
}
const contrastOf = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
const palEl = (which) => document.querySelector(`.bcolors[data-pal="${which}"]`);
function readPal(which) {
  const out = {};
  for (const k of ["bg", "text", "accent", "card"]) out[k] = (palEl(which).querySelector(`input.hex[data-k="${k}"]`).value || "").trim().toLowerCase();
  return out;
}
function setPal(which, p) {
  for (const k of ["bg", "text", "accent", "card"]) {
    const v = (p && p[k]) || KP_LOOK[which][k];
    palEl(which).querySelector(`input.hex[data-k="${k}"]`).value = v;
    palEl(which).querySelector(`input[type=color][data-k="${k}"]`).value = v;
  }
}
document.querySelectorAll(".bcolors").forEach((box) => box.querySelectorAll("input").forEach((inp) => inp.addEventListener("input", () => {
  const other = box.querySelector(`input${inp.type === "color" ? ".hex" : "[type=color]"}[data-k="${inp.dataset.k}"]`);
  if (inp.type === "color") other.value = inp.value; else if (hexOk(inp.value.trim())) other.value = inp.value.trim();
  brandPreview();
})));
function brandForm() {
  return {
    light: $("bColorsOn").checked ? readPal("light") : null,
    dark: $("bColorsOn").checked && $("bDarkOn").checked ? readPal("dark") : null,
    head_font: $("bHead").value.trim(),
    body_font: $("bBody").value.trim(),
    corners: $("bCorners").value,
    back_url: $("bBack").value.trim(),
    start: $("bStart").value,
  };
}
// Readability: text on the page and on cards, and the words on buttons.
function contrastNote(p, label) {
  if (![p.bg, p.text, p.accent, p.card].every(hexOk)) return { ok: false, msg: `${label}: every color must look like #1a2b3c.` };
  const t = Math.min(contrastOf(p.text, p.bg), contrastOf(p.text, p.card));
  const ink = contrastOf("#ffffff", p.accent) >= contrastOf("#000000", p.accent) ? "#ffffff" : "#0a0a0a";
  const b = contrastOf(ink, p.accent);
  const bad = t < 3, weak = t < 4.5 || b < 3;
  return { ok: !bad, msg: `${label}: ${bad ? "✗ text is too hard to read" : weak ? "⚠ text could be easier to read" : "✓ easy to read"}` };
}
function brandPreview() {
  if (!$("bErr").classList.contains("hidden")) brandSay("");
  const f = brandForm();
  const pv = $("bPv");
  const p = f.light || KP_LOOK.light;
  const ink = hexOk(p.accent) && contrastOf("#ffffff", p.accent) >= contrastOf("#000000", p.accent) ? "#ffffff" : "#0a0a0a";
  const set = (k, v) => pv.style.setProperty(k, v);
  if ([p.bg, p.text, p.accent, p.card].every(hexOk)) {
    set("--pv-bg", p.bg); set("--pv-text", p.text); set("--pv-accent", p.accent); set("--pv-card", p.card); set("--pv-ink", f.light ? ink : "#031312");
    set("--pv-muted", `color-mix(in srgb, ${p.text} 64%, ${p.bg})`);
  }
  // Fonts show in the preview once saved (they're downloaded then).
  const saved = (n) => brand && n && (n === brand.head_font || n === brand.body_font);
  set("--pv-head", saved(f.head_font) ? `'${f.head_font}', sans-serif` : "Montserrat, sans-serif");
  set("--pv-body", saved(f.body_font) ? `'${f.body_font}', sans-serif` : "Inter, sans-serif");
  const r = { round: ["20px", "999px"], soft: ["14px", "10px"], square: ["6px", "6px"] }[f.corners] || ["14px", "10px"];
  set("--pv-rc", r[0]); set("--pv-rb", r[1]);
  const host = (f.back_url.match(/^https:\/\/(?:www\.)?([^/?#]+)/) || [])[1];
  $("bPvBack").textContent = host ? "← Back to " + host : "";
  const name = (data && data.creator_name) || (data && data.name) || "Your page";
  $("bPvName").textContent = name; $("bPvTitle").textContent = name;
  const notes = [];
  if (f.light) notes.push(contrastNote(f.light, "Light"));
  if (f.dark) notes.push(contrastNote(f.dark, "Dark"));
  $("bContrast").textContent = notes.map((n) => n.msg).join(" ");
  $("bContrast").style.color = notes.some((n) => !n.ok) ? "var(--bad)" : "";
  $("bPvNote").textContent = f.dark ? (f.start === "device" ? "Light shown. Visitors whose device is set to dark get your dark colors; everyone can switch." : f.start === "dark" ? "Light shown. Visitors start in your dark colors and can switch." : "Light shown. Visitors start in light and can switch to your dark colors.") : (f.head_font || f.body_font) && !(saved(f.head_font) || saved(f.body_font)) ? "New fonts show here once you save." : "";
}
["bHead", "bBody", "bCorners", "bBack"].forEach((id) => $(id).addEventListener("input", brandPreview));
$("bColorsOn").onchange = () => { show("bColors", $("bColorsOn").checked); brandPreview(); };
$("bDarkOn").onchange = () => { show("bDark", $("bDarkOn").checked); show("bStartBox", $("bDarkOn").checked); brandPreview(); };
function fillPictures() {
  document.querySelectorAll(".bpic").forEach((box) => {
    const url = brand && brand[box.dataset.kind];
    const img = box.querySelector(".bprev img");
    if (url) img.src = url;
    img.classList.toggle("hidden", !url);
    box.querySelector(".bprev span").classList.toggle("hidden", !!url);
    box.querySelector(".brm").classList.toggle("hidden", !url);
  });
  if (brand && brand.logo) $("bPvLogo").src = brand.logo;
  $("bPvLogo").classList.toggle("hidden", !(brand && brand.logo));
}
async function loadBrand() {
  try { brand = await (await fetch("/v1/brand", { cache: "no-store" })).json(); } catch { return; }
  if (!brandFilled) {
    brandFilled = true;
    $("bColorsOn").checked = !!(brand.light || brand.dark);
    $("bDarkOn").checked = !!(brand.light && brand.dark);
    setPal("light", brand.light || brand.dark);
    setPal("dark", brand.dark);
    show("bColors", $("bColorsOn").checked); show("bDark", $("bDarkOn").checked); show("bStartBox", $("bDarkOn").checked);
    $("bStart").value = brand.start || "light";
    $("bHead").value = brand.head_font || ""; $("bBody").value = brand.body_font || "";
    $("bCorners").value = brand.corners || "";
    $("bBack").value = brand.back_url || "";
    $("bScanUrl").value = brand.back_url || "";
  }
  fillPictures();
  brandPreview();
}
function brandSay(msg, err) {
  $("bStatus").textContent = err ? "" : msg; show("bStatus", !err && !!msg);
  $("bErr").textContent = err ? msg : ""; show("bErr", !!err && !!msg);
}
async function brandPost(url, body) {
  const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, ...body }) });
  const t = await r.text();
  if (!r.ok) throw new Error(t || "that didn't work");
  return JSON.parse(t);
}
$("bSave").onclick = async () => {
  const f = brandForm();
  for (const p of [f.light, f.dark].filter(Boolean)) { const n = contrastNote(p, ""); if (!n.ok) return brandSay("Some text would be too hard to read. Pick a text color further from the background.", true); }
  $("bSave").disabled = true;
  brandSay((f.head_font || f.body_font) ? "Saving (downloading your fonts)…" : "Saving…");
  try {
    brand = await brandPost("/v1/creator/brand", f);
    fillPictures(); brandPreview();
    brandSay("Saved. Your page has the new look now.");
  } catch (e) { brandSay(e.message, true); }
  $("bSave").disabled = false;
};
$("bReset").onclick = async () => {
  if ($("bReset").dataset.armed !== "1") { $("bReset").dataset.armed = "1"; $("bReset").textContent = "Click again to go back to the KasPass look"; setTimeout(() => { $("bReset").dataset.armed = ""; $("bReset").textContent = "Back to the KasPass look"; }, 4000); return; }
  $("bReset").dataset.armed = ""; $("bReset").textContent = "Back to the KasPass look";
  try {
    brand = await brandPost("/v1/creator/brand", { reset: true });
    brandFilled = false; await loadBrand();
    brandSay("Back to the KasPass look. Your pictures and website link stay until you remove them.");
  } catch (e) { brandSay(e.message, true); }
};
document.querySelectorAll(".bpic").forEach((box) => {
  const kind = box.dataset.kind;
  box.querySelector("input[type=file]").addEventListener("change", async (ev) => {
    let file = ev.target.files && ev.target.files[0];
    ev.target.value = "";
    if (!file) return;
    brandSay("Uploading…");
    try {
      file = await shrinkPicture(file, kind);
      const r = await fetch(`/v1/creator/brand/picture?kind=${kind}`, { method: "POST", headers: { "content-type": file.type || "application/octet-stream", "x-kaspass-session": session }, body: file });
      const t = await r.text();
      if (!r.ok) throw new Error(t);
      brand = JSON.parse(t); fillPictures();
      brandSay("Uploaded. It's on your page now.");
    } catch (e) { brandSay(e.message || "upload failed", true); }
  });
  box.querySelector(".brm").onclick = async () => {
    try { brand = await brandPost("/v1/creator/brand/picture-remove", { kind }); fillPictures(); brandSay("Removed."); } catch (e) { brandSay(e.message, true); }
  };
});
// Big photos are shrunk in the browser before upload (nothing leaves your computer until then):
// at most 512 px for a logo or photo and 1600 px wide for a banner, as WebP.
const PIC = { logo: { px: 512, max: 300000 }, photo: { px: 512, max: 400000 }, banner: { px: 1600, max: 1500000 } };
async function shrinkPicture(file, kind) {
  const lim = PIC[kind];
  if (!/^image\/(png|jpeg|webp)$/.test(file.type)) return file; // SVG and others go as they are
  const img = await createImageBitmap(file).catch(() => null);
  if (!img) return file;
  const scale = Math.min(1, lim.px / Math.max(img.width, kind === "banner" ? img.width : img.height));
  if (scale >= 1 && file.size <= lim.max) return file;
  const c = document.createElement("canvas");
  c.width = Math.max(1, Math.round(img.width * scale)); c.height = Math.max(1, Math.round(img.height * scale));
  c.getContext("2d").drawImage(img, 0, 0, c.width, c.height);
  for (const q of [0.9, 0.8, 0.7, 0.55]) {
    const blob = await new Promise((r) => c.toBlob(r, "image/webp", q));
    if (blob && blob.type === "image/webp" && blob.size <= lim.max) return new File([blob], "picture.webp", { type: "image/webp" });
  }
  const jpg = await new Promise((r) => c.toBlob(r, "image/jpeg", 0.75));
  return jpg ? new File([jpg], "picture.jpg", { type: "image/jpeg" }) : file;
}
let foundLogo = null;
$("bScan").onclick = async () => {
  const url = $("bScanUrl").value.trim();
  if (!url) return brandSay("Type your website's address first.", true);
  $("bScan").disabled = true;
  brandSay("Reading your website…");
  try {
    const r = await brandPost("/v1/creator/brand/scan", { url });
    const got = [];
    if (r.light) { $("bColorsOn").checked = true; show("bColors", true); setPal("light", r.light); got.push("colors"); }
    if (r.dark) { $("bDarkOn").checked = true; show("bDark", true); show("bStartBox", true); setPal("dark", r.dark); got.push("dark colors"); }
    if (r.head_font) { $("bHead").value = r.head_font; got.push("heading font"); }
    if (r.body_font) { $("bBody").value = r.body_font; got.push("text font"); }
    if (r.back_url && !$("bBack").value) { $("bBack").value = r.back_url; got.push("website link"); }
    if (r.corners) { $("bCorners").value = r.corners; got.push("button shape"); }
    foundLogo = r.logo_url || null;
    if (foundLogo) $("bLogoFoundName").textContent = foundLogo.split("/").pop().split("?")[0];
    show("bLogoFound", !!foundLogo);
    brandPreview();
    brandSay(got.length ? `Found ${got.join(", ")}${foundLogo ? " and a logo" : ""}. Check them, then press Save.` : "Couldn't find colors or fonts on that page. You can set them by hand below.", !got.length && !foundLogo);
  } catch (e) { brandSay(e.message, true); }
  $("bScan").disabled = false;
};
$("bLogoUse").onclick = async () => {
  if (!foundLogo) return;
  try { brand = await brandPost("/v1/creator/brand/logo-from-site", { url: foundLogo }); fillPictures(); show("bLogoFound", false); brandSay("Logo saved. It's on your page now."); } catch (e) { brandSay(e.message, true); }
};



// ---- Holder promotion: free passes for holders, sent by the keeper ------------
let PR = null;
const NACHO_KCC20 = "41d44970ba6307bcec418f677fd253778d0ffa47750d179dff647061aa6c0d40";
const toLocalInput = (ms) => { const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60000); return d.toISOString().slice(0, 16); };
function needRow(n) {
  const d = document.createElement("div");
  d.className = "row prneed";
  d.style.cssText = "gap:8px;align-items:flex-end;flex-wrap:wrap;margin:4px 0";
  d.innerHTML = `<label class="small">At least<input type="number" class="prMin" min="0" step="any" style="width:120px"></label>
    <label class="small">of<select class="prKind"><option value="nft">NFTs (KRC-721)</option><option value="token">tokens (KRC-20)</option><option value="kcc20">tokens (KCC-20)</option><optgroup label="More"><option value="migrated">tokens moved to KCC-20 (KAT Bridge)</option><option value="burned">tokens burned (KAT Bridge)</option></optgroup></select></label>
    <label class="small">Ticker<input class="prTick" maxlength="20" placeholder="NACHO" style="width:120px;text-transform:uppercase"></label>
    <label class="small prIdBox hidden" style="flex:1 1 260px">Token id (covenant id)<input class="prId mono" maxlength="64" placeholder="64 letters and digits, from the token's official page"></label>
    <label class="small prSinceBox hidden">Only since (optional)<input class="prSince" type="date"></label>
    <label class="check prPlainBox hidden" style="align-self:center"><input type="checkbox" class="prPlain"> Also count plain burns</label>
    <button class="ghost prDel" style="padding:4px 10px">Remove</button>`;
  d.querySelector(".prMin").value = n ? n.min : 1;
  d.querySelector(".prKind").value = n ? n.kind : "nft";
  d.querySelector(".prTick").value = n ? n.tick : "";
  d.querySelector(".prId").value = n && n.id ? n.id : "";
  if (n && n.since_ms) d.querySelector(".prSince").value = new Date(n.since_ms).toISOString().slice(0, 10);
  d.querySelector(".prPlain").checked = !!(n && n.plain);
  const kindShown = () => {
    const kind = d.querySelector(".prKind").value, k = kind === "kcc20";
    d.querySelector(".prIdBox").classList.toggle("hidden", !k);
    d.querySelector(".prSinceBox").classList.toggle("hidden", kind !== "migrated" && kind !== "burned");
    d.querySelector(".prPlainBox").classList.toggle("hidden", kind !== "burned");
    // NACHO's official KCC-20 id (nachothekat.xyz › Migration), filled in for convenience.
    if (k && !d.querySelector(".prId").value && d.querySelector(".prTick").value.trim().toUpperCase() === "NACHO") d.querySelector(".prId").value = NACHO_KCC20;
  };
  kindShown();
  d.querySelector(".prTick").addEventListener("change", kindShown);
  d.querySelector(".prDel").onclick = () => { d.remove(); if (PR) renderPromo(); };
  d.querySelector(".prKind").onchange = () => { kindShown(); if (PR) renderPromo(); };
  $("prNeeds").append(d);
}
$("prAddNeed").onclick = () => { if ($("prNeeds").children.length < 6) needRow(null); };

async function loadPromo() {
  try {
    const r = await fetch("/v1/creator/promo?session=" + encodeURIComponent(session));
    if (!r.ok) { show("promoPanel", false); return; }
    PR = await r.json();
  } catch { show("promoPanel", false); return; }
  show("promoPanel", !!PR.keeper && PR.levels.length > 0);
  renderPromo();
}

function renderPromo() {
  const p = PR.promo, v = PR.view, unit = PR.network === "mainnet" ? "KAS" : "test KAS";
  const sel = $("prLevel");
  if (!sel.options.length || sel.dataset.n !== String(PR.levels.length)) {
    sel.replaceChildren(...PR.levels.map((l) => { const o = document.createElement("option"); o.value = l.level; o.textContent = l.name; return o; }));
    sel.dataset.n = String(PR.levels.length);
    // Default: the shortest plan (usually the monthly one).
    const short = [...PR.levels].sort((a, b) => a.period_ms - b.period_ms)[0];
    if (short) sel.value = short.level;
  }
  const running = p && !p.ended && v && v.closed !== "ended";
  if (p && !$("prNeeds").children.length) {
    sel.value = p.level;
    $("prLabel").value = p.label;
    $("prMatch").value = p.needs.match;
    p.needs.list.forEach(needRow);
    $("prStart").value = toLocalInput(p.start_ms);
    $("prEnd").value = toLocalInput(p.end_ms);
    $("prMax").value = p.max_claims;
  }
  if (!$("prNeeds").children.length) needRow(null);
  if (!$("prStart").value) $("prStart").value = toLocalInput(Date.now());
  if (!$("prEnd").value) $("prEnd").value = toLocalInput(Date.now() + 30 * 86400000);
  if (!$("prLabel").value) $("prLabel").value = "Holders: your first month is free";
  show("prNewBox", !!p);
  show("prEndGo", !!running);
  $("prGo").textContent = running && !$("prNew").checked ? "Save changes" : "Start promotion";
  show("prNow", !!p);
  if (p) {
    const state = p.ended || (v && v.closed === "ended") ? "Ended" : v && v.closed === "not_started" ? "Starts " + new Date(p.start_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : v && v.closed === "all_claimed" ? "Every free month has been claimed" : v && v.paused ? "Paused: top up your fees wallet" : "Running";
    $("prNow").innerHTML = `${escHtml(p.label)}<div class="sub">${escHtml(state)} · ${PR.claims_count} claimed${p.max_claims ? ` of ${p.max_claims}` : ""} · ends ${escHtml(new Date(p.end_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }))}<br>${escHtml(v ? v.requirement : "")}</div>`;
  }
  const per = PR.cost_per_claim_sompi, vault = PR.vault_per_claim_sompi;
  const tokenRule = [...$("prNeeds").querySelectorAll(".prKind")].some((x) => x.value === "token" || x.value === "kcc20");
  $("prCost").textContent = (tokenRule ? "Tokens can be moved to a new wallet to claim again, so a token requirement needs a limit. " : "") + `Each free month: about ${kas(vault)} goes into your vault (so it comes back to you), ${kas(PR.deposit_sompi)} becomes the member's refundable deposit, plus a tiny network fee. Your fees wallet pays it, and keeps ${kas(PR.spare_sompi)} spare for fees.`;
  const bal = PR.keeper_sompi;
  $("prKeeper").innerHTML = `<b>Fees wallet</b> (it sends the free months and pays auto-renewal fees)<br><span class="mono">${escHtml(PR.keeper)}</span> <button class="ghost" id="prCopy" style="padding:4px 10px;font-size:12px">Copy</button>` +
    `<br>Balance <b>${bal == null ? "unknown" : kas(bal)}</b>${bal != null ? `, enough for about ${PR.claims_affordable} free month${PR.claims_affordable === 1 ? "" : "s"}` : ""} (${kas(PR.keeper_floor_sompi)} always stays for auto-renewal fees). To top it up, send ${unit} to this address from your wallet (for example ${Math.ceil((per * 25 + PR.keeper_floor_sompi) / KAS)} ${unit} for 25 free months). Claims pause by themselves when it runs low, and you get a message.` +
    (PR.can_discount ? "" : `<br><span style="color:var(--bad)">Free months can't be turned on for your plans yet. Message KasPass.</span>`);
  $("prCopy").onclick = () => navigator.clipboard.writeText(PR.keeper).then(() => { $("prCopy").textContent = "Copied"; setTimeout(() => ($("prCopy").textContent = "Copy"), 1500); }).catch(() => {});
  const t = $("prClaims");
  t.replaceChildren();
  if (PR.claims.length) {
    t.innerHTML = "<tr><th>When</th><th>Wallet</th><th>NFTs used</th><th>Transaction</th></tr>";
    for (const c of PR.claims) {
      const tr = document.createElement("tr");
      for (const x of [new Date(c.at_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }), c.wallet.slice(0, 14) + "…" + c.wallet.slice(-6), (c.nfts || []).join(", ") || "none", c.txid.slice(0, 12) + "…"]) {
        const td = document.createElement("td"); td.textContent = x; tr.append(td);
      }
      tr.children[1].className = "mono"; tr.children[3].className = "mono";
      t.append(tr);
    }
  }
}
$("prNew").onchange = () => renderPromo();

$("prGo").onclick = async () => {
  say("prErr", ""); say("prStatus", "");
  const list = [...$("prNeeds").querySelectorAll(".prneed")].map((d) => {
    const kind = d.querySelector(".prKind").value, since = d.querySelector(".prSince").value;
    return {
      kind, tick: d.querySelector(".prTick").value.trim(), min: Number(d.querySelector(".prMin").value),
      ...(kind === "kcc20" ? { id: d.querySelector(".prId").value.trim() } : {}),
      ...((kind === "migrated" || kind === "burned") && since ? { since_ms: new Date(since + "T00:00:00").getTime() } : {}),
      ...(kind === "burned" ? { plain: d.querySelector(".prPlain").checked } : {}),
    };
  }).filter((n) => n.tick || n.id);
  if (!list.length) { say("prErr", "Add at least one thing holders must hold (a ticker and an amount)."); return; }
  const start_ms = new Date($("prStart").value).getTime(), end_ms = new Date($("prEnd").value).getTime();
  if (!(start_ms > 0 && end_ms > 0)) { say("prErr", "Pick a start and an end."); return; }
  try {
    const r = await fetch("/v1/creator/promo", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, label: $("prLabel").value, level: $("prLevel").value, needs: { match: $("prMatch").value, list }, start_ms, end_ms, max_claims: Number($("prMax").value) || 0, new_campaign: $("prNew").checked }) });
    if (!r.ok) throw new Error(await r.text());
    $("prNew").checked = false;
    say("prStatus", "Saved. Your page shows it now.");
    await loadPromo();
  } catch (e) { say("prErr", e.message || String(e)); }
};
$("prEndGo").onclick = async () => {
  if ($("prEndGo").dataset.armed !== "1") { $("prEndGo").dataset.armed = "1"; $("prEndGo").textContent = "Click again to end it"; setTimeout(() => { $("prEndGo").dataset.armed = "0"; $("prEndGo").textContent = "End it now"; }, 5000); return; }
  say("prErr", ""); say("prStatus", "");
  try {
    const r = await fetch("/v1/creator/promo/end", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    say("prStatus", "Ended. The plan's card is back to normal.");
    await loadPromo();
  } catch (e) { say("prErr", e.message || String(e)); }
};


// ---- Alerts: a direct message when someone gets a pass ------------------------
let AL = null;
async function loadAlerts() {
  try {
    const r = await fetch("/v1/creator/alerts?session=" + encodeURIComponent(session));
    if (!r.ok) { show("alertsPanel", false); return; }
    AL = await r.json();
  } catch { show("alertsPanel", false); return; }
  show("alertsPanel", true);
  $("alNew").checked = AL.settings.new_passes;
  $("alRenew").checked = AL.settings.renewals;
  const tg = AL.telegram, dc = AL.discord;
  const linked = (tg && tg.linked) || (dc && dc.linked);
  $("alStatus").innerHTML = !tg && !dc
    ? `<b>Not set up yet.</b> Message KasPass to switch on Telegram for your site, then come back here to connect.`
    : linked
      ? `<span class="pill ok">On</span> Alerts go to your ${tg && tg.linked ? `Telegram (@${escHtml(tg.bot)})` : "Discord"} as a direct message.`
      : `<span class="pill no">Not connected</span> Press <b>Connect my Telegram</b>, then press <b>Start</b> in the chat with @${escHtml(tg ? tg.bot : "")}.`;
  show("alLink", !!tg && !(tg && tg.linked));
  show("alTest", !!linked);
}
$("alLink").onclick = async () => {
  say("alErr", "");
  try {
    const r = await fetch("/v1/telegram/link", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    const { url } = await r.json();
    window.open(url, "_blank", "noopener");
    say("alSaved", "Telegram opened. Press Start there, then come back: this box updates in a few seconds.");
    for (let i = 0; i < 20; i++) { await new Promise((res) => setTimeout(res, 3000)); await loadAlerts(); if (AL.telegram && AL.telegram.linked) { say("alSaved", "Connected."); break; } }
  } catch (e) { say("alErr", e.message || String(e)); }
};
$("alTest").onclick = async () => {
  say("alErr", "");
  try {
    const r = await fetch("/v1/creator/alerts/test", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    say("alSaved", "Test sent. Check your messages.");
  } catch (e) { say("alErr", e.message || String(e)); }
};
for (const id of ["alNew", "alRenew"]) {
  $(id).onchange = async () => {
    say("alErr", "");
    try {
      const r = await fetch("/v1/creator/alerts", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, new_passes: $("alNew").checked, renewals: $("alRenew").checked }) });
      if (!r.ok) throw new Error(await r.text());
      say("alSaved", "Saved.");
    } catch (e) { say("alErr", e.message || String(e)); }
  };
}


// ---- Who can see the page: everyone, or only people with a private link -------
let VIS = null;
const pagePath = () => (document.querySelector('.sitebar a[data-c="m"]') || {}).getAttribute?.("href")?.split("?")[0] || "/";
function visLinks() {
  // "Open my page" links carry the private key so the creator can still open it.
  document.querySelectorAll('a[href="/"], a[href="/demo"], a[href^="/?k="], a[href^="/demo?k="]').forEach((a) => {
    const base = a.getAttribute("href").split("?")[0];
    a.setAttribute("href", VIS && VIS.private ? `${base}?k=${encodeURIComponent(VIS.key)}` : base);
  });
}
async function loadVis() {
  try {
    const r = await fetch("/v1/creator/visibility?session=" + encodeURIComponent(session));
    if (!r.ok) { show("visPanel", false); return; }
    VIS = await r.json();
  } catch { show("visPanel", false); return; }
  show("visPanel", true);
  $("visPublic").checked = !VIS.private;
  $("visPrivate").checked = VIS.private;
  show("visLinkBox", VIS.private);
  if (VIS.private) $("visLink").textContent = `${location.origin}${pagePath()}?k=${VIS.key}`;
  visLinks();
}
async function setVis(priv, fresh) {
  say("visErr", ""); say("visSaved", "");
  try {
    const r = await fetch("/v1/creator/visibility", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, private: priv, new_link: !!fresh }) });
    if (!r.ok) throw new Error(await r.text());
    await loadVis();
    say("visSaved", priv ? (fresh ? "New private link made." : "Your page is now private: only people with the link can open it.") : "Your page is public again.");
  } catch (e) { say("visErr", e.message || String(e)); }
}
$("visPublic").onchange = () => setVis(false);
$("visPrivate").onchange = () => setVis(true);
$("visNew").onclick = () => setVis(true, true);
$("visCopy").onclick = () => navigator.clipboard.writeText($("visLink").textContent).then(() => { $("visCopy").textContent = "Copied"; setTimeout(() => ($("visCopy").textContent = "Copy"), 1500); }).catch(() => {});
