// The "Get started" guide on a brand-new site (update 101, WK100 W29): a creator-only site with no
// plan yet, the way KasPass starts one for a new creator. Run by dev/check.sh against a second
// simulated site (KASPASS_SIM_EMPTY=1 KASPASS_SIM_CREATOR_ONLY=1). Signs in with the simulator's
// creator test key through the same stand-in for Kaspire as dev/dashboard.js. The stand-in can't
// approve transactions, so the states after "Turn on payouts" and "Create your first plan" are
// stubbed (Playwright routes) with what the server returns once they're done.
const { chromium } = require('playwright');
const req2 = (a, b) => { try { return require(a); } catch { return require(b); } };
const { schnorr } = req2('@noble/curves/secp256k1.js', '@noble/curves/secp256k1');
const { blake2b } = req2('@noble/hashes/blake2.js', '@noble/hashes/blake2b');
const leaks = require('./leaks.js');
const S = (process.argv[2] || 'http://127.0.0.1:8791').replace('127.0.0.1', 'localhost');
const SHOTS = process.env.KASPASS_SHOTS || '';
let ok = true;
const check = (name, cond, extra) => { ok = ok && !!cond; console.log((cond ? 'PASS ' : 'FAIL ') + name + (!cond && extra !== undefined ? '   ' + JSON.stringify(extra).slice(0, 600) : '')); };
const KEY = new Uint8Array(32).fill(0x11);
const sign = (message) => {
  const h = blake2b(new TextEncoder().encode(message), { dkLen: 32, key: new TextEncoder().encode('PersonalMessageSigningHash') });
  return Buffer.from(schnorr.sign(h, KEY)).toString('hex');
};

async function signedIn(b, { record = false, pending = false } = {}) {
  // 1280 x 720: the size of a stream capture.
  const ctx = await b.newContext({ viewport: { width: 1280, height: 720 } });
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
  // Payouts turned on: the server would return the owner record.
  if (record) await ctx.route(/\/v1\/creator\/record(\?|$)/, async (route) => {
    const t = await (await ctx.request.get(S + '/v1/tier')).json().catch(() => ({}));
    await route.fulfill({ json: { record: { owner: t.creator_address || '', recovery: t.creator_address || '' }, vault_address: t.creator_address || '', vault_balance_sompi: 0, vault_payments: 0 } });
  });
  // A first plan created but not on sale yet (a reload between "created" and "on sale").
  if (pending) await ctx.route(/\/v1\/creator\/cards(\?|$)/, (route) => route.fulfill({ json: { cards: [{ file: 'card-test.json', covenant_id: 'ab'.repeat(32), name: 'Kaspa VIP', yours: true, on_sale: false, saved_ms: Date.now(), version: 5 }] } }));
  const p = await ctx.newPage();
  const errs = []; p.on('pageerror', (e) => errs.push(e.message));
  await p.goto(S + '/dashboard');
  await p.click('#signin');
  await p.waitForSelector('#walkCard:not(.hidden)', { timeout: 30000 });
  await p.waitForTimeout(3000);
  return { ctx, p, errs };
}

