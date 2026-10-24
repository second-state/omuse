"""Hotel booking with a travel data connector: hotel-booking skill, hotels_search + hotel_rooms on the connector
(tests/fake_travel.py), options as cards, one purchase confirmation, then the browser only for the hotel's booking page.

Default: the scripted fake model (HOTELBOOK). OMUSE_E2E_REAL=1: a real model with a plain request (see travel_common.py)."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_common as T  # noqa: E402

SUITE = "travel_hotel_e2e"
CI, CO = T.day(28), T.day(31)
try:
    T.setup()
    if T.REAL:
        info = T.run(f"Book the cheapest 4-star hotel in Tokyo with free cancellation, check-in {CI}, check-out {CO}, "
                     "2 guests. Book it under Lucas Lu, lucas@example.com, +65 8000 0000.",
                     follow_up="Yes, go ahead and book the cheapest 4-star room with free cancellation now.", timeout=1200)
        tools, expect, max_browser = ("hotels_search",), None, 14
    else:
        info = T.run(f"HOTELBOOK Book the cheapest 4-star hotel in Tokyo with free cancellation, check-in {CI}, "
                     f"check-out {CO}, 2 guests. Lucas Lu, lucas@example.com, +65 8000 0000.")
        tools, max_browser = ("hotels_search", "hotel_rooms"), 8
        from tests import fake_travel
        h = fake_travel.hotels_search({"city": "Tokyo", "check_in": CI, "check_out": CO, "guests": 2, "min_stars": 4})["hotels"][0]
        rooms = fake_travel.hotel_rooms({"hotel_id": h["hotel_id"], "check_in": CI, "check_out": CO, "guests": 2})["rooms"]
        room = min((r for r in rooms if r["cancellation"].startswith("Free")), key=lambda r: r["total_amount"])
        expect = {"id": room["room_offer_id"], "amount": float(room["total_amount"]), "currency": "SGD"}
    T.verify(SUITE, "hotel", "hotel-booking", tools, info, max_browser=max_browser, expect=expect)
    b = (T.booked() or [{}])[-1]
    if T.REAL:   # informative in real mode: was it the cheapest 4-star free-cancellation room?
        from tests import fake_travel
        try:
            h, room = fake_travel._room_by_id(str(b.get("id")))
            print(f"INFO booked {h['name']} ({h['stars']}-star) {room['room_type']} {room['total']} — {room['cancellation']}")
            T.check("the booked room is in a 4-star hotel and can be cancelled for free", h["stars"] == 4 and room["cancellation"].startswith("Free"), (h, room))
        except fake_travel.ToolError as e:
            T.check("the booked id is a room offer", False, e)
    T.check("the stay dates in the booking are the requested ones", CI.replace("-", "") in str(b.get("id")) and CO.replace("-", "") in str(b.get("id")), b)
finally:
    T.finish()
