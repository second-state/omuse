"""Flight disruption and delayed baggage with a real model, from the user's point of view (tests/user_common.py,
tests/claims_common.py). SKELETON awaiting the real sites (TODO lines below): Singapore Airlines' flight status and
"Delays and cancellations" pages, and WorldTracer's baggage tracing. No mailbox: the user states the flight and the
policy terms (TravelSure TI-2026-77: SGD 100 for every full 6 hours of delay with the airline's delay certificate;
baggage delay SGD 150 after 6 hours with the PIR and receipts) in the request. Nothing is submitted or sent.

1. delay — the user's SQ 638 Singapore → Tokyo Narita today at 09:25. The user asks whether it is delayed, the options,
   what insurance and the airline give, and whether they can get their money back; drafts only. Checked: the Insurance
   & claims bot answers; the block code appends has the flight's status from the airline's page (TODO real URL: the
   live status decides what it says); nothing submitted or sent.
2. baggage — the bag did not arrive in Tokyo on yesterday's SQ 638; the PIR file is NRTSQ12345 (clearly fake); the user
   bought essentials for JPY 9,850. The user asks what to do, the bag's status, the expenses claim drafted (not sent)
   and a check on the bag. Checked: the Insurance & claims bot answers; the block has the tracing status for the file
   from WorldTracer and the 21-day rule; the Automations page lists a check on the bag.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh claims_real"""
# TODO real URL: https://www.singaporeair.com/en_UK/sg/travel-info/flight-status/
# TODO real URL: https://wtrweb.worldtracer.aero/ (WorldTracer baggage tracing)
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

POLICY = ("My travel insurance is TravelSure policy TI-2026-77: SGD 100 for every full 6 hours of delay with the airline's delay "
          "certificate; baggage delay SGD 150 after 6 hours with the airline's PIR and receipts.")


def no_send(dialog: str) -> bool:
    """The user asked for drafts: they deny a dialog about sending mail, by the tool it names."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


async def look(pg, why: str) -> None:
    url = await pg.locator(".bview input.url").input_value() if await pg.locator(".bview input.url").count() else ""
    print(f"INFO takeover asked: {why[:200]!r}; the browser shows {url!r}")


async def delay(u: U.User) -> None:
    a = await u.ask("Is my Singapore Airlines flight SQ 638 to Tokyo today (09:25 from Singapore) delayed? What are my options, what can I "
                    f"claim from my travel insurance and the airline, and can I get my money back instead? {POLICY} Prepare any emails I "
                    "need as drafts — don't send anything.", label="delay", approve=no_send, on_takeover=look, timeout=1200)
    check("delay: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("delay: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
    check("delay: the block code appends after the answer has the flight's status from the airline's page",
          a.shows("SQ 638 on", "scheduled departure 09:25"), (a.block, a.text[:300]))
    said = C.SUBMITTED.search(a.text) or C.SENT.search(a.text)
    check("delay: nothing submitted or sent for the user", not said,
          a.text[max(0, said.start() - 160):said.end() + 80] if said else "")


async def baggage(u: U.User) -> None:
    a = await u.ask("My checked bag didn't arrive in Tokyo on SQ 638 yesterday. I filed a report at Narita, file reference NRTSQ12345, "
                    "and I've bought toiletries and a change of clothes for JPY 9,850 (I kept the receipts). What should I do? Check where my bag is, "
                    f"draft my expenses claim (don't send it), and keep an eye on the bag for me. {POLICY}",
                    label="baggage", approve=no_send, on_takeover=look, timeout=1200)
    check("baggage: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("baggage: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
    check("baggage: the block code appends after the answer has the bag's status from the tracing page",
          a.shows("NRTSQ12345", "Bag status:"), (a.block, a.text[:300]))
    check("baggage: the block code appends after the answer has the 21-day rule (treated as lost after)",
          a.shows("Treated as lost after:", "21 days after"), a.block)
    autos = await C.page_text(u, "schedules")
    print(f"INFO Automations page: {' '.join(autos.split())[:600]}")
    check("baggage: a check on the bag is on the Automations page",
          re.search(r"NRTSQ12345|baggage|bag", autos, re.I) is not None, autos[:300])


async def main():
    async with U.User() as u:
        await delay(u)
        await baggage(u)


U.setup(profile={"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567",
                 "address_home": "10 Anson Road, Singapore 079903"})
U.run(main)
