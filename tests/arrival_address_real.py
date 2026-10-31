"""The user's own address on an arrival card, with a real model, from the user's point of view (the world, the user's
part and the checks: tests/arrival_common.py; the rule: tests/user_common.py). The mailbox holds only the e-ticket to
Singapore, no hotel booking; the profile has the home address in Sydney and addresses in Singapore.

1. own — one Singapore address in the profile ("Singapore flat"): asked for the SG Arrival Card, the agent fills it in
   with that address as the accommodation (own residence), never the Sydney home, and asks nothing; the user types the
   CAPTCHA and submits as in tests/arrival_real.py.
Two Singapore addresses (asked which one): tests/arrival_address2_real.py, on an install of its own (a second card for
the same trip in one install is not a user story — the Travel bot already knows it was filed).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_address_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests.user_common import check  # noqa: E402

SG = A.CARDS["sg"]
FLAT = "10 Bayfront Avenue, Singapore 018956"
ASK = "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday."


def address(label: str, value: str) -> None:
    """An address the user keeps on Memory → Profile (a fixture)."""
    K.c.put(K.B + "/api/profile", json={"key": f"address:{label}", "value": value}, headers=K.H).raise_for_status()


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        address("Singapore flat", FLAT)
        answers = await A.fill_card(u, SG, label="own", ask=ASK, hotel="Bayfront", accom=("OWN",))
        check("own: nothing was asked — the one Singapore address in the profile is the accommodation", len(answers) == 1,
              answers[0].text[:400])


A.world(SG.mail()[:1])
U.run(main)
