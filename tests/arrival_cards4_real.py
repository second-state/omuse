"""The UK ETA and the EU's ETIAS with a real model, from the user's point of view (tests/arrival_common.py).
SKELETON for the UK ETA, awaiting the real official site (TODO line below); nothing is ever submitted or paid.

1. UK ETA: the user asks Main's chat to apply for it (holiday trip, their job in the request); the Travel bot fills the
   application from the profile and the Vault (passport number and expiry, home address, criminality questions No)
   with no approval dialog and hands it over at the user's own step (the face photo, the payment); the handed-over
   form holds the right values; the passport number never shows in the chat.
2. ETIAS: asked what to fill before a trip to Paris, the agent reads the official status, files nothing (ETIAS is not
   in operation) and points to the official EU site — no form, no takeover, no dialog.

Not in run_in_docker.sh's default list (it needs a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_cards4_real"""
# TODO real URL: https://apply-for-an-eta.homeoffice.gov.uk/ (from https://www.gov.uk/eta)
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402

UK = A.CARDS["uk"]


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        await A.fill_card(u, UK, label="uk-eta")
        await A.etias(u)


A.world()
U.run(main)
