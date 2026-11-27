// The website's pages, signed out, on a phone and a computer, in light and dark: each loads with no
// script errors and says what it should. Run by dev/check.sh against a local simulated site.
const { chromium, devices } = require('playwright');
const S = process.argv[2] || 'http://127.0.0.1:8790';
let ok = true;
const check = (name, cond, extra) => { ok = ok && !!cond; console.log((cond ? 'PASS ' : 'FAIL ') + name + (!cond && extra !== undefined ? '   ' + JSON.stringify(extra).slice(0, 400) : '')); };
// Plain http, so a test can ask as another host name (fetch won't change the Host header).
const text = (u, h = {}) => new Promise((resolve, reject) => {
  require('http').get(S + u, { headers: h }, (r) => { let body = ''; r.setEncoding('utf8'); r.on('data', (c) => (body += c)); r.on('end', () => resolve({ status: r.statusCode, body })); }).on('error', reject);
});

(async () => {
  // Every page answers.
  for (const u of ['/', '/creators', '/help', '/privacy', '/terms', '/demo/podcast', '/demo/nft', '/demo/coffee', '/demo/newsletter', '/dashboard', '/club', '/robots.txt', '/sitemap.xml', '/favicon.ico', '/v1/levels', '/v1/tier', '/v1/page']) {
    check(u + ' answers', (await text(u)).status === 200, u);
  }
  const home = (await text('/')).body;
  check('home: the headline', home.includes('Get paid in KAS, straight to your vault.'));
  check('home: access running itself is said in the hero', home.includes('removed when they stop, without you doing a thing'));
  check('home: the three points', ['Paid to you, not to us.', 'Access runs itself.', "2%, and that's all."].every((t) => home.includes(t)));
  check('home: "Why not a card platform, or doing it by hand?"', home.includes('Why not a card platform, or doing it by hand?'));
  check('home: Kaspa Vision is said to be made by the same team', home.includes('made by the same team as KasPass'));
  check('home: the first question is "Is it ready, and is it safe?"', /<div class="faq">\s*<details open><summary>Is it ready, and is it safe\?/.test(home));
  check('home: the contact from contact.json', home.includes('@example_handle on Telegram'));
  check('home: no em dashes', !home.includes('—'));
  // Promised to KasTax in reply 93 (102): every plan in /v1/levels has its full id as `level`,
  // and `id` is its first 12 characters.
  {
    const lv = JSON.parse((await text('/v1/levels')).body).levels || [];
    const bad = lv.filter((l) => !/^[0-9a-f]{64}$/.test(l.level || '') || l.id !== l.level.slice(0, 12));
    check('/v1/levels: every plan has level (64 hex) and id is its first 12 characters (102)', lv.length > 0 && bad.length === 0, bad);
  }
  // Update 102 (RH101 R18): a pass is a coin the member owns, not something in their wallet's balance
  // (reply 81's wording), and the share card is fetched again under a new version.
  {
    const old = /(?:lives|sits|sitting) in (?:your|their|a|the member's)(?: own)?(?: Kaspa)? wallet|In the member's own Kaspa wallet|is in their wallet a few seconds|pass in the member's Kaspa wallet/;
    const bodies = await Promise.all(['/', '/creators', '/help', '/guide.md', '/assets/index.js', '/demo/podcast'].map(async (u) => [u, (await text(u)).body]));
    const bad = bodies.filter(([, b]) => old.test(b)).map(([u, b]) => u + ': ' + b.match(old)[0]);
    check('pages: no "lives in your own Kaspa wallet" wording left (102, R18)', bad.length === 0, bad);
    check('home and creators: the new share card, og.png?v=3, with matching alt text (102, R18)', ['/', '/creators'].every((u) => { const b = bodies.find(([x]) => x === u)[1]; return b.includes('og.png?v=3') && b.includes("a coin the member owns on Kaspa's L1"); }));
  }
  for (const u of ['/', '/creators', '/privacy', '/terms']) {
    const b = (await text(u)).body;
    check(u + ': one footer, with the X page', b.includes('<a href="https://x.com/Kas_Pass" rel="noopener">X (Twitter)</a>') && b.includes('>Security</a>'));
    check(u + ': the logo in the header', /<a class="logo"[^>]*>\s*<svg viewBox="19.6 14 64.7 72"/.test(b));
  }
  for (const u of ['/privacy', '/terms']) check(u + ': last updated', (await text(u)).body.includes('Last updated'));
  check('creators: the form heading', (await text('/creators')).body.includes('Tell us about your membership'));
  check('help on the KasPass site: its look and a way home', (await text('/help')).body.includes('Bricolage Grotesque') && (await text('/help')).body.includes('href="/">Home</a>'));
  check("help on a creator's site: the creator's look", !(await text('/help', { host: 'members.example.com' })).body.includes('Bricolage Grotesque'));
  // Update 97 (UI88 M1): Help, Guide and Holder club on a creator's site in the membership page's
  // look: its card style from the first frame, cards.css, the creator's name at the top instead of
  // "KasPass", a day and night switch, and the membership page's footer sentence.
  for (const u of ['/help', '/guide', '/club']) {
    const h = (await text(u, { host: 'members.example.com' })).body;
    const v = { style: /<html lang="en" data-cards="(?:kaspass|open|kaspa-vision)"/.test(h), css: h.includes('/assets/cards.css'), name: /<a class="brandlink" href="\/">(?:<img[^>]*>)?<span class="word">(?!KasPass<)[^<]+<\/span>/.test(h), sw: h.includes('id="themeToggle"') && !h.includes('<!--THEME-->'), foot: h.includes('Test network only. New software, not yet reviewed by an outside security firm.') };
    check(`${u} on a creator's site: the membership page's look, name, switch and footer (97)`, Object.values(v).every(Boolean), v);
  }
  const demo = (await text('/demo/podcast')).body;
  check('example page: a link preview with its own address', /<meta property="og:url" content="http:\/\/127\.0\.0\.1:\d+\/demo\/podcast">/.test(demo) && demo.includes('<meta property="og:title"'));
  check('example page: no stand-ins left in the preview', !demo.includes('%%PAGE_'));

  const b = await chromium.launch(process.env.KASPASS_CHROME ? { executablePath: process.env.KASPASS_CHROME } : {});
  const contexts = {
    phone: await b.newContext({ ...devices['iPhone 13'] }),
    desk: await b.newContext({ viewport: { width: 1280, height: 800 } }),
    dark: await b.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: 'dark' }),
  };
  for (const [name, ctx] of Object.entries(contexts)) {
    for (const u of ['/', '/creators', '/help', '/privacy', '/terms', '/demo/podcast']) {
      const p = await ctx.newPage();
      const errs = []; p.on('pageerror', (e) => errs.push(e.message));
      await p.goto(S + u); await p.waitForTimeout(u.startsWith('/demo') ? 3000 : 800);
      check(`${name} ${u}: no script errors`, errs.length === 0, errs);
      check(`${name} ${u}: no sideways scrolling`, await p.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1));
      await p.close();
    }
  }
  // Update 97 (UI88 M1, M20): in Kaspa Vision's card style every page starts in the device's own
  // look (day or night), with a 44 px switch that changes it; no sideways scrolling.
  {
    const L = S.replace('127.0.0.1', 'localhost');
    for (const scheme of ['light', 'dark']) {
      const ctx = await b.newContext({ viewport: { width: 390, height: 844 }, colorScheme: scheme });
      for (const u of ['/', '/help', '/guide', '/club']) {
        const q = await ctx.newPage();
        const errs = []; q.on('pageerror', (e) => errs.push(e.message));
        await q.goto(`${L}${u}?style=kaspa-vision`); await q.waitForTimeout(u === '/' ? 3000 : 1000);
        const v = await q.evaluate(() => { const t = document.getElementById('themeToggle'); const r = t ? t.getBoundingClientRect() : { width: 0, height: 0 }; return { theme: document.documentElement.dataset.theme, bg: getComputedStyle(document.body).backgroundColor, toggle: !!t && !t.classList.contains('hidden') && r.width >= 44 && r.height >= 44, flat: document.documentElement.scrollWidth <= innerWidth + 1 }; });
        const want = scheme === 'light' ? 'rgb(244, 249, 247)' : 'rgb(8, 16, 19)';
        check(`${u} in Kaspa Vision's style on a ${scheme} device: starts in that look, with a 44 px day and night switch, no sideways scrolling (97)`, v.theme === scheme && v.bg === want && v.toggle && v.flat && !errs.length, [v, errs]);
        if (v.toggle) {
          await q.click('#themeToggle'); await q.waitForTimeout(200);
          const after = await q.evaluate(() => document.documentElement.dataset.theme);
          check(`${u} in Kaspa Vision's style: the switch changes the look`, after === (scheme === 'light' ? 'dark' : 'light'), after);
        }
        await q.evaluate(() => { try { localStorage.removeItem('kp-theme'); } catch {} });
        await q.close();
      }
      await ctx.close();
    }
    // The membership page: 44 px tap targets on a phone (UI88 M18), the menu closes on Escape and
    // on a click outside it, and a 2 px focus ring for the keyboard (UI88 M19).
    const q = await contexts.phone.newPage();
    await q.goto(L + '/'); await q.waitForTimeout(3000);
    const small = await q.evaluate(() => [...document.querySelectorAll('#navSignin, .brandmenu summary, #themeToggle, #signin, #qrGo, .lvl .chg, button.linkish, .welcome .close, details:not(.brandmenu) > summary, .meta a')].filter((e) => e.getClientRects().length && e.offsetParent).filter((e) => e.getBoundingClientRect().height < 44).map((e) => (e.id || e.className || e.tagName) + ' ' + Math.round(e.getBoundingClientRect().height)));
    check('membership page on a phone: header, plan and fold buttons are at least 44 px tall (97, UI88 M18)', small.length === 0, small);
    await q.click('.brandmenu summary'); await q.waitForTimeout(200);
    const opened = await q.evaluate(() => document.querySelector('.brandmenu').open);
    await q.keyboard.press('Escape'); await q.waitForTimeout(200);
    const esc = await q.evaluate(() => !document.querySelector('.brandmenu').open && document.activeElement === document.querySelector('.brandmenu summary'));
    await q.click('.brandmenu summary'); await q.waitForTimeout(200);
    await q.mouse.click(12, 600); await q.waitForTimeout(200);
    const outside = await q.evaluate(() => !document.querySelector('.brandmenu').open);
    check('membership page: the menu closes on Escape and on a click outside it (97, UI88 M19)', opened && esc && outside, { opened, esc, outside });
    await q.close();
    const k = await contexts.desk.newPage();
    await k.goto(L + '/'); await k.waitForTimeout(3000);
    await k.keyboard.press('Tab'); await k.keyboard.press('Tab');
    const ring = await k.evaluate(() => { const e = document.activeElement; const cs = getComputedStyle(e); return { el: e.tagName + '#' + e.id, w: cs.outlineWidth, s: cs.outlineStyle }; });
    check('membership page: a 2 px focus ring for keyboard users (97, UI88 M19)', ring.w === '2px' && ring.s === 'solid', ring);
    await k.close();
  }
  {
    const g = (await text('/guide.md')).body;
    check('guide: the plan badge, the card fields partner sites read, and the matching day and night look (97)', g.includes('its **badges**') && g.includes('`badges_custom`') && g.includes('**Back to automatic**') && g.includes('`highlight`') && g.includes('sun and moon button'));
  }
  {
    const g = (await text('/guide.md')).body;
    const top = g.slice(0, g.indexOf('## Contents'));
    check('guide: people read the guide first; the AI note and review date come after the contents, and each section ends with "Back to contents" (98, UI88 L6)', !top.includes('For AI assistants') && !top.includes('Last reviewed') && /About this guide[\s\S]*For AI assistants/.test(g) && (g.match(/\[Back to contents\]\(#contents\)/g) || []).length >= 10);
  }
  // Membership page: each "Choose this plan" button names its plan for screen readers (98, UI88 L3);
  // a card with two lines or fewer has no "Hide details" toggle (98, UI88 L8).
  {
    const L = S.replace('127.0.0.1', 'localhost');
    const q = await contexts.desk.newPage();
    await q.goto(L + '/?style=kaspass'); await q.waitForTimeout(3500);
    const v = await q.evaluate(async () => {
      const ls = (await (await fetch('/v1/levels')).json()).levels;
      const choose = [...document.querySelectorAll('#levelCards .lvl button')].filter((b) => /^Choose this /.test(b.textContent));
      const labels = choose.map((b) => b.getAttribute('aria-label') || '');
      const short = ls.filter((l) => !l.about_html && ((l.rank > 0 ? 1 : 0) + (l.perks || []).length) <= 2).map((l) => l.name);
      const cards = [...document.querySelectorAll('#levelCards .lvl')];
      const toggles = short.map((n) => { const c = cards.find((x) => ((x.querySelector('.ln') || {}).textContent || '').startsWith(n)); return c ? !!c.querySelector('.more-t') : null; });
      return { labels, names: ls.map((l) => l.name), toggles };
    });
    check('membership page: "Choose this plan" buttons name their plan for screen readers (98, UI88 L3)', v.labels.length > 0 && v.labels.every((x) => v.names.some((n) => x === `Choose ${n}`)) && new Set(v.labels).size === v.labels.length, v.labels);
    check('membership page: no "Hide details" toggle over two lines or fewer (98, UI88 L8)', v.toggles.length > 0 && v.toggles.every((t) => t === false), v);
    await q.close();
    const rc = await b.newContext({ viewport: { width: 1366, height: 900 }, reducedMotion: 'reduce' });
    const r = await rc.newPage();
    await r.goto(L + '/'); await r.waitForTimeout(1500);
    check('reduced motion: no smooth scrolling (98, UI88 L9)', await r.evaluate(() => !/native code/.test(Element.prototype.scrollIntoView.toString())));
    await rc.close();
  }
  // The guide on a phone: code blocks scroll inside themselves, not the page. (A plain 390 px window:
  // a phone emulation widens the window to fit the page, so it can't see this.)
  {
    const nc = await b.newContext({ viewport: { width: 390, height: 844 } });
    const q = await nc.newPage();
    await q.goto(S.replace('127.0.0.1', 'localhost') + '/guide'); await q.waitForTimeout(800);
    check('guide on a phone: no sideways scrolling (97)', await q.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    await nc.close();
  }
  // Plan cards: "What's included" open, the trial explained once, buttons lined up.
  const p = await contexts.desk.newPage();
  await p.goto(S + '/demo/podcast'); await p.waitForTimeout(4000);
  const cards = await p.$$eval('#levelCards .lvl', (cs) => cs.map((c) => ({ open: [...c.querySelectorAll('.det')].every((d) => d.hidden === !d.querySelector('li, .about, .inc')), btn: [...c.querySelectorAll(':scope > button:not(.more-t)')].map((x) => Math.round(x.getBoundingClientRect().bottom)).pop(), sel: c.classList.contains('sel'), trial: !!c.querySelector('.trialtag') })));
  check('plan cards: at least two', cards.length >= 2, cards);
  check('plan cards: what each includes is open', cards.every((c) => c.open), cards);
  check('plan cards: the buttons line up at the bottom', new Set(cards.map((c) => c.btn)).size === 1, cards);
  check('plan cards: no plan outlined before the visitor picks one', cards.every((c) => !c.sel), cards);
  const about = await p.textContent('#trialAbout').catch(() => '');
  check('plan cards: the free trial explained once, above the plans, and its plan tagged', /free for 7 days/.test(about) && /One free trial per person/.test(about) && cards.some((c) => c.trial), [about, cards]);
  // Every amount carries its currency: never a bare "$" (en-CA and en-US browsers write CA$ and US$ as "$"),
  // in every card style, the Compare view and on a phone, and in the dashboard's preview text.
  const bare = (t) => (t.match(/(^|[^A-Za-z])\$\s?\d[\d,.]*|\d\s?\$(?![A-Za-z0-9{])/g) || []);
  for (const locale of ['en-CA', 'en-US', 'fr-CA']) {
    for (const timezoneId of ['America/Vancouver', 'America/New_York']) {
      const ctx = await b.newContext({ locale, timezoneId, viewport: { width: 390, height: 844 } });
      for (const st of ['kaspass', 'open', 'kaspa-vision']) {
        const q = await ctx.newPage();
        await q.goto(`${S}/demo?style=${st}`); await q.waitForTimeout(2500);
        let t = await q.evaluate(() => document.body.innerText);
        if (st === 'kaspass') { await q.click('#vCompare').catch(() => {}); await q.waitForTimeout(500); t += await q.evaluate(() => document.body.innerText); }
        check(`prices show their currency: ${locale}, ${timezoneId}, ${st}`, !bare(t).length && /CA\$\d/.test(t), bare(t));
        if (st === 'kaspass') {
          // A site set to show prices in the visitor's currency: the converted amount gets its own label.
          const v = await q.evaluate(() => { PRICE_DISPLAY = 'viewer'; FX = FX || { usd_cad: 1.38, eur_usd: 1.08 }; const r = { main: shown('CAD', 10).main, line: localLine('CAD', 10).replace(/<[^>]+>/g, '') }; PRICE_DISPLAY = 'plan'; return r; });
          const want = locale === 'en-US' && timezoneId === 'America/New_York' ? /^about US\$7 in your currency$/ : /^$/; // a Canadian language or time zone keeps CA$
          check(`CA$ first, plus about the visitor's own: ${locale}, ${timezoneId}`, v.main === 'CA$10' && want.test(v.line) && !bare(v.main + ' ' + v.line).length, v);
        }
        await q.close();
      }
      await ctx.close();
    }
  }
  // Update 99 (UI88 L1): one left edge on a computer (heading, plans, the boxes under them and the
  // footer), and one centred column under 940 px. (UI88 L14): "a year" never splits from the price
  // with a stray indent.
  for (const w of [1366, 768]) {
    const q = await b.newPage({ viewport: { width: w, height: 900 } });
    await q.goto(S.replace('127.0.0.1', 'localhost') + '/?style=kaspa-vision'); await q.waitForTimeout(3500);
    const e = await q.evaluate(() => {
      const box = (x) => { const r = x.getBoundingClientRect(); return [Math.round(r.left), Math.round(r.right)]; };
      const items = [document.querySelector('#levelsBox .lvhead'), document.getElementById('levelCards'), ...document.querySelectorAll('.wrap > section.panel:not(#levelsBox)'), document.querySelector('.wrap > footer')].filter((x) => x && x.offsetParent);
      const per = document.querySelector('.lvl .lp .per'); const cs = per && getComputedStyle(per);
      const fw = typeof forWallets === 'function' ? [forWallets('migrated at least 1,000,000 NACHO to KCC-20'), forWallets('at least 1 NACHO NFT, or at least 100 NACHO tokens'), forWallets('migrated at least 5 NACHO to KCC-20, or at least 1 NACHO NFT')] : null;
      return { fw, lefts: items.map((x) => box(x)[0]), mids: items.map((x) => { const [l, r] = box(x); return Math.round((l + r) / 2); }), per: cs ? cs.display + ' ' + cs.whiteSpace : null, flat: document.documentElement.scrollWidth <= innerWidth + 1 };
    });
    if (w > 940) check('membership page at 1366: heading, plans, boxes and footer share one left edge (99, UI88 L1)', e.lefts.length >= 3 && new Set(e.lefts).size === 1 && e.flat, e);
    else check('membership page at 768: heading, plans, boxes and footer in one centred column (99, UI88 L1)', e.mids.length >= 3 && Math.max(...e.mids) - Math.min(...e.mids) <= 2 && e.flat, e);
    check(`membership page at ${w}: "a year" stays whole after the price (99, UI88 L14)`, e.per === 'inline-block nowrap', e.per);
    if (w > 940) check('free month: the requirement reads "For wallets that…" (99, UI88 L7)', JSON.stringify(e.fw) === JSON.stringify(['For wallets that migrated at least 1,000,000 NACHO to KCC-20', 'For wallets that hold at least 1 NACHO NFT, or at least 100 NACHO tokens', 'For wallets that migrated at least 5 NACHO to KCC-20, or hold at least 1 NACHO NFT']), e.fw);
    await q.close();
  }
  // A plan chosen by link (?level=, as a partner site's plan button does): that plan alone, outlined,
  // with sign-in beside it as the one main button; "Change plan" shows the rest.
  {
    const ids = await (await fetch(S + '/v1/levels')).json().then((j) => j.levels.map((l) => l.level));
    const q = await contexts.phone.newPage();
    const errs = []; q.on('pageerror', (e) => errs.push(e.message));
    await q.goto(`${S}/demo?level=${ids[1]}&return=https://kaspa.vision/signin`); await q.waitForTimeout(3000);
    const f = await q.evaluate(() => ({ cards: document.querySelectorAll('#levelCards .lvl').length, tag: !!document.querySelector('#levelCards .lvl .yourtag'), wallet: document.getElementById('wallet').parentElement.id, buttons: [...document.querySelectorAll('#levelCards .lvl > button:not(.chg)')].filter((b) => b.offsetParent).length, step: document.getElementById('walletTitle').textContent }));
    check('chosen plan: shown alone, labelled, with the sign-in beside it as step 1', f.cards === 1 && f.tag && f.wallet === 'levelCards' && /^1\. Sign in/.test(f.step), f);
    check('chosen plan: no pay buttons until signed in', f.buttons === 0, f);
    check('chosen plan: labelled "You chose", not "Your plan", before buying (UI88 L4, 99)', await q.evaluate(() => document.querySelector('#levelCards .lvl .yourtag').textContent === 'You chose'));
    await q.click('#levelCards .chg'); await q.waitForTimeout(600);
    check('chosen plan: "Change plan" shows every plan, the pick still labelled', await q.evaluate(() => document.querySelectorAll('#levelCards .lvl').length > 1 && !!document.querySelector('#levelCards .lvl .yourtag')));
    check('chosen plan: no script errors', errs.length === 0, errs);
    await q.goto(`${S}/demo?level=nope`); await q.waitForTimeout(2500);
    check('unknown plan in a link: all plans and a note', await q.evaluate(() => document.querySelectorAll('#levelCards .lvl').length > 1 && /couldn't find that/.test(document.getElementById('levelsLead').textContent)));
    await q.close();
  }
  // Euro visitors: by the language's country, or by a euro time zone with a US-English browser; Bulgaria counts.
  for (const [locale, timezoneId, want] of [['de-DE', 'America/New_York', 'EUR'], ['en-US', 'Europe/Berlin', 'EUR'], ['bg-BG', 'Europe/Sofia', 'EUR'], ['en-GB', 'Europe/London', 'USD'], ['en-US', 'America/Chicago', 'USD'], ['en-US', 'America/Vancouver', 'CAD']]) {
    const ctx = await b.newContext({ locale, timezoneId });
    const q = await ctx.newPage();
    await q.goto(`${S}/demo`); await q.waitForTimeout(1500);
    const v = await q.evaluate(() => viewerCurrency());
    check(`visitor's currency: ${locale} in ${timezoneId} is ${want}`, v === want, v);
    await ctx.close();
  }
  await b.close();
  console.log(ok ? 'ALL OK' : 'SOME FAILED');
  process.exit(ok ? 0 : 1);
})().catch((e) => { console.log('Error', e); process.exit(1); });
