"""Local events with a real model on SerpApi's real Google Events data, from the user's point of view
(tests/user_common.py). A real-data suite (run_in_docker.sh REAL_DATA_SUITES), with the SerpApi key from the e2e env
file (OMUSE_E2E_SERPAPI_KEY), pasted on Connections → Flights & hotels as the user would.

1. specific — "Are there any jazz concerts in Tokyo this weekend? Give me a link to book one." Checked: the Travel bot
   answers; nothing to approve, no takeover; at least one page to book or see the events is linked — code appends the
   ticket links when Google Events has the user's words (Google Events has few narrow queries in Asia: then the city's
   events, or a web search, and the answer links the matching pages). Which events are listed is the live data's.
2. concerts — "Any concerts in Tokyo this week? I'd like to book tickets." (Google Events has Tokyo's concerts): the block
   code appends links at least one event's ticket page.
3. browse — "What's on in Singapore next month — concerts and shows?" Checked: the Travel bot answers and the block or
   the answer links at least one page (a ticket seller, or the city guide's listings).
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_events_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

KEY = os.environ.get("OMUSE_E2E_SERPAPI_KEY", "")
if not KEY:
    print("travel_events_real needs a SerpApi key (OMUSE_E2E_SERPAPI_KEY in the e2e env file)")
    raise SystemExit(0)


def by_travel(a: U.Answer, step: str) -> None:
    check(f"{step}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check(f"{step}: the Travel bot answered", "Travel" in a.bot_link, a.bot_link)
    check(f"{step}: nothing to approve and no takeover", not a.approvals and not a.takeovers, (a.approvals, a.takeovers))


async def main():
    async with U.User() as u:
        pg = u.page
        await pg.goto(U.APP + "#connections")
        card = pg.locator(".card", has_text="Flights & hotels").first
        await card.wait_for(timeout=20000)
        await card.locator("input[type=password]").fill(KEY)
        await card.locator("button", has_text="Connect & test").click()
        await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && /searches left/.test(c.textContent))",
                                   timeout=60000)

        a = await u.ask("Are there any jazz concerts in Tokyo this weekend? Give me a link to book one.", label="specific", timeout=900)
        by_travel(a, "specific")
        # code appends the ticket links when Google Events has the user's words; when it has only the city's events (or
        # nothing: web search) the answer links the matching events' pages itself
        page = [x for x in a.block_links + a.links if x.startswith("https://") and "google." not in x]
        check("specific: the answer links at least one page to book or see the events (https, not Google)", bool(page),
              (a.block[:800], a.block_links, a.links))

        # Google Events has this one (Concerts in Tokyo: 9 events, 6 with ticket links on 2026-11-04): code appends them
        a = await u.ask("Any concerts in Tokyo this week? I'd like to book tickets.", label="concerts", timeout=900)
        by_travel(a, "concerts")
        check("concerts: the block code appends links at least one event's ticket page (https, from Google Events)",
              any(x.startswith("https://") and "google." not in x for x in a.block_links), (a.block[:800], a.block_links))

        a = await u.ask("What's on in Singapore next month — concerts and shows?", label="browse", timeout=900)
        by_travel(a, "browse")
        check("browse: a page to see or book the events is linked", bool(a.block_links or a.links), (a.block_links, a.links))


U.setup()
U.run(main)
