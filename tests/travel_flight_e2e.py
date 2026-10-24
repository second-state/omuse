"""Flight booking with a travel data connector: the agent loads the flight-booking skill, searches and compares with the
connector's MCP tools (tests/fake_travel.py on 8097), shows the options, confirms the purchase once and only then opens
the airline's booking page in the browser to fill in the traveller (passport and card from the vault) and pay.

Default: the scripted fake model (FLIGHTBOOK). OMUSE_E2E_REAL=1: a real model with a plain request (see travel_common.py)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_common as T  # noqa: E402

SUITE = "travel_flight_e2e"
D = T.day(21)
try:
    T.setup()
    if T.REAL:
        info = T.run(f"Book me the cheapest direct flight from Singapore to Tokyo on {D} for one adult. I'm Lucas Lu, "
                     "lucas@example.com, +65 8000 0000. My passport and card are in the vault.",
                     follow_up="Yes, please book the cheapest direct flight now with my passport and card from the vault.",
                     timeout=1200)
        tools, expect, max_browser = ("flights_search",), None, 18
    else:
        info = T.run(f"FLIGHTBOOK Book the cheapest direct flight from Singapore to Tokyo on {D} for one adult. "
                     "I'm Lucas Lu, lucas@example.com, +65 8000 0000.")
        tools, max_browser = ("flights_search", "flight_offer"), 12
        from tests import fake_travel
        direct = sorted((o for o in fake_travel.flights_search({"origin": "SIN", "destination": "TYO", "date": D})["offers"]
                         if o["stops"] == 0), key=lambda o: o["fare_amount"])
        expect = {"id": direct[0]["offer_id"], "amount": float(direct[0]["fare_amount"]), "currency": "SGD"}
    T.verify(SUITE, "flight", "flight-booking", tools, info, max_browser=max_browser, expect=expect)
    b = (T.booked() or [{}])[-1]
    T.check("passport and card were filled from the vault (never typed)", b.get("passport_last4") == "567X" and b.get("card_last4") == "1111", b)
    typed = [x["args"] for x in T.calls_of(info["tasks"]) if x["name"] == "browser_type"]
    T.check("no passport or card number was typed by the agent", not any(s in str(typed) for s in ("E1234567X", "4111")), typed)
    fills = [a for a in info["approvals"] if a["tool"] == "browser_fill_secret"]
    T.check("the passport fill was approved by the user", any(a["status"] == "approved" for a in fills), info["approvals"])
finally:
    T.finish()
