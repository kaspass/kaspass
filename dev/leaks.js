// Shared by dev/dashboard.js and dev/empty.js: run in the page with p.evaluate(leaks). It lists every
// piece of text, box, dropdown, tooltip or link a viewer could read that looks private, for the
// stream mode checks. It must stay self-contained (Playwright sends its source to the page).
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
    // Words a page marks as public (the Get started guide, the locked-in list's labels) stay
    // readable on purpose; amounts inside them still carry kp-priv and are checked below.
    if (el.closest('[data-public]') && !el.closest('.kp-priv')) continue;
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
    // A box the page marks public (a plan's name, price and lengths in the create form) stays readable.
    if (i.closest('[data-public]')) continue;
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

module.exports = leaks;
