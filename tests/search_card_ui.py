"""Settings → Connections: the Web search card — one row per provider in try order (the free Exa MCP, Tavily, Brave, Exa)
with its free tier and a key field, the try-order note, English only in the English UI, the chip listing the providers
with a key once one is saved, Test all, a remove button per key."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
TAVILY = "tvly-test-key-0123456789"
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
        check("fresh: connected through the server's Brave key and the free Exa MCP; the card says the browser's engines go first, then Brave → Exa, Tavily for news, and the SerpApi key is not for web search",
              "Connected Brave Search, Exa (free MCP)" in txt and "2,000" in txt and "1,000" in txt and "browser's own search engines first" in txt and "Brave → Exa (free MCP) → Exa key" in txt
              and "Tavily is only for news search" in txt and "server key" in txt and "SerpApi key is only for flight, hotel and restaurant search" in txt and "Google (SerpApi)" not in txt, txt[:700])
        brow = card.locator(".provider[data-provider=brave]")
        check("the Brave row is marked as using the server key and its field invites an override", await brow.locator(".chip").first.inner_text() == "server key"
              and "paste your own to override" in (await brow.locator("input").get_attribute("placeholder")), await brow.text_content())
        check("the note explains server keys and overriding them", "config.toml" in txt and "override" in txt)
        rows = await card.locator(".provider").evaluate_all("els => els.map(e => e.dataset.provider)")
        check("one row per provider: the web chain in try order, then Tavily (news); no Google (SerpApi) row", rows == ["brave", "exa-mcp", "exa", "tavily"]
              and "Tavily (news)" in txt, rows)
        check("English UI: no Chinese on the card", not han(txt), han(txt)[:3])
        trow = card.locator(".provider[data-provider=tavily]")
        await trow.locator("input[type=password]").fill(TAVILY)
        await trow.locator("button", has_text="Save & test").click()
        await pg.wait_for_selector(".toast")
        toast = await pg.locator(".toast").inner_text()
        check("saving names the provider", "Tavily" in toast and "connected" in toast.lower(), toast)
        await pg.wait_for_function("() => { const c = document.querySelector('#websearch'); const r = c && c.querySelector('.provider[data-provider=tavily] .chip'); return r && r.textContent.trim() === 'your key'; }", timeout=15000)
        card = pg.locator("#websearch")
        txt = await card.text_content()
        trow_chip = await card.locator(".provider[data-provider=tavily] .chip").first.inner_text()
        check("connected: the chip lists the web providers (Tavily is for news); the Tavily row says 'your key' and has a remove button; Test all is offered",
              "Connected Brave Search, Exa (free MCP)" in txt and trow_chip == "your key" and await card.locator(".provider[data-provider=tavily] button", has_text="🗑").count() == 1 and "Test all" in txt, (txt[:300], trow_chip))
        before = await pg.locator(".toast").all_inner_texts()
        await card.get_by_role("button", name="Test all", exact=True).click()
        await pg.wait_for_function("(n) => document.querySelectorAll('.toast').length > n || [...document.querySelectorAll('.toast')].some(t => t.textContent.includes('Tavily: '))", arg=len(before), timeout=15000)
        toasts = await pg.locator(".toast").all_inner_texts()
        check("Test all reports every provider: the free Exa MCP, Tavily and the server-keyed Brave all working", any("Tavily: ✓" in t and "Exa (free MCP): ✓" in t and "Brave Search: ✓" in t for t in toasts), toasts)
        await card.locator(".provider[data-provider=tavily] button", has_text="🗑").click()
        await pg.wait_for_function("() => { const c = document.querySelector('#websearch'); const r = c && c.querySelector('.provider[data-provider=tavily] .chip'); return r && r.textContent.trim() === '—' && c.textContent.includes('Connected Brave Search, Exa (free MCP)'); }", timeout=15000)
        check("removing the key leaves the free Exa MCP and the server's Brave key", True)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
K.c.delete(K.B + "/sentinel/api/connections/search/credential", headers=K.H)
finish()
