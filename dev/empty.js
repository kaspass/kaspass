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
