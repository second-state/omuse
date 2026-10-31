"""Connecting the Flights & hotels key as the user does it, against the real SerpApi (OMUSE_E2E_REAL=1, the e2e env file
with OMUSE_E2E_SERPAPI_KEY): on Connections → Flights & hotels the user pastes something that is not a key (refused before
anything is asked), a well-formed key SerpApi does not know (SerpApi refuses it and the page says so), then their real
key: the card shows the plan and the searches left. Disconnecting takes the card back to Kiwi.com's free flight data.
Only the page is checked — its messages and the card — nothing inside the system; no model request is made.

run_in_docker.sh runs it against the real SerpApi (REAL_DATA_SUITES):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_connect_real"""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

KEY = os.environ.get("OMUSE_E2E_SERPAPI_KEY", "")
if not KEY:
    print("travel_connect_real needs a SerpApi key (OMUSE_E2E_SERPAPI_KEY in the e2e env file)")
    raise SystemExit(0)
WRONG = "0" * 24 + "deadbeef" * 5   # shaped like a SerpApi key (64 hex characters), but no account has it


async def main():
    async with U.User() as u:
        pg = u.page
        await pg.goto(U.APP + "#connections")
        card = pg.locator(".card", has_text="Flights & hotels").first
        await card.wait_for(timeout=20000)

        async def paste(token: str) -> str:
            """The user pastes `token`, clicks Connect & test and reads the message that pops up."""
            n = await pg.locator(".toast").count()
            await card.locator("input[type=password]").fill(token)
            await card.locator("button", has_text="Connect & test").click()
            await pg.wait_for_function("n => document.querySelectorAll('.toast').length > n", arg=n, timeout=60000)
            t = await pg.locator(".toast").last.inner_text()
            err = "err" in (await pg.locator(".toast").last.get_attribute("class") or "")
            print(f"INFO toast after pasting {token[:4]}…: {t!r} (error: {err})")
            return ("ERROR " if err else "") + t

        t = await paste("short")
        check("something that is not a key is refused at once, asking for the SerpApi API key", t.startswith("ERROR") and "API key" in t, t)
        t = await paste(WRONG)
        check("a key SerpApi does not know is refused, and the page says SerpApi refused it", t.startswith("ERROR") and "refused" in t, t)
        txt = await card.inner_text()
        check("after the refusals the card is still not connected (no plan)", "searches left" not in txt, " ".join(txt.split())[:200])

        t = await paste(KEY)
        check("the user's real key connects: the message names the plan and the searches left", not t.startswith("ERROR") and "searches left" in t, t)
        await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && /searches left/.test(c.textContent))",
                                   timeout=30000)
        card = pg.locator(".card", has_text="Flights & hotels").first
        txt = " ".join((await card.inner_text()).split())
        check("the card shows the plan and the searches left, and a Disconnect button", "searches left" in txt and "Disconnect" in txt, txt[:300])

        # disconnect: back to Kiwi.com's free flight data
        pg.once("dialog", lambda d: asyncio.ensure_future(d.accept()))
        await card.locator("button", has_text="Disconnect").click()
        await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && !/searches left/.test(c.textContent))",
                                   timeout=30000)
        card = pg.locator(".card", has_text="Flights & hotels").first
        txt = " ".join((await card.inner_text()).split())
        check("after Disconnect the card no longer shows the plan and says flights come from Kiwi.com's free data",
              "searches left" not in txt and "Kiwi.com" in txt and "Disconnect" not in txt, txt[:300])


U.setup(vault=False)
U.run(main)
