"""The user's own address on an arrival card, with a real model, from the user's point of view (the world, the user's
part and the checks: tests/arrival_common.py; the rule: tests/user_common.py). The request gives only the flight to Singapore, no
hotel; the profile has the home address in Sydney and addresses in Singapore.

1. own — one Singapore address in the profile ("Singapore flat"): asked for the SG Arrival Card, the agent fills it in
   with that address as the accommodation (own residence), never the Sydney home, and asks nothing; it is handed over as in
   tests/arrival_real.py (nothing submitted).
Two Singapore addresses (asked which one): tests/arrival_address2_real.py, on an install of its own (a second card for
the same trip in one install is not a user story — the Travel bot already knows it was filed).

On the REAL official site (https://eservices.ica.gov.sg/sgarrivalcard/); nothing is ever submitted.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_address_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SG = A.CARDS["sg"]
FLAT = "10 Bayfront Avenue, #12-05, Singapore 018956"
ASK = "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday. " + SG.flight_text()


def address(label: str, value: str) -> None:
    """An address the user keeps on Memory → Profile (a fixture)."""
    U.c.put(U.B + "/api/profile", json={"key": f"address:{label}", "value": value}, headers=U.H).raise_for_status()


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        address("Singapore flat", FLAT)
        answers = await A.fill_card(u, SG, label="own", ask=ASK)
        check("own: nothing was asked — the one Singapore address in the profile is the accommodation", len(answers) == 1,
              answers[0].text[:400])


A.world()
U.run(main)
