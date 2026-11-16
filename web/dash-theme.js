// Creator dashboard themes. Only the creator sees these; the membership page has its own look.
// One file format for the dashboard, the member page and Kaspa Vision:
// {"kaspass_theme":1,"name","light":{bg,text,accent,card},"dark":{...},"corners","head_font","body_font","chart":{"light":[4],"dark":[4]}}
// The dashboard uses the colours only. Fonts and corners stay KasPass's own.
// Saved in this browser (kp-dash-look = [saved_at_ms, look]) and with the creator's account
// (/v1/creator/prefs); the newer one wins. Day or night is apart, in kp-theme, shared with the
// public pages.
(function () {
"use strict";
const $ = (id) => document.getElementById(id);
const $$ = (s, r) => Array.from((r || document).querySelectorAll(s));
const root = document.documentElement;
const ico = (n) => `<svg class="i"><use href="#i-${n}"/></svg>`;
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

const T = (id, name, note, l, d) => ({ id, note, theme: { kaspass_theme: 1, name, light: { bg: l[0], text: l[1], accent: l[2], card: l[3] }, dark: { bg: d[0], text: d[1], accent: d[2], card: d[3] }, corners: "round", head_font: "", body_font: "" } });
const PRESETS = [
  T("kaspass", "KasPass", "The teal it comes with", ["#f2f5f4", "#0f2321", "#0b7a6e", "#ffffff"], ["#0b1615", "#f2f5f4", "#49d9c8", "#12211f"]),
  T("mono", "Mono", "Black and white, nothing else", ["#e4e4e2", "#141414", "#2a2a2a", "#f4f4f2"], ["#0d0d0d", "#ededeb", "#e8e8e6", "#181818"]),
  T("gold", "Gold", "Warm gold on deep brown", ["#efe4cb", "#2a1f0c", "#7a5200", "#f9f2e1"], ["#14100a", "#f4ead6", "#f0b93a", "#1f1810"]),
  T("glacier", "Glacier", "Ice blue on deep navy", ["#dbe6f1", "#0f1c2c", "#1d4f91", "#edf3f9"], ["#0a131f", "#e6eef8", "#7fb8ff", "#121e2e"]),
  T("ember", "Ember", "Orange on charcoal", ["#faf4ee", "#2a1810", "#b03d0e", "#ffffff"], ["#16100d", "#f4e9e0", "#ff8f52", "#211814"]),
  T("rose", "Rose", "Soft pink by day, dusk rose at night", ["#fbf3f4", "#2b1820", "#a92657", "#ffffff"], ["#171014", "#f5e8ec", "#ff8fb1", "#22171c"]),
];
// "My page's look": a copy of the colours from Your page, Look and brand, added once they're known.
let PAGE = null;
const presets = () => (PAGE ? [PRESETS[0], PAGE].concat(PRESETS.slice(1)) : PRESETS);
// Old ids keep working if a preset is ever renamed.
const ALIASES = { classic: "kaspass", sakura: "rose" };
const presetById = (id) => presets().find((p) => p.id === (ALIASES[id] || id));
const HEX = /^#[0-9a-f]{6}$/i, KEYS = ["bg", "card", "text", "accent"];
const clone = (o) => JSON.parse(JSON.stringify(o));

// mix(a, b, t): t = 0 is all a, 1 is all b. The same helper Kaspa Vision uses.
function mix(a, b, t) { const f = (h, i) => parseInt(h.slice(i, i + 2), 16); return "#" + [1, 3, 5].map((i) => Math.round(f(a, i) * (1 - t) + f(b, i) * t).toString(16).padStart(2, "0")).join(""); }
function lum(hex) { const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4))); return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]; }
function ratio(a, b) { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); }
const onColor = (hex) => (ratio(hex, "#ffffff") >= ratio(hex, "#0a0a0a") ? "#ffffff" : "#0a0a0a");
const chartOf = (t, m) => (t.chart && t.chart[m]) || [t[m].accent, mix(t[m].accent, t[m].text, 0.55), mix(t[m].text, t[m].bg, 0.3), mix(t[m].accent, t[m].bg, 0.45)];
// Text under 3 to 1 is refused. Under 4.5 it warns but can be saved. Buttons only warn.
function judge(p) {
  const out = { stop: [], warn: [] };
  const tb = ratio(p.text, p.bg), tc = ratio(p.text, p.card);
  if (tb < 3) out.stop.push("The text is too hard to read on this background.");
  else if (tb < 4.5) out.warn.push("The text is a little faint on this background. It works, but it's easier to read with more contrast.");
  if (tc < 3) { if (tb >= 3) out.stop.push("The text is too hard to read on the cards."); }
  else if (tc < 4.5 && tb >= 4.5) out.warn.push("The text is a little faint on the cards. It works, but it's easier to read with more contrast.");
  if (ratio(p.accent, p.bg) < 3) out.warn.push("The buttons are hard to see against this background.");
  else if (ratio(p.accent, p.card) < 2.5) out.warn.push("The buttons are hard to see on the cards.");
  return out;
}

