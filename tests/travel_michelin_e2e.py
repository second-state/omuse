"""MICHELIN dining with a travel data connector: michelin-dining skill, michelin_search + restaurant_availability on the
connector (tests/fake_travel.py), options as cards, the deposit confirmed once, then the browser only for the
restaurant's booking page (card from the vault for the deposit).

Default: the scripted fake model (MICHELINBOOK). OMUSE_E2E_REAL=1: a real model with a plain request (see travel_common.py)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_common as T  # noqa: E402

SUITE = "travel_michelin_e2e"
D = T.day(21)
try:
    T.setup()
    if T.REAL:
        info = T.run(f"Book a table for 2 at a 2-star Michelin restaurant in Tokyo on {D} around 7pm. Lucas Lu, "
                     "lucas@example.com, +65 8000 0000. Use my card in the vault for any deposit.",
                     follow_up="Yes, book the 2-star restaurant at the time closest to 7pm now, card from the vault for the deposit.",
                     timeout=1200)
        tools, expect, max_browser = ("michelin_search", "restaurant_availability"), None, 16
    else:
        info = T.run(f"MICHELINBOOK Book a table for 2 at a 2-star restaurant in Tokyo on {D} around 19:00. "
                     "Lucas Lu, lucas@example.com, +65 8000 0000.")
        tools, max_browser = ("michelin_search", "restaurant_availability"), 11
        from tests import fake_travel
        r = fake_travel.michelin_search({"city": "Tokyo", "distinction": "2 stars"})["restaurants"][0]
        av = fake_travel.restaurant_availability({"restaurant_id": r["restaurant_id"], "date": D, "party_size": 2})
        slot = min(av["slots"], key=lambda s: abs(int(s["time"][:2]) * 60 + int(s["time"][3:]) - 19 * 60))
        expect = {"id": slot["slot_id"], "amount": float(av["deposit_due_now"].split()[-1].replace(",", "")), "currency": "JPY"}
    T.verify(SUITE, "restaurant", "michelin-dining", tools, info, max_browser=max_browser, expect=expect)
    b = (T.booked() or [{}])[-1]
    T.check("a 2-star restaurant was booked for 2 on the requested date",
            "2 Stars" in str(b.get("title")) or str(b.get("id", "")).startswith("R-TYO-2-"), b)
    T.check("the requested date and party size", D.replace("-", "") in str(b.get("id")) and str(b.get("id", "")).endswith("-2"), b)
    T.check("the deposit card was filled from the vault", b.get("card_last4") == "1111", b)
finally:
    T.finish()
