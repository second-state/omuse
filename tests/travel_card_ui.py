"""Settings → Connections: the Flights & hotels card (SerpApi key, English only in the English UI, the plan in the chip once
connected; flights come free from Kiwi.com data meanwhile) and that the MCP card offers no provider preset."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
KEY = "serp-test-key-0123456789abcdef"
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
        check("fresh: flights work free with Kiwi.com data first; the card explains Google Flights as the fallback with a key", "Flights: Kiwi.com data, free (information only)" in txt
              and "Hotels & restaurants: Google data with a key, web search without" in txt and "Google Flights is the fallback" in txt and "Google Maps" in txt and "250 searches" in txt and "serpapi.com" in txt
              and "never its links" in txt and "not used for web search" in txt, txt[:600])
        check("English UI: no Chinese on the card", not han(txt), han(txt)[:3])
        await card.locator("input[type=password]").fill(KEY)
        await card.locator("button", has_text="Connect & test").click()
        await pg.wait_for_selector(".toast")
        toast = await pg.locator(".toast").inner_text()
        check("connecting shows the plan and the searches left", "Free Plan" in toast and "238" in toast, toast)
        await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && c.textContent.includes('Free Plan'))", timeout=15000)
        card = pg.locator(".card", has_text="Flights & hotels").first
        txt = await card.text_content()
        check("connected: the chip shows the plan and the searches left; Disconnect is offered", "Free Plan" in txt and "238 searches left" in txt and "Disconnect" in txt, txt[:300])
        check("the MCP card offers no search-provider or Kiwi preset: the model only sees the generic tools", await pg.locator("button.preset").count() == 0)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
K.c.delete(K.B + "/sentinel/api/connections/travel/credential", headers=K.H)
finish()
