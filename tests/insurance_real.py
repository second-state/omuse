"""FWD travel insurance in Singapore with a real model, from the user's point of view (tests/user_common.py,
tests/claims_common.py). SKELETON awaiting FWD Singapore's real site (TODO lines below): the test stops at the hand-over,
nothing is paid or submitted. The profile has the user's name, birthday and contact; the Vault has their Visa card and
passport (for fwd.com.sg; clearly fake test values). "Auto-approve browser actions" is on (a fresh install): nothing in
the browser asks, and the agent hands the final Pay / Submit to the user.

1. buy — the user asks Main's chat for the cheapest FWD plan with at least SGD 10,000 trip cancellation for next month's
   Tokyo trip (the flights in the request), filled in with their Vault card; Pay is theirs. Checked from the user's side:
   the Insurance & claims bot answers (its "Continue in" link); the agent asks the user to take over on FWD's site; the
   handed-over form holds the passport and the card (filled); the block code appends names the plan and the amount;
   neither the passport number nor the card number appears in the chat; afterwards the browser view is still on FWD's
   site, not a payment confirmation.
2. claim — the user gives the policy number and the delay (SQ 636 two days ago, 10:30 → 17:45) and asks to claim it;
   the agent fills FWD's claim form and hands it over for Submit. Checked: the takeover on FWD's site; the handed-over
   form holds the policy number, the name, the passport number, the date and the amount; the block gives the delay
   (7 h 15 min) and the amount (SGD 100) and FWD's 30-day deadline as a date; no passport number in the chat.
One TIMING line per request; the live pictures go to OMUSE_E2E_OUT when it is set.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh insurance_real"""
# TODO real URL: https://www.fwd.com.sg/travel-insurance/ (buy)
# TODO real URL: https://www.fwd.com.sg/claims/ (the travel claim form)
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
D1, D2 = (TODAY + dt.timedelta(days=24)).isoformat(), (TODAY + dt.timedelta(days=31)).isoformat()
INC = TODAY - dt.timedelta(days=2)
D30 = INC + dt.timedelta(days=30)
POLICY = "TI-SG-2026-55123"
PASSPORT_NO, CARD_NO = "K1234567Z", ("4111 1111 1111 1111", "4111111111111111")
OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
FWD = "fwd.com.sg"


def leaks(text: str) -> list[str]:
    f = text.replace(" ", "")
    return [x for x in (PASSPORT_NO, CARD_NO[1]) if x in f]


