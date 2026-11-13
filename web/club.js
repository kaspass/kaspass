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
const esc = (t) => String(t ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const shortAddr = (a) => (a ? a.slice(0, 12) + "…" + a.slice(-4) : "");
const keyLabel = (k) => (k.startsWith("tier:") ? k.slice(5) : k === "pass" ? "pass holders" : k);
const say = (id, text) => { $(id).textContent = text || ""; show(id, !!text); };

let club = null;          // /v1/club
let session = null, address = null;
let kaspireNow = null, linking = false, linkCandidate = null;

// Keep the sign-in for this browser tab (so coming back from Discord doesn't sign you out).
const store = {
  get() { try { return JSON.parse(sessionStorage.getItem("kaspass.club") || "null"); } catch { return null; } },
  set(v) { try { v ? sessionStorage.setItem("kaspass.club", JSON.stringify(v)) : sessionStorage.removeItem("kaspass.club"); } catch {} },
};

const netName = () => (club && club.collection.network === "mainnet" ? "Mainnet" : club ? club.collection.network : "mainnet");
const wantedPrefix = () => (club && club.collection.network.startsWith("testnet") ? "kaspatest:" : "kaspa:");

// ---- The club and its tiers ---------------------------------------------

// The server describes rules in short phrases ("holds at least 1 NFT", "owns a Legacy OR 200+ credits").
// Turn each into a plain line people can read.
function phrase(p) {
  p = p.trim();
  let m;
  if ((m = p.match(/^holds at least (\d+) NFTs?$/))) return `Hold at least ${m[1]} ${club.collection.tick} NFT${m[1] === "1" ? "" : "s"}`;
  if ((m = p.match(/^(\d+)\+ credits$/))) return `Have ${m[1]} or more credits`;
  if ((m = p.match(/^owns a (.+)$/))) {
    const b = (club.bands || []).find((x) => x.name === m[1]);
    return `Own any ${m[1]} NFT` + (b ? ` (rank ${b.ranks[0]} to ${b.ranks[1]})` : "");
  }
  if ((m = p.match(/^owns one of (\d+) specific NFTs$/))) return `Own one of ${m[1]} specific NFTs`;
  if (p === "has an active KasPass pass") return "Hold an active paid KasPass pass";
  return p.charAt(0).toUpperCase() + p.slice(1);
}

function ways(rule) {
  const hasOr = rule.includes(" OR "), hasAnd = rule.includes(" AND ");
  if (hasOr && hasAnd) return `<ul class="ways"><li>${esc(phrase(rule))}</li></ul>`;
  const parts = rule.split(hasOr ? " OR " : " AND ").map(phrase);
  const joiner = hasOr ? "or" : "and";
  return `<ul class="ways">${parts.map((x, i) => (i ? `<li class="or">${joiner}</li>` : "") + `<li>${esc(x)}</li>`).join("")}</ul>`;
}

function perks(t) {
  const out = [];
  if (t.discord_role && club.discord && club.discord.ready) {
    out.push(`<div class="perk"><span class="k">Discord</span><span><b>${esc(t.discord_role)}</b> role<span class="sub">${club.discord.server ? `in the “${esc(club.discord.server)}” Discord server` : "in the club's Discord server"}</span></span></div>`);
  }
  for (const g of club.telegram || []) {
    if (g.key === "tier:" + t.name) out.push(`<div class="perk"><span class="k">Telegram</span><span><b>${esc(g.title)}</b><span class="sub">private group</span></span></div>`);
  }
  return out.length ? out.join("") : `<div class="soon">Groups for this level are coming soon.</div>`;
}

// mine = null before sign-in (no highlighting), or the tiers the member has.
function renderTiers(mine = null) {
  const signedIn = Array.isArray(mine);
  const have = club.tiers.filter((t) => signedIn && mine.includes(t.name)).map((t) => t.name);
  $("qualifyBanner").textContent = have.length ? `★ You qualify for ${have.length === club.tiers.length && have.length > 1 ? "every level" : have.length === 1 ? "1 level" : have.length + " levels"}: ${have.join(" and ")}` : "";
  show("qualifyBanner", have.length > 0);
  $("tiers").innerHTML = club.tiers.map((t) => {
    const ok = signedIn && mine.includes(t.name);
    const cls = ok ? " mine" : signedIn ? " notyet" : "";
    const tag = ok ? '<span class="check">✓ You qualify</span>' : signedIn ? '<span class="notyet-tag">Not yet</span>' : "";
    return `<div class="tier${cls}">
      <div class="name">${esc(t.name)}${tag}</div>
      <div class="sec">How to qualify</div>${ways(t.rule)}
      <div class="sec">What you get</div>${perks(t)}
    </div>`;
  }).join("");
}

// Rank groups that unlock a tier on their own (for example "own any Legacy" for Whale).
function instantTiers(band) {
  return club.tiers.filter((t) => t.rule.split(" OR ").some((p) => p.trim() === "owns a " + band)).map((t) => t.name);
}

async function loadClub() {
  try {
    club = await (await fetch("/v1/club")).json();
  } catch { club = { enabled: false }; }
  if (!club.enabled) {
    $("clubTitle").textContent = "No holder club here yet";
    $("clubLead").textContent = "This KasPass site doesn't run a holder club right now.";
    ["tiersPanel", "signPanel"].forEach((id) => show(id, false));
    return;
  }
  document.title = club.club + " · Holder club";
  $("clubTitle").textContent = club.club;
  $("clubLead").textContent = `A free club for holders of the ${club.collection.tick} collection. Sign in with your Kaspa wallet to see your level and join the private groups.`;
  $("pill").textContent = `Free · ${netName()}`;
  document.querySelectorAll(".netName").forEach((e) => (e.textContent = netName()));
  $("lookup").placeholder = wantedPrefix() + "...";
  const facts = ["Free, no payment", "Sign in by signature, nothing is sent"];
  if (!club.count_listed) facts.push("NFTs listed for sale don't count");
  facts.push("Several wallets? They all count");
  $("facts").innerHTML = facts.map((f) => `<li>${esc(f)}</li>`).join("");
  renderTiers();
  if (club.ranked && club.bands && club.bands.length) {
    $("bands").innerHTML = club.bands.map((b) => {
      const inst = instantTiers(b.name);
      const val = inst.length ? `<span class="val instant">Instant ${esc(inst.join(" + "))}</span>` + (b.credits ? `<div class="sub" style="text-align:right">plus ${b.credits} credits each</div>` : "")
        : `<span class="val">${b.credits} credit${b.credits === 1 ? "" : "s"} each</span>`;
      return `<div class="bandrow"><div><b>${esc(b.name)}</b><div class="sub">Rank ${b.ranks[0]} to ${b.ranks[1]} · ${b.nfts} exist</div></div><div>${val}</div></div>`;
    }).join("");
    $("bandsLead").textContent = `Every ${club.collection.tick} NFT has a rank, from 1 (rarest) upward. Each NFT you hold adds the credits for its rank group, and your total across all your wallets decides your levels. Some rare groups unlock a level on their own.`;
    show("bandsBox");
  }
}

// ---- Kaspire ---------------------------------------------------------------

let MOBILE = null;
const isPhone = /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
async function detectKaspire(timeoutMs = 3000) {
  if (MOBILE) return MOBILE;
  if (window.kaspire?.isKaspire) return window.kaspire;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(isPhone ? "On a phone, sign in with the Kaspire app. Tap the button below." : "We couldn't find the Kaspire wallet in this browser. Install it (free) from kaspire.kaslab.space, then reload this page. Or use the Kaspire app on your phone.")), timeoutMs);
    window.addEventListener("kaspire#initialized", () => { clearTimeout(t); resolve(window.kaspire); }, { once: true });
  });
}

