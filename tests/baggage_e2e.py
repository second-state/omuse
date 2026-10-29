"""baggage-delayed-lost skill: the policy from mail, the airline's tracing page READ, the expenses claim drafted (not
sent), a twice-daily check scheduled, and the 21-day rule in the answer."""
import imaplib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "insurance"   # the Insurance & claims bot has this suite's skills (crate::bots)

MAIL = [
    ("TravelSure <policy@travelsure.example>", "Your travel insurance policy TI-2026-77",
     "Policy TI-2026-77. Baggage delay: SGD 150 after 6 hours, PIR and receipts required. Baggage loss: up to SGD 3,000, SGD 500 per item.", 5),
]
K.setup(mail=MAIL)
before = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
t = K.run("BAGLOST my checked bag did not arrive in Tokyo on SQ 638, the airline gave me file reference SINSQ12345")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the baggage-delayed-lost skill was loaded", K.loaded(t, "baggage-delayed-lost"), names)
K.check("the policy was looked up in mail", "gmail_search" in names and "TI-2026-77" in str(K.results(t, "gmail_search")), K.results(t, "gmail_search"))
K.check("the tracing page was read, not navigated", "browser_read" in names and "browser_navigate" not in names, names)
K.check("the claim to the airline was drafted, not sent", "gmail_create_draft" in names and "gmail_send" not in names
        and K.results(t, "gmail_create_draft") and K.results(t, "gmail_create_draft")[0].get("ok"), K.results(t, "gmail_create_draft"))
m = imaplib.IMAP4("127.0.0.1", 1143)
m.login(K.USER, K.PW)
typ, data = m.select("Drafts")
drafts = int(data[0]) if typ == "OK" else 0
m.logout()
K.check("the draft sits in the mailbox's Drafts folder", drafts >= 1, drafts)
after = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.check("a twice-daily check of the file was scheduled", "Check baggage file SINSQ12345" in after - before, after)
res = t.get("result") or ""
K.check("the answer carries the file reference, the live status and the 21-day rule",
        "SINSQ12345" in res and "Tracing continues" in res and "21 days" in res, res[:600])
K.check("the policy's requirement (PIR + receipts) is named", "PIR" in res and "receipts" in res, res[:600])
K.finish()
