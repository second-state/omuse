"""Mail only for the past, with a real model, from the user's point of view (see tests/user_common.py): the user's
mailbox holds an earlier Tokyo hotel booking (Hotel Gracery Shinjuku, reservation 77120455). The user asks Main's chat
for a new hotel and a new flight: the Travel bot answers each (the "Continue in ✈️ Travel" link) with options, booking
links and a Kayak link to compare, and the answers say nothing about the earlier booking (that a new search reads no mail
is enforced by the product and not visible to the user beyond this). Then the user asks which hotel they booked: the
answer names Hotel Gracery Shinjuku and the stay's dates. No request asks the user to approve anything or to take over a
browser. One TIMING line per request (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_nomail_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

D1 = dt.date.today() + dt.timedelta(days=30)
STAY = dt.date.today() + dt.timedelta(days=12)
MAIL = [
    ("Hotel Gracery Shinjuku <reservations@gracery.example>", f"Booking confirmed — Hotel Gracery Shinjuku, check-in {STAY.isoformat()}",
     f"Dear Lucas Lu, your reservation 77120455 is confirmed: Hotel Gracery Shinjuku, Tokyo, 2 nights from {STAY.isoformat()}, "
     "Standard Double, JPY 46,000 paid.", 9),
    ("Boss <boss@acme.com>", "Quarterly plan", "Please review the quarterly plan by Friday.", 1),
]
FLIGHT_NO = re.compile(r"\b(SQ|TR|NH|JL|CX|MM|ZG|UA|TG|BR|KE|D7|3K|GK)\s?\d{2,4}\b")
AIRLINE = re.compile(r"Singapore Airlines|Scoot|Cathay|\bANA\b|All Nippon|Japan Airlines|\bJAL\b|Zipair|ZIPAIR|Peach|United|Thai|EVA Air|Korean Air|AirAsia|Jetstar")
# the earlier booking as the mail tells it: a new search's answer must not bring it up
EARLIER = re.compile(r"77120455|your (existing |earlier |previous |current )?(reservation|booking)|you(?:'ve| have)? (already )?booked|already have a (reservation|booking)", re.I)


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def mentions_day(text: str, d: dt.date) -> bool:
    """The date in any common English form, also as the start or end of a range ("10–12 November", "Nov 10-12")."""
    mon = rf"({d:%b}|{d:%B})\.?"
    return bool(re.search(rf"\b{mon} {d.day}\b|\b{d.day} {mon}|\b{d.day}\s?[–-]\s?\d{{1,2}} {mon}|\b{mon} \d{{1,2}}\s?[–-]\s?{d.day}\b"
                          rf"|{d.isoformat()}", text))


def common(label: str, a: U.Answer) -> None:
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])
    check(f"{label}: the user was never asked to approve anything or to take over a browser", not a.approvals and not a.takeovers,
          (a.approvals, a.takeovers))


async def main():
    async with U.User() as u:
        a = await u.ask(f"Find me a quiet hotel near Shinjuku station for 2 nights from {day(D1)}.", label="hotel")
        common("hotel", a)
        check("hotel: the Travel bot answered (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
        check("hotel: the answer names hotels near Shinjuku", bool(re.search(r"[Hh]otel|Inn|Hostel", a.text)) and a.mentions("Shinjuku"), a.text[:500])
        check("hotel: the block code appends after the answer links a hotel page and Kayak to compare", any("kayak" not in x for x in a.block_links) and any("kayak" in x for x in a.block_links), a.block_links)
        check("hotel: the answer says nothing about the earlier booking in the mail", not EARLIER.search(a.text), a.text[:600])

        a = await u.ask(f"What is the cheapest flight from Singapore to Tokyo on {day(D1)}?", label="flight")
        common("flight", a)
        check("flight: the Travel bot answered (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
        check("flight: the answer gives flights (airline or flight number) with prices", a.has_prices() and bool(FLIGHT_NO.search(a.text) or AIRLINE.search(a.text)), a.text[:500])
        check("flight: the block code appends after the answer links a booking page and Kayak to compare", any("kayak" not in x for x in a.block_links) and any("kayak" in x for x in a.block_links), a.block_links)
        check("flight: the answer says nothing about the hotel booked in the mail", not EARLIER.search(a.text) and not a.mentions("Gracery"), a.text[:600])

        a = await u.ask("Which hotel did I book for my Tokyo trip, and for which dates?", label="booked")
        common("booked", a)
        check("booked: the answer names the booked hotel (Hotel Gracery Shinjuku)", a.mentions("Gracery"), a.text[:400])
        check("booked: the answer gives the stay's dates (check-in, and check-out or the 2 nights)",
              mentions_day(a.text, STAY) and (mentions_day(a.text, STAY + dt.timedelta(days=2)) or a.mentions("2 nights", "two nights")), a.text[:400])


U.setup(mail=MAIL, travel_connector=True)
U.run(main)
