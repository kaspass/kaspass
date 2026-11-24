// The creator dashboard, signed in, in a real browser: stream mode hides every private detail on
// every tab, and the update 92 fixes hold (date boxes line up, no "Mainnet" badge on test
// networks' look, plan parts with their own fills, the holder discount in Prices & sales), and
// Give a membership's Plan choice and period names (95), the update 97 dashboard fixes, and update 98's badges and Plans tab on a phone.
// Run by dev/check.sh against the local simulated site, after dev/pages.js.
//
// Signing in: a stand-in for Kaspire signs the sign-in message with the simulator's creator test
// key (0x11 repeated), the same way Kaspire does (KIP-5: a Schnorr signature over
// blake2b-256 keyed "PersonalMessageSigningHash"). Nothing here touches a real wallet.
const { chromium } = require('playwright');
// @noble/curves and @noble/hashes (npm install @noble/curves @noble/hashes); version 2 names its files with .js.
const req2 = (a, b) => { try { return require(a); } catch { return require(b); } };
const { schnorr } = req2('@noble/curves/secp256k1.js', '@noble/curves/secp256k1');
const { blake2b } = req2('@noble/hashes/blake2.js', '@noble/hashes/blake2b');
// As "localhost": the simulator serves KasPass's own home page on 127.0.0.1 and a creator's site
// (membership page and dashboard, one origin) on any other name.
const S = (process.argv[2] || 'http://127.0.0.1:8790').replace('127.0.0.1', 'localhost');
const SHOTS = process.env.KASPASS_SHOTS || '';
let ok = true;
const check = (name, cond, extra) => { ok = ok && !!cond; console.log((cond ? 'PASS ' : 'FAIL ') + name + (!cond && extra !== undefined ? '   ' + JSON.stringify(extra).slice(0, 600) : '')); };
const KEY = new Uint8Array(32).fill(0x11);
const sign = (message) => {
  const h = blake2b(new TextEncoder().encode(message), { dkLen: 32, key: new TextEncoder().encode('PersonalMessageSigningHash') });
  return Buffer.from(schnorr.sign(h, KEY)).toString('hex');
};

// In the page: every piece of text a viewer could read, that looks private.
function leaks() {
  const RX = [
    /kaspa(?:test)?:[a-z0-9]{6,}/,
    /\b\d[\d,]*(?:\.\d+)?\s?(?:test\s)?KAS\b/,
    /(?:CA|US)\$\s?\d/,
    /\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\.? \d{1,2}\b/,
    /\b\d{4}-\d{2}-\d{2}\b/,
    /\b[0-9a-f]{12,}/,
    /…[a-z0-9]{4}\b/,
    /\b\d+ (?:minutes?|hours?|days?|weeks?|months?) (?:left|ago)\b/,
    /\b(?:Expires in|Expired) \d+[dhms]/,
    /\b\d+\.(?:kas|k)\b/,
    /\bending (?:in )?[a-z0-9]{4}\b/,
  ];
  const out = [];
  const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let t = w.nextNode(); t; t = w.nextNode()) {
    const el = t.parentElement;
    if (!el || !t.nodeValue.trim() || el.closest('#kpStream, script, style, option, select')) continue;
    const r = el.getClientRects();
    if (!r.length || !el.offsetParent && getComputedStyle(el).position !== 'fixed') continue;
    const cs = getComputedStyle(el);
    if (cs.visibility === 'hidden' || cs.color === 'rgba(0, 0, 0, 0)' || cs.webkitTextFillColor === 'rgba(0, 0, 0, 0)') continue;
    if (RX.some((x) => x.test(t.nodeValue))) out.push(t.nodeValue.trim().slice(0, 80));
    // A bare number whose unit sits in the next element ("362" then "KAS").
    else if (/^\s*\d[\d,.\s]*$/.test(t.nodeValue)) {
      const next = t.nextSibling || el.nextSibling;
      if (next && /^\s*(?:test\s)?(?:KAS|CAD|USD|EUR)\b/.test(next.textContent || '')) out.push('number: ' + t.nodeValue.trim() + ' ' + next.textContent.trim().slice(0, 20));
    }
  }
  // Dropdowns showing a private choice are blurred.
  for (const s of document.querySelectorAll('select')) {
    if (!s.getClientRects().length || !s.offsetParent || document.activeElement === s) continue;
    const shown = s.options[s.selectedIndex] ? s.options[s.selectedIndex].text : '';
    if (RX.some((x) => x.test(shown)) && getComputedStyle(s).color !== 'rgba(0, 0, 0, 0)') out.push('dropdown: ' + shown.slice(0, 60));
  }
  // Boxes with something in them are blurred.
  for (const i of document.querySelectorAll('input, textarea')) {
    if (!i.getClientRects().length || !i.offsetParent || ['checkbox', 'radio', 'file', 'button', 'submit', 'range', 'color', 'hidden'].includes(i.type)) continue;
    if (!i.value || document.activeElement === i || i.readOnly && !RX.some((x) => x.test(i.value))) continue;
    const ic = getComputedStyle(i);
    if (ic.color !== 'rgba(0, 0, 0, 0)' && ic.webkitTextFillColor !== 'rgba(0, 0, 0, 0)') out.push('box: ' + i.value.slice(0, 60));
  }
  // Hover tooltips and link addresses (the browser's status bar) with private details.
  for (const el of document.querySelectorAll('[title], a[href]')) {
    const t = (el.getAttribute('title') || '') + ' ' + (el.tagName === 'A' ? el.getAttribute('href') || '' : '');
    if (RX.some((x) => x.test(t)) || /[?&](?:k|invite)=/.test(t)) out.push('tooltip or link: ' + t.trim().slice(0, 60));
  }
  return out;
}

