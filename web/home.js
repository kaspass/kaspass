// KasPass's own website: the "Get your own page" form. It sends a short note to the site's owner,
// who reads it in the dashboard.
const f = document.getElementById("contactForm");
if (f) {
  const out = document.getElementById("cMsgOut");
  const say = (text, good) => { out.textContent = text; out.className = "msg " + (good ? "ok" : "bad"); };
  f.addEventListener("submit", async (e) => {
    e.preventDefault();
    const name = document.getElementById("cName").value.trim();
    const reach = document.getElementById("cReach").value.trim();
    const message = document.getElementById("cMsg").value.trim();
    if (!name || !reach) { say("Please add your name and how to reach you.", false); return; }
    if (message.length < 10) { say("Please say a little about what you'd like to sell.", false); return; }
    const go = document.getElementById("cGo");
    go.disabled = true; go.textContent = "Sending…";
    try {
      const r = await fetch("/v1/contact", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name, reach, message, website: document.getElementById("cWebsite").value }) });
      if (!r.ok) throw new Error((await r.text()) || "Something went wrong");
      f.reset();
      say("Thanks! Your note was sent. We'll reply through the contact you gave.", true);
      go.textContent = "Sent";
    } catch (err) {
      say((err && err.message) || "Couldn't send it right now. Please try again in a minute.", false);
      go.disabled = false; go.textContent = "Send";
    }
  });
}
