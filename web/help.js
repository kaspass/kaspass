// /help: a note for test networks (no real money), filled in from the site's own settings.
(async () => {
  // On a phone, the phone steps come first and link to the app store.
  const android = /Android/i.test(navigator.userAgent), phone = /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
  if (!phone) document.getElementById("startComputer").after(document.getElementById("startPhone"));
  // The QR-code section only where this site takes QR payments.
  try {
    const o = await (await fetch("/v1/offer")).json();
    if (!o.payin) {
      document.getElementById("qr").hidden = true;

    }
  } catch {
    document.getElementById("qr").hidden = true;
  }
  try {
    const j = await (await fetch("/v1/levels")).json();
    if (!j.network || j.network === "mainnet") return;
    const p = document.getElementById("testnet");
    const net = j.network === "testnet-10" ? "Testnet-10" : String(j.network).replace(/[^a-z0-9-]/gi, "");
    p.innerHTML = `This site runs on <b>${net}</b>, a test network: switch Kaspire to it and get free test KAS from the <a href="https://faucet-testnet.kaspanet.io" target="_blank" rel="noopener">faucet</a> instead of buying KAS. Nothing here costs real money.`;
    p.hidden = false;
  } catch {}
})();
