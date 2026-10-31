"""SG Arrival Card and an arrival card without a mailbox, with a real model, from the user's point of view (the world,
the user's part and the checks: tests/arrival_common.py; the rule: tests/user_common.py).

1. SG Arrival Card on ICA's site (the stand-in at https://eservices.ica.gov.sg, tests/pages/sgac.html: masked
   DD/MM/YYYY dates, the health and declaration questions, a CAPTCHA): the user asks Main's chat to fill it in; the
   Travel bot fills everything from the mail, the profile and the Vault with no approval dialog, leaves the CAPTCHA and
   asks the user to take over; the user types the CAPTCHA and presses Submit; the confirmation page's address holds the
   right values (passport expiry 01/05/2031 in the field's order, health and declarations No, next city Sydney,
   departure = arrival + 7) and the answer quotes the DE number; the passport number never shows in the chat.
2. No mailbox (the user removed it): asked to fill in the Thailand TDAC, the agent asks ONCE for the flight and the
   stay (and says a connected mailbox would find them), the user answers in the chat, and the form is filled with
   those answers and handed over as above — no second question.

Not in run_in_docker.sh's default list (it needs a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SG, TH = A.CARDS["sg"], A.CARDS["th"]


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        await A.fill_card(u, SG, label="sgac")

        A.fresh_trip([])   # a new story: no mail, nothing remembered from the Singapore trip
        A.disconnect_mail()
        details = (f"I arrive on {TH.flight} from Sydney on {A.ARRIVE.isoformat()}, and I'm staying at the {TH.hotel[0]}, "
                   f"{TH.hotel[1]}. It's a holiday.")
        answers = await A.fill_card(u, TH, label="nomail", follow_up=details)
        first = answers[0]
        check("nomail: with no mailbox the agent first asked (once) for the flight and the accommodation",
              len(answers) == 2 and re.search(r"flight", first.text, re.I) is not None
              and re.search(r"hotel|stay|accommodation|address", first.text, re.I) is not None, first.text[:500])
        check("nomail: the question says a connected mailbox would let it find the bookings",
              re.search(r"mail|inbox", first.text, re.I) is not None, first.text[:500])


A.world(SG.mail())
U.run(main)
