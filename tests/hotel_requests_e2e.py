"""hotel-requests skill: the booking from mail (search, then the full message), the cancellation deadline quoted from
it with its time zone, the early check-in request drafted to the hotel's own address (not sent), and no browser."""
import imaplib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel booking bot has this suite's skills (crate::bots)

MAIL = [
    ("Hotel Ginza Grand <reservations@ginzagrand.example>", "Booking confirmed: Hotel Ginza Grand 20-23 Nov 2026",
     "Confirmation 88812345 for Lucas Lu. Check-in 20 Nov 2026 15:00, check-out 23 Nov 2026 11:00. Superior twin, JPY 26,000 per night. "
     "Free cancellation until 18 Nov 2026 18:00 JST; afterwards one night is charged. Contact: reservations@ginzagrand.example, +81 3 0000 5678.", 2),
]
K.setup(mail=MAIL)
t = K.run("HOTELREQ ask the Ginza hotel for an early check-in on 20 Nov, and tell me the cancellation deadline")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the hotel-requests skill was loaded", K.loaded(t, "hotel-requests"), names)
K.check("the booking was found and read in full", "gmail_search" in names and "gmail_get_message" in names
        and "88812345" in str(K.results(t, "gmail_get_message")), K.results(t, "gmail_get_message"))
d = [x for x in K.calls(t) if x["name"] == "gmail_create_draft"]
K.check("the request was drafted to the hotel's own address with the confirmation number, not sent",
        d and d[0]["args"].get("to") == "reservations@ginzagrand.example" and "88812345" in str(d[0]["args"]) and "gmail_send" not in names, d)
m = imaplib.IMAP4("127.0.0.1", 1143)
m.login(K.USER, K.PW)
typ, data = m.select("Drafts")
m.logout()
K.check("the draft is in the mailbox", typ == "OK" and int(data[0]) >= 1, data)
K.check("no browser", not any(n.startswith("browser_") for n in names), names)
res = t.get("result") or ""
K.check("the deadline is quoted from the mail with its time zone", "18 Nov 2026 18:00 JST" in res and "88812345" in res, res[:400])
K.finish()
