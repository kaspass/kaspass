// Stream mode: hides private details while the screen is shared or recorded. Wallet addresses,
// .kas and .k names, transaction ids, KAS and money amounts, dates, times and time left, member
// names and notes, group names and counts turn into grey bars of one width. One switch (in the
// dashboard's side menu), remembered in this browser and shared by the dashboard, the membership
// page and the holder club page of the same site. `?stream=1` in a link turns it on; no link can
// turn it off.
//
// A box a page marks data-public (a plan's name, price and lengths in the create form, which end up
// on the public page anyway) stays readable, so a creator can see what they typed on air (102, RH101).
//
// A hidden detail shows when it's clicked (and the Reveal button beside an address does the same),
// and hides again by itself after 10 seconds. A box you're typing in shows while you type.
// Turning stream mode off takes two separate presses, so one slip (or a double-click) on a live
// stream can't show everything.
//
// Loaded in <head>, before the page draws, so nothing private is ever painted first: new text is
// wrapped as soon as it's added (a MutationObserver runs before the browser paints). The page's
// boot script hides the page until this file has started, so if it ever fails to load, the page
// stays blank instead of showing everything.
//
// Reviewed by Fable on Nov 23, 2026 (claude/kaspass-fable-stream-mode-review-92.md); the findings
// S1 to S21 are covered here and in the pages that mark their own private details.
(() => {
  "use strict";
  const KEY = "kaspass.stream";
  const root = document.documentElement;
  let on = false;
  try { on = localStorage.getItem(KEY) === "1"; } catch {}
  // A link can turn it on, never off (S7). The parameter leaves the address bar either way.
  try {
    const u = new URL(location.href);
    if (u.searchParams.has("stream")) {
      if (u.searchParams.get("stream") === "1") { on = true; try { localStorage.setItem(KEY, "1"); } catch {} }
      u.searchParams.delete("stream");
      history.replaceState(history.state, "", u.pathname + u.search + u.hash);
    }
  } catch {}
  root.classList.toggle("stream", on);

  // ---- What counts as private text ---------------------------------------------------------
  const MON = "\\b(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|June?|July?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\\.?";
  // French and German month names, for browsers set to those languages (S19).
  const MON_FD = "(?:janv|févr|mars|avr|mai|juin|juil|août|sept|oct|nov|déc|Jan|Feb|Mär|Apr|Mai|Jun|Jul|Aug|Sep|Okt|Nov|Dez)[a-zéû]*\\.?";
  const TIME = "\\d{1,2}:\\d{2}(?::\\d{2})?(?:\\s?[AaPp]\\.?\\s?[Mm]\\.?)?";
  // A number with thousands separators (commas, spaces, or the no-break spaces French uses).
  const NUM = "\\d(?:[\\d,\\u00a0\\u202f]|\\s(?=\\d{3}\\b))*(?:[.,]\\d+)?";
  const CUR = "(?:[A-Z]{1,3}\\$|\\$|€|£)";
  const UNIT = "(?:seconds?|minutes?|hours?|days?|weeks?|months?|years?)";
  const PARTS = [
    "kaspa(?:test)?:[a-z0-9]{3,}(?:(?:…|\\.\\.\\.)[a-z0-9]{2,})?", // an address, whole or shortened
    "\\b[a-z0-9]{2,16}(?:…|\\.\\.\\.)[a-z0-9]{2,10}\\b", // a shortened address without its prefix
    "(?:…|\\.\\.\\.)[a-z0-9]{3,10}\\b", // just an address's last letters ("…mp7v")
    "\\b(?:ending|ends)\\s(?:in\\s)?[a-z0-9]{4}\\b", // "the wallet ending in cmd3" (S21)
    "\\b(?:mig|burn):[0-9a-f]{6,}", // a KAT Bridge migration or burn
    "\\b[0-9a-f]{10,}…?", // a transaction or covenant id
    "[\\p{L}\\p{N}\\p{Extended_Pictographic}_-]+\\.(?:kas|k|kachat|kaspa)\\b", // a .kas, .k, .kachat or .kaspa name, digits and emoji too (S6)
    "@[A-Za-z0-9_]{3,32}\\b", // a Telegram or X handle
    CUR + "\\s?" + NUM, // CA$12.50, $12, €12
    NUM + "\\s?" + CUR, // 12,50 $ (French)
    NUM + "\\s?(?:test\\s)?(?:KAS|CAD|USD|EUR|sompi)\\b", // 1,250 KAS, 12 CAD
    MON + "\\s\\d{1,2}(?:,?\\s\\d{4})?(?:,?\\s(?:at\\s)?" + TIME + ")?", // Nov 18, 2026, 6:09 PM
    "\\b\\d{1,2}\\.?\\s" + MON + "(?:\\s\\d{4})?", // 18 Nov 2026
    "\\b\\d{1,2}\\.?\\s" + MON_FD + "(?:\\s\\d{4})?(?:,?\\s\\d{1,2}\\s?h\\s?\\d{2})?", // 22 nov. 2026, 14. Okt. 2026
    "\\b\\d{4}-\\d{2}-\\d{2}(?:[T\\s]" + TIME + ")?", // 2026-11-18
    "\\b\\d{1,2}/\\d{1,2}/\\d{2,4}(?:,?\\s" + TIME + ")?", // 11/18/2026
    "\\b\\d{1,2}\\.\\d{1,2}\\.\\d{2,4}\\b", // 23.11.2026
    "\\b\\d{1,2}\\s?h\\s?\\d{2}\\b", // 13 h 36
    "\\b" + TIME, // 6:09 PM
    // Time left or since, which gives away a date (S5).
    "\\b\\d+\\s" + UNIT + "\\s(?:left|ago)\\b", // 12 days left, 3 hours ago
    "\\bunder a minute left\\b",
    "\\b\\d+[dhms](?:\\s\\d+[dhms])+(?:\\sago)?\\b", // 12d 3h, 12m 20s ago
    "\\b(?:in|for)\\s\\d+\\s" + UNIT + "\\b", // in 3 days
  ];
  const RX = new RegExp(PARTS.join("|"), "gu");
  const test = (s) => { RX.lastIndex = 0; const r = RX.test(s); RX.lastIndex = 0; return r; };

  // Whole elements that are private even without a pattern: big numbers drawn without a unit,
  // counts, people's names and group names, address chips and QR codes. Pages mark anything else
  // with class="kp-priv".
  const PRIV = [
    ".kp-priv", ".chip", "#sideAddr", "#addr",
    // dashboard figures and counts
    ".stats .v", ".fact .v", ".vault .amt", ".vault dd", "#navMembers", "#navVault", "#byPlanKey .n", "#byPlanBar",
    // people and groups (S3, S6, S13)
    ".invname", "#members .name", "#notesList .who", "#notesList .reach", "#notesList .msgtext", "#notesList .muted",
    "#tgLinks td:first-child", "#tgLinks td:last-child", "#dcLinks td:first-child", "#dcLinks td:last-child",
    "#tgStatus b", "#dcStatus b", "#tgPill", "#dcPill",
    // promotion claims: which NFTs counted (S18)
    "#prClaims td:nth-child(3)",
    // the membership page's big KAS price ("362" beside a separate "KAS")
    ".kasn",
    // holder club figures (S17)
    "#clubStats", "#bandLine", "#listedNote", "#walletList .small",
    // QR codes hold an address and an amount (S1)
    "#qrImg", "#wcImg", ".kpm-qr", "#qrOpen",
  ].join(",");
  const HIDE = `:is(.sx, ${PRIV}):not(input, textarea, select, option)`;
  const BAR = "#80868f";

  // The text is made see-through AND covered by a box, so a contrast theme or a colour-changing
  // extension that forces text colours still shows only the box (S10). Bars are one width, so their
  // length doesn't give away how long the number or name is (S20).
  const BOX = `content: ""; position: absolute; inset: 0; background: ${BAR}; border-radius: 4px; forced-color-adjust: none; pointer-events: none; z-index: 1;`;
  const FIELDS = "input:not([type=checkbox], [type=radio], [type=file], [type=button], [type=submit], [type=range], [type=color]), textarea";
  const css = `
.kp-reveal, #kpStream { display: none; }
html.stream .kp-reveal { display: inline-flex; }
html.stream ${HIDE}:not(.kp-shown):not(:empty) { position: relative; color: transparent !important; -webkit-text-fill-color: transparent !important; text-shadow: none !important; user-select: none; cursor: pointer; }
html.stream ${HIDE}:not(.kp-shown):not(:empty)::after { ${BOX} }
html.stream .sx:not(.kp-shown) { display: inline-block; width: 7ch; overflow: hidden; vertical-align: bottom; white-space: nowrap; }
html.stream ${HIDE}:not(.kp-shown) * { color: transparent !important; -webkit-text-fill-color: transparent !important; text-shadow: none !important; border-color: transparent !important; }
html.stream ${HIDE}:not(.kp-shown) .sx::after { display: none; }
html.stream ${HIDE}:not(.kp-shown) :is(img, svg, canvas) { visibility: hidden; }
html.stream :is(${FIELDS}):not(:focus):not(:placeholder-shown):not(.kp-shown):not([data-public]),
html.stream :is(input[type=date], input[type=datetime-local], input[type=time], input[type=number]):not(:focus):not(.kp-shown):not([data-public]),
html.stream select.kp-sx:not(:focus),
html.stream input.invlink { color: transparent !important; -webkit-text-fill-color: transparent !important; text-shadow: none !important; background-color: ${BAR} !important; forced-color-adjust: none; }
html.stream :is(input[type=date], input[type=datetime-local], input[type=time], input.invlink):not(.kp-shown)::-webkit-datetime-edit { color: transparent; }
html.stream :is(canvas, .qr) { visibility: hidden; }
html.stream #kpStream { display: flex; position: fixed; right: 14px; bottom: 14px; z-index: 2147483000; align-items: center; gap: 10px; padding: 6px 8px 6px 14px; border-radius: 999px; background: #15171a; color: #f2f4f6; border: 1px solid #3a3f46; box-shadow: 0 6px 20px rgba(0,0,0,.35); font: 500 13px/1.3 system-ui, -apple-system, "Segoe UI", sans-serif; max-width: calc(100vw - 28px); forced-color-adjust: none; }
html.stream #kpStream button { all: unset; cursor: pointer; padding: 5px 11px; border-radius: 999px; background: #2a2e34; color: #f2f4f6; font: 600 12px/1.2 system-ui, -apple-system, "Segoe UI", sans-serif; white-space: nowrap; }
html.stream #kpStream button:focus-visible { outline: 2px solid #8ab4ff; outline-offset: 2px; }
html.stream #kpStream button.armed { background: #8a2b2b; }
@media print { html.stream #kpStream { display: none; } }
`;
  const style = document.createElement("style");
  style.id = "kpStreamStyle";
  style.textContent = css;
  (document.head || root).appendChild(style);

  // ---- Wrapping private text in grey bars ----------------------------------------------------
  const SKIP_TAGS = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "TEXTAREA", "INPUT", "SELECT", "OPTION", "TITLE", "HEAD"]);
  // Editors are left alone: wrapping text while someone types moves the cursor and scrambles it (S15).
  const skipped = (el) => !el || SKIP_TAGS.has(el.tagName) || !!el.closest(".sx, [data-public], #kpStream, head, [contenteditable]");
  function wrapText(t) {
    const v = t.nodeValue;
    if (!v || v.length < 2 || skipped(t.parentElement)) return;
    RX.lastIndex = 0;
    let m, last = 0, frag = null;
    while ((m = RX.exec(v))) {
      if (!m[0]) { RX.lastIndex++; continue; }
      frag = frag || document.createDocumentFragment();
      if (m.index > last) frag.append(v.slice(last, m.index));
      const s = document.createElement("span");
      s.className = "sx";
      s.title = "Hidden for stream mode. Click to show it.";
      s.textContent = m[0];
      frag.append(s);
      last = m.index + m[0].length;
    }
    if (!frag) return;
    if (last < v.length) frag.append(v.slice(last));
    t.replaceWith(frag);
  }

  // Tooltips and link addresses that hold private details move aside while stream mode is on, so
  // hovering shows nothing and the browser's status bar shows no address (S2, S16).
  function guardAttrs(el) {
    if (el.nodeType !== 1 || el.closest("#kpStream")) return;
    if (el.hasAttribute("title") && el.className !== "sx" && test(el.getAttribute("title"))) {
      el.dataset.kpTitle = el.getAttribute("title");
      el.removeAttribute("title");
    }
    if (el.tagName === "A" && el.hasAttribute("href")) {
      const h = el.getAttribute("href");
      if (test(h) || /[?&](?:k|invite|code|session)=/.test(h) || /^(?:kaspa|kaspatest):/.test(h)) {
        el.dataset.kpHref = h;
        el.removeAttribute("href");
        el.setAttribute("role", "link");
        el.tabIndex = 0;
      }
    }
  }
  function unguardAll() {
    document.querySelectorAll("[data-kp-title]").forEach((el) => { el.setAttribute("title", el.dataset.kpTitle); delete el.dataset.kpTitle; });
    document.querySelectorAll("a[data-kp-href]").forEach((el) => { el.setAttribute("href", el.dataset.kpHref); delete el.dataset.kpHref; });
  }

  // A dropdown can't hold grey bars, so one with a private choice in it is covered whole.
  function markSelect(sel) {
    if (!sel) return;
    sel.classList.toggle("kp-sx", test([...sel.options].map((o) => o.text).join("\n")));
  }
  function scan(node) {
    if (!node) return;
    if (node.nodeType === 3) {
      const pe = node.parentElement;
      if (pe && (pe.tagName === "OPTION" || pe.tagName === "SELECT")) return markSelect(pe.closest("select"));
      return wrapText(node);
    }
    if (node.nodeType !== 1) return;
    if (node.tagName === "OPTION" || node.tagName === "SELECT") return markSelect(node.closest("select"));
    guardAttrs(node);
    node.querySelectorAll("[title], a[href]").forEach(guardAttrs);
    node.querySelectorAll("select").forEach(markSelect);
    if (skipped(node)) return;
    if (node.matches(".addrrow")) addReveal(node);
    node.querySelectorAll(".addrrow").forEach(addReveal);
    const w = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
    const list = [];
    for (let t = w.nextNode(); t; t = w.nextNode()) list.push(t);
    list.forEach(wrapText);
  }

  // ---- Showing one detail, for 10 seconds ----------------------------------------------------
  const SHOW_MS = 10000;
  function showFor(el, button) {
    const all = [el, ...el.querySelectorAll(".sx")];
    all.forEach((x) => x.classList.add("kp-shown"));
    clearTimeout(el._kpTimer);
    el._kpTimer = setTimeout(() => hideAgain(el, button), SHOW_MS);
  }
  function hideAgain(el, button) {
    clearTimeout(el._kpTimer);
    [el, ...el.querySelectorAll(".kp-shown")].forEach((x) => x.classList.remove("kp-shown"));
    if (button) { button.textContent = "Reveal"; button.setAttribute("aria-pressed", "false"); }
  }

  // A Reveal button beside each address row's Copy button (shown only in stream mode).
  function addReveal(row) {
    if (row.querySelector(".kp-reveal")) return;
    const chip = row.querySelector(".chip");
    if (!chip) return;
    const b = document.createElement("button");
    b.type = "button";
    b.className = "ghost sm kp-reveal";
    b.textContent = "Reveal";
    b.setAttribute("aria-pressed", "false");
    b.onclick = () => {
      if (chip.classList.contains("kp-shown")) return hideAgain(chip, b);
      showFor(chip, b);
      b.textContent = "Hide";
      b.setAttribute("aria-pressed", "true");
    };
    chip.after(b);
  }

  const mo = new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === "attributes") { guardAttrs(r.target); continue; }
      // New text inside a shown detail hides it again (a redraw with a new amount, S14).
      const host = (r.target.nodeType === 1 ? r.target : r.target.parentElement);
      const shown = host && host.closest && host.closest(".kp-shown");
      if (shown && !shown.classList.contains("sx")) hideAgain(shown);
      if (r.type === "characterData") wrapText(r.target);
      else r.addedNodes.forEach(scan);
    }
  });
  const OPTS = { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["title", "href"] };
  function watch(yes) {
    if (yes) {
      mo.observe(root, OPTS);
      if (document.body) scan(document.body);
    } else mo.disconnect();
  }

  // Clicking a hidden detail shows it for 10 seconds (the click does nothing else). Clicking it
  // again while it shows works as normal: a link opens, a button presses. A link whose address
  // was moved aside still opens when clicked.
  document.addEventListener("click", (e) => {
    if (!on || !e.target.closest) return;
    const el = e.target.closest(HIDE);
    if (el && !el.classList.contains("kp-shown") && !el.closest("#kpStream")) {
      e.preventDefault();
      e.stopPropagation();
      showFor(el);
      return;
    }
    const a = e.target.closest("a[data-kp-href]");
    if (a) {
      e.preventDefault();
      if (a.target === "_blank") window.open(a.dataset.kpHref, "_blank", "noopener");
      else location.href = a.dataset.kpHref;
    }
  }, true);
  // After a choice in a dropdown, it's covered again straight away (S14).
  document.addEventListener("change", (e) => { if (on && e.target.matches && e.target.matches("select.kp-sx")) e.target.blur(); }, true);

  // ---- The badge, and switching -------------------------------------------------------------
  let badge = null, armTimer = 0, armedAt = 0;
  function makeBadge() {
    if (badge || !document.body) return;
    badge = document.createElement("div");
    badge.id = "kpStream";
    badge.setAttribute("role", "status");
    badge.innerHTML = '<span>Stream mode: private details are hidden</span><button type="button">Turn off</button>';
    badge.querySelector("button").onclick = () => request(false);
    document.body.append(badge);
  }
  const listeners = [];
  function apply(v) {
    on = v;
    try { localStorage.setItem(KEY, v ? "1" : "0"); } catch {}
    root.classList.toggle("stream", v);
    if (!v) unguardAll();
    watch(v);
    disarm();
    listeners.forEach((f) => { try { f(v); } catch {} });
  }
  function disarm() {
    clearTimeout(armTimer);
    armTimer = 0;
    const b = badge && badge.querySelector("button");
    if (b) { b.textContent = "Turn off"; b.classList.remove("armed"); }
  }
  // Turning on is one press. Turning off needs a second press between 0.8 and 5 seconds after the
  // first, so a double-click or a double tap of Space doesn't count (S7).
  function request(v) {
    if (v === on) return true;
    if (v) { apply(true); return true; }
    if (armTimer) {
      if (Date.now() - armedAt < 800) return false;
      apply(false);
      return true;
    }
    armedAt = Date.now();
    const b = badge && badge.querySelector("button");
    if (b) { b.textContent = "Press again to show everything"; b.classList.add("armed"); }
    armTimer = setTimeout(disarm, 5000);
    listeners.forEach((f) => { try { f(on, "armed"); } catch {} });
    return false;
  }

  // A checkbox with data-stream-toggle (the dashboard's switch) follows and drives stream mode.
  function bindSwitches() {
    document.querySelectorAll("input[data-stream-toggle]").forEach((box) => {
      if (box.dataset.bound) return;
      box.dataset.bound = "1";
      box.checked = on;
      box.addEventListener("change", () => { if (!request(box.checked)) box.checked = on; });
    });
  }
  listeners.push((v, armed) => {
    document.querySelectorAll("input[data-stream-toggle]").forEach((box) => { box.checked = v; });
    document.querySelectorAll("[data-stream-hint]").forEach((el) => {
      el.textContent = armed ? "Press again to show everything." : v ? "It's on now." : "";
    });
  });

  // Turned on in another tab of the same site, or coming back to a page with Back: follow it
  // (only ever "on" this way, never "off"; S8).
  addEventListener("storage", (e) => { if (e.key === KEY && e.newValue === "1" && !on) apply(true); });
  addEventListener("pageshow", () => { let v = false; try { v = localStorage.getItem(KEY) === "1"; } catch {} if (v && !on) apply(true); });

  function start() {
    makeBadge();
    bindSwitches();
    listeners.forEach((f) => { try { f(on); } catch {} });
    if (on) watch(true);
    // The boot script hid the page until now (S9).
    root.classList.remove("kp-wait");
  }
  if (on) mo.observe(root, OPTS);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
  else start();

  window.KasPassStream = { get on() { return on; }, request, scan: (n) => scan(n || document.body) };
})();
