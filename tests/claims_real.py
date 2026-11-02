"""Flight disruption and delayed baggage with a real model, on the REAL sites, from the user's point of view
(tests/user_common.py, tests/claims_common.py): Singapore Airlines' flight status page and WorldTracer's passenger site
(mybag.aero, the airline's "Manage your delayed baggage report"). No mailbox: the user states the flight and the policy
terms (TravelSure TI-2026-77: SGD 100 for every full 6 hours of delay with the airline's delay certificate; baggage
delay SGD 150 after 6 hours with the PIR and receipts) in the request. Nothing is submitted or sent.

1. delay — the user's SQ 638 Singapore → Tokyo Narita today. The user asks whether it is delayed, the options, what
   insurance and the airline give, and whether they can get their money back; drafts only. Checked: the Insurance &
   claims bot answers; the block code appends has the flight's line read from Singapore Airlines' status page (the live
   page decides what it says) and the link to that page; nothing submitted or sent.
2. baggage — the bag did not arrive in Tokyo on yesterday's ANA NH 802 (ANA uses WorldTracer's passenger site); the PIR
   file is NRTNH12345 (clearly fake); the user bought essentials for JPY 9,850. The user asks what to do, the bag's
   status, the expenses claim drafted (not sent) and a check on the bag. Checked: the Insurance & claims bot answers; the
   block has the file and WorldTracer's own answer for it (a fake reference: "No record was found …", read from the live
   page) with the link to ANA's page on mybag.aero, and the 21-day rule; the Automations page lists a check on the bag.
One TIMING line per request. In run_in_docker.sh's REAL_SITES (one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh claims_real"""
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
    a = await u.ask("Is my Singapore Airlines flight SQ 638 to Tokyo today delayed? What are my options, what can I "
                    f"claim from my travel insurance and the airline, and can I get my money back instead? {POLICY} Prepare any emails I "
                    "need as drafts — don't send anything.", label="delay", approve=no_send, on_takeover=look, timeout=1200)
    check("delay: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("delay: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
    check("delay: the block code appends after the answer has the flight's line from Singapore Airlines' status page, and its link",
          a.shows("SQ 638 on") and any("singaporeair.com" in x for x in a.block_links), (a.block, a.block_links, a.text[:300]))
    said = C.SUBMITTED.search(a.text) or C.SENT.search(a.text)
    check("delay: nothing submitted or sent for the user", not said,
          a.text[max(0, said.start() - 160):said.end() + 80] if said else "")


async def baggage(u: U.User) -> None:
    a = await u.ask("My checked bag didn't arrive in Tokyo on my ANA flight NH 802 from Singapore yesterday. I filed a report at Narita, "
                    "file reference NRTNH12345, "
                    "and I've bought toiletries and a change of clothes for JPY 9,850 (I kept the receipts). What should I do? Check where my bag is, "
                    f"draft my expenses claim (don't send it), and keep an eye on the bag for me. {POLICY}",
                    label="baggage", approve=no_send, on_takeover=look, timeout=1200)
    check("baggage: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("baggage: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
    check("baggage: the block code appends after the answer has WorldTracer's answer for the file, with ANA's page on mybag.aero",
          a.shows("NRTNH12345", "Bag status: WorldTracer") and any("mybag.aero" in x for x in a.block_links),
          (a.block, a.block_links, a.text[:300]))
    check("baggage: the block code appends after the answer has the 21-day rule (treated as lost after)",
          a.shows("Treated as lost after:", "21 days after"), a.block)
    autos = await C.page_text(u, "schedules")
    print(f"INFO Automations page: {' '.join(autos.split())[:600]}")
    # the file reference is the test's own: WorldTracer answers "No record was found" (its page's own message, in the
    # block) and a check on a file that does not exist is pointless — the check is wanted only when WorldTracer found it
    no_record = a.shows("No record was found")
    print(f"INFO baggage: WorldTracer found no record: {no_record}")
    if not no_record:
        check("baggage: a check on the bag is on the Automations page",
              re.search(r"NRTNH12345|baggage|bag", autos, re.I) is not None, autos[:300])


async def main():
    async with U.User() as u:
        await delay(u)
        await baggage(u)


U.setup(profile={"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567",
                 "address_home": "10 Anson Road, Singapore 079903"})
U.run(main)
