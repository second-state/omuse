"""Two of the user's own addresses on an arrival card, with a real model, from the user's point of view (the world, the
user's part and the checks: tests/arrival_common.py; the rule: tests/user_common.py). The request gives only the flight to Singapore, no
hotel; the profile has the home address in Sydney and addresses in Singapore.

two — "Singapore flat" and "Parents in Singapore": the agent asks in the chat which one, naming both; the user picks the
parents' and the form goes in with that address (a relative's residence); it is handed over as in tests/arrival_real.py
(nothing submitted). One Singapore address (used without asking): tests/arrival_address_real.py.

On the REAL official site (https://eservices.ica.gov.sg/sgarrivalcard/); nothing is ever submitted.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_address2_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SG = A.CARDS["sg"]
FLAT, PARENTS = "10 Bayfront Avenue, #12-05, Singapore 018956", "5 Jalan Kayu, #03-01, Singapore 799487"
ASK = "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday. " + SG.flight_text()


def address(label: str, value: str) -> None:
    """An address the user keeps on Memory → Profile (a fixture)."""
    U.c.put(U.B + "/api/profile", json={"key": f"address:{label}", "value": value}, headers=U.H).raise_for_status()


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        address("Singapore flat", FLAT)
        address("Parents in Singapore", PARENTS)
        answers = await A.fill_card(u, SG, label="two", ask=ASK,
                                    follow_up="The Parents in Singapore one, please.")
        # the question is a choice card (present_choices): its options name both addresses
        cards = await u.choices()
        options = U.choice_text(cards).lower()
        check("two: the agent asked which Singapore address with a choice card whose options name both",
              len(answers) == 2 and bool(cards) and ("bayfront" in options or "flat" in options) and ("jalan kayu" in options or "parents" in options),
              options[:400])


A.world()
U.run(main)