let listening = false;
function watchAccounts(k) {
  if (listening || typeof k.on !== "function") return;
  listening = true;
  k.on("accountsChanged", (acc) => {
    const now = (acc && acc[0]) || null;
    if (linking) { linkCandidate = now; $("linkSeen").textContent = now ? "Kaspire now shows: " + now : ""; return; }
    // Switching wallets or networks never signs you out; Kaspire reports every step of a switch.
    kaspireNow = now;
    walletNote();
  });
}

function walletNote() {
  const off = !!(session && kaspireNow && kaspireNow !== address);
  say("walletNote", off ? `Kaspire is on ${shortAddr(kaspireNow)} now. You're still signed in as ${shortAddr(address)}.` : "");
}

async function currentAccount(k, ask) {
  let a = null;
  try { a = (await k.request({ method: "getAccounts" }))?.[0] || null; } catch {}
  if (!a && ask) a = (await k.request({ method: "requestAccounts" }))?.[0] || null;
  return a;
}

$("signin").onclick = async () => {
  say("signErr", "");
  $("signin").disabled = true;
  $("signin").textContent = "Check Kaspire…";
  try {
    const k = await detectKaspire();
    watchAccounts(k);
    let a = await currentAccount(k, true);
    if (!a) throw new Error("Kaspire didn't share a wallet. Open Kaspire, approve the connection, then try again.");
    if (!a.startsWith(wantedPrefix())) {
      // Kaspire may still be on testnet: its approval is per network, so ask once more.
      a = (await k.request({ method: "requestAccounts" }))?.[0] || a;
      if (!a.startsWith(wantedPrefix())) throw new Error(`Switch Kaspire to ${netName()} first (your address should start with "${wantedPrefix()}"), then press Sign in again.`);
    }
    const { nonce } = await (await fetch("/v1/nonce")).json();
    const message = ["Sign in to KasPass", "Domain: " + location.host, "Address: " + a, "Nonce: " + nonce, "Issued At: " + new Date().toISOString()].join("\n");
    const signed = await k.request({ method: "signMessage", params: { address: a, message } });
    const r = await fetch("/v1/login", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ address: a, message, signature: signed.signature }) });
    if (!r.ok) throw new Error(await r.text());
    const out = await r.json();
    session = out.session; address = a; kaspireNow = a;
    store.set({ session, address });
    signedIn();
    await renderMe();
  } catch (e) {
    say("signErr", e.code === 4001 ? "Cancelled in Kaspire. Press Sign in to try again." : String(e.message || e).replace(/^./, (c) => c.toUpperCase()));
  } finally {
    $("signin").disabled = false;
    $("signin").textContent = "Sign in with Kaspire";
  }
};

