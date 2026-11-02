"""Four research-only travel questions with a real model on the real flight data, from the user's point of view (see
tests/user_common.py): the built-in flight search (Kiwi.com without a SerpApi key). In Main's chat the user asks for
tomorrow's last Hong Kong → Singapore flight that lands before midnight (the answer is that one flight: it lands
before midnight and is the last such of every flight the search found), the Michelin 3-star restaurants in Tokyo for next
Friday, the cheapest Singapore → Tokyo flight on any day of a five-day range, and — after setting ANA as the preferred
airline in the profile — the cheapest direct business-class flight Tokyo → Singapore (when the answer has no fare, the
user says yes to all airlines). Each answer comes without the user being asked to approve anything or to take over a
browser; the flight answers' block links a booking page and Kayak to compare. (Which flights the model names is the
live data's and its wording, not checked.) One TIMING line per question (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_ask_real"""
import datetime as dt
import os
import re
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


TIMES = re.compile(r"(\d{4}-\d\d-\d\d) (\d\d:\d\d) → \S+ (\d{4}-\d\d-\d\d) (\d\d:\d\d)")


async def last_flight(u: U.User, a: U.Answer) -> None:
    """The answer to "the last flight that lands before midnight" tomorrow, as the user reads it: the block code appends
    names ONE flight with its times, it lands tomorrow before midnight, and it is the last such flight of every flight
    the search found for the day (the search's own structured times, from the run's record) — no other flight is
    suggested beside it, no card of alternatives."""
    tomorrow = (TODAY + dt.timedelta(days=1)).isoformat()
    shown = TIMES.findall(a.block)
    print(f"INFO flight: the block's times {shown}")
    check("flight: the block names exactly one flight (no other suggestion)", len(re.findall(r"(?m)^\W*Flight( \d+)?:", a.block)) == 1, a.block[:400])
    check("flight: the block gives the flight's departure and arrival times", len(shown) == 1, a.block[:400])
    if len(shown) != 1:
        return
    dep_d, dep_t, arr_d, arr_t = shown[0]
    check(f"flight: it leaves tomorrow ({tomorrow})", dep_d == tomorrow, shown[0])
    check("flight: it lands before midnight on the day it leaves", arr_d == dep_d, shown[0])
    # every flight the search found (the run's record: the search's result as JSON, its times from the API's fields)
    listed = []
    for t in await u.tasks():
        for e in t.get("events", []):
            r = (e.get("data") or {}).get("result") or {}
            if e.get("type") == "workflow_result" and r.get("status") == "pick_needed":
                listed = r.get("flights") or []
    same_day = [f for f in listed if (f.get("departure") or {}).get("time", "").startswith(tomorrow)
                and (f.get("arrival") or {}).get("time", "").startswith(tomorrow)]
    print(f"INFO flight: {len(listed)} flights found, {len(same_day)} land the same day")
    check("flight: the search listed the day's flights with their times", bool(same_day), len(listed))
    if not same_day:
        return
    last_dep = max(f["departure"]["time"] for f in same_day)
    check(f"flight: it is the last flight landing before midnight (leaves {last_dep[11:]})",
          f"{dep_d} {dep_t}" == last_dep, (shown[0], last_dep))
    cards = U.conv_choices(await u.conv_id())
    check("flight: no choice card of alternatives for the one flight asked for",
          not any(len(c.get("options") or []) > 1 and "Flights" in str(c.get("question") or "") for c in cards),
          [c.get("question") for c in cards])


async def main():
    async with U.User() as u:
        # 1. tomorrow's last flight that lands before midnight: THE one flight, not the cheapest or suggestions
        # (the user's request of 2026-11-02, word for word; also the Travel bot's example in the UI — bots/travel.md,
        # checked by bots.rs tests)
        a = await u.ask("I need to go from HK to singapore tomorrow. Find me the last flight that lands before midnight", label="flight")
        quiet("flight", a)
        links("flight", a)
        await last_flight(u, a)

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