(async () => {
  const b = await chromium.launch(process.env.KASPASS_CHROME ? { executablePath: process.env.KASPASS_CHROME } : {});
  const allErrs = [];

  // 1. A fresh site: the guide opens by itself at step 1.
  {
    const { ctx, p, errs } = await signedIn(b);
    const st = await p.evaluate(() => ({
      placeholder: card.placeholder,
      bar: !document.getElementById('walkBar').classList.contains('hidden'),
      card: !document.getElementById('walkCard').classList.contains('hidden'),
      n: document.getElementById('walkN').textContent, title: document.getElementById('walkTitle').textContent,
      lead: document.getElementById('walkLead').textContent, net: document.getElementById('walkNet').textContent,
      first: document.querySelector('#walkList li.now') && document.querySelector('#walkList li.now').textContent,
      nameBox: !document.getElementById('lNameBox').classList.contains('hidden'),
      ph: document.getElementById('recRecovery').placeholder,
      cardTop: document.getElementById('walkCard').getBoundingClientRect().top < document.getElementById('launchHome').getBoundingClientRect().top,
    }));
    check('new site: a placeholder site for this test', st.placeholder === true, st);
    check('new site: the guide opens by itself, card and bar, at step 1 Turn on payouts', st.bar && st.card && /Step 1 of/.test(st.n) && st.title === 'Turn on payouts', st);
    check('new site: the card is above the setup forms and says how long it takes', st.cardTop && /About 10 minutes/.test(st.lead), st);
    check('new site: test network words and the faucet (W19)', /free test KAS/.test(st.net) && !!(await p.$('#walkNet a[href*="faucet"]')), st);
    check('new site: step 1 says the backup wallet can\'t be swapped later', /can't be swapped later/.test(st.first), st);
    check('new site: the first plan has a name box (W12)', st.nameBox, st);
    check('new site: the backup box shows a kaspatest: placeholder (W11)', st.ph.startsWith('kaspatest:'), st);
    await p.click('#walkShow'); await p.waitForTimeout(400);
    check('new site: Show me rings the payouts box and focuses the address box', await p.evaluate(() => !!document.querySelector('.walk-here #recSetup') && document.activeElement.id === 'recRecovery'));
    if (SHOTS) await p.screenshot({ path: SHOTS + '/empty-step1-1280x720.png' });
    // Stream mode on this screen: the stream-only line about recovery words shows, nothing private does.
    await p.click('label:has(#streamSw)'); await p.waitForTimeout(500);
    await p.evaluate(() => document.activeElement && document.activeElement.blur());
    const l = await p.evaluate(leaks);
    check('new site, stream mode: nothing private readable at step 1', l.length === 0, l);
    check('new site, stream mode: the line about not showing recovery words on air shows', await p.isVisible('#recSetup .walk-streamonly'));
    if (SHOTS) await p.screenshot({ path: SHOTS + '/empty-step1-stream-1280x720.png' });
    // Not now: put away, the menu button stays, and it isn't offered again after a reload.
    await p.click('#kpStream button'); await p.waitForTimeout(1000); await p.click('#kpStream button'); await p.waitForTimeout(300);
    await p.click('#walkLater'); await p.waitForTimeout(300);
    check('new site: Not now puts the bar away and says where to find it', !(await p.isVisible('#walkBar')) && /under Get started in the menu/.test(await p.textContent('#walkCard')));
    await p.reload(); await p.waitForTimeout(5000);
    check('new site: after Not now and a reload, it stays put away, and the menu button brings it back', !(await p.isVisible('#walkBar')) && !(await p.isVisible('#walkCard')) && await p.isVisible('#walkOpen'));
    await p.click('#walkOpen'); await p.waitForTimeout(400);
    check('new site: the menu button opens it again at the first step not done', await p.isVisible('#walkBar') && (await p.textContent('#walkTitle')) === 'Turn on payouts');
    allErrs.push(...errs); await ctx.close();
  }

  // 2. Payouts on: step 1 reads Done and the bar moves to step 2.
  {
    const { ctx, p, errs } = await signedIn(b, { record: true });
    const st = await p.evaluate(() => ({ s1: document.querySelector('#walkList li[data-walk="payouts"] .ws').textContent, title: document.getElementById('walkTitle').textContent }));
    check('new site with payouts on: step 1 Done, the bar on Create your first plan', st.s1 === 'Done' && st.title === 'Create your first plan', st);
    // Update 102: the rehearsal's create-form findings (claude/kaspass-rehearsal-101.md), on air:
    // 1280 x 720, stream mode on.
    // A site with payouts on is v5 (the server says so in /v1/levels); the stub only covers the record.
    await p.evaluate(() => applyV5(true)); await p.waitForTimeout(200);
    await p.click('label:has(#streamSw)'); await p.waitForTimeout(500);
    const f0 = await p.evaluate(() => ({ amount: document.getElementById('lAmount').value, ph: document.getElementById('lAmount').placeholder, box: !document.getElementById('lV5Box').classList.contains('hidden') }));
    check('create form: the price box starts empty, with a placeholder (102, R1)', f0.box && f0.amount === '' && /for example/.test(f0.ph), f0);
    await p.fill('#lName', 'Supporter');
    await p.click('#lGo'); await p.waitForTimeout(300);
    const g0 = await p.evaluate(() => ({ err: document.getElementById('lErr').textContent, go: document.getElementById('lGo').textContent, focus: document.activeElement.id }));
    check('create: with no price, the first press says so and arms nothing (102, R1)', /Type the price/.test(g0.err) && !/Press again/.test(g0.go) && g0.focus === 'lAmount', g0);
    await p.fill('#lAmount', '100');
    check('create: typing a price clears that message', (await p.textContent('#lErr')) === '');
    await p.evaluate(() => document.activeElement && document.activeElement.blur());
    await p.click('#lAmount'); await p.keyboard.type('5'); await p.waitForTimeout(150);
    check('create form: one click and typing replaces the price instead of adding to it, in stream mode (102, R1)', (await p.inputValue('#lAmount')) === '5', await p.inputValue('#lAmount'));
    await p.evaluate(() => document.activeElement && document.activeElement.blur());
    const rd = await p.evaluate(() => Object.fromEntries(['lAmount', 'lPeriod', 'lTrial', 'lName'].map((id) => [id, getComputedStyle(document.getElementById(id)).color !== 'rgba(0, 0, 0, 0)'])));
    check('stream mode: the create form\'s price, period, trial and name boxes stay readable (102, R1, R2)', Object.values(rd).every(Boolean), rd);
    const l2 = await p.evaluate(leaks);
    check('new site, stream mode: nothing private readable on the create form', l2.length === 0, l2);
    // R3: the locked-in list's top off screen, the button low on the screen, the way it was on air.
    // Room below the form, so the page can scroll that far (on air the menu and footer gave it).
    await p.evaluate(() => { document.body.style.paddingBottom = '600px'; const lb = document.getElementById('lockBox').getBoundingClientRect(); window.scrollBy(0, lb.top + 60); });
    await p.waitForTimeout(300);
    const r0 = await p.evaluate(() => { const r = document.getElementById('lGo').getBoundingClientRect(); return { top: r.top, bottom: r.bottom, vh: innerHeight, lock: document.getElementById('lockBox').getBoundingClientRect().top }; });
    await p.click('#lGo'); await p.waitForTimeout(500);
    const r1 = await p.evaluate(() => ({ top: document.getElementById('lGo').getBoundingClientRect().top, text: document.getElementById('lGo').textContent }));
    check('create: the first press leaves the button where it was at 1280 x 720 (102, R3)', r0.lock < 0 && r0.bottom <= r0.vh && Math.abs(r1.top - r0.top) < 2 && /Press again/.test(r1.text), { r0, r1 });
    if (SHOTS) await p.screenshot({ path: SHOTS + '/empty-create-stream-1280x720.png' });
    await p.evaluate(() => { document.body.style.paddingBottom = ''; });
    await p.waitForTimeout(7000);
    check('create: the second press still waits after 7 seconds (102, R4)', /Press again/.test(await p.textContent('#lGo')));
    // R5: once the plan is created, the form goes at once (no reload) and one Put it on sale button shows.
    await p.evaluate(() => launchDone({ card: { covenant_id: 'cd'.repeat(32) }, txid: 'ef'.repeat(32), card_file: 'card-new.json' }));
    await p.waitForTimeout(800);
    const r5 = await p.evaluate(() => ({
      body: !document.getElementById('lBody').classList.contains('hidden'),
      go: !!document.getElementById('lGo').offsetParent,
      add: document.querySelectorAll('#lStatus #lAdd').length,
      title: document.getElementById('launchTitle').textContent,
      bar: document.getElementById('walkTitle').textContent,
    }));
    check('create: once created, the form is put away at once and one Put it on sale button shows (102, R5)', !r5.body && !r5.go && r5.add === 1 && /Step 3/.test(r5.title), r5);
    check('create: once created, the guide moves to Put it on sale without a reload (102, R5)', r5.bar === 'Put it on sale', r5);
    await p.evaluate(() => document.activeElement && document.activeElement.blur());
    const l3 = await p.evaluate(leaks);
    check('new site, stream mode: nothing private readable after creating', l3.length === 0, l3);
    if (SHOTS) await p.screenshot({ path: SHOTS + '/empty-created-stream-1280x720.png' });
    allErrs.push(...errs); await ctx.close();
  }

  // 3. A plan created but not on sale (reloaded in between): one "Put it on sale" button, no create form.
  {
    const { ctx, p, errs } = await signedIn(b, { record: true, pending: true });
    const st = await p.evaluate(() => ({
      resume: !!document.getElementById('lResume') && !!document.querySelector('#lResume #lAdd'),
      form: !document.getElementById('lBody').classList.contains('hidden'),
      title: document.getElementById('launchTitle').textContent,
      words: document.getElementById('lResume') ? document.getElementById('lResume').textContent : '',
      s2: document.querySelector('#walkList li[data-walk="plan"] .ws').textContent,
      bar: document.getElementById('walkTitle').textContent,
      buttons: document.querySelectorAll('#lResume button').length,
    }));
    check('new site, plan created but not on sale: the create form is hidden and one Put it on sale button shows (W13)', st.resume && !st.form && st.buttons === 1 && /One thing left: put it on sale/.test(st.words), st);
    check('new site, plan created but not on sale: step 2 Done, the bar on Put it on sale', st.s2 === 'Done' && st.bar === 'Put it on sale', st);
    await p.click('#walkShow'); await p.waitForTimeout(400);
    check('new site: Show me on step 3 rings the Put it on sale button', await p.evaluate(() => !!document.querySelector('#lAdd.walk-here')));
    await p.click('#lAdd'); await p.waitForTimeout(200);
    check('new site: Put it on sale asks for a second press', (await p.textContent('#lAdd')) === 'Press again to save');
    await p.click('label:has(#streamSw)'); await p.waitForTimeout(500);
    await p.evaluate(() => document.activeElement && document.activeElement.blur());
    const l = await p.evaluate(leaks);
    check('new site, stream mode: nothing private readable on the Put it on sale screen', l.length === 0, l);
    if (SHOTS) await p.screenshot({ path: SHOTS + '/empty-resume-stream-1280x720.png' });
    allErrs.push(...errs); await ctx.close();
  }

  check('new site: no script errors', allErrs.length === 0, allErrs);
  await b.close();
  console.log(ok ? 'EMPTY SITE OK' : 'EMPTY SITE FAILED');
  process.exit(ok ? 0 : 1);
})().catch((e) => { console.log('FAIL empty-site test crashed: ' + e.message); process.exit(1); });
