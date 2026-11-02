"""Chase Sapphire Reserve trip cancellation claim, with a real model on the REAL claims portal and from the user's point
of view (tests/user_common.py); nothing is filed. www.eclaimsline.com now redirects to Card Claim Center
(https://cardclaimcenter.com/internal/log-in): the whole claim lies behind the account's sign-in, which is the user's. The user's
Vault holds the Sapphire Reserve by its name only (USA; number, expiry and CVC UNKNOWN). No mailbox: the user gives the
United booking (UA 837 SFO → NRT, confirmation ABC123, USD 1,840.00 Basic Economy, non-refundable, paid with the card;
USD 86.40 taxes and fees refunded) in the request, says food poisoning kept them from the flight three days ago (the
doctor's letter given up front) and asks for the card's trip cancellation claim. What the user sees and is checked:
* the Credit cards (or Insurance & claims) bot answers (its link); the block code appends names the benefit, Trip
  Cancellation, the amount to claim (paid minus refunded, USD 1,753.60), the 20-day notice and 90-day documents
  deadlines as dates;
* the agent asks the user to take over the browser on Card Claim Center's sign-in; the user hands it back without signing in, and the block says the sign-in is theirs and nothing was filed;
* the user is never asked to approve pressing Submit, and afterwards the browser view is still on the portal;
* no card number in any answer.
If the bot stops for a question, the user answers once in the same chat. One TIMING line per request. In
run_in_docker.sh's REAL_SITES (one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_claim_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import cards_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

LOSS = dt.date.today() - dt.timedelta(days=3)
BOOKED = (dt.date.today() - dt.timedelta(days=40)).isoformat()
D20, D90 = LOSS + dt.timedelta(days=20), LOSS + dt.timedelta(days=90)
OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
PORTAL = "cardclaimcenter.com"


def no_submit(dialog: str) -> bool:
    """The user's decision on an approval dialog, by the tool it names: anything but a click for them on the claim
    (browser actions are auto-approved; a dialog about a click is Sentinel's escalation of a submit) or sending,
    replying to or forwarding an email (the user asked for none — a draft is fine)."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS | {"browser_click", "browser_click_at"}
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


async def main():
    async with U.User() as u:
        seen = []

        async def look_then_hand_back(pg, request: str) -> None:
            # the user reads the request, looks at the sign-in page in the browser view and hands it back without signing in
            url = await u.view_url()
            form = await u.view_form()
            await u.view_picture(os.path.join(OUT, f"cards_claim_takeover{len(seen)}.png") if OUT else None)
            seen.append({"url": url, "form": form, "request": request})
            print(f"INFO takeover: {request!r} url {url!r}")

        a = await u.ask(f"I came down with food poisoning and couldn't take my United flight UA 837 SFO to Tokyo Narita on {LOSS} "
                        f"(confirmation ABC123, booked on {BOOKED}: USD 1,840.00 Basic Economy, non-refundable; United refunded USD 86.40 "
                        f"in taxes and fees). I paid for it with my Chase Sapphire Reserve — can you claim the card's trip cancellation insurance for me? I saw Dr. Amy Chen at "
                        f"Mission Bay Clinic on {LOSS} and she gave me a letter saying I was unable to travel. I'll press Submit myself.",
                        label="claim", approve=no_submit, on_takeover=look_then_hand_back, timeout=1500)
        answers = [a]
        if not seen and not a.timed_out:
            # the bot stopped for a question: the user answers once in the same chat
            answers.append(await u.ask("I don't have anything else to add — please go ahead and fill in the claim on the portal; "
                                       "I'll press Submit myself.", new_chat=False, label="claim-followup", approve=no_submit,
                                       on_takeover=look_then_hand_back, timeout=1500))
        text = "\n".join(x.text for x in answers)
        dialogs = [d for x in answers for d in x.approvals]
        check("claim: answered by the Credit cards or Insurance & claims bot", any(re.search(r"Credit cards|Insurance", x.bot_link) for x in answers),
              [x.bot_link for x in answers])
        check("claim: the answer finished (no time-out)", not any(x.timed_out for x in answers), [x.seconds for x in answers])
        block = "\n".join(x.block for x in answers)
        check("claim: the block code appends after the answer names the benefit, Trip Cancellation",
              "Trip Cancellation claim on the" in block, (block, text[:300]))
        check("claim: the block code appends after the answer gives the amount to claim, paid minus refunded (1,753.60)",
              C.money_in(block, "1753.60"), block)
        check("claim: the block code appends after the answer gives the 20-day notice deadline as a date",
              "Notify the administrator by:" in block and C.date_in(block, D20), (D20, block))
        check("claim: the block code appends after the answer gives the 90-day documents deadline as a date",
              "Documents by:" in block and C.date_in(block, D90), (D90, block))
        portal = [s for s in seen if PORTAL in s["url"]]
        check("claim: the user was asked to take over the browser on Card Claim Center's sign-in", bool(portal), [x["url"] for x in seen])
        check("claim: the block says the sign-in is the user's and nothing was filed",
              "sign in or create your account" in block and "nothing has been filed" in block, block)
        check("claim: the user was never asked to approve pressing Submit", not [d for d in dialogs if re.search(r"submit", d, re.I)], dialogs)
        check("claim: no card number in the answers", not C.CARD_NO.search(text), text[:300])
        # afterwards the user opens the browser view: the portal shows no claim confirmation (Submit never pressed)
        await u.page.goto(U.APP + "#browser")
        await u.page.wait_for_selector(".bview", timeout=20000)
        await u.page.wait_for_timeout(2500)
        now = await u.view_url() if await u.page.locator(".bview .bbar input.url").count() else ""
        print(f"INFO browser view afterwards: url {now!r}")
        check("claim: the browser view is still on the portal afterwards (Submit was left to the user)", PORTAL in now, now)
        print(f"INFO claim: takeover requests {[t for x in answers for t in x.takeovers]}; files {answers[-1].files}")


U.setup(vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100",
                                         "address_home": "12 Mission St, San Francisco, CA 94105"})
U.vault_item("Chase Sapphire Reserve", "card", {}, country="USA")   # added on the Vault card by its name
U.run(main)
