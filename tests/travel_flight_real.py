"""Booking a flight with a real model, from the user's point of view — tests/travel_book_user.py has the scenario and the
checks: the user asks Main to find and book through their travel agency's connector, the agent fills the booking page
(the traveller from the profile, passport and card from the Vault) and hands it over, the user presses the final button
in the live view and hands back, and "My trips" on the booking site shows the booking.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_flight_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_book_user  # noqa: E402

travel_book_user.main("flight")
