"""Arrival cards with a real model, from the user's point of view: Japan Visit Japan Web (the hand-over is the sign-in to the user's own account), China's online arrival card, Taiwan Arrival Card.
On the REAL official sites. For each card the user asks Main's chat to fill it in
for a holiday trip (the flight and the hotel in the request, profile, passport in the Vault); the Travel bot fills the
official form with no approval dialog and asks the user to take over at the first step that is theirs; the values the
handed-over form holds are checked, and nothing is submitted.
Taiwan verifies the email by a code: the test inbox is connected (OMUSE_E2E_IMAP_*), code sends the code, the model reads
it from the mail and code types it — then the traveller's form is filled and handed over (no inbox: a SKIP line and the
hand-over at the email step). What is checked and how: tests/arrival_common.py.

In REAL_SITES (run_in_docker.sh `realsites`, one suite at a time; a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_cards2_real"""
# the official site: https://www.vjw.digital.go.jp/main/#/vjwplo001 (moved from vjw-lp.digital.go.jp)
# the official site: https://s.nia.gov.cn/ArrivalCardFillingPC/
# the official site: https://twac.immigration.gov.tw/
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402

A.run_cards(["jp", "cn", "tw"], mailbox=True)
