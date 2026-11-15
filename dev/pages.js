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
  for (const u of ['/', '/creators', '/privacy', '/terms']) {
    const b = (await text(u)).body;
    check(u + ': one footer, with the X page', b.includes('<a href="https://x.com/Kas_Pass" rel="noopener">X (Twitter)</a>') && b.includes('>Security</a>'));
    check(u + ': the logo in the header', /<a class="logo"[^>]*>\s*<svg viewBox="19.6 14 64.7 72"/.test(b));
  }
  for (const u of ['/privacy', '/terms']) check(u + ': last updated', (await text(u)).body.includes('Last updated'));
  check('creators: the form heading', (await text('/creators')).body.includes('Tell us about your membership'));
  check('help on the KasPass site: its look and a way home', (await text('/help')).body.includes('Bricolage Grotesque') && (await text('/help')).body.includes('href="/">Home</a>'));
  check("help on a creator's site: the creator's look", !(await text('/help', { host: 'members.example.com' })).body.includes('Bricolage Grotesque'));
  const demo = (await text('/demo/podcast')).body;
  check('example page: a link preview with its own address', /<meta property="og:url" content="http:\/\/127\.0\.0\.1:\d+\/demo\/podcast">/.test(demo) && demo.includes('<meta property="og:title"'));
  check('example page: no stand-ins left in the preview', !demo.includes('%%PAGE_'));

  const b = await chromium.launch();
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
  await b.close();
  console.log(ok ? 'ALL OK' : 'SOME FAILED');
  process.exit(ok ? 0 : 1);
})().catch((e) => { console.log('Error', e); process.exit(1); });
