"""Settings → Connections: the Web search card before a key is added — one row per provider in try order with its free
tier and a key field, the try-order note, English only in the English UI. Saving and testing real keys is
web_search_real's."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en"}, headers=K.H)
K.c.delete(K.B + "/sentinel/api/connections/search/credential", headers=K.H)


def han(s):
    return [l for l in s.splitlines() if any("一" <= ch <= "鿿" for ch in l)]


r = K.c.get(K.B + "/sentinel/api/connections", headers=K.H)
check("the connections API answers (the page cannot render otherwise)", r.status_code == 200 and any(x["name"] == "search" for x in r.json()["connections"]), r.text[:300])


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
        await pg.wait_for_selector("#websearch")
        card = pg.locator("#websearch")
        txt = await card.text_content()
        check("fresh: the card says the browser's engines go first, then Brave → Exa, Tavily for news, and the SerpApi key is not for web search",
              "Exa (free MCP)" in txt and "2,000" in txt and "1,000" in txt and "browser's own search engines first" in txt and "Brave → Exa (free MCP) → Exa key" in txt
              and "Tavily is only for news search" in txt and "SerpApi key is only for flight, hotel and restaurant search" in txt and "Google (SerpApi)" not in txt, txt[:700])
        rows = await card.locator(".provider").evaluate_all("els => els.map(e => e.dataset.provider)")
        check("one row per provider: the web chain in try order, then Tavily (news); no Google (SerpApi) row", rows == ["brave", "exa-mcp", "exa", "tavily"]
              and "Tavily (news)" in txt, rows)
        check("English UI: no Chinese on the card", not han(txt), han(txt)[:3])
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
K.c.delete(K.B + "/sentinel/api/connections/search/credential", headers=K.H)
finish()