// Reading a theme, from a file or from storage. Missing day or night is fine: the other is used for both.
function checkTheme(t) {
  const bad = (why) => ({ ok: false, why });
  if (!t || typeof t !== "object" || !("kaspass_theme" in t)) return bad("That isn't a theme file. Look for one saved from KasPass or Kaspa Vision.");
  if (t.kaspass_theme !== 1) return bad("That theme file is a newer kind than this page knows.");
  const pal = (p) => (p && KEYS.every((key) => HEX.test(p[key] || "")) ? Object.fromEntries(KEYS.map((key) => [key, p[key].toLowerCase()])) : null);
  if ((t.light && !pal(t.light)) || (t.dark && !pal(t.dark))) return bad("One of the colours in that file isn't a colour code like #49d9c8.");
  const light = pal(t.light), dark = pal(t.dark);
  if (!light && !dark) return bad("That theme file has no colours in it.");
  for (const p of [light, dark]) if (p && judge(p).stop.length) return bad("The text in that theme is too hard to read on its background, so it can't be used.");
  const theme = { kaspass_theme: 1, name: String(t.name || "My colours").replace(/[\u0000-\u001f\u007f]+/g, " ").trim().slice(0, 40) || "My colours", light: light || dark, dark: dark || light, corners: "round", head_font: "", body_font: "" };
  const four = (a) => Array.isArray(a) && a.length === 4 && a.every((c) => HEX.test(c));
  if (t.chart && (four(t.chart.light) || four(t.chart.dark))) theme.chart = { light: four(t.chart.light) ? t.chart.light : t.chart.dark, dark: four(t.chart.dark) ? t.chart.dark : t.chart.light };
  return { ok: true, theme };
}
// A saved look, from this browser or the account. Presets are re-read by id, so a fixed preset
// reaches everyone who uses it.
function readLook(l) {
  if (!l || typeof l !== "object" || typeof l.id !== "string") return null;
  const r = checkTheme(l.theme); if (!r.ok) return null;
  const out = { id: l.id, theme: r.theme, glow: l.glow !== false, pattern: l.pattern !== false };
  const p = l.id !== "page" && presetById(l.id);
  if (p) { out.id = p.id; out.theme = clone(p.theme); }
  return out;
}