function signedIn() {
  $("addr").textContent = address ? "Signed in as the wallet ending in " + address.slice(-4) : "";
  show("addr", !!session);
  const phone = isPhone && !window.kaspire;
  show("signin", !session && !phone);
  show("wcGo", !session && !!window.KaspireMobile && !(phone && /iPhone|iPad|iPod/i.test(navigator.userAgent)));
  $("wcGo").classList.toggle("ghost", !phone);
  $("wcGo").textContent = phone ? "Sign in with the Kaspire app" : "Use the Kaspire app on my phone instead";
  $("getWallet").innerHTML = session ? "" : phone
    ? (/iPhone|iPad|iPod/i.test(navigator.userAgent) ? `The Kaspire app isn't on iPhone yet. Open this page on a computer and use the free <a href="https://kaspire.kaslab.space" target="_blank" rel="noopener">Kaspire browser extension</a>.` : `Don't have it yet? <a href="https://kaspire.kaslab.space" target="_blank" rel="noopener">Get the free Kaspire app</a>, then come back to this page.`)
    : `No wallet yet? <a href="https://kaspire.kaslab.space" target="_blank" rel="noopener">Get Kaspire for your browser</a> (free).`;
  show("logout", !!session);
  walletNote();
}

// The Kaspire app on a phone (WalletConnect): pair, then the same sign-in as above.
$("wcGo").onclick = async () => {
  say("signErr", "");
  $("wcGo").disabled = true;
  try {
    MOBILE = await window.KaspireMobile.connect(wantedPrefix() === "kaspa:" ? "mainnet" : "testnet-10", {});
    await $("signin").onclick();
    if (!session) MOBILE = null;
  } catch (e) {
    MOBILE = null;
    say("signErr", e.code === 4001 ? "Cancelled in the Kaspire app. Press the button to try again." : String(e.message || e).replace(/^./, (c) => c.toUpperCase()));
  } finally {
    $("wcGo").disabled = false;
  }
};

