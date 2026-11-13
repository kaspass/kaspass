// Runs before the page draws: picks light or dark from the visitor's own choice (remembered), or
// from a partner site's link (?theme=light or ?theme=dark, also day/night), so nothing flashes.
(function () {
  try {
    var q = new URLSearchParams(location.search).get("theme");
    var t = { light: "light", day: "light", dark: "dark", night: "dark" }[(q || "").toLowerCase()];
    if (t) localStorage.setItem("kp-theme", t); else t = localStorage.getItem("kp-theme");
    if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  } catch (e) {}
})();
