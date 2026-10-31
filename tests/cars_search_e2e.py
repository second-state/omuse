"""cars_search, kept as a scripted suite for what a real run cannot force or count: Kayak's page failing (an error, a bot
check) so the booking link falls back to Rentalcars.com's page with a note, neither page answering, only Kayak's page
checked when it answers (one request, no Rentalcars.com), and a car question answered with cars_search alone (no
web_search beside it, no browser). The rental the user sees — the companies, "from" prices and Kayak's page for the
airport, dates, times and class — is tests/travel_user_real.py (real model, real Kayak); the links' formats and the
argument checks (an airport, a return after the pickup, a known class) are unit tests in crates/omuse-apps (cars.rs,
travel_args.rs)."""
import datetime as dt
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # cars_search is a Travel bot tool (crate::bots)

B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
FAKE = "http://127.0.0.1:8094"
D1 = (dt.date.today() + dt.timedelta(days=30)).isoformat()
D3 = (dt.date.today() + dt.timedelta(days=33)).isoformat()
KAYAK, RC = FAKE + "/kayak", FAKE + "/rentalcars"


def act(args, task="cars-t1"):
    return c.post(B + "/internal/act", headers=RT, json={"task_id": task, "call_id": "c" + str(time.time()), "tool": "cars_search", "args": args}).json()


def mode(**m):
    c.post(FAKE + "/cars/_mode", json={"kayak": "ok", "rentalcars": "ok", **m})


def car(r):
    return (r.get("result") or {}).get("car_rental") or {}


K.setup(language="en")
names = [t["function"]["name"] for t in c.get(B + "/internal/catalog", headers=RT).json()["tools"]]
K.check("cars_search is in the catalog with no key needed", "cars_search" in names, [n for n in names if "car" in n])

mode()
n0 = len(c.get(FAKE + "/cars/_log").json())
r = act({"pickup": "LAX", "dropoff": "SFO", "pickup_date": D1, "dropoff_date": D3, "pickup_time": "9:30 AM", "dropoff_time": "2 PM",
         "car_class": "SUV", "companies": ["Hertz"]})
log = c.get(FAKE + "/cars/_log").json()[n0:]
K.check("only Kayak's page was checked (Rentalcars.com is the fallback only)", [x["site"] for x in log] == ["kayak"], log)

for m, why in (("down", "HTTP 503"), ("bot", "a bot check page")):
    mode(kayak=m)
    r = act({"pickup": "NRT", "pickup_date": D1, "dropoff_date": D3, "car_class": "economy"})
    cr = car(r)
    K.check(f"Kayak's page {m}: the link is Rentalcars.com's page for the rental, and the note says why",
            cr.get("book_with") == "Rentalcars.com" and cr.get("booking_url", "").startswith(f"{RC}/search-results?locationIata=NRT&dropLocationIata=NRT&puDay=")
            and "driversAge=30" in cr["booking_url"] and why in str(cr.get("booking_note")), cr)
mode(kayak="down", rentalcars="down")
r = act({"pickup": "NRT", "pickup_date": D1, "dropoff_date": D3})
cr = car(r)
K.check("neither page answers: still Kayak's link, with a note that it could not be checked from here", r.get("status") == "ok" and cr.get("book_with") == "Kayak"
        and cr.get("booking_url") == f"{KAYAK}/cars/NRT/{D1}-10h/{D3}-10h" and "neither Kayak" in str(cr.get("booking_note")), cr)
mode()

t = K.run(f"CARSEARCH a compact car at Narita DATE={D1} TO={D3}")
K.trace(t)
nm = K.names(t)
K.check("a car question is cars_search alone — no web_search beside it, no browser — and the answer gives Kayak's link",
        t["status"] == "COMPLETED" and nm == ["cars_search"] and f"{KAYAK}/cars/NRT/{D1}-10h/{D3}-10h?fs=carclass=small" in (t.get("result") or ""), (nm, (t.get("result") or "")[:300]))
K.finish()
