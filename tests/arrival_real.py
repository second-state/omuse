"""SG Arrival Card, and an arrival card whose trip the user did not give, on the REAL official sites with a real model, from
the user's point of view (the install, the user's part and the checks: tests/arrival_common.py; the rule:
tests/user_common.py). Nothing is ever submitted.

1. SG Arrival Card on ICA's site (https://eservices.ica.gov.sg/sgarrivalcard/ → Fill in Manually as a visitor): the
   user asks Main's chat to fill it in (flight and hotel in the request); the Travel bot fills everything from the
   request, the profile and the Vault with no approval dialog and asks the user to take over for their review and
   Submit; the page handed over holds the right values (passport and expiry from the Vault, health and declarations No,
   last city Sydney, departure = arrival + 7); the passport number never shows in the chat.
2. No trip details: asked to fill in the Thailand TDAC (https://tdac.immigration.go.th/) without the flight or the stay,
   the agent asks ONCE for them, the user answers in the chat, and the card goes to the official site — handed over at
   the home page's Cloudflare check, the user's step (never solved or got around) — no second question.

In REAL_SITES (run_in_docker.sh `realsites`, one suite at a time; a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SG, TH = A.CARDS["sg"], A.CARDS["th"]


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        await A.fill_card(u, SG, label="sgac")

        A.fresh_trip()   # a new story: nothing remembered from the Singapore trip
        answers = await A.fill_card(u, TH, label="nodetails", ask=TH.ask, follow_up=f"{TH.details()} It's a holiday.")
        first = answers[0]
        # the first turn asked (an answer with no takeover, so the user's details were needed) and the form went in
        # after the one answer: two turns, one hand-over
        check("nodetails: without the trip the agent first asked once (a turn with no takeover), then filled the form from the answer",
              len(answers) == 2 and not first.takeovers and bool(answers[1].takeovers), [len(x.takeovers) for x in answers])


A.world()
U.run(main)
