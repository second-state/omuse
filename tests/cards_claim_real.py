"""Chase Sapphire Reserve trip cancellation claim, with a real model and from the user's point of view
(tests/user_common.py). The Money bot's example, word for word: "My father went into hospital so I had to cancel my
Tokyo trip. I paid the flights and the hotel with my Chase Sapphire Reserve — please start the trip cancellation
claim." The model asks once for what the claim needs; the user answers in the same chat (the bookings, what was paid
and refunded, the dates, the doctor's letter). Code prepares the claim — the benefit, the amount, the deadlines, the
documents — and hands over Card Claim Center's link, where the user signs in, files and uploads; no page is opened in
the agent's browser and nothing is filed. The user's Vault holds the Sapphire Reserve by its name only (USA). Checked
from the user's side (smoke checks on the block code appends):
* the Money bot answers (its link); no takeover, no approval dialog;
* the block names the benefit (Trip Cancellation), the amount (paid minus refunded: USD 2,900.00), the 20-day notice
  deadline as a date, and Card Claim Center's link; it says nothing has been filed yet;
* no card number in any answer.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_claim_real"""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import cards_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

LOSS = dt.date.today() - dt.timedelta(days=3)
BOOKED = (dt.date.today() - dt.timedelta(days=45)).isoformat()
TRIP = dt.date.today() + dt.timedelta(days=4)
D20 = LOSS + dt.timedelta(days=20)


async def main():
    async with U.User() as u:
        ask = "My father went into hospital so I had to cancel my Tokyo trip. I paid the flights and the hotel with my Chase Sapphire Reserve — please start the trip cancellation claim."  # noqa: E501
        a = await u.ask(ask, label="csr", timeout=1200)
        answers = [a]
        if "Card Claim Center" not in a.block and not a.timed_out:
            # the bot asked once for what the claim needs: the user answers in the same chat
            answers.append(await u.ask(
                f"He was admitted on {LOSS} and I cancelled the same day; the trip was {TRIP} to {TRIP + dt.timedelta(days=5)}, "
                f"booked on {BOOKED}. United UA 837 SFO–Narita, confirmation ABC123: USD 1,840.00, non-refundable, nothing "
                f"refunded. Park Hyatt Tokyo, booking HY-55821: 5 prepaid nights, USD 1,250.00, they refunded USD 190.00. "
                "His doctor gave me a letter. Nothing else — please go ahead.",
                new_chat=False, label="csr-details", timeout=1200))
        block = "\n".join(x.block for x in answers)
        links = [x for y in answers for x in y.block_links]
        text = "\n".join(x.text for x in answers)
        check("csr: the answer finished (no time-out)", not any(x.timed_out for x in answers), [x.seconds for x in answers])
        check("csr: the Money bot answered", any("Money" in x.bot_link for x in answers), [x.bot_link for x in answers])
        check("csr: no takeover and no approval dialog (the claim is prepared, not filed)",
              not any(x.takeovers or x.approvals for x in answers), [(x.takeovers, x.approvals) for x in answers])
        check("csr: the block names the benefit, Trip Cancellation", "Trip Cancellation claim on the" in block, block[:800])
        check("csr: the block gives the amount, paid minus refunded (USD 2,900.00)", C.money_in(block, "2900.00"), block[:800])
        check("csr: the block gives the 20-day notice deadline as a date", "Notify the administrator by:" in block
              and C.date_in(block, D20), (D20, block[:800]))
        check("csr: the block hands over Card Claim Center's link and says nothing has been filed yet",
              any("cardclaimcenter.com" in x for x in links) and "nothing has been filed yet" in block, (links, block[:800]))
        check("csr: no card number in the answers", not C.CARD_NO.search(text), text[:300])


U.setup(vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100",
                              "address_home": "12 Mission St, San Francisco, CA 94105"})
U.vault_item("Chase Sapphire Reserve", "card", {}, country="USA")   # added on the Vault card by its name
U.run(main)