const endSession = (t) => { if (t) fetch("/v1/logout", { method: "POST", headers: { "x-kaspass-session": t } }).catch(() => {}); };
$("logout").onclick = () => {
  MOBILE = null;
  endSession(session);
  session = null; address = null; store.set(null);
  signedIn();
  show("you", false);
  renderTiers();
};

// ---- Your standing -------------------------------------------------------

async function renderMe() {
  if (!session) return;
  say("youErr", "");
  const r = await fetch("/v1/club/me?session=" + encodeURIComponent(session));
  if (r.status === 401) { $("logout").onclick(); say("signErr", "Your sign-in expired. Please sign in again."); return; }
  if (!r.ok) { show("you"); say("youErr", await r.text()); return; }
  const me = await r.json();
  const st = me.standing;
  show("you");
  if (st) {
    $("credits").textContent = st.credits;
    $("nfts").textContent = st.tokens;
    $("nftsLabel").textContent = st.tokens === 1 ? "NFT counted" : "NFTs counted";
    $("tierCount").textContent = st.tiers.length;
    $("tierLabel").textContent = st.tiers.length === 1 ? "level" : "levels";
    const bands = st.bands.filter((b) => b[1] > 0).map((b) => `${b[1]} ${esc(b[0])}`).join(" · ");
    $("bandLine").innerHTML = bands ? "By rank: " + bands : "";
    say("listedNote", st.listed && !club.count_listed ? `${st.listed} more ${st.listed === 1 ? "is" : "are"} listed for sale and won't count until you take ${st.listed === 1 ? "it" : "them"} off the market.` : "");
    renderTiers(st.tiers);
  }
  $("walletList").innerHTML = me.wallets.map((w, i) => {
    const h = st && st.wallets.find((x) => x.address === w);
    const other = !w.startsWith(wantedPrefix());
    const info = other ? "other network, not counted" : h ? `${h.tokens} NFT${h.tokens === 1 ? "" : "s"}` : "";
    return `<div><span class="mono">${esc(shortAddr(w))}</span><span class="small muted">${esc(info)} ${i === 0 ? "· signed in" : `· <a href="#" data-unlink="${esc(w)}">remove</a>`}</span></div>`;
  }).join("");
  document.querySelectorAll("[data-unlink]").forEach((a) => (a.onclick = async (e) => {
    e.preventDefault();
    await fetch("/v1/wallets/unlink", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, address: a.dataset.unlink }) });
    renderMe();
  }));

  // Where you can go: only groups and roles for tiers you actually have.
  const mine = new Set(me.access || []);
  const dc = club.discord && club.discord.ready && club.tiers.some((t) => t.discord_role && mine.has(t.name));
  show("dcBox", !!dc);
  if (dc) {
    const roles = club.tiers.filter((t) => t.discord_role && mine.has(t.name)).map((t) => t.discord_role);
    $("dcNote").textContent = `Sign in with Discord to get ${roles.join(" and ")}${club.discord.server ? " in " + club.discord.server : ""}. Roles update by themselves if your holdings change.`;
  }
  const groups = (club.telegram || []).filter((g) => mine.has(keyLabel(g.key)));
  show("tgBox", groups.length > 0);
  if (groups.length) $("tgNote").textContent = `Opens the KasPass bot, which links your Telegram to this wallet and sends a one-time invite to ${groups.map((g) => g.title).join(" and ")}.`;
  show("noTier", !me.has_access);
}

