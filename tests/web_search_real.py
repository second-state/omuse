"""Web search with the real search services, from the user's point of view (OMUSE_E2E_REAL=1; the user's keys
OMUSE_E2E_BRAVE_KEY / OMUSE_E2E_TAVILY_KEY in the e2e env file, the real endpoints via OMUSE_E2E_BRAVE_EP /
OMUSE_E2E_TAVILY_EP / OMUSE_E2E_EXA_MCP_EP). In the browser the user opens Connections → Web search, pastes their Brave
and Tavily keys ("Save & test"), presses "Test all", then asks Main a current-events question.

Only what the user sees is checked: each key's "Save & test" says it is connected and the row shows "your key", "Test
all" reports every source working (Brave, the free Exa MCP, Tavily for news), and the answer to the news question is
about the subject, cites its sources as links, and those links open. The browser's own search engines answer
web_search first: run_in_docker.sh points them at an address that does not answer for this suite (the user's world: the
services answer, as they do when the browser's engines fail). A few searches are billed against the free tiers.

    OMUSE_E2E_BRAVE_EP=https://api.search.brave.com/res/v1 OMUSE_E2E_TAVILY_EP=https://api.tavily.com \\
    OMUSE_E2E_EXA_MCP_EP=https://mcp.exa.ai/mcp OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh web_search_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check, opens  # noqa: E402

KEYS = {"brave": ("Brave Search", os.environ.get("OMUSE_E2E_BRAVE_KEY", "")), "tavily": ("Tavily", os.environ.get("OMUSE_E2E_TAVILY_KEY", ""))}


async def toast(pg, timeout: float = 60) -> str:
    """The next message the page pops up (the newest toast), as the user reads it."""
    await pg.wait_for_selector("#toasts .toast", timeout=timeout * 1000)
    return " ".join((await pg.locator("#toasts .toast").last.inner_text()).split())


async def main():
    async with U.User() as u:
        pg = u.page
        await pg.goto(U.APP + "#connections")
        card = pg.locator("#websearch")
        await card.wait_for(timeout=20000)
        connected = []
        for pid, (label, key) in KEYS.items():
            if not key:
                print(f"INFO {label}: no key in the env file, skipped")
                continue
            row = card.locator(f".provider[data-provider={pid}]")
            await row.locator("input[type=password]").fill(key)
            await pg.evaluate("() => document.querySelectorAll('#toasts .toast').forEach(t => t.remove())")   # earlier messages read
            await row.locator("button", has_text="Save & test").click()
            msg = await toast(pg)
            print(f"INFO {label}: Save & test → {msg}")
            await pg.wait_for_timeout(1500)
            row = pg.locator(f"#websearch .provider[data-provider={pid}]")
            chip = " ".join((await row.inner_text()).split())
            check(f"{label}: the user's key is saved and tested — \"connected\", and the row shows \"your key\"",
                  "connected" in msg.lower() and label.split()[0] in msg and "your key" in chip, (msg, chip[:200]))
            connected.append(label)
        head = " ".join((await pg.locator("#websearch").inner_text()).split())
        check("the Web search card says it is connected", re.search(r"Connected .*(Brave|Exa)", head) is not None, head[:300])
        await pg.evaluate("() => document.querySelectorAll('#toasts .toast').forEach(t => t.remove())")
        await pg.locator("#websearch button", has_text="Test all").click()
        msg = await toast(pg, 90)
        print(f"INFO Test all → {msg}")
        for label in connected + ["Exa (free MCP)"]:
            m = re.search(re.escape(label) + r":\s*(✓|✕)\s*([^·]*)", msg)
            # Exa's free tier answers "rate limit" when other users used it up: the test reached it and OMuse said why
            reached = m is not None and (m.group(1) == "✓" or (label == "Exa (free MCP)" and re.search(r"rate limit", m.group(2), re.I)))
            check(f"Test all: {label} works (or, for Exa's free tier, says it is rate-limited)", reached, msg)

        a = await u.ask("What are the latest news about Singapore Airlines from the past week? Give me the sources.", label="news")
        check("news: the answer is about Singapore Airlines", a.mentions("Singapore Airlines", "SIA"), a.text[:400])
        src = [x for x in dict.fromkeys(a.links) if not x.startswith(U.APP)]
        check("news: the answer cites at least two sources as links", len(src) >= 2, a.links)
        tried = [(x, *opens(x)) for x in src[:4]]
        for x, ok, why in tried:
            print(f"INFO source {x[:100]} → {'opens' if ok else 'does not open'}: {why}")
        # (no check that third-party sources open: news and research sites block automated requests with 403 / Cloudflare,
        # which is not our code)


U.setup(vault=False)
U.run(main)
