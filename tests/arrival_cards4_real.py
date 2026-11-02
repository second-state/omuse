"""The UK ETA and the EU's ETIAS with a real model, from the user's point of view (tests/arrival_common.py).
On the REAL official site; nothing is ever submitted or paid.

1. UK ETA (https://apply-for-an-eta.homeoffice.gov.uk/, one question per page): the user asks Main's chat to apply for it
   (holiday trip, their job in the request); the Travel bot walks the start pages to the web application and its email
   step with no approval dialog: with the test inbox connected (OMUSE_E2E_IMAP_*) code puts the email in, presses
   "Continue application", the model reads the security code from the mail and code types it — the page handed over is
   past the email step (the passport and face photos are the user's); without an inbox (a SKIP line) the hand-over is at
   the email step with the email filled in. A Queue-it waiting room, when the site has one up, is handed over as it is.
   The passport number never shows in the chat.
2. ETIAS: asked what to fill before a trip to Paris, the agent reads the official status, files nothing (ETIAS is not
   in operation) and points to the official EU site — no form, no takeover, no dialog.

In REAL_SITES (run_in_docker.sh `realsites`, one suite at a time; a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_cards4_real"""
# the official site: https://apply-for-an-eta.homeoffice.gov.uk/apply/electronic-travel-authorisation (from https://www.gov.uk/eta)
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402

UK = A.CARDS["uk"]


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        await A.fill_card(u, UK, label="uk-eta", mailbox=MAILBOX)
        await A.etias(u)


MAILBOX = A.world(mailbox=True)
U.run(main)