$("dcBtn").onclick = async () => {
  say("dcErr", "");
  try {
    const r = await fetch("/v1/discord/link", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    location.href = (await r.json()).url;
  } catch (e) { say("dcErr", e.message || String(e)); }
};

$("tgBtn").onclick = async () => {
  say("tgErr", "");
  try {
    const r = await fetch("/v1/telegram/link", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session }) });
    if (!r.ok) throw new Error(await r.text());
    window.open((await r.json()).url, "_blank", "noopener");
    $("tgNote").textContent = "Telegram should open now. Press Start in the chat with the bot. The link works for 10 minutes; press the button again for a new one.";
  } catch (e) { say("tgErr", e.message || String(e)); }
};

// ---- Linking more wallets -----------------------------------------------

$("addWallet").onclick = () => { linking = true; linkCandidate = null; $("linkSeen").textContent = ""; show("linkBox"); show("addWallet", false); };
$("linkCancel").onclick = () => { linking = false; if (linkCandidate) { kaspireNow = linkCandidate; walletNote(); } show("linkBox", false); show("addWallet"); };
$("linkSign").onclick = async () => {
  say("youErr", "");
  try {
    const k = await detectKaspire();
    watchAccounts(k);
    let a = linkCandidate;
    if (!a || a === address) a = await currentAccount(k, false);
    if (!a || a === address) a = (await k.request({ method: "requestAccounts" }))?.[0] || null;
    if (!a) throw new Error("Kaspire didn't share a wallet. Approve the connection in Kaspire and try again.");
    if (a === address) throw new Error("Kaspire still shows the wallet you signed in with. Switch to the other wallet in Kaspire first.");
    const { nonce } = await (await fetch("/v1/nonce")).json();
    const message = ["Link wallet to KasPass", "Domain: " + location.host, "Address: " + a, "Link to: " + address, "Nonce: " + nonce, "Issued At: " + new Date().toISOString()].join("\n");
    const signed = await k.request({ method: "signMessage", params: { address: a, message } });
    const r = await fetch("/v1/wallets/link", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ session, address: a, message, signature: signed.signature }) });
    if (!r.ok) throw new Error(await r.text());
    linking = false; kaspireNow = a;
    show("linkBox", false); show("addWallet");
    walletNote();
    await renderMe();
  } catch (e) {
    say("youErr", e.code === 4001 ? "Cancelled in Kaspire." : "Couldn't add that wallet: " + (e.message || e));
  }
};

// ---- Public preview ------------------------------------------------------

$("lookupBtn").onclick = async () => {
  const a = $("lookup").value.trim();
  const out = $("lookupOut");
  if (!a) return;
  out.textContent = "Checking…";
  try {
    const r = await fetch("/v1/club/check?address=" + encodeURIComponent(a));
    if (!r.ok) throw new Error(await r.text());
    const st = await r.json();
    const tiers = st.tiers.length ? st.tiers.join(", ") : "no level yet";
    out.innerHTML = `<b>${st.credits} credits</b> from ${st.tokens} NFT${st.tokens === 1 ? "" : "s"}${st.listed ? ` (${st.listed} listed, not counted)` : ""}. Levels: ${esc(tiers)}.`;
  } catch (e) { out.textContent = e.message || String(e); }
};
$("lookup").addEventListener("keydown", (e) => { if (e.key === "Enter") $("lookupBtn").click(); });

// ---- Start ---------------------------------------------------------------

(async () => {
  await loadClub();
  if (!club.enabled) return;
  const saved = store.get();
  if (saved && saved.session) {
    session = saved.session; address = saved.address;
    signedIn();
    await renderMe();
  } else signedIn();
  try {
    const k = await detectKaspire(1500);
    watchAccounts(k);
    kaspireNow = await currentAccount(k, false);
    walletNote();
  } catch {}
})();
