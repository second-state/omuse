"""Settings → Connections: the Google Drive card, not connected: the own-client steps, three permission rows, English
only in the English UI. (The connected state and the one-click sign-in went with the fake Google server.)"""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
K.c.put(B + "api/settings", json={"language": "en", "ui_language": "en"}, headers=K.H)


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
        check("not connected", "Not connected" in txt, txt[:300])
        check("the own-client steps mention enabling the Drive API and the relay page", "Google Drive API" in txt and "relay" in txt.lower() or "redirect" in txt.lower(), txt[:600])
        perms = await card.locator(".perm").all_inner_texts()
        check("three permission rows: read, upload, write", len(perms) == 3
              and all(any(x in p for p in perms) for x in ("View, search and download files", "Upload files and create folders", "Move, rename and move to the bin")), perms)
        check("English UI: no Chinese on the card", not han(txt), han(txt)[:3])
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
finish()
