"""Settings → Connections: the Google Drive card. Not connected: one "Sign in with Google (Calendar + Drive)" button when
the build has a Google client, the own-client steps, three permission rows, English only in the English UI. Connected
(through the API): the chip shows the account, the quota line shows the usage, Test / Enable / Disconnect appear."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import drive_common as D  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = D.B + "/"
D.c.put(B + "api/settings", json={"language": "en", "ui_language": "en"}, headers=D.H)
D.disconnect_all()
D.reset_drive()
D.managed_client(True)


def han(s):
    return [l for l in s.splitlines() if any("一" <= ch <= "鿿" for ch in l)]


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        ctx = await br.new_context(viewport={"width": 1400, "height": 1200})
        await ctx.add_init_script("try { localStorage.setItem('omuse_ui', 'en') } catch (e) {}")
        pg = await ctx.new_page()
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#connections")
        await pg.wait_for_selector("#drive")
        card = pg.locator("#drive")
        txt = await card.text_content()   # includes the collapsed own-client steps
        check("not connected: the one-click button connects Calendar + Drive together",
              "Not connected" in txt and await card.locator("button", has_text="Sign in with Google (Calendar + Drive)").count() == 1, txt[:300])
        check("the own-client steps mention enabling the Drive API and the relay page", "Google Drive API" in txt and "relay" in txt.lower() or "redirect" in txt.lower(), txt[:600])
        perms = await card.locator(".perm").all_inner_texts()
        check("three permission rows: read, upload, write", len(perms) == 3
              and all(any(x in p for p in perms) for x in ("View, search and download files", "Upload files and create folders", "Move, rename and move to the bin")), perms)
        check("English UI: no Chinese on the card", not han(txt), han(txt)[:3])
        # connect through the API (the sign-in itself is covered by drive_connect_e2e) and reload
        D.connect_drive()
        await pg.reload()
        await pg.wait_for_selector("#drive")
        txt = await pg.locator("#drive").inner_text()
        check("connected: the chip names the account and the quota line shows the usage",
              "lucas@example.com" in txt and "GB used of 16" in txt, txt[:400])
        btns = {w: await pg.locator("#drive button", has_text=w).count() for w in ("Test", "Disconnect")}
        check("connected: Test and Disconnect are offered; the button now says sign in again",
              btns == {"Test": 1, "Disconnect": 1} and "Sign in with Google again" in txt, (btns, D.conn("drive").get("mode"), [l for l in txt.splitlines() if "Sign in" in l]))
        await pg.locator("#drive button", has_text="Test").click()
        await pg.wait_for_selector(".toast")
        toast = await pg.locator(".toast").inner_text()
        check("Test reads the account", "lucas@example.com" in toast and "GB used" in toast, toast)
        check("the calendar card shows the same account (one sign-in)", "lucas@example.com" in await pg.locator(".card", has_text="Google Calendar").first.inner_text())
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
D.disconnect_all()
finish()
