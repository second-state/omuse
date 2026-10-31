"""Two of the user's own addresses on an arrival card, with a real model, from the user's point of view (the world, the
user's part and the checks: tests/arrival_common.py; the rule: tests/user_common.py). The mailbox holds only the e-ticket to
Singapore, no hotel booking; the profile has the home address in Sydney and addresses in Singapore.

two — "Singapore flat" and "Parents in Singapore": the agent asks in the chat which one, naming both; the user picks the
parents' and the form goes in with that address (a relative's residence); the user types the CAPTCHA and submits as in
tests/arrival_real.py. One Singapore address (used without asking): tests/arrival_address_real.py.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_address2_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests.user_common import check  # noqa: E402

SG = A.CARDS["sg"]
FLAT, PARENTS = "10 Bayfront Avenue, Singapore 018956", "5 Jalan Kayu, Singapore 799487"
ASK = "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday."


def address(label: str, value: str) -> None:
    """An address the user keeps on Memory → Profile (a fixture)."""
    K.c.put(K.B + "/api/profile", json={"key": f"address:{label}", "value": value}, headers=K.H).raise_for_status()


async def main():
    async with U.User(viewport=(1440, 1500)) as u:
        address("Singapore flat", FLAT)
        address("Parents in Singapore", PARENTS)
        answers = await A.fill_card(u, SG, label="two", ask=ASK, hotel="Jalan Kayu", accom=("RESIDENCE", "OWN"),
                                    follow_up="The Parents in Singapore one, please.")
        first = (answers[0].text + " " + await u.thread_text()).lower()   # the question, or its choice cards
        check("two: the agent asked in the chat which Singapore address, naming both",
              len(answers) == 2 and ("bayfront" in first or "flat" in first) and ("jalan kayu" in first or "parents" in first),
              first[:400])


A.world(SG.mail()[:1])
U.run(main)
