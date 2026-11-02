"""Settings → Connections: the Flights & hotels card before a key is added (flights come free from Kiwi.com data; English
only in the English UI) and that the MCP card offers no provider preset. Connecting a real SerpApi key is
travel_connect_real's."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en"}, headers=K.H)
K.c.delete(K.B + "/sentinel/api/connections/travel/credential", headers=K.H)


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
        pg.on("console", lambda m: errs.append("console." + m.type + ": " + m.text) if m.type == "error" else None)
        await pg.goto(B + "#connections")
        await pg.wait_for_timeout(1500)
        if errs:
            print("INFO page errors:", errs[:3])
        card = pg.locator(".card", has_text="Flights & hotels").first
        await card.wait_for()
        txt = await card.text_content()
        check("fresh: flights work free with Kiwi.com data and its booking links; the card explains Google Flights first with a key, Kiwi.com as the fallback",
              "Flights: Kiwi.com data, free, with booking links" in txt and "Hotels & restaurants: Google data with a key, web search without" in txt
              and "a single date is searched on Google Flights" in txt and "Kiwi.com is used only when Google fails" in txt and "A range of dates or self-transfer is one Kiwi.com search" in txt
              and "Each request is one search" in txt and "Google Maps" in txt and "250 searches" in txt and "serpapi.com" in txt and "not used for web search" in txt, txt[:900])
        check("English UI: no Chinese on the card", not han(txt), han(txt)[:3])
        check("the MCP card offers no search-provider or Kiwi preset: the model only sees the generic tools", await pg.locator("button.preset").count() == 0)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
K.c.delete(K.B + "/sentinel/api/connections/travel/credential", headers=K.H)
finish()