const mq = window.matchMedia("(prefers-color-scheme: dark)");
let look = { id: "kaspass", theme: clone(PRESETS[0].theme), glow: true, pattern: true };
let savedAt = 0;
let mode = "device";          // "light", "dark" or "device"
try {
  const s = JSON.parse(localStorage.getItem("kp-dash-look") || "null");
  const l = Array.isArray(s) && readLook(s[1]);
  if (l) { look = l; savedAt = Number(s[0]) || 0; }
  const m = localStorage.getItem("kp-theme"); if (m === "light" || m === "dark") mode = m;
} catch (e) {}
let draft = null;             // { light, dark } while Make your own has unsaved changes
let peek = null;              // the palette Make your own is showing, if it isn't the current one
let session = null;           // set once the creator signs in
const liveMode = () => peek || (mode === "device" ? (mq.matches ? "dark" : "light") : mode);
const current = () => (draft ? { kaspass_theme: 1, name: "My colours", light: draft.light, dark: draft.dark, corners: "round", head_font: "", body_font: "" } : look.theme);
function storeMode() { try { if (mode === "device") localStorage.removeItem("kp-theme"); else localStorage.setItem("kp-theme", mode); } catch (e) {} }
function storeLocal() { try { localStorage.setItem("kp-dash-look", JSON.stringify([savedAt, look])); } catch (e) {} }
// Save the look here and with the account. Resolves to true when the account has it too.
async function store() {
  savedAt = Date.now(); storeLocal();
  if (!session) return false;
  try {
    const r = await fetch("/v1/creator/prefs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, look: [savedAt, look] }) });
    if (!r.ok) return false;
    // The server's own clock stamps the save, so a device whose clock is behind can't lose its newer choice.
    const j = await r.json().catch(() => ({}));
    if (Number(j.saved_at) > 0) { savedAt = Number(j.saved_at); storeLocal(); }
    return true;
  } catch (e) { return false; }
}
const NOT_SYNCED = "Saved on this device. It couldn't be saved to your account just now, so other devices still show the old colours.";

function applyTheme(animate) {
  const t = current(), m = liveMode(), p = t[m];
  if (animate) { root.classList.add("theming"); clearTimeout(applyTheme.t); applyTheme.t = setTimeout(() => root.classList.remove("theming"), 320); }
  KEYS.forEach((key) => root.style.setProperty("--" + key, p[key]));
  root.style.setProperty("--on-accent", onColor(p.accent));
  chartOf(t, m).forEach((c, i) => root.style.setProperty("--chart-" + (i + 1), c));
  root.dataset.tone = lum(p.bg) < 0.3 ? "dark" : "light";   // by the colour itself, so a dark "day" palette still gets dark-tone states
  root.dataset.glow = look.glow ? "on" : "off";
  root.dataset.pattern = "off";
  const meta = document.querySelector('meta[name="theme-color"]'); if (meta) meta.content = p.bg;
  $("nightSw").checked = m === "dark";
}

function tiny(t, m) {
  const p = t[m];
  return `<span class="tiny" style="--t-bg:${p.bg};--t-text:${p.text};--t-accent:${p.accent};--t-card:${p.card}"><span class="c"><i class="l1"></i><i class="b"></i><i class="l2"></i></span><span class="d">${chartOf(t, m).map((c) => `<i style="background:${c}"></i>`).join("")}</span></span>`;
}
function renderPresets() {
  const card = (id, t, note, on) => `<button type="button" class="preset" data-id="${id}" aria-pressed="${on}"><span class="duo">${tiny(t, "light")}${tiny(t, "dark")}</span><span class="pname">${esc(t.name)}${ico("check")}</span><span class="pnote">${esc(note)}</span></button>`;
  const own = look.id === "custom" || draft ? card("custom", current(), draft ? "Not saved yet" : "Your own four colours", look.id === "custom" && !draft) : "";
  $("thPresets").innerHTML = presets().map((p) => card(p.id, p.theme, p.note, !draft && look.id === p.id)).join("") + own;
  $$("#thPresets .preset").forEach((b) => (b.onclick = () => {
    if (b.dataset.id === "custom") { $("thOwn").open = true; $("thOwn").scrollIntoView({ block: "nearest" }); return; }
    const p = presetById(b.dataset.id);
    use(p.id, clone(p.theme));
  }));
}
// Picking a theme uses it and saves it in one go.
async function use(id, theme) {
  look = { id, theme, glow: look.glow, pattern: look.pattern }; draft = null; peek = null; refresh(true);
  say(`Using ${theme.name}.`);
  const ok = await store();
  if (look.id !== id || look.theme !== theme) return;   // another pick came in meanwhile
  say(ok ? `Using ${theme.name}. Saved, and your other devices use it too.` : session ? NOT_SYNCED : `Using ${theme.name}. Saved on this device.`);
}
let said = "";
function say(text) { said = text; $("thStatus").textContent = text; }

function renderEditor() {
  const t = current(), m = liveMode(), p = t[m], other = m === "dark" ? "light" : "dark";
  $$("#thPal button").forEach((b) => b.classList.toggle("on", b.dataset.p === m));
  $$("#thMode button").forEach((b) => b.classList.toggle("on", b.dataset.m === mode));
  $("thEditing").textContent = `Your colours for ${m === "dark" ? "night" : "day"}.`;
  $$("#thColors input").forEach((inp) => { if (document.activeElement !== inp) inp.value = p[inp.dataset.k]; });
  const j = judge(p), jo = judge(t[other]);
  const lines = j.stop.map((x) => `<div class="issue stop">${x}</div>`).concat(j.warn.map((x) => `<div class="issue">${x}</div>`));
  if (jo.stop.length) lines.push(`<div class="issue stop">Your ${other === "dark" ? "night" : "day"} colours have text that's too hard to read. Switch to ${other === "dark" ? "Night" : "Day"} above and fix them first.</div>`);
  $("thIssues").innerHTML = lines.join("") || `<div class="fine">${ico("check")}Easy to read.</div>`;
  $("thSave").disabled = !draft || j.stop.length > 0 || jo.stop.length > 0;
  $("thUndo").disabled = !draft;
  $("fxFor").textContent = "for " + (draft ? "your colours" : look.theme.name);
  $("fxGlow").checked = look.glow; $("fxPattern").checked = look.pattern;
  $("fxPatternRow").classList.add("hidden");
  $("thFile").value = JSON.stringify(current(), null, 2);
  $("thStatus").textContent = draft ? "Trying your own colours. Not saved yet." : said;
}
function refresh(animate) { applyTheme(animate); renderPresets(); renderEditor(); }

// Day or night is its own choice, apart from the theme. It stays on this device.
function setMode(m) { mode = m; peek = null; storeMode(); refresh(true); }
$$("#thMode button").forEach((b) => (b.onclick = () => setMode(b.dataset.m)));
$("nightSw").onchange = () => setMode($("nightSw").checked ? "dark" : "light");
$$("[data-mode-toggle]").forEach((b) => (b.onclick = () => setMode(liveMode() === "dark" ? "light" : "dark")));
mq.addEventListener && mq.addEventListener("change", () => { if (mode === "device") refresh(true); });
// The public pages share kp-theme: a change in another tab shows here too.
window.addEventListener("storage", (e) => {
  if (e.key === "kp-theme") { const m = e.newValue; mode = m === "light" || m === "dark" ? m : "device"; refresh(true); }
  if (e.key === "kp-dash-look") { try { const s = JSON.parse(e.newValue || "null"); const l = Array.isArray(s) && readLook(s[1]); if (l) { look = l; savedAt = Number(s[0]) || 0; draft = null; refresh(true); } } catch (err) {} }
});
// Inside Make your own: a small Day and Night switch, so both sets can be edited in one place.
$$("#thPal button").forEach((b) => (b.onclick = () => { peek = b.dataset.p; refresh(true); }));
$$("#thColors input").forEach((inp) => (inp.oninput = () => {
  let v = inp.value.trim().toLowerCase(); if (v && v[0] !== "#") v = "#" + v;
  if (!HEX.test(v)) return;
  if (!draft) draft = { light: clone(look.theme.light), dark: clone(look.theme.dark) };
  draft[liveMode()][inp.dataset.k] = v; refresh(false);
}));
$("thSave").onclick = () => use("custom", clone(current()));
$("thUndo").onclick = () => { draft = null; peek = null; refresh(true); $("thPicOut").innerHTML = ""; };
$("fxGlow").onchange = async () => { look.glow = $("fxGlow").checked; applyTheme(false); if (!(await store()) && session) say(NOT_SYNCED); };
$("fxPattern").onchange = async () => { look.pattern = $("fxPattern").checked; applyTheme(false); if (!(await store()) && session) say(NOT_SYNCED); };

// Hold to preview: the real Overview, in the theme, for as long as the button is held.
(function () {
  const b = $("thHold"), on = () => root.classList.add("peeking"), off = () => root.classList.remove("peeking");
  b.addEventListener("pointerdown", (e) => { e.preventDefault(); on(); });
  ["pointerup", "pointercancel", "blur"].forEach((ev) => window.addEventListener(ev, off));
  b.addEventListener("keydown", (e) => { if (e.key === " " || e.key === "Enter") { e.preventDefault(); on(); } });
  b.addEventListener("keyup", off);
  b.addEventListener("contextmenu", (e) => e.preventDefault());
})();

// Colours from a picture. All of it happens on this device: the picture is never uploaded.
$("thPicBtn").onclick = () => $("thPicIn").click();
$("thPicIn").onchange = () => {
  const f = $("thPicIn").files[0]; if (!f) return;
  // Read as a data: link: the page's security policy allows those for pictures, not blob: links.
  const rd = new FileReader();
  rd.onerror = () => { $("thPicOut").innerHTML = `<div class="err" style="margin:0">${ico("warn")}<span>That picture couldn't be opened. Try a PNG or a JPEG.</span></div>`; };
  rd.onload = () => pictureColours(String(rd.result));
  rd.readAsDataURL(f); $("thPicIn").value = "";
};
function pictureColours(url) {
  const img = new Image();
  img.onload = () => {
    const c = document.createElement("canvas"); c.width = c.height = 64;
    const g = c.getContext("2d"); g.drawImage(img, 0, 0, 64, 64);
    const d = g.getImageData(0, 0, 64, 64).data, bins = {};
    for (let i = 0; i < d.length; i += 4) {
      if (d[i + 3] < 128) continue;
      const key = (d[i] >> 4) + "," + (d[i + 1] >> 4) + "," + (d[i + 2] >> 4), o = bins[key] || (bins[key] = [0, 0, 0, 0]);
      o[0]++; o[1] += d[i]; o[2] += d[i + 1]; o[3] += d[i + 2];
    }
    let best = null, bestScore = -1;
    Object.values(bins).forEach(([n, r, gg, bl]) => {
      r /= n; gg /= n; bl /= n;
      const mx = Math.max(r, gg, bl), mn = Math.min(r, gg, bl), sat = mx ? (mx - mn) / mx : 0, val = mx / 255;
      const score = Math.sqrt(n) * sat * sat * (val > 0.3 ? val : 0.05);   // vivid and bright beats big and dull
      if (score > bestScore) { bestScore = score; const up = Math.max(1, 150 / (mx || 1)); best = "#" + [r, gg, bl].map((v) => Math.min(255, Math.round(v * up)).toString(16).padStart(2, "0")).join(""); }
    });
    const seed = best || "#49d9c8";
    // Build both sets from the main colour, then nudge the button colour until it's easy to see.
    const night = { bg: mix("#080808", seed, 0.08), card: mix("#0e0e0e", seed, 0.14), text: mix("#ffffff", seed, 0.08), accent: seed };
    const day = { bg: mix("#ffffff", seed, 0.08), card: mix("#ffffff", seed, 0.02), text: mix("#0a0a0a", seed, 0.14), accent: seed };
    for (let i = 0; i < 20 && ratio(night.accent, night.bg) < 4.5; i++) night.accent = mix(night.accent, "#ffffff", 0.08);
    for (let i = 0; i < 20 && ratio(day.accent, day.bg) < 4.5; i++) day.accent = mix(day.accent, "#0a0a0a", 0.08);
    draft = { light: day, dark: night }; peek = null; refresh(true);
    $("thOwn").open = true;
    const out = $("thPicOut"); out.innerHTML = `<div class="picout"><img alt=""><span class="small">Found its colours and made them easy to read. They're in Make your own: adjust them, or press Use my colours to keep them.</span></div>`;
    out.querySelector("img").src = url;
  };
  img.onerror = () => { $("thPicOut").innerHTML = `<div class="err" style="margin:0">${ico("warn")}<span>That picture couldn't be opened. Try a PNG or a JPEG.</span></div>`; };
  img.src = url;
}

// Theme files.
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "theme";
$("thExport").onclick = () => {
  const t = current(), a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([JSON.stringify(t, null, 2)], { type: "application/json" }));
  a.download = slug(t.name) + ".kaspass-theme.json"; document.body.append(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 2000);
};
function importText(text) {
  let t; try { t = JSON.parse(text); } catch (e) { t = null; }
  const r = checkTheme(t), err = $("thFileErr");
  err.classList.toggle("hidden", r.ok);
  if (!r.ok) { err.innerHTML = ico("warn") + `<span>${esc(r.why)}</span>`; return; }
  use("custom", r.theme);
}
$("thImportBtn").onclick = () => $("thImport").click();
$("thImport").onchange = () => { const f = $("thImport").files[0]; if (!f) return; const rd = new FileReader(); rd.onload = () => importText(String(rd.result)); rd.readAsText(f); $("thImport").value = ""; };
$("thCopy").onclick = () => {
  const s = $("thCopy").querySelector("span"), done = () => { s.textContent = "Copied"; setTimeout(() => (s.textContent = "Copy"), 1400); };
  try { navigator.clipboard.writeText($("thFile").value).then(done, () => { $("thFile").select(); }); } catch (e) { $("thFile").select(); }
};

