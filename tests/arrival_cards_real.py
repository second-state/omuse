"""Arrival cards with a real model, from the user's point of view: Thailand TDAC, Malaysia MDAC, Korea e-Arrival Card.
SKELETON awaiting the real official sites (TODO lines below). For each card the user asks Main's chat to fill it in
for a holiday trip (the flight and the hotel in the request, profile, passport in the Vault); the Travel bot fills the
official form with no approval dialog and asks the user to take over at the first step that is theirs; the values the
handed-over form holds are checked, and nothing is submitted. What is checked and how: tests/arrival_common.py.

Not in run_in_docker.sh's default list (it needs a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_cards_real"""
# TODO real URL: https://tdac.immigration.go.th/arrival-card/
# TODO real URL: https://imigresen-online.imi.gov.my/mdac/main
# TODO real URL: https://www.e-arrivalcard.go.kr/
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402

A.run_cards(["th", "my", "kr"])
