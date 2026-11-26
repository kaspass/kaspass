// "Get started" (update 101, planned in WK100): a guide for a new creator, not a tour. A card
// lists every step with its state in words, and a slim bar follows the creator across tabs while
// the guide is open. Nothing is blocked or dimmed, and the guide never presses a button or opens
// Kaspire: it points, the creator acts. Every tick comes from what the dashboard already loaded
// (no new requests, no polling); the browser keeps only a few small flags (open, put away,
// skipped steps, link shared). Loaded after dashboard.js, whose state it reads.
(() => {
  const KEY = "kaspass.walk";
  const testnet = () => !!(card && card.network !== "mainnet");
  const kasWord = () => (testnet() ? "test KAS" : "KAS");
  const isCreator = () => !!(card && session && address && address === card.creator_address);
  const live = () => !!(card && !card.placeholder);
  const hasKeeper = () => !(data && !(data.v4 && data.v4.keeper));
  const groupOff = () => !!(TG && DC && !TG.enabled && !DC.enabled);

  // ---- Flags, kept per wallet in this browser --------------------------------------------------
  // Read again on every redraw, so a second tab or a cleared browser agrees with what's stored.
  const load = () => { try { const f = JSON.parse(localStorage.getItem(KEY) || "null"); if (f && f.v === 1) return f; } catch {} return { v: 1 }; };
  let flags = load();
  const save = () => { try { localStorage.setItem(KEY, JSON.stringify(flags)); } catch {} };
  function forThisWallet() {
    const me = (address || "").slice(-6);
    if (flags.for === me) return;
    flags = { v: 1, for: me, open: null, dismissed: false, skipped: [], shared: false };
    // The old "I don't have a Telegram group" choice (92) carries over.
    try { if (localStorage.getItem("kaspass.noTelegram") === "1") flags.skipped.push("group"); } catch {}
    save();
  }

  // ---- The steps -------------------------------------------------------------------------------
  const STEPS = [
    {
      id: "payouts", group: "Set up", title: "Turn on payouts", tab: "payouts", target: "recSetup", focus: "recRecovery", ring: "#payPanel",
      words: () => `Members pay into your payout vault. Only your wallet can take money out. First choose a backup wallet: a second wallet of yours that can rescue your plans if you ever lose this one. <b>Take care with this one.</b> The backup wallet can't be swapped later from this dashboard. Keep its recovery words on paper, somewhere safe. Costs 1 ${kasWord()} once, plus a tiny network fee. Kaspire asks you to approve.`,
      bar: "Paste your backup wallet's address, then press Turn on payouts.",
      done: () => !!HAS_RECORD || live(),
    },
    {
      id: "plan", group: "Set up", title: "Create your first plan", tab: "levels", target: "launch", focus: "lName",
      words: () => `A plan is what people join. Give it a name, a price, and how often members pay. Some choices are locked in for good when you create it. You'll see them all in one list before anything is sent. Costs 1 ${kasWord()}, which stays locked in the plan, plus a tiny network fee.`,
      bar: "Fill in the name, the price and how often members pay. Check the locked-in list, then create it.",
      done: () => live() || !!PENDING,
    },
    {
      id: "sale", group: "Set up", title: "Put it on sale", tab: "levels", target: () => ($("lAdd") ? "lAdd" : "launch"), focus: () => ($("lAdd") ? "lAdd" : null),
      words: () => "Your plan exists, but nobody can buy it yet. Put it on sale to show it on your page. Your page is away for about a minute while it restarts. This screen reloads by itself. You can take a plan off sale any time.",
      bar: "Press Put it on sale, then press again to confirm.",
      done: () => live(),
    },
    {
      id: "who", group: "Make it yours", title: "Say who you are", tab: "page", sub: "words", target: "cName", focus: "cName", ring: "section",
      words: () => "Add the name people know you by and one line about what you offer. Visitors see both at the top of your page. Change them any time.",
      bar: "Type your name and a short tagline, then press Save page.",
      done: () => !!(data && data.creator_name),
    },
    {
      id: "perks", group: "Make it yours", title: "Say what members get", tab: "levels", target: "perks", focus: () => (data && data.level && !data.level.name ? "tName" : "perks"), ring: "section",
      words: () => "List what your plan includes, one per line. It shows on the plan's card on your page.",
      bar: "List what members get, then press Save.",
      done: () => !!(data && data.level && data.level.name && (data.level.perks || []).some((p) => p.trim())),
    },
    {
      id: "group", group: "Finish", title: "Connect your group", optional: true, tab: "groups", target: "tgStatus", ring: "section",
      words: () => groupOff()
        ? "Telegram and Discord aren't switched on for your page yet. Message KasPass and we'll turn them on."
        : TG && (TG.groups || []).some((g) => g.pending) ? "Your group is waiting. Approve it here if it's yours."
        : "Have a private Telegram group or a Discord server? KasPass lets paying members in by itself and removes them when their membership ends.",
      bar: "Follow the steps in the Telegram box, then approve your group here.",
      done: () => !!((TG && (TG.groups || []).some((g) => g.assigned)) || (DC && DC.ready)),
      off: groupOff,
    },
    {
      id: "keeper", group: "Finish", title: "Top up your fees wallet", optional: true, recommended: true, tab: "promos", target: "v4Panel", focus: "keeperCopy",
      words: () => `This small wallet pays the tiny network fee each time a membership renews by itself, and pays for free months you give away. It can't touch your money or your members' money. Send it 2 ${kasWord()} from Kaspire. That covers about 60 renewals.`,
      bar: `Copy the address, send it 2 ${testnet() ? "test KAS" : "KAS"} from Kaspire, then come back.`,
      done: () => !!(data && data.v4 && data.v4.keeper && data.v4.keeper.balance_sompi >= KAS),
      hide: () => !hasKeeper(),
    },
    {
      id: "share", group: "Finish", title: "See your page and share it", tab: "overview", target: "copyPage",
      words: () => "That's it. Open your page to see what visitors see, then copy the link and send it to your people.",
      bar: "Open your page, then copy the link and send it to your people.",
      done: () => !!((data && data.totals && data.totals.active > 0) || flags.shared),
    },
  ];
  window.WALK = STEPS;
  const val = (v) => (typeof v === "function" ? v() : v);
  const steps = () => STEPS.filter((s) => !(s.hide && s.hide()));
  const stateOf = (s) => (s.done() ? "done" : (s.optional && ((flags.skipped || []).includes(s.id) || (s.off && s.off()))) ? "skipped" : "todo");
  const STATE_WORDS = { done: "Done", todo: "To do", skipped: "Skipped" };
  function now() {
    const list = steps();
    const i = list.findIndex((s) => stateOf(s) === "todo");
    return { list, i, step: i >= 0 ? list[i] : null, done: list.filter((s) => stateOf(s) !== "todo").length };
  }
  // Steps people must do to have a page worth sharing (the group and fees wallet are extras).
  const essentialOpen = () => steps().some((s) => !s.optional && stateOf(s) === "todo");

  // ---- Drawing ---------------------------------------------------------------------------------
  let lastSig = "", lastStep = null, later = false;
  function draw() {
    const cardEl = $("walkCard"), bar = $("walkBar"), open = $("walkOpen");
    if (!isCreator()) { show("walkCard", false); show("walkBar", false); show("walkOpen", false); document.body.classList.remove("walking"); return; }
    flags = load();
    forThisWallet();
    const s = now();
    const N = s.list.length, finished = !s.step;
    // On a new site the guide opens by itself the first time; never once there are members.
    const members = !!(data && data.totals && data.totals.active > 0);
    if (flags.open == null) { flags.open = !live() && !flags.dismissed && !members; save(); }
    // The card: on a new site at the top (it's the step list); on a selling site on the Overview,
    // while the guide is open, or while something essential is left and nobody has joined yet.
    const slot = live() ? $("walkSlotOv") : $("notCreator").parentElement;
    if (live() && cardEl.parentElement !== slot) slot.append(cardEl);
    if (!live() && cardEl.parentElement !== slot) $("notCreator").before(cardEl);
    const cardOn = !flags.dismissed && (flags.open || (!finished && (!live() || (essentialOpen() && !members))));
    show("walkCard", cardOn || later);
    // Just put away with "Not now": the card says where to find it, until the next visit.
    for (const id of ["walkLead", "walkMeter", "walkList"]) show(id, !later);
    show("walkLaterMsg", later);
    show("walkBar", !!flags.open && !flags.dismissed);
    document.body.classList.toggle("walking", !!flags.open && !flags.dismissed);
    show("walkOpen", true);
    $("walkOpenN").textContent = finished ? "" : `${s.done} of ${N}`;

    // Card
    $("walkLead").textContent = finished
      ? "You're set up. When someone joins, they show in Members and their payment lands in your vault."
      : live() && s.done > 0 ? `${N - s.done} short ${N - s.done === 1 ? "step" : "steps"} left before your page is ready to share. Stop any time: your place is saved.`
      : `${N === 8 ? "Eight" : N === 7 ? "Seven" : N} short steps to a page you can share. About 10 minutes. Stop any time: your place is saved.`;
    $("walkNet").innerHTML = finished ? "" : testnet()
      ? `This is the test network. It uses free test KAS, so nothing here costs real money. Get some from the <a href="https://faucet-testnet.kaspanet.io" target="_blank" rel="noopener">test faucet</a>.`
      : "You'll need about 5 KAS in this wallet: 1 to turn on payouts, 1 for your plan, 2 for the fees wallet, and a little for network fees.";
    show("walkNet", !finished && !later);
    const m = $("walkMeter");
    m.setAttribute("aria-valuemax", String(N)); m.setAttribute("aria-valuenow", String(s.done)); m.setAttribute("aria-valuetext", `${s.done} of ${N} done`);
    m.querySelector("i").style.width = `${Math.round((s.done / Math.max(1, N)) * 100)}%`;
    const sig = JSON.stringify([s.list.map((x) => x.id + stateOf(x)), s.i, testnet(), groupOff(), TG && (TG.groups || []).some((g) => g.pending), flags.open]);
    if (sig !== lastSig) {
      lastSig = sig;
      let grp = "";
      $("walkList").innerHTML = s.list.map((x, i) => {
        const st = stateOf(x), here = i === s.i;
        const head = x.group !== grp ? `<li class="wg" aria-hidden="true">${escHtml((grp = x.group))}</li>` : "";
        const extra = here ? `<p class="small" style="flex-basis:100%;margin:2px 0 0">${x.words()}</p>` : "";
        const opt = x.recommended ? " (recommended)" : x.optional ? " (optional)" : "";
        const noGroup = x.id === "group" && st === "todo" ? `<button class="ghost walkbtn" type="button" data-walk-skip="group">I don't have one</button>` : "";
        return `${head}<li class="${st}${here ? " now" : ""}" data-walk="${x.id}"><span class="wt">${i + 1}. ${escHtml(x.title)}${opt}</span><span class="ws">${STATE_WORDS[st]}</span>${noGroup}${extra}</li>`;
      }).join("");
    }
    show("walkActs", !finished && !later);
    $("walkStart").textContent = flags.open ? "Show me" : s.done > 0 ? "Continue" : "Start";

    // Bar
    if (finished) {
      $("walkN").textContent = "All done";
      $("walkTitle").textContent = "You're set up";
      $("walkDo").textContent = "When someone joins, they show in Members and their payment lands in your vault.";
      show("walkShow", false); show("walkSkip", false);
    } else {
      $("walkN").textContent = `Step ${s.i + 1} of ${N}`;
      $("walkTitle").textContent = s.step.title;
      $("walkDo").textContent = val(s.step.bar);
      show("walkShow", true); show("walkSkip", !!s.step.optional);
    }
    const id = finished ? "done" : s.step.id;
    if (flags.open && id !== lastStep) $("walkLive").textContent = finished ? "You're set up." : `Step ${s.i + 1} of ${N}. ${s.step.title}.`;
    lastStep = id;
  }

  // ---- Acting on a step ------------------------------------------------------------------------
  function showMe(id) {
    const step = typeof id === "string" ? STEPS.find((x) => x.id === id) : now().step;
    if (!step) return;
    if (live() && step.tab) { try { closeSheet(); } catch {} showTab(step.tab, step.sub); }
    const t = $(val(step.target));
    if (!t) return;
    document.querySelectorAll(".walk-here").forEach((e) => e.classList.remove("walk-here"));
    const ring = step.ring ? t.closest(step.ring) || t : t;
    ring.classList.add("walk-here");
    // Scroll so the box starts just under the bar (sticky at the top, under the phone header).
    ring.scrollIntoView({ block: "start" });
    for (let i = 0; i < 2; i++) {
      const b = $("walkBar"), under = b.offsetParent ? b.getBoundingClientRect().bottom : 0;
      const off = ring.getBoundingClientRect().top - (under + 12);
      if (Math.abs(off) > 1) window.scrollBy(0, off);
    }
    const f = $(val(step.focus)) || t;
    if (!/^(INPUT|TEXTAREA|SELECT|BUTTON|A)$/.test(f.tagName) && !f.hasAttribute("tabindex")) f.setAttribute("tabindex", "-1");
    f.focus({ preventScroll: true });
  }
  function openGuide() {
    flags.open = true; flags.dismissed = false; later = false; save();
    draw(); showMe();
  }
  function closeGuide() {
    flags.open = false; save();
    document.querySelectorAll(".walk-here").forEach((e) => e.classList.remove("walk-here"));
    draw();
    const back = $("walkOpen");
    (back && back.offsetParent ? back : $("mainContent")).focus();
  }
  $("walkStart").onclick = openGuide;
  $("walkShow").onclick = () => showMe();
  $("walkOpen").onclick = () => { try { closeSheet(); } catch {} openGuide(); };
  $("walkClose").onclick = closeGuide;
  $("walkLater").onclick = () => {
    flags.dismissed = true; flags.open = false; later = true; save(); draw();
    const back = $("walkOpen");
    (back && back.offsetParent ? back : $("mainContent")).focus();
  };
  $("walkSkip").onclick = () => { const { step } = now(); if (step && step.optional) { flags.skipped = [...new Set([...(flags.skipped || []), step.id])]; save(); draw(); } };
  $("walkTitle").onclick = () => { const b = $("walkBar"), o = !b.classList.contains("more"); b.classList.toggle("more", o); $("walkTitle").setAttribute("aria-expanded", String(o)); };
  $("walkList").addEventListener("click", (e) => {
    const b = e.target.closest("[data-walk-skip]");
    if (!b) return;
    flags.skipped = [...new Set([...(flags.skipped || []), b.dataset.walkSkip])]; save();
    try { localStorage.setItem("kaspass.noTelegram", "1"); } catch {}
    draw();
  });
  // Escape puts the guide away, but only from inside it (never while typing in a form).
  for (const el of [$("walkBar"), $("walkCard")]) el.addEventListener("keydown", (e) => { if (e.key === "Escape" && flags.open) { e.stopPropagation(); closeGuide(); } });
  // Step 8: opening the page or copying its link counts as shared (the one step with nothing on the server to read).
  document.addEventListener("click", (e) => {
    if (e.target.closest("#copyPage, #copyPage2, #whoBox a[href='/']")) { if (!flags.shared) { flags.shared = true; save(); draw(); } }
  });
  window.walkUpdate = draw;
  // For the browser test: point at any step, not only the current one.
  window.walkShow = (id) => showMe(id);
  draw();
})();