async def buy(u: U.User) -> None:
    seen = {"url": "", "form": []}

    async def look_then_hand_back(pg, request: str) -> None:
        # the user reads the request and looks at the application in the browser view; they do not press Pay
        seen["url"] = await u.view_url()
        seen["form"] = await u.view_form()
        await u.view_picture(os.path.join(OUT, "insurance_application.png") if OUT else None)
        print(f"INFO takeover: {request!r} url {seen['url']!r}")

    a = await u.ask(f"I need travel insurance from FWD for my trip to Tokyo next month, just me: SQ 636 Singapore to Tokyo Haneda on {D1}, "
                    f"back on SQ 637 on {D2}. Get me the cheapest plan with at least SGD 10,000 trip cancellation "
                    "and fill in the application with my card from the Vault; I'll press Pay myself.",
                    label="buy", on_takeover=look_then_hand_back, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        # the answer asked something before opening the quote page: the user answers once, in the same chat
        a = await u.ask("Everything else is in my profile (name, birthday, contact). Only the flights are paid so far, no pre-existing "
                        "conditions. Please go ahead.", label="buy follow-up", new_chat=False, on_takeover=look_then_hand_back, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("buy: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("buy: the Insurance & claims bot did it (the link to continue in it)", any("nsurance" in x.bot_link for x in answers),
          [x.bot_link for x in answers])
    check("buy: the user was asked to take over the browser on FWD's site (a takeover card; Pay is theirs)",
          bool(takeovers) and FWD in seen["url"], (takeovers, seen["url"]))
    form = seen["form"]
    check("buy: the handed-over application holds the passport number and the card (filled)",
          C.flat(U.field(form, r"passport")) == PASSPORT_NO and bool(U.field(form, r"card number|card no")), [(f["label"][:30], f["value"][:20]) for f in form][:30])
    buy_block = "\n".join(x.block for x in answers)
    check("buy: the block code appends after the answer names the plan and the amount to pay",
          re.search(r"Chosen: .+: SGD \d", buy_block) is not None, (buy_block, a.text[:300]))
    everything = await u.thread_text()
    check("buy: neither the passport nor the card number appears in the chat", not leaks(everything), leaks(everything))
    # afterwards the user opens the browser view: still FWD's page, Pay never pressed
    await u.page.goto(U.APP + "#browser")
    await u.page.wait_for_selector(".bview", timeout=20000)
    await u.page.wait_for_timeout(2500)
    now = await u.view_url()
    print(f"INFO browser view afterwards: url {now!r}")
    check("buy: the browser view is still on FWD's site afterwards (Pay never pressed)", FWD in now, now)


async def claim(u: U.User) -> None:
    seen = {"url": "", "form": []}

    async def look_then_hand_back(pg, request: str) -> None:
        # the user reads the request and looks at the filled-in claim form; Submit is theirs, and the test never presses it
        seen["url"] = await u.view_url()
        seen["form"] = await u.view_form()
        await u.view_picture(os.path.join(OUT, "insurance_claim.png") if OUT else None)
        print(f"INFO takeover: {request!r} url {seen['url']!r}")

    a = await u.ask(f"My flight SQ 636 to Tokyo on {INC} was scheduled at 10:30 and left at 17:45. Please claim the delay on my FWD travel "
                    f"insurance, policy {POLICY} (SGD 100 for every 6 hours of delay; claims within 30 days). Fill in FWD's claim form; "
                    "I'll check it and submit it myself.", label="claim", on_takeover=look_then_hand_back, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        a = await u.ask("All the other details are in my profile; nothing else to add. Please go ahead and fill in the claim form.",
                        label="claim follow-up", new_chat=False, on_takeover=look_then_hand_back, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("claim: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("claim: the Insurance & claims bot did it", any("nsurance" in x.bot_link for x in answers), [x.bot_link for x in answers])
    check("claim: the user was asked to take over the browser on FWD's site (a takeover card; Submit is theirs)",
          bool(takeovers) and FWD in seen["url"], (takeovers, seen["url"]))
    form = seen["form"]
    check("claim: the handed-over form holds the policy number, the name, the passport number, the date and the amount",
          C.flat(U.field(form, r"policy")) == POLICY and "TAN" in U.field(form, r"name").upper()
          and C.flat(U.field(form, r"passport|nric|id")) == PASSPORT_NO and bool(U.field(form, r"date"))
          and bool(U.field(form, r"amount")), [(f["label"][:30], f["value"][:20]) for f in form][:30])
    block = "\n".join(x.block for x in answers)
    check("claim: the block code appends after the answer gives the delay (7 h 15 min) and the amount (SGD 100)",
          "delayed 7 h 15 min" in block and "Claim amount: SGD 100.00" in block, (block, a.text[:300]))
    check("claim: the block code appends after the answer gives FWD's 30-day deadline as a date",
          C.date_in(block, D30), (D30, block))
    everything = await u.thread_text()
    check("claim: the passport number never appears in the chat", PASSPORT_NO not in everything.replace(" ", ""), "")


async def main():
    async with U.User() as u:
        await buy(u)
        await claim(u)


U.setup(vault=False, profile={"name_en": "TAN WEI MING", "birthday": "1985-04-12", "email_personal": "weiming@example.com",
                              "phone": "+65 9123 4567", "address_home": "10 Anson Road, Singapore 079903"})
U.vault_item("Visa Wei Ming", "card", {"number": CARD_NO[0], "expiry": "12/29", "cvc": "123", "holder": "TAN WEI MING"}, domains=FWD)
U.vault_item("Passport Wei Ming", "id_document", {"number": PASSPORT_NO, "name": "TAN WEI MING", "expiry": "2031-05-01", "country": "SG"},
             domains=FWD)
U.run(main)
