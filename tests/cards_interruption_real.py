"""Chase Sapphire Reserve trip interruption claim, with a real model and from the user's point of view
(tests/user_common.py). The user flew to Tokyo, broke a wrist on the second day and flew home that evening on the
doctor's advice. The mailbox has the United round trip (the return UA 838 moved forward for a USD 300.00 change fee,
charged to the card ending 4321), the Park Hyatt Tokyo's prepaid non-refundable stay (5 nights at USD 250.00) and its
early-departure letter (one night used, one more refunded as a courtesy: USD 750.00 for 3 nights not refunded), and
Chase's charge alerts. The Vault has the Sapphire Reserve by its name only. The card-benefit claims site, eClaimsLine,
is the stand-in at its real address (https://www.eclaimsline.com, tests/pages/eclaims.html).

The user asks Main's chat to start the card's insurance claim and says they will press Submit themselves. What the
user sees and is checked:
* the Credit cards (or Insurance & claims) bot answers (its link); the benefit is Trip Interruption (the trip had
  started), not Trip Cancellation;
* the amount: the unused, unrefunded hotel nights plus the extra cost of getting home, 750.00 + 300.00 = USD 1,050.00;
  the 90-day documents deadline from the day the trip was cut short as a date, and the 20-day notice deadline too
  unless the user already filed the claim (the notice is then given);
* the agent asks the user to take over the browser for Submit; the user looks at eClaimsLine's form in the browser view,
  presses Submit themselves, and the portal's confirmation shows what it received: Trip Interruption, the date of loss,
  USD 1,050.00 and the cardholder's name; then the user hands the browser back;
* the answer never says the assistant submitted the claim; no card number in any answer.
If the bot stops for a question, the user answers once in the same chat. One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_interruption_real"""
import datetime as dt
import os
import re
import sys
from urllib.parse import parse_qs, urlsplit

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import cards_common as C  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
ARRIVE = TODAY - dt.timedelta(days=8)        # check-in at the hotel
LOSS = ARRIVE + dt.timedelta(days=1)         # the wrist broken, the trip cut short, the flight home that evening
OUT_DAY, BACK_DAY = ARRIVE - dt.timedelta(days=1), ARRIVE + dt.timedelta(days=5)
BOOKED = (TODAY - dt.timedelta(days=60)).isoformat()
D20, D90 = LOSS + dt.timedelta(days=20), LOSS + dt.timedelta(days=90)
OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
PORTAL = "https://www.eclaimsline.com/"
MAIL = [
    ("United Airlines <noreply@united.example>", f"Your eTicket receipt — Confirmation QX7P2L — SFO-NRT {OUT_DAY}, NRT-SFO {BACK_DAY}",
     f"Passenger LUCAS LU. UA 837 San Francisco (SFO) {OUT_DAY} 11:05 → Tokyo Narita (NRT) {ARRIVE} 14:30. UA 838 Tokyo Narita (NRT) "
     f"{BACK_DAY} 17:00 → San Francisco (SFO) {BACK_DAY} 10:20. Fare USD 1,620.00 round trip, Economy. Paid with Visa ending 4321 on {BOOKED}.", 60),
    ("Chase <no.reply.alerts@chase.example>", "Your $1,620.00 transaction with UNITED AIRLINES",
     f"A $1,620.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on {BOOKED}.", 60),
    ("Park Hyatt Tokyo <reservations@parkhyatttokyo.example>", f"Reservation confirmed — HY-55821 — {ARRIVE} to {BACK_DAY}",
     f"Guest LUCAS LU. 5 nights, check-in {ARRIVE}, check-out {BACK_DAY}, Park Deluxe King. Prepaid rate USD 250.00 per night, "
     f"USD 1,250.00 total, non-refundable. Charged to Visa ending 4321 on {BOOKED}.", 60),
    ("Chase <no.reply.alerts@chase.example>", "Your $1,250.00 transaction with PARK HYATT TOKYO",
     f"A $1,250.00 transaction with PARK HYATT TOKYO was made on your Sapphire Reserve (...4321) on {BOOKED}.", 60),
    ("United Airlines <noreply@united.example>", f"Itinerary change confirmed — QX7P2L — UA 838 now on {LOSS}",
     f"Your return flight UA 838 Tokyo Narita (NRT) → San Francisco (SFO) was moved from {BACK_DAY} to {LOSS} at 17:00 at your request. "
     f"Change fee USD 300.00 charged to Visa ending 4321 on {LOSS}.", 7),
    ("Chase <no.reply.alerts@chase.example>", "Your $300.00 transaction with UNITED AIRLINES",
     f"A $300.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on {LOSS}.", 7),
    ("Park Hyatt Tokyo <reservations@parkhyatttokyo.example>", "Early departure — reservation HY-55821",
     f"Dear Mr Lu, you checked out on {LOSS} after 1 night of your 5-night stay. As a courtesy we have refunded USD 250.00 (one night) "
     f"to Visa ending 4321. The remaining 3 nights (USD 750.00) are non-refundable under the prepaid rate. We wish you a speedy recovery.", 6),
]


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

        async def review_and_submit(pg, request: str) -> None:
            # the user reads the request, looks at the claim form, takes over and presses Submit themselves
            url = await u.view_url()
            await u.take_over()
            await u.view_picture(os.path.join(OUT, f"cards_interruption_form{len(seen)}.png") if OUT else None)
            after = ""
            if "eclaimsline.com" in url and "eclaims_done" not in url:
                where = await u.layout(PORTAL, {"submit": "#submit"})
                x, y, w, h = where["submit"]
                await u.click_view(x + w / 2, y + h / 2)
                for _ in range(10):
                    after = await u.view_url()
                    if "eclaims_done" in after:
                        break
                    await pg.wait_for_timeout(1000)
                await u.view_picture(os.path.join(OUT, "cards_interruption_received.png") if OUT else None)
            seen.append({"url": url, "after": after, "request": request})
            print(f"INFO takeover: {request!r} url {url!r}; after Submit the URL bar shows {after!r}")

        a = await u.ask(f"I broke my wrist in Tokyo on {LOSS} and the doctor at St. Luke's International Hospital told me to go home, so I cut "
                        "my trip short and flew back that evening. The flights and the hotel were paid with my Chase Sapphire Reserve. "
                        "Please start the card's insurance claim for me on the claims site — I'll review it and press Submit myself.",
                        label="interruption", approve=no_send, on_takeover=review_and_submit, timeout=1500)
        answers = [a]
        if not seen and not a.timed_out:
            answers.append(await u.ask("Nothing else to add — please fill in the claim on the claims site now; I'll press Submit myself.",
                                       new_chat=False, label="interruption-followup", approve=no_send, on_takeover=review_and_submit,
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
        check("interruption: the user was asked to take over the browser on eClaimsLine's claim form",
              any("eclaimsline.com" in s["url"] for s in seen), seen)
        done = [s["after"] for s in seen if "eclaims_done" in s["after"]]
        q = {k: v[0] for k, v in parse_qs(urlsplit(done[-1] if done else "").query).items()}
        print(f"INFO the portal received: {q}")
        check("interruption: the portal received the Trip Interruption benefit", q.get("benefit") == "Trip Interruption", q)
        check("interruption: the portal received the date of loss", q.get("loss") == LOSS.isoformat(), q)
        check("interruption: the portal received USD 1,050.00", C.money_in(q.get("amount", ""), "1050.00"), q)
        # once the user has filed the claim the 20-day notice is given; before that, the answer must give its date
        check("interruption: the block code appends after the answer gives the 20-day notice deadline as a date",
              C.date_in(block, D20), (D20, block))
        check("interruption: the portal received the cardholder's name", "lucas" in q.get("name", "").lower() and "lu" in q.get("name", "").lower(), q)
        check("interruption: the answer never says the assistant submitted the claim",
              not re.search(r"\bI( have|'ve)? (submitted|filed|sent)( in)? (the|your) claim", text, re.I), text[-600:])
        check("interruption: no card number in the answers", not C.CARD_NO.search(text), text[:300])
        print(f"INFO interruption: takeover requests {[t for x in answers for t in x.takeovers]}; files {answers[-1].files}")


U.setup(mail=MAIL, vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100",
                                         "address_home": "12 Mission St, San Francisco, CA 94105"})
K.vault_item("Chase Sapphire Reserve", "card", {}, country="USA")   # added on the Vault card by its name
U.run(main)
