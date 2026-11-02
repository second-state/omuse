"""Four research-only travel questions with a real model, from the user's point of view (see tests/user_common.py).
The user has a travel data connector (Connections → a travel MCP server, tests/fake_travel.py) besides the built-in
flight search (Kiwi.com without a SerpApi key). In Main's chat the user asks for today's last Hong Kong → Singapore
flight that lands before midnight, the Michelin 3-star restaurants in Tokyo for next Friday, the cheapest Singapore →
Tokyo flight on any day of a five-day range, and — after setting ANA as the preferred airline in the profile — the
cheapest direct business-class flight Tokyo → Singapore. Each answer is complete without the user being asked to
approve anything or to take over a browser, and nothing is booked; each carries the facts: a flight that lands the same
day (never the overnight connection), restaurants from the guide for that Friday, the cheapest day of the range with
its price, business-class direct fares (never an economy one offered instead) with ANA covered — when ANA has none and
the answer offers all airlines, the user says yes. The flight answers link a booking page and Kayak to compare. Nothing inside the system is looked at. One TIMING line per question (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_ask_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import fake_travel  # noqa: E402
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
FLIGHT_NO = re.compile(r"\b([A-Z][A-Z0-9]|[0-9][A-Z])\s?(\d{2,4})\b")
# the user's sources as a website would show them: the connector's flights and Kiwi.com's (tests/fake_apps.py)
KIWI_SAME_DAY, KIWI_NEXT_DAY = {"SQ638"}, {"CX710", "CX500"}
BOOKED = re.compile(r"TRV-[A-Z0-9]{7}|I(?:'ve| have) (?:booked|reserved)|reservation (?:is )?confirmed|booking (?:is )?confirmed", re.I)


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def mentions_day(text: str, d: dt.date) -> bool:
    return bool(re.search(rf"\b({d:%b}|{d:%B})\.? {d.day}\b|\b{d.day} ({d:%b}|{d:%B})\b|{d.isoformat()}", text))


def next_friday() -> dt.date:
    return TODAY + dt.timedelta(days=(4 - TODAY.weekday()) % 7 or 7)


def nos(text: str) -> list[str]:
    return [m.group(1) + m.group(2) for m in FLIGHT_NO.finditer(text)]


def first_leg(flight_no: str) -> str:
    return flight_no.split(" + ")[0].replace(" ", "")


def quiet(label: str, a: U.Answer) -> None:
    """Research only: the answer came without the user being asked anything, and nothing was booked."""
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])
    check(f"{label}: no approval dialog and no request to take over a browser — nothing opened for the user", not a.approvals and not a.takeovers,
          (a.approvals, a.takeovers))
    check(f"{label}: nothing was booked", not BOOKED.search(a.text), a.text[:400])


def links(label: str, a: U.Answer) -> None:
    check(f"{label}: the block code appends after the answer links a booking page and Kayak to compare", any("kayak" not in x for x in a.block_links) and any("kayak" in x for x in a.block_links), a.block_links)


async def main():
    async with U.User() as u:
        # 1. today's last flight that lands before midnight
        offers = fake_travel.flights_search({"origin": "HKG", "destination": "SIN", "date": str(TODAY), "adults": 1})["offers"]
        same_day = {first_leg(o["flight_no"]) for o in offers if "(+" not in o["schedule"]} | KIWI_SAME_DAY
        next_day = {first_leg(o["flight_no"]) for o in offers if "(+" in o["schedule"]} | KIWI_NEXT_DAY
        print(f"INFO same-day flights: {sorted(same_day)}; next-day: {sorted(next_day)}")
        a = await u.ask("What is today's last flight from Hong Kong to Singapore that arrives before mid-night?", label="flight")
        quiet("flight", a)
        named = [n for n in nos(a.text) if n in same_day | next_day]
        check("flight: the answer picks a flight that lands the same day (the first flight it names)", bool(named) and named[0] in same_day,
              (named, a.text[:600]))
        check(f"flight: the answer is for the user's today ({TODAY}), not another day", mentions_day(a.text, TODAY) or not re.search(
            r"\b\d{1,2} (Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)|\b(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]* \d{1,2}\b", a.text), a.text[:400])
        links("flight", a)

        # 2. Michelin 3 stars in Tokyo for next Friday: options only
        rs = [r["name"] for r in fake_travel.michelin_search({"city": "Tokyo", "distinction": "3 Stars"})["restaurants"]]
        a = await u.ask("I need to book a Michelin 3 star restaurant in Tokyo next Friday. What are my options?", label="michelin")
        quiet("michelin", a)
        hit = [r for r in rs if r.lower() in a.text.lower()]
        check(f"michelin: the answer lists 3-star restaurants from the guide ({len(hit)} of {len(rs)})", bool(hit), (rs, a.text[:600]))
        check(f"michelin: the options are for next Friday ({next_friday()})", mentions_day(a.text, next_friday()), a.text[:600])

        # 3. any day of a range
        r1, r2 = TODAY + dt.timedelta(days=40), TODAY + dt.timedelta(days=44)
        a = await u.ask(f"What is the cheapest flight from Singapore to Tokyo on any day between {day(r1)} and {day(r2)}?", label="range")
        quiet("range", a)
        check("range: the answer names the cheapest day in the range with its price",
              a.has_prices() and any(mentions_day(a.text, r1 + dt.timedelta(days=i)) for i in range(5)), a.text[:500])
        links("range", a)

        # 4. business class, direct, from a city with two airports, ANA preferred in the profile (last: the preference stays set)
        bday = TODAY + dt.timedelta(days=30)
        U.c.put(U.B + "/api/profile", json={"key": "preferred_airline", "value": "ANA"}, headers=U.H)   # the user's profile (world, not a check)
        a = await u.ask(f"What is the cheapest business class direct flight from Tokyo to Singapore on {day(bday)}?", label="business")
        quiet("business", a)
        check("business: the answer covers the preferred airline (ANA) — its fare, or that it has none",
              bool(re.search(r"\bANA\b|All Nippon", a.text)), a.text[:600])
        if not a.has_prices():
            # no fare in the answer (ANA has no such flight): the user asks for all airlines
            a = await u.ask("Yes, please search all airlines.", label="business-all", new_chat=False)
            quiet("business (all airlines)", a)
        check("business: the answer gives business-class fares with prices", a.mentions("business") and a.has_prices(), a.text[:600])
        check("business: no economy fare is offered instead",
              not re.search(r"economy[- ](class )?(fare|ticket|option|seat)s?\b|cheapest economy|in economy for", a.text, re.I), a.text[:600])
        check("business: the answer offers direct flights, not the connection via Hong Kong",
              bool(re.search(r"\b(direct|nonstop|non-stop)\b", a.text, re.I)) and not ({"CX710", "CX500"} & set(nos(a.text))), a.text[:600])
        links("business", a)


U.setup(travel_connector=True)
U.run(main)
