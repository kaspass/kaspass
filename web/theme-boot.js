// Runs before the page draws: picks light or dark from the visitor's own choice (remembered), or
// from a partner site's link (?theme=light or ?theme=dark, also day/night), so nothing flashes.
(function () {
  try {
    var q = new URLSearchParams(location.search).get("theme");
    var t = { light: "light", day: "light", dark: "dark", night: "dark" }[(q || "").toLowerCase()];
    if (t) localStorage.setItem("kp-theme", t); else t = localStorage.getItem("kp-theme");
    if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  } catch (e) {}
  // The plan cards' style from a partner site's link (?style=), one of the built-in styles only,
  // remembered for this tab so it survives signing in.
  try {
    var s = (new URLSearchParams(location.search).get("style") || "").toLowerCase();
    if (["kaspass", "open", "kaspa-vision"].indexOf(s) >= 0) sessionStorage.setItem("kaspass.style", s); else s = sessionStorage.getItem("kaspass.style");
    if (s === "kaspass" || s === "open" || s === "kaspa-vision") {
      document.documentElement.dataset.cards = s;
      if (s !== "kaspass") document.documentElement.classList.add("cards-open");
    }
  } catch (e) {}
})();
// Stream mode is on in this browser: keep the page hidden until stream.js has started, so if
// stream.js ever fails to load the page stays blank instead of showing private details.
(function () {
  try {
    if (localStorage.getItem("kaspass.stream") === "1" || /[?&]stream=1(?:&|$)/.test(location.search)) {
      document.documentElement.classList.add("kp-wait");
      var s = document.createElement("style");
      s.textContent = "html.kp-wait body { visibility: hidden !important; }";
      document.head.appendChild(s);
    }
  } catch (e) {}
})();
