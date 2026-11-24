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
      document.documentElement.classList.toggle("cards-open", s !== "kaspass");
    }
  } catch (e) {}
  // Kaspa Vision's card style has a day and a night look: with no choice made yet, start in the
  // visitor's own (as kaspa.vision does), not always at night (UI88 M20).
  try {
    var r = document.documentElement;
    if (!r.dataset.theme && r.dataset.cards === "kaspa-vision") r.dataset.theme = matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch (e) {}
})();
// The day and night switch (#themeToggle), on every page that shows one: the visitor's choice is
// remembered. Pages call it again after showing the switch late (the membership page's card style).
window.kpThemeSwitch = function () {
  var b = document.getElementById("themeToggle");
  // Kaspa Vision's card style always has both looks (also when a link chose it).
  if (b && b.classList.contains("hidden") && document.documentElement.dataset.cards === "kaspa-vision") { b.dataset.start = "device"; b.classList.remove("hidden"); }
  if (!b || b.classList.contains("hidden") || b.dataset.wired) return;
  b.dataset.wired = "1";
  var start = b.dataset.start || "light";
  var root = document.documentElement;
  function current() { return root.dataset.theme || (start === "device" ? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light") : start); }
  function paint() { var d = current() === "dark"; b.classList.toggle("isdark", d); b.setAttribute("aria-label", d ? "Switch to light" : "Switch to dark"); }
  b.addEventListener("click", function () {
    var next = current() === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    try { localStorage.setItem("kp-theme", next); } catch (e) {}
    paint();
  });
  paint();
};
document.addEventListener("DOMContentLoaded", function () { window.kpThemeSwitch(); });
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
