// Creator dashboard: paint the creator's saved theme before anything shows, so the page never
// flashes the default colours first. dash-theme.js does everything else.
(function () {
  var root = document.documentElement, HEX = /^#[0-9a-f]{6}$/i, KEYS = ["bg", "card", "text", "accent"];
  function lum(h) {
    var c = [1, 3, 5].map(function (i) { var v = parseInt(h.slice(i, i + 2), 16) / 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  }
  function ratio(a, b) { var x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); }
  try {
    var mode = localStorage.getItem("kp-theme");
    if (mode !== "light" && mode !== "dark") mode = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    var s = JSON.parse(localStorage.getItem("kp-dash-look") || "null");
    var look = Array.isArray(s) ? s[1] : null;
    var p = look && look.theme && look.theme[mode];
    if (!p || !KEYS.every(function (k) { return HEX.test(p[k] || ""); })) {
      // No saved theme: the KasPass teal, in day or night.
      p = mode === "dark" ? { bg: "#0b1615", text: "#f2f5f4", accent: "#49d9c8", card: "#12211f" } : { bg: "#f2f5f4", text: "#0f2321", accent: "#0b7a6e", card: "#ffffff" };
      look = null;
    }
    KEYS.forEach(function (k) { root.style.setProperty("--" + k, p[k]); });
    root.style.setProperty("--on-accent", ratio(p.accent, "#ffffff") >= ratio(p.accent, "#0a0a0a") ? "#ffffff" : "#0a0a0a");
    var ch = look && look.theme.chart && look.theme.chart[mode];
    if (Array.isArray(ch) && ch.length === 4 && ch.every(function (c) { return HEX.test(c); })) ch.forEach(function (c, i) { root.style.setProperty("--chart-" + (i + 1), c); });
    root.setAttribute("data-tone", lum(p.bg) < 0.3 ? "dark" : "light");
    root.setAttribute("data-glow", look && look.glow === false ? "off" : "on");
  } catch (e) {}
})();
