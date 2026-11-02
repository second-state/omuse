"""Chase Sapphire Reserve trip interruption claim, with a real model on the REAL claims portal and from the user's point
of view (tests/user_common.py); nothing is filed. www.eclaimsline.com now redirects to Card Claim Center
(https://cardclaimcenter.com/internal/log-in): the whole claim lies behind the account's sign-in, which is the user's. The user flew
to Tokyo, broke a wrist on the second day and flew home that evening on the doctor's advice. No mailbox: the user gives
the costs in the request (the return UA 838 moved forward for a USD 300.00 change fee; the Park Hyatt Tokyo's prepaid
non-refundable stay, 5 nights at USD 250.00, one night used and one refunded as a courtesy: USD 750.00 for 3 nights not
refunded; all paid with the card). The Vault has the Sapphire Reserve by its name only.

The user asks Main's chat to start the card's insurance claim and says they will press Submit themselves. What the
user sees and is checked:
* the Credit cards (or Insurance & claims) bot answers (its link); the block code appends names the benefit, Trip
  Interruption (the trip had started), the amount (750.00 + 300.00 = USD 1,050.00), the 90-day documents deadline and
  the 20-day notice deadline as dates;
* the agent asks the user to take over the browser on Card Claim Center's sign-in; the user hands it back without signing in;
* no card number in any answer.
If the bot stops for a question, the user answers once in the same chat. One TIMING line per request. In
run_in_docker.sh's REAL_SITES (one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_interruption_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import cards_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
ARRIVE = TODAY - dt.timedelta(days=8)        # check-in at the hotel
LOSS = ARRIVE + dt.timedelta(days=1)         # the wrist broken, the trip cut short, the flight home that evening
D20, D90 = LOSS + dt.timedelta(days=20), LOSS + dt.timedelta(days=90)
OUT = "/repo-out" if os.path.isdir("/repo-out") else ""


def no_send(dialog: str) -> bool:
    """The user's decision on an approval dialog, by the tool it names: anything but sending, replying to or forwarding
    an email, or a click for them (the user said they would press Submit; browser actions are auto-approved, so a
    dialog about a click is Sentinel's escalation of a submit)."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS | {"browser_click", "browser_click_at"}
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


async def main():
    async with U.User() as u:
        seen = []

        async def look_then_hand_back(pg, request: str) -> None:
            # the user reads the request, looks at the sign-in page and hands it back without signing in
            url = await u.view_url()
            form = await u.view_form()
            await u.view_picture(os.path.join(OUT, f"cards_interruption_form{len(seen)}.png") if OUT else None)
            seen.append({"url": url, "form": form, "request": request})
            print(f"INFO takeover: {request!r} url {url!r}")

        a = await u.ask(f"I broke my wrist in Tokyo on {LOSS} and the doctor at St. Luke's International Hospital told me to go home, so I cut "
                        f"my trip short and flew back that evening: United moved my return UA 838 to {LOSS} for a USD 300.00 change fee, and the Park "
                        f"Hyatt Tokyo (prepaid, non-refundable, 5 nights at USD 250.00 from {ARRIVE}) refunded one night as a courtesy, so 3 "
                        "nights (USD 750.00) are lost. The flights and the hotel were paid with my Chase Sapphire Reserve. "
                        "Please start the card's insurance claim for me on the claims site — I'll review it and press Submit myself.",
                        label="interruption", approve=no_send, on_takeover=look_then_hand_back, timeout=1500)
        answers = [a]
        if not seen and not a.timed_out:
            answers.append(await u.ask("Nothing else to add — please fill in the claim on the claims site now; I'll press Submit myself.",
                                       new_chat=False, label="interruption-followup", approve=no_send, on_takeover=look_then_hand_back,
                                       timeout=1500))
        text = "\n".join(x.text for x in answers)
        check("interruption: answered by the Credit cards or Insurance & claims bot",
              any(re.search(r"Credit cards|Insurance", x.bot_link) for x in answers), [x.bot_link for x in answers])
        check("interruption: the answer finished (no time-out)", not any(x.timed_out for x in answers), [x.seconds for x in answers])
        block = "\n".join(x.block for x in answers)
        check("interruption: the block code appends after the answer names the benefit, Trip Interruption",
              "Trip Interruption claim on the" in block, (block, text[:300]))
        check("interruption: the block code appends after the answer gives the amount, the unrefunded nights plus the change fee "
              "(USD 1,050.00)", C.money_in(block, "1050.00"), block)
        check("interruption: the block code appends after the answer gives the 90-day documents deadline as a date",
              "Documents by:" in block and C.date_in(block, D90), (D90, block))
        portal = [x for x in seen if "cardclaimcenter.com" in x["url"]]
        check("interruption: the user was asked to take over the browser on Card Claim Center's sign-in", bool(portal),
              [x["url"] for x in seen])
        check("interruption: the block code appends after the answer gives the 20-day notice deadline as a date",
              C.date_in(block, D20), (D20, block))
        check("interruption: no card number in the answers", not C.CARD_NO.search(text), text[:300])
        print(f"INFO interruption: takeover requests {[t for x in answers for t in x.takeovers]}; files {answers[-1].files}")


U.setup(vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100",
                                         "address_home": "12 Mission St, San Francisco, CA 94105"})
U.vault_item("Chase Sapphire Reserve", "card", {}, country="USA")   # added on the Vault card by its name
U.run(main)