refresh(false);
say(`Using ${look.theme.name}.`);

// Called by dashboard.js.
window.kpTheme = {
  // Signed in as the creator: fetch the account's copy; the newer one wins.
  async signedIn(tok) {
    session = tok;
    let j = null;
    try { const r = await fetch("/v1/creator/prefs?session=" + encodeURIComponent(tok)); if (r.ok) j = await r.json(); } catch (e) {}
    if (!j) return;
    const s = j.look, at = Array.isArray(s) ? Number(s[0]) || 0 : 0, l = Array.isArray(s) && readLook(s[1]);
    if (l && at > savedAt) { look = l; savedAt = at; storeLocal(); if (!draft) refresh(true); say(`Using ${look.theme.name}.`); }
    else if (savedAt > at) {
      // This device changed the theme while signed out (or the last save didn't reach the account).
      try { await fetch("/v1/creator/prefs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ session, look: [savedAt, look] }) }); } catch (e) {}
    }
  },
  signedOut() { session = null; },
  // The page's own colours, from Look and brand. A copy: changing the page later doesn't change this.
  pageLook(brand, name) {
    const r = brand && (brand.light || brand.dark) ? checkTheme({ kaspass_theme: 1, name: "My page's look", light: brand.light, dark: brand.dark }) : { ok: false };
    PAGE = r.ok ? { id: "page", note: (name ? name + "'s colours" : "Your page's colours") + ", from Look and brand", theme: r.theme } : null;
    renderPresets();
  },
};
})();
