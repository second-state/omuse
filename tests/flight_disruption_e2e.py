"""flight-disruption skill: the booking and the insurance policy from mail (one turn), the live status READ from the
airline's status page, alternatives from the travel data connector (no airline-site clicking), the insurance proof
named, and a watch left on the status page."""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "insurance"   # the Insurance & claims bot has this suite's skills (crate::bots)

TODAY = dt.date.today().isoformat()
MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", f"Your e-ticket: SQ 638 SIN-NRT {TODAY}",
     f"Booking ref 7HK2LM. SQ 638 departs Singapore (SIN) {TODAY} 09:25, arrives Tokyo Narita (NRT) 17:15.", 2),
    ("TravelSure <policy@travelsure.example>", "Your travel insurance policy TI-2026-77",
     "Policy TI-2026-77 covers 15-30 Nov 2026. Flight delay benefit: SGD 100 per full 6 hours (delay certificate required). "
     "Baggage delay: SGD 150 after 6 hours. 24-hour assistance: +65 6000 0000.", 5),
]
K.setup(mail=MAIL, travel=True)
before = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
t = K.run(f"FLIGHTDELAY is my flight SQ 638 today delayed? What are my options? DATE={TODAY}")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the flight-disruption skill was loaded", K.loaded(t, "flight-disruption"), names)
K.check("booking and policy were searched in one turn", any(b.count("gmail_search") == 2 for b in K.batches(t)), K.batches(t))
K.check("the status page was read, not navigated", "browser_read" in names and "browser_navigate" not in names, names)
K.check("alternatives came from the travel data connector", any(n.startswith("mcp_travel__") and n.endswith("flights_search") for n in names), names)
after = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.check("a watch on the status page was created", "SQ 638 status" in after - before, after)
res = t.get("result") or ""
K.check("the new time comes from the status page", "12:40" in res and "09:25" in res, res[:600])
K.check("the policy number and the delay-certificate proof are named", "TI-2026-77" in res and "certificate" in res, res[:600])
K.check("an alternative flight from the connector is named", "Alternatives today (connector): " in res and "none found" not in res, res[:600])
K.check("no approval was needed (reads only)", not t["_approvals"], t["_approvals"])
K.finish()
