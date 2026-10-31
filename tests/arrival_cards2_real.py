"""Arrival cards with a real model, from the user's point of view: Japan Visit Japan Web (the user logs in to their account when asked), China's online arrival card, Taiwan Arrival Card. For each card the user asks Main's chat to
fill it in for a holiday trip (e-ticket and hotel booking in the mail, profile, passport in the Vault); the Travel bot
fills the official form (the stand-in at its real address) with no approval dialog, answers the health and declaration
questions No, takes the sex from the profile, the next city from where the flight came from and departure = arrival +
7 days, leaves the CAPTCHA and asks the user to take over; the user types the CAPTCHA and presses Submit, and the
confirmation page's address must hold the right values. What is checked and how: tests/arrival_common.py.

Not in run_in_docker.sh's default list (it needs a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_cards2_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402

A.run_cards(["jp", "cn", "tw"])
