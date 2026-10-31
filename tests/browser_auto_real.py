"""Auto-approve browser actions, with a real model on a real website, from the user's point of view (see
tests/user_common.py). The user's mailbox has their Tokyo hotel booking (Hotel Ginza Grand, 8-7-6 Ginza).

The user asks Main's chat to find the booking in their mail and look the hotel up on Google Maps (named, not its
domain: a domain written in the request is trusted and would skip the check). OMuse finds the place with SerpApi
(places_search, the Google Maps data) and loads its Google Maps page (the result's map_url) in the browser. Reading mail taints the task, and
opening a page with the address in its URL on a site the task has not used yet is exactly what Sentinel stops for
approval — so that step always needs one. What the user does and sees:
* on: in Settings the user turns "Auto-approve browser actions" on and asks; no approval dialog appears, the chat (Main's
  or the bot's conversation behind "Continue in …") shows the "Sentinel auto-approved: Open page · auto-approved"
  card, and the answer names the hotel from the mail and tells what the map found;
* off: in Settings the user turns it off and asks the same; an approval dialog to open the Google Maps page appears (the user
  approves it), no auto-approved card is shown, and the answer names the hotel.
One TIMING line per request (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_MODELS=alt bash tests/run_in_docker.sh browser_auto_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

PROMPT = ("Find my Tokyo hotel booking in my email, then look the hotel up on Google Maps and open its map page, "
          "and tell me the hotel name, address and whether the map found it. Research only.")
MAIL = [
    ("Hotel Ginza Grand <reservations@ginzagrand.example>", "Booking confirmed: Hotel Ginza Grand 20-23 Nov 2026",
     "Confirmation 88812345 for Lucas Lu. Hotel Ginza Grand, 8-7-6 Ginza, Chuo City, Tokyo 104-0061, Japan. Check-in 20 Nov 2026 15:00.", 2),
]
AUTO = re.compile(r"auto-approved", re.I)


async def set_auto(u: U.User, on: bool) -> None:
    """Settings → "Auto-approve actions": the user ticks or unticks the box (it saves at once, with a toast)."""
    pg = u.page
    await pg.goto(U.APP + "#settings")
    box = pg.locator("label.toggle", has_text="Auto-approve actions").locator("input[type=checkbox]")
    await box.wait_for(timeout=20000)
    if await box.is_checked() != on:
        await box.click()
        toast = "approved automatically" if on else "need your approval"
        await pg.wait_for_function("t => document.body.innerText.includes(t)", arg=toast, timeout=10000)
    await pg.reload()
    await box.wait_for(timeout=20000)
    check(f"Settings shows \"Auto-approve actions\" {'on' if on else 'off'} after a reload", await box.is_checked() == on)


async def cards(u: U.User) -> list[str]:
    """The cards the user sees for this request: in Main's chat and, when a bot answered, in that bot's conversation."""
    out = await u.thread_cards()
    pg = u.page
    link = pg.locator(".msg.assistant .handoff-link").last
    if await link.count():
        back = pg.url
        await link.click()
        try:
            await pg.wait_for_selector(".msg.assistant:not(.live) .bubble", timeout=30000)
        except Exception:   # noqa: BLE001
            pass
        await pg.wait_for_timeout(1000)
        out += await u.thread_cards()
        await pg.goto(back)
        await pg.wait_for_selector("#chatInput")
    print(f"INFO cards: {out}")
    return out


async def main():
    async with U.User() as u:
        # ---- on: the map page is approved automatically; no dialog, an auto-approved card
        await set_auto(u, True)
        a = await u.ask(PROMPT, label="on")
        c = await cards(u)
        check("on: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("on: no approval dialog appeared", not a.approvals, a.approvals)
        auto = [x for x in c if AUTO.search(x)]
        check("on: the chat shows the browser action as auto-approved", bool(auto), c)
        check("on: the auto-approved card is the page load (\"Open page\")", any(re.search(r"open page", x, re.I) for x in auto), auto)
        check("on: the answer tells what the map (Google Maps) found", a.mentions("Google Maps") or any("google.com/maps" in x for x in a.links),
              (a.text[:400], a.links))
        check("on: the answer names the hotel from the mail", "Ginza Grand" in a.text, a.text[:400])

        # ---- off: the same request stops at the map page until the user approves
        await set_auto(u, False)
        a = await u.ask(PROMPT, label="off")
        c = await cards(u)
        check("off: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("off: an approval dialog for the map site appeared (approved by the user)",
              any("google.com/maps" in x.lower() or "google maps" in x.lower() for x in a.approvals), a.approvals)
        check("off: no action was auto-approved", not [x for x in c if AUTO.search(x)], c)
        check("off: the answer names the hotel from the mail", "Ginza Grand" in a.text, a.text[:400])


U.setup(mail=MAIL, vault=False)
U.run(main)
