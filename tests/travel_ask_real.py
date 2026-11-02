"""Four research-only travel questions with a real model on the real flight data, from the user's point of view (see
tests/user_common.py): the built-in flight search (Kiwi.com without a SerpApi key). In Main's chat the user asks for
today's last Hong Kong → Singapore flight that lands before midnight, the Michelin 3-star restaurants in Tokyo for next
Friday, the cheapest Singapore → Tokyo flight on any day of a five-day range, and — after setting ANA as the preferred
airline in the profile — the cheapest direct business-class flight Tokyo → Singapore (when the answer has no fare, the
user says yes to all airlines). Each answer comes without the user being asked to approve anything or to take over a
browser; the flight answers' block links a booking page and Kayak to compare. (Which flights the model names is the
live data's and its wording, not checked.) One TIMING line per question (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_ask_real"""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


def user_today() -> dt.date:
    """Today where the user is: the timezone in their Settings (the stack runs in UTC, the user in Asia/Singapore)."""
    from zoneinfo import ZoneInfo
    j = U.c.get(U.B + "/api/settings", headers=U.H).json()
    tz = (j.get("settings") or j).get("timezone") or "UTC"
    today = dt.datetime.now(ZoneInfo(tz)).date()
    print(f"INFO the user's today: {today} ({tz})")
    return today


TODAY = user_today()


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def quiet(label: str, a: U.Answer) -> None:
    """Research only: the answer came without the user being asked anything."""
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])
    check(f"{label}: no approval dialog and no request to take over a browser — nothing opened for the user", not a.approvals and not a.takeovers,
          (a.approvals, a.takeovers))


def links(label: str, a: U.Answer) -> None:
    check(f"{label}: the block code appends after the answer links a booking page and Kayak to compare", any("kayak" not in x for x in a.block_links) and any("kayak" in x for x in a.block_links), a.block_links)


async def main():
    async with U.User() as u:
        # 1. today's last flight that lands before midnight
        # (this request is also the Travel bot's example in the UI — bots/travel.md, checked by bots.rs tests)
        a = await u.ask("Find the last flight from HK to singapore tonight. It must arrive before mid-night.", label="flight")
        quiet("flight", a)
        links("flight", a)

        # 2. Michelin 3 stars in Tokyo for next Friday: options only
        a = await u.ask("I need to book a Michelin 3 star restaurant in Tokyo next Friday. What are my options?", label="michelin")
        quiet("michelin", a)

        # 3. any day of a range
        r1, r2 = TODAY + dt.timedelta(days=40), TODAY + dt.timedelta(days=44)
        a = await u.ask(f"What is the cheapest flight from Singapore to Tokyo on any day between {day(r1)} and {day(r2)}?", label="range")
        quiet("range", a)
        links("range", a)

        # 4. business class, direct, from a city with two airports, ANA preferred in the profile
        bday = TODAY + dt.timedelta(days=30)
        U.c.put(U.B + "/api/profile", json={"key": "preferred_airline", "value": "ANA"}, headers=U.H)   # the user's profile (world, not a check)
        a = await u.ask(f"What is the cheapest business class direct flight from Tokyo to Singapore on {day(bday)}?", label="business")
        quiet("business", a)
        if not a.has_prices():
            # no fare in the answer (ANA has no such flight): the user asks for all airlines
            a = await u.ask("Yes, please search all airlines.", label="business-all", new_chat=False)
            quiet("business (all airlines)", a)
        links("business", a)


U.setup()
U.run(main)