(async () => {
  const b = await chromium.launch(process.env.KASPASS_CHROME ? { executablePath: process.env.KASPASS_CHROME } : {});
  const ctx = await b.newContext({ viewport: { width: 1366, height: 900 } });
  await ctx.exposeBinding('kpSign', (_src, message) => sign(message));
  await ctx.addInitScript(() => {
    const me = async () => (await (await fetch('/v1/tier')).json()).creator_address;
    window.kaspire = {
      isKaspire: true,
      request: async ({ method, params }) => {
        if (method === 'getAccounts' || method === 'requestAccounts') return [await me()];
        if (method === 'signMessage') return { signature: await window.kpSign(params.message) };
        throw new Error('not in this test: ' + method);
      },
    };
  });
  const p = await ctx.newPage();
  const errs = []; p.on('pageerror', (e) => errs.push(e.message));
  // GET /v1/owner (94): the site's current owner, its owner record, vault and plans, no backup wallet.
  await p.goto(S + '/v1/health');
  const own = await p.evaluate(async () => {
    const o = await (await fetch('/v1/owner')).json();
    const t = await (await fetch('/v1/tier')).json();
    const l = await (await fetch('/v1/levels')).json();
    return { o, creator: t.creator_address, vaults: [...new Set(l.levels.map((x) => x.vault))], ids: l.levels.map((x) => x.level), network: l.network };
  });
  check('/v1/owner: names the owner (the creator, on a site never handed over)', own.o.owner === own.creator, own.o);
  check('/v1/owner: the vault is the one /v1/levels shows for every plan', own.vaults.length === 1 && own.o.vault === own.vaults[0], [own.o.vault, own.vaults]);
  check('/v1/owner: lists the same plans, the record id and the network', JSON.stringify(own.o.levels) === JSON.stringify(own.ids) && /^[0-9a-f]{64}$/.test(own.o.record_id) && own.o.network === own.network, own.o);
  check('guide: explains Kaspa Vision\'s KasPass tab and what it reads from /v1/owner (96)', await p.evaluate(async () => { const g = await (await fetch('/guide')).text(); return g.includes("Kaspa Vision's KasPass tab") && g.includes('one site per vault'); }));
  check('/v1/owner: no backup wallet in it', !('recovery' in own.o) && !JSON.stringify(own.o).includes('recovery'), Object.keys(own.o));
  await p.goto(S + '/dashboard');
  await p.click('#signin');
  await p.waitForSelector('#creatorArea:not(.hidden)', { timeout: 30000 });
  await p.waitForTimeout(4000);
  check('dashboard: signs in with the creator test key', await p.isVisible('#creatorArea'));

  // ---- The fixes, with stream mode off ----
  check('no "Mainnet" badge logic: a test network still shows its badge', (await p.textContent('#net')).includes('Testnet'));
  await p.click('#tabs [data-tab="promos"]'); await p.waitForTimeout(1500);
  if (await p.isVisible('#prStart')) {
    const tops = await p.$$eval('#prStart, #prEnd, #prMax', (els) => els.map((e) => Math.round(e.getBoundingClientRect().top)));
    check('promotions: the Starts, Ends and Most free months boxes line up', new Set(tops).size === 1, tops);
    if (SHOTS) await p.locator('#promoPanel').screenshot({ path: SHOTS + '/promo-dates.png' });
  } else console.log('SKIP promotions: no promotion panel on this site');
  check('promotions: the holder discount is no longer in the fees wallet panel', !(await p.$('#v4Panel #dPct')));
  await p.click('#tabs [data-tab="prices"]'); await p.waitForTimeout(1200);
  check('prices & sales: the holder discount is here', !!(await p.$('[data-pane="prices"] #dPct')));
  const saleTitle = await p.textContent('#saleTitle');
  const many = await p.evaluate(() => (data.levels || []).length);
  check('run a sale: names the plan when there are several', many > 1 ? /Run a sale on ./.test(saleTitle) && (await p.isVisible('#sAllBox')) : saleTitle === 'Run a sale', [saleTitle, many]);
  if (SHOTS) await p.locator('[data-pane="prices"]').screenshot({ path: SHOTS + '/prices.png' });
  // Give a membership (95): the paid period is named from the plan, and the form has its own Plan choice.
  await p.click('#tabs [data-tab="members"]'); await p.waitForTimeout(1000);
  const names = await p.evaluate(() => typeof periodName === 'function' ? [365, 366, 30, 7, 1, 10].map((d) => periodName(d * 86400000)).concat(periodName(600000)) : null);
  check('give a membership: periods are named from the plan (One year, One month, One week, One day, 10 days)', JSON.stringify(names) === JSON.stringify(['One year', 'One year', 'One month', 'One week', 'One day', '10 days', 'One month']), names);
  const paid = await p.evaluate(() => { const o = [...document.querySelectorAll('#gKind option')].find((x) => x.value === 'monthly'); return o ? [o.text, periodName(card.settings.period_ms)] : null; });
  check('give a membership: the paid choice starts with the plan\'s period', !paid || paid[0].startsWith(paid[1] + ' ('), paid);
  const gp = await p.evaluate(() => { const s = document.getElementById('gPlan'); return s ? { shown: !document.getElementById('gPlanBox').classList.contains('hidden'), ids: [...s.options].map((o) => o.value), names: [...s.options].map((o) => o.text), value: s.value, levels: (data.levels || []).map((l) => l.covenant_id), want: (data.levels || []).map((l) => taxFill(l.name)), level: LEVEL || (data.levels[0] || {}).covenant_id } : null; });
  check('give a membership: a Plan choice in the form, listing every plan when there are several', gp && (many > 1 ? gp.shown && JSON.stringify(gp.ids) === JSON.stringify(gp.levels) && JSON.stringify(gp.names) === JSON.stringify(gp.want) && gp.value === gp.level : !gp.shown), gp);
  if (gp && many > 1) {
    const first = gp.value, second = gp.ids.find((x) => x !== first);
    await p.selectOption('#gPlan', second); await p.waitForTimeout(2500);
    const after = await p.evaluate(() => ({ level: LEVEL, card: card.covenant_id, on: (document.querySelector('#levelBtns button.on') || {}).textContent, value: document.getElementById('gPlan').value, go: document.getElementById('gGo').disabled }));
    check('give a membership: choosing a plan in the form switches the whole page to it, and Give it works again', after.level === second && after.card === second && after.value === second && !after.go && after.on === (await p.evaluate((id) => data.levels.find((l) => l.covenant_id === id).name, second)), after);
    await p.click('#levelBtns button:not(.on)'); await p.waitForTimeout(2500);
    check('give a membership: the picker at the top moves the form\'s Plan choice too', await p.evaluate(() => document.getElementById('gPlan').value === (LEVEL || data.levels[0].covenant_id)));
  } else console.log('SKIP give a membership plan switch: one plan on this site');
  // Update 97 (UI88 M13): "Start sale" starts nothing on the first press, and the percent is only an example.
  await p.click('#tabs [data-tab="prices"]'); await p.waitForTimeout(800);
  const sp = await p.evaluate(() => ({ v: document.getElementById('sPct').value, ph: document.getElementById('sPct').placeholder, sale: !!data.sale }));
  check('run a sale: percent off starts empty, with 20 only as an example (97)', sp.v === '' && sp.ph === '20', sp);
  if (!sp.sale) {
    await p.fill('#sPct', '15'); await p.click('#sGo'); await p.waitForTimeout(1200);
    const t = await p.textContent('#sGo');
    const on = await p.evaluate(async () => !!(await (await fetch(L('/v1/creator?session=' + encodeURIComponent(session)))).json()).sale);
    check('run a sale: the first press asks for a second, naming the discount, and starts nothing (97, UI88 M13)', /^Press again to start 15% off ./.test(t) && !on, [t, on]);
    await p.fill('#sPct', '16'); await p.waitForTimeout(100);
    check('run a sale: changing the form puts the button back', (await p.textContent('#sGo')) === 'Start sale');
    await p.fill('#sPct', '');
  } else console.log('SKIP start sale: a sale is already on');
  // Update 98: a plan's badges. Automatic until edited; the creator's own list (up to 3, in six
  // colours) replaces them, in /v1/levels and on the card; "Back to automatic" undoes it.
  await p.click('#tabs [data-tab="levels"]'); await p.waitForTimeout(1500);
  const lvl = async () => p.evaluate(async () => { const id = data.level.covenant_id; return (await (await fetch('/v1/levels')).json()).levels.find((x) => x.level === id); });
  const b0 = await lvl();
  check('badges: automatic until edited (98)', b0.badges_custom === false && Array.isArray(b0.badges) && /Automatic for now|No badge on this plan/.test(await p.textContent('#badgeNote')), [b0.badges, b0.badges_custom]);
  await p.click('#badgeAdd'); await p.waitForTimeout(300);
  const rows0 = await p.$$eval('#badgeRows .brow', (r) => r.length);
  await p.fill('#badgeRows .brow:last-child input', '  Most   popular ');
  await p.selectOption('#badgeRows .brow:last-child select.btone', 'gold');
  if (rows0 < 3) { await p.click('#badgeAdd'); await p.waitForTimeout(200); await p.selectOption('#badgeRows .brow:last-child select:first-child', 'early'); await p.waitForTimeout(200); await p.selectOption('#badgeRows .brow:last-child select.btone', 'red'); }
  // The first badge row up: the text badge goes to the top.
  const n0 = await p.$$eval('#badgeRows .brow', (r) => r.length);
  check('badges: at most 3 rows, and "Add a badge" stops at 3', n0 <= 3 && (n0 < 3 || await p.$eval('#badgeAdd', (x) => x.disabled)), n0);
  if (SHOTS) { await p.locator('#badgeSet').scrollIntoViewIfNeeded(); await p.screenshot({ path: SHOTS + '/badges-editor.png' }); }
  await p.click('#lvNameGo'); await p.waitForTimeout(2500);
  const b1 = await lvl();
  const txt = (b1.badges || []).find((x) => x.kind === 'text');
  check('badges: the creator\'s list saved, cleaned, in /v1/levels, in its colour (98)', b1.badges_custom === true && txt && txt.text === 'Most popular' && txt.tone === 'gold' && b1.badge === 'Most popular' && !(b1.badges || []).some((x) => x.kind === 'early' && !b1.founding), b1.badges);
  const mp = await ctx.newPage();
  for (const st of ['kaspass', 'kaspa-vision']) {
    await mp.goto(`${S}/?style=${st}`); await mp.waitForTimeout(3500);
    const shown = await mp.evaluate((id) => { const c = [...document.querySelectorAll('#levelCards .lvl')].find((x) => x.querySelector('.cs-badges')); return c ? { chips: [...c.querySelectorAll('.cs-badges .cs-badge')].map((e) => [e.textContent, e.className, getComputedStyle(e).backgroundColor]), auto: !!c.querySelector('.cs-early, .cs-save') } : null; });
    if (SHOTS) await mp.screenshot({ path: `${SHOTS}/badges-card-${st}.png` });
    check(`badges: on the card in the ${st} style, gold, with no automatic badge beside them (98)`, !!shown && shown.chips[0][0] === 'Most popular' && /t-gold/.test(shown.chips[0][1]) && shown.chips[0][2] === 'rgb(242, 200, 107)' && !shown.auto, shown);
  }
  // Hiding the automatic badges on a plan with an early price: an empty list.
  const earlyPlan = await p.evaluate(async () => { const ls = (await (await fetch('/v1/levels')).json()).levels; const t = ls.find((x) => x.founding); return t ? t.level : null; });
  if (earlyPlan) {
    const post = (body) => p.evaluate(async (b) => (await fetch(L('/v1/creator/badges'), { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ session, covenant_id: b.id, ...b.x }) })).status, body);
    const st = await post({ id: earlyPlan, x: { badges: [] } });
    await mp.goto(`${S}/?style=kaspa-vision`); await mp.waitForTimeout(3500);
    const tax = await mp.evaluate(async (id) => { const l = (await (await fetch('/v1/levels')).json()).levels.find((x) => x.level === id); const cards = [...document.querySelectorAll('#levelCards .lvl')]; return { custom: l.badges_custom, n: l.badges.length, early: cards.some((c) => c.querySelector('.cs-early')) }; }, earlyPlan);
    check('badges: an empty list hides the automatic early-price badge (98)', st === 200 && tax.custom === true && tax.n === 0 && !tax.early, [st, tax]);
    await post({ id: earlyPlan, x: { automatic: true } });
    const back = await p.evaluate(async (id) => { const l = (await (await fetch('/v1/levels')).json()).levels.find((x) => x.level === id); return { custom: l.badges_custom, early: l.badges.some((b) => b.kind === 'early' && b.until_ms) }; }, earlyPlan);
    check('badges: back to automatic brings the early-price badge back', back.custom === false && back.early, back);
  } else console.log('SKIP badges on a plan with an early price: none in this simulator');
  await mp.close();
  await p.click('#badgeAuto'); await p.waitForTimeout(300); await p.click('#lvNameGo'); await p.waitForTimeout(2500);
  const b2 = await lvl();
  check('badges: "Back to automatic" in the dashboard undoes the list (98)', b2.badges_custom === false && b2.badge === '', b2);
  // Update 98: the Plans tab on a phone-sized window.
  await p.setViewportSize({ width: 390, height: 844 }); await p.waitForTimeout(500);
  await p.evaluate(() => showTab('levels')); await p.waitForTimeout(800);
  const ph = await p.evaluate(() => {
    const seg = document.querySelector('#levelPick .seg');
    const bs = seg ? [...seg.querySelectorAll('button')].map((b) => b.getBoundingClientRect()) : [];
    const info = document.querySelector('#cardsT .pcard > div:nth-child(2)');
    return { segFits: !seg || seg.scrollWidth <= seg.clientWidth + 1, btnsIn: bs.every((r) => r.right <= innerWidth + 1 && r.left >= -1), info: info ? Math.round(info.getBoundingClientRect().width) : 0, flat: document.documentElement.scrollWidth <= innerWidth + 1 };
  });
  check('Plans tab on a phone: the plan picker wraps instead of running off the side, and each plan\'s name gets the full width (98)', ph.segFits && ph.btnsIn && ph.info >= 280 && ph.flat, ph);
  if (SHOTS) await p.screenshot({ path: SHOTS + '/plans-390.png' });
  await p.fill('#perks', 'One\nTwo\nThree\nFour\nFive\nSix');
  if (SHOTS) { await p.locator('#badgeSet').scrollIntoViewIfNeeded(); await p.click('#badgeAdd'); await p.waitForTimeout(300); await p.screenshot({ path: SHOTS + '/badges-editor-390.png' }); await p.click('#badgeAuto'); }
  const grow = await p.$eval('#perks', (t) => ({ sh: t.scrollHeight, ch: t.clientHeight }));
  check('"What it includes" grows to show every line (98)', grow.sh <= grow.ch + 2, grow);
  await p.evaluate(() => { $("perks").value = (data.level.perks || []).join("\n"); });
  const sp2 = await p.evaluate(() => { document.getElementById('wcOtherLine').classList.remove('hidden'); const d = getComputedStyle(document.getElementById('signPanel')).display; document.getElementById('wcOtherLine').classList.add('hidden'); return { d, other: !!document.getElementById('wcOther2') }; });
  check('signed in: no leftover sign-in box ("Not you?" is with Sign out) (98)', sp2.d === 'none' && sp2.other, sp2);
  await p.setViewportSize({ width: 1366, height: 900 }); await p.waitForTimeout(300);
  // Update 97 (UI88 M14): the side menu fits a 768 px high laptop, Themes and Sign out included.
  await p.setViewportSize({ width: 1366, height: 768 }); await p.waitForTimeout(500);
  const side = await p.evaluate(() => { const s = document.querySelector('.side'); const lo = document.getElementById('logout').getBoundingClientRect(); const th = document.querySelector('.sidefoot [data-go="settings"]').getBoundingClientRect(); return { fits: s.scrollHeight <= s.clientHeight + 1, logout: Math.round(lo.bottom), themes: Math.round(th.bottom), h: s.scrollHeight }; });
  check('side menu: Themes and Sign out fit on a 1366 x 768 laptop (97, UI88 M14)', side.fits && side.logout <= 768 && side.themes <= 768, side);
  if (SHOTS) await p.screenshot({ path: SHOTS + '/side-1366x768.png' });
  await p.setViewportSize({ width: 1366, height: 900 }); await p.waitForTimeout(300);
  await p.click('#tabs [data-tab="overview"]'); await p.waitForTimeout(800);
  const fills = await p.$$eval('#byPlanBar i', (els) => els.map((e) => e.className));
  check('members by plan: each part has its own fill', fills.length < 2 || new Set(fills).size === fills.length, fills);
  check('the tax line on payments', (await p.content()).includes("It isn't tax or legal advice"));

  // ---- Stream mode ----
  await p.click('label:has(#streamSw)');
  await p.waitForTimeout(500);
  check('stream mode: the switch turns it on and the badge shows', await p.evaluate(() => document.documentElement.classList.contains('stream')) && await p.isVisible('#kpStream'));
  const tabs = await p.$$eval('#tabs button[data-tab]:not(.hidden)', (els) => els.map((e) => e.dataset.tab));
  for (const tab of tabs) {
    await p.click(`#tabs [data-tab="${tab}"]`); await p.waitForTimeout(1500);
    // Open every fold, so what's inside counts too.
    await p.$$eval(`[data-pane="${tab}"] details`, (ds) => ds.forEach((d) => (d.open = true)));
    await p.waitForTimeout(200);
    const l = await p.evaluate(leaks);
    check(`stream mode: nothing private readable on ${tab}`, l.length === 0, l);
    if (SHOTS) await p.screenshot({ path: `${SHOTS}/stream-${tab}.png`, fullPage: true });
  }
  // Again with a member, payments and promotion claims, so every list has something private in it.
  const fx = await p.evaluate(() => {
    try {
      const A = 'kaspatest:qq2u5wd0q4yfh0s4l7gj0xkq7v8m8tfx2ygz6f5yh0lcpp2nxku9wk7xglqe8';
      const T = 'ab12'.repeat(16), H = 'cd34'.repeat(16), now = Date.now();
      data.events = [{ txid: T, kind: 'monthly', owner_hash: H, amount_sompi: 25617000000, fee_sompi: 0, creator_sompi: 25617000000, time_ms: now - 86400000 }, { txid: 'ef56'.repeat(16), kind: 'monthly', owner_hash: H, amount_sompi: 100000000, fee_sompi: 0, creator_sompi: 100000000, time_ms: now - 3600000 }];
      data.free_txids = { ['ef56'.repeat(16)]: 'promotion' };
      data.passes = [{ owner_hash: H, owner_address: A, active: true, expiry_ms: now + 9e8, kind: 'monthly', paid_sompi: 25617000000, balance_sompi: 0, rate_sompi: 1, lifetime: false, bought_by: A, note: 'Alex from Telegram', txid: T }];
      data.totals = { ...data.totals, earned_sompi: 25717000000, active: 1, sales: 2 };
      memberNames[A] = { kas: '888.kas' };
      ALL = null; render();
      if (typeof PR !== 'undefined' && PR) { PR.claims = [{ at_ms: now - 3600000, wallet: A, nfts: ['mig:' + 'ef56'.repeat(16)], txid: 'ef56'.repeat(16) }]; renderPromo(); }
      return 'ok';
    } catch (e) { return e.message; }
  });
  check('stream mode: sample members, payments and claims drawn', fx === 'ok', fx);
  check('activity: a free month paid by the fees wallet is named as one', (await p.textContent('#recent')).includes('Free month (promotion)'));
  if (await p.$('#prClaims tr')) check('promotion claims: "What counted" says it in words', (await p.textContent('#prClaims')).includes('1 KAT Bridge migration'));
  for (const tab of ['overview', 'members', 'promos']) {
    await p.click(`#tabs [data-tab="${tab}"]`); await p.waitForTimeout(600);
    const l = await p.evaluate(leaks);
    check(`stream mode: nothing private readable on ${tab}, with sample data`, l.length === 0, l);
    if (SHOTS) await p.screenshot({ path: `${SHOTS}/stream-${tab}-data.png`, fullPage: true });
  }
  // Fable's findings S2, S7, S10, S15, S20 (claude/kaspass-fable-stream-mode-review-92.md).
  check('stream mode: no tooltip on the side menu wallet line', await p.evaluate(() => !document.getElementById('sideAddr').title));
  // Bars in the same text size and font are one width, whatever is under them (a bar is 7 characters wide).
  const widths = await p.$$eval('.sx', (els) => { const g = {}; for (const e of els) { if (!e.getClientRects().length || e.closest('.kp-shown')) continue; const cs = getComputedStyle(e); const k = cs.fontSize + ' ' + cs.fontFamily + ' ' + cs.fontWeight; (g[k] = g[k] || new Set()).add(Math.round(e.getBoundingClientRect().width)); } return Object.entries(g).filter(([, s]) => s.size > 1).map(([k, s]) => k + ': ' + [...s].join('/')); });
  check('stream mode: grey bars in the same text size are all one width, whatever is under them', widths.length === 0, widths);
  await p.click('#tabs [data-tab="page"]'); await p.waitForTimeout(600);
  const ed = await p.$('#pgIntroEd [contenteditable], #pgIntroEd[contenteditable]');
  if (ed && await ed.isVisible()) {
    await ed.click(); await p.keyboard.press('End');
    await p.keyboard.type(' Join by Nov 18 for $5 a month. Ask alex.kas about it');
    const txt = await ed.evaluate((e) => e.textContent);
    check('stream mode: typing in the intro editor keeps the text in order', txt.endsWith('Join by Nov 18 for $5 a month. Ask alex.kas about it'), txt.slice(-80));
    await ed.evaluate((e, t) => { e.textContent = e.textContent.replace(t, ''); }, ' Join by Nov 18 for $5 a month. Ask alex.kas about it');
  } else console.log('SKIP editor: no intro editor visible');
  // A double-click on Turn off, or on the switch, does not turn it off.
  await p.dblclick('#kpStream button'); await p.waitForTimeout(300);
  check('stream mode: a double-click on Turn off does not turn it off', await p.evaluate(() => document.documentElement.classList.contains('stream')));
  await p.waitForTimeout(5500);
  await p.dblclick('label:has(#streamSw)'); await p.waitForTimeout(300);
  check('stream mode: a double-click on the switch does not turn it off', await p.evaluate(() => document.documentElement.classList.contains('stream')));
  await p.waitForTimeout(5500);
  // A link with ?stream=0 doesn't turn it off, and the parameter leaves the address bar.
  const z = await ctx.newPage();
  await z.goto(S + '/?stream=0'); await z.waitForTimeout(1500);
  check('stream mode: a ?stream=0 link does not turn it off', await z.evaluate(() => document.documentElement.classList.contains('stream') && !location.search.includes('stream')));
  await z.close();
  // A contrast theme (forced colours) still shows only boxes.
  const fc = await b.newContext({ viewport: { width: 1366, height: 900 }, forcedColors: 'active' });
  await fc.addInitScript(() => { try { localStorage.setItem('kaspass.stream', '1'); } catch {} });
  const fp = await fc.newPage();
  await fp.goto(S + '/'); await fp.waitForTimeout(3500);
  const cover = await fp.evaluate(() => { const s = [...document.querySelectorAll('.sx, .kasn')].find((e) => e.getClientRects().length); if (!s) return 'none'; const a = getComputedStyle(s, '::after'); return a.content !== 'none' && a.position === 'absolute' ? 'covered' : 'bare'; });
  check('stream mode: in a contrast theme, hidden details are covered by a box', cover === 'covered' || cover === 'none', cover);
  if (SHOTS) await fp.screenshot({ path: SHOTS + '/stream-forced-colors.png', fullPage: true });
  await fc.close();
  // Another tab that was already open follows the switch.
  const other = await ctx.newPage();
  await p.waitForTimeout(5500);
  await p.click('#kpStream button'); await p.waitForTimeout(1000); await p.click('#kpStream button'); await p.waitForTimeout(300);
  await other.goto(S + '/'); await other.waitForTimeout(2500);
  check('stream mode: off in this tab (two presses a second apart)', await p.evaluate(() => !document.documentElement.classList.contains('stream')));
  await p.click('label:has(#streamSw)'); await p.waitForTimeout(800);
  check('stream mode: a tab that was already open turns on with the switch', await other.evaluate(() => document.documentElement.classList.contains('stream')));
  await other.close();
  // The side menu and the top too.
  check('stream mode: the signed-in wallet in the side menu is hidden', await p.evaluate(() => getComputedStyle(document.getElementById('sideAddr')).color === 'rgba(0, 0, 0, 0)'));
  // Reveal beside the fees wallet's address.
  await p.click('#tabs [data-tab="promos"]'); await p.waitForTimeout(800);
  const rev = await p.$('#v4Panel .kp-reveal');
  if (rev && await rev.isVisible()) {
    await rev.click();
    check('stream mode: Reveal shows the fees wallet address', await p.evaluate(() => getComputedStyle(document.getElementById('kAddr')).color !== 'rgba(0, 0, 0, 0)'));
    await rev.click();
    check('stream mode: Hide hides it again', await p.evaluate(() => getComputedStyle(document.getElementById('kAddr')).color === 'rgba(0, 0, 0, 0)'));
  } else console.log('SKIP reveal: no fees wallet address on this site');
  // Clicking a grey bar shows just that one.
  const bar = await p.$('.sx:visible');
  if (bar) { await bar.click(); check('stream mode: clicking a grey bar shows it', await bar.evaluate((e) => e.classList.contains('kp-shown'))); }
  // Turning off takes two presses, at least 0.8 seconds apart.
  await p.waitForTimeout(5500);
  await p.click('#kpStream button');
  check('stream mode: one press of Turn off does not turn it off', await p.evaluate(() => document.documentElement.classList.contains('stream')) && /Press again/.test(await p.textContent('#kpStream button')));
  await p.waitForTimeout(1000);
  await p.click('#kpStream button');
  check('stream mode: the second press turns it off', await p.evaluate(() => !document.documentElement.classList.contains('stream')));
  // On the membership page of the same site, from the dashboard's switch (same browser).
  await p.click('label:has(#streamSw)'); await p.waitForTimeout(300);
  const m = await ctx.newPage();
  await m.goto(S + '/'); await m.waitForTimeout(4000);
  const ml = await m.evaluate(leaks);
  check('stream mode: the membership page follows the switch and hides prices and dates', (await m.evaluate(() => document.documentElement.classList.contains('stream'))) && ml.length === 0, ml);
  if (SHOTS) await m.screenshot({ path: SHOTS + '/stream-member-page.png', fullPage: true });
  // Update 97 (UI88 M16, M17): sign-in at 360 px fits, and a phone with a Kaspire wallet in the page
  // (the Kaspire app's own browser) gets the ordinary sign-in button; a phone without one, the app.
  {
    const { devices } = require('playwright');
    const n = await b.newContext({ viewport: { width: 360, height: 640 } });
    const q = await n.newPage();
    await q.goto(S + '/dashboard'); await q.waitForTimeout(2500);
    const fit = await q.evaluate(() => { const r = [...document.querySelectorAll('#wcGo, #signin')].filter((e) => e.getClientRects().length).map((e) => Math.round(e.getBoundingClientRect().right)); return { flat: document.documentElement.scrollWidth <= innerWidth + 1, right: Math.max(0, ...r) }; });
    check('dashboard sign-in at 360 px: nothing runs off the side (97, UI88 M16)', fit.flat && fit.right <= 360, fit);
    if (SHOTS) await q.screenshot({ path: SHOTS + '/dash-signin-360.png' });
    await n.close();
    for (const wallet of [false, true]) {
      const c = await b.newContext({ ...devices['Pixel 5'] });
      if (wallet) await c.addInitScript(() => { window.kaspire = { isKaspire: true, request: async () => { throw new Error('not in this test'); } }; });
      const w = await c.newPage();
      await w.goto(S + '/dashboard'); await w.waitForTimeout(2500);
      const v = await w.evaluate(() => ({ signin: !document.getElementById('signin').classList.contains('hidden'), app: document.getElementById('wcGo').textContent }));
      check(wallet ? 'dashboard on a phone with a Kaspire wallet in the page: the ordinary sign-in button (97, UI88 M17)' : 'dashboard on a phone without a wallet in the page: the Kaspire app is the way in', wallet ? v.signin : !v.signin && /Kaspire app/.test(v.app), v);
      await c.close();
    }
  }
  check('dashboard: no script errors', errs.length === 0, errs);
  await b.close();
  console.log(ok ? 'DASHBOARD OK' : 'DASHBOARD FAILED');
  process.exit(ok ? 0 : 1);
})().catch((e) => { console.log('FAIL dashboard test crashed: ' + e.message); process.exit(1); });
