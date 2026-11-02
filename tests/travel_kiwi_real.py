"""Flights without any travel key, as the user does it, with a real model and the real Kiwi.com (OMUSE_E2E_REAL=1, the e2e
env file): a fresh install has no SerpApi key on Connections → Flights & hotels, and the user asks Main's chat for a
flight about 30 days out and for the cheapest day in a range. Flights still come back — from Kiwi.com's public data — and
only what the user sees is checked: real flights with the airline, the times and the price, and a booking link that opens.
One TIMING line per request: from Enter to the answer.

run_in_docker.sh runs it against the real Kiwi.com, Kayak and the web (REAL_DATA_SUITES):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_kiwi_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check, opens  # noqa: E402

TODAY = dt.date.today()
D1 = TODAY + dt.timedelta(days=30)
AIRLINES = r"Singapore Airlines|Scoot|ANA|All Nippon|Japan Airlines|JAL|ZIPAIR|Zipair|Cathay|Peach|Jetstar|Air Asia|AirAsia|Vietjet|VietJet|" \
           r"Thai|China Airlines|EVA|Korean|Asiana|China Eastern|China Southern|Air China|Philippine|Cebu|Malaysia|Delta|United|Hong Kong Airlines|Batik|T'way|Jeju"
FLIGHT_NO = r"\b(SQ|TR|NH|JL|ZG|CX|MM|GK|3K|AK|D7|VJ|TG|CI|BR|KE|OZ|MU|CZ|CA|PR|5J|MH|DL|UA|HX|OD|TW|7C)\s?\d{2,4}\b"


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def mentions_day(text: str, d: dt.date) -> bool:
    return bool(re.search(rf"\b({d:%b}|{d:%B})\.? {d.day}\b|\b{d.day} ({d:%b}|{d:%B})\b|{d.isoformat()}", text))


def flights_shown(a: U.Answer) -> bool:
    """Flights as the user reads them: an airline (by name or flight number), departure times and a price."""
    return (a.has_prices() and bool(re.search(AIRLINES, a.text) or re.search(FLIGHT_NO, a.text))
            and len(re.findall(r"\b\d{1,2}:\d\d\b", a.text)) >= 2)


async def main():
    async with U.User() as u:
        pg = u.page
        # a fresh install: no SerpApi key on Connections → Flights & hotels
        await pg.goto(U.APP + "#connections")
        card = pg.locator(".card", has_text="Flights & hotels").first
        await card.wait_for(timeout=20000)
        txt = await card.inner_text()
        check("no travel key: the Flights & hotels card has no SerpApi plan and says flights come from Kiwi.com's free data",
              "searches left" not in txt and "Kiwi.com" in txt, " ".join(txt.split())[:300])

        # 1. one date
        a = await u.ask(f"Find me the cheapest flight from Singapore to Tokyo on {day(D1)}.", label="flight")
        check("flight: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("flight: the answer gives real flights — the airline, departure and arrival times and the price", flights_shown(a), a.text[:600])
        check("flight: the answer is for the day asked", mentions_day(a.text, D1), a.text[:400])
        bk = [x for x in a.block_links if "kiwi.com" in x]
        check("flight: the block code appends after the answer has a Kiwi.com booking link for a flight", bool(bk), a.block_links)
        if bk:
            ok, why = opens(bk[0])
            check("flight: the booking link opens", ok, why)

        # 2. the cheapest day in a range
        a = await u.ask(f"What's the cheapest flight from Singapore to Tokyo on any day between {day(D1)} and {day(D1 + dt.timedelta(days=4))}?",
                        label="range")
        check("range: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("range: the answer names the cheapest day in the range, with the airline and the price",
              a.has_prices() and any(mentions_day(a.text, D1 + dt.timedelta(days=i)) for i in range(5))
              and bool(re.search(AIRLINES, a.text) or re.search(FLIGHT_NO, a.text)), a.text[:600])
        bk = [x for x in a.block_links if "kiwi.com" in x]
        check("range: the block code appends after the answer has a Kiwi.com booking link, and it opens", bool(bk) and opens(bk[0])[0], a.block_links)


U.setup(vault=False)
U.run(main)
