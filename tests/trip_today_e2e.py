"""trip-today skill: bookings come from mail and the calendar, searched in ONE turn (the independent searches run at the
same time), the timeline names every booking found, the gap night is called out, and no browser is used."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", "Your e-ticket: SQ 638 SIN-NRT 20 Nov 2026",
     "Booking ref 7HK2LM. SQ 638 departs Singapore (SIN) 20 Nov 2026 09:25, arrives Tokyo Narita (NRT) 17:15. Return SQ 637 NRT-SIN 24 Nov 2026 11:05.", 3),
    ("Hotel Ginza Grand <reservations@ginzagrand.example>", "Booking confirmed: Hotel Ginza Grand 20-23 Nov 2026",
     "Confirmation 88812345. Check-in 20 Nov 2026 15:00, check-out 23 Nov 2026 11:00. Free cancellation until 18 Nov 2026 18:00 JST.", 2),
    ("Sushi Hoshizora <book@hoshizora.example>", "Reservation: Sushi Hoshizora 21 Nov 2026 19:00, 2 guests",
     "Your table for 2 on 21 Nov 2026 at 19:00 is confirmed. Deposit JPY 18,000 charged.", 1),
    ("Shop <news@shop.com>", "Big sale today", "50% off everything.", 0),
]
K.setup(mail=MAIL, calendar=True)
t = K.run("TRIPTODAY what have I booked for my Tokyo trip, and is there a gap?")
K.trace(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the trip-today skill was loaded first", K.loaded(t, "trip-today") and K.names(t)[0] == "load_skill", K.names(t))
K.check("mail searches and the calendar were requested in one turn and ran at the same time",
        any(b.count("gmail_search") >= 3 and "calendar_list_events" in b for b in K.batches(t)), K.batches(t))
found = "\n".join(str(r.get("preview")) for r in K.results(t, "gmail_search"))
K.check("the three bookings came back from the mailbox", all(s in found for s in ("SQ 638", "Hotel Ginza Grand", "Sushi Hoshizora")), found[:800])
K.check("the calendar was read (iCal connection)", K.results(t, "calendar_list_events") and K.results(t, "calendar_list_events")[0].get("ok"),
        K.results(t, "calendar_list_events"))
K.check("no browser was used", not any(n.startswith("browser_") for n in K.names(t)), K.names(t))
res = t.get("result") or ""
K.check("the answer lists every booking found", all(s in res for s in ("SQ 638", "Hotel Ginza Grand", "Sushi Hoshizora")), res[:800])
K.check("the answer names the night without a hotel", "GAP" in res and "2026-11-23" in res, res[:800])
K.check("the newsletter is not in the timeline", "Big sale" not in res, res[:800])
K.check("one insurance line, no selling", "insurance" in res.lower() and "buy" not in res.lower(), res[:800])
K.finish()
