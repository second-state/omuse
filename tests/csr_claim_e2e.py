"""csr-trip-claim skill: the booking, the card charge and the airline's refund from mail in ONE turn (with the profile),
the claimable amount computed (paid minus refunded), the refund / non-refundable letter drafted to the airline (approval),
a real claim packet PDF shown in the chat, the administrator's portal form filled and handed over for the sign-in and
the Submit (never clicked), then a reminder schedule and a memory note — and an answer with the benefit, the two
deadlines as dates, the amount, the phone number and no medical detail."""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

LOSS = (dt.date.today() - dt.timedelta(days=3)).isoformat()
BOOKED = (dt.date.today() - dt.timedelta(days=40)).isoformat()
D20 = (dt.date.fromisoformat(LOSS) + dt.timedelta(days=20)).isoformat()
D90 = (dt.date.fromisoformat(LOSS) + dt.timedelta(days=90)).isoformat()
PDF = f"reports/CSR-claim-UA837-{LOSS}.pdf"
MAIL = [
    ("United Airlines <noreply@united.example>", f"Your eTicket receipt — Confirmation ABC123 — UA 837 SFO-NRT {LOSS}",
     f"Passenger LUCAS LU. UA 837 San Francisco (SFO) {LOSS} 11:05 → Tokyo Narita (NRT) 14:30 (+1). Fare USD 1,840.00, Basic Economy, "
     f"non-refundable. Paid with Visa ending 4321 on {BOOKED}.", 40),
    ("Chase <no.reply.alerts@chase.example>", "Your $1,840.00 transaction with UNITED AIRLINES",
     f"A $1,840.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on {BOOKED}.", 40),
    ("United Refunds <refunds@united.example>", "Refund processed — confirmation ABC123",
     "Taxes and fees of USD 86.40 have been refunded to Visa ending 4321. The Basic Economy fare of USD 1,753.60 is non-refundable "
     "under the fare rules.", 2),
]
K.setup(mail=MAIL)
K.set_profile(name_en="LUCAS LU", email_personal="lucas@example.com", phone="+1 415 555 0100", address_home="12 Mission St, San Francisco, CA 94105")
before = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}

t = K.run(f"CSRCLAIM I got food poisoning and missed my United flight to Tokyo on {LOSS}; it was booked on my Chase Sapphire Reserve. "
          f"Get my money back from the card's insurance. LOSS={LOSS}")
K.trace(t)
names = K.names(t)
K.check("the task fills the portal form and hands the browser over for the sign-in and the Submit (takeover card)", t["status"] == "WAITING_EXTERNAL"
        and "Submit" in str((t.get("waiting") or {}).get("reason")), (t["status"], t.get("waiting"), t.get("error")))
K.check("the csr-trip-claim skill was loaded", K.loaded(t, "csr-trip-claim"), names)
# (PLANNING itself is too brief to catch with the fake planner: the plan event and the stored plan are the evidence)
K.check("a long skill (plan: on) brings a plan with its sheet: the planner ran once after load_skill", (t.get("plan") or {}).get("steps")
        and sum(e["type"] == "plan" for e in t["events"]) >= 1, (t.get("plan"), [e["type"] for e in t["events"]]))
K.check("booking, charge and refund were searched in mail in one turn, with the profile", any(b.count("gmail_search") == 3 for b in K.batches(t))
        and "profile_get" in names, (K.batches(t), names))
K.check("the claimable amount was computed with calculate (paid minus refunded)", "calculate" in names
        and any("1,753.6" in str(r.get("preview")) or "1753.6" in str(r.get("preview")) for r in K.results(t, "calculate")), K.results(t, "calculate"))
dr = [c for c in K.calls(t) if c.get("name") == "gmail_create_draft"]
K.check("the refund / non-refundable letter to the airline was drafted (a draft, so no approval): booking ref in, no diagnosis, no card number",
        len(dr) == 1 and "ABC123" in str(dr[0]["args"]) and "food poisoning" not in str(dr[0]["args"]).lower() and "4321" not in str(dr[0]["args"])
        and not any(a["tool"] == "gmail_send" for a in t["_approvals"]), dr)
raw = K.c.get(K.B + "/api/files/raw", params={"path": PDF}, headers=K.H)
K.check("the claim packet is a real PDF in reports/", raw.status_code == 200 and raw.content[:5] == b"%PDF-", (raw.status_code, raw.content[:8]))
K.check("the packet was shown in the chat", any(PDF in str(c) for c in K.conv_cards(t["_conv"], "file")), K.conv_cards(t["_conv"], "file"))
K.check("the portal page was opened and the fields typed; Submit never clicked", "browser_navigate" in names and names.count("browser_type") == 4
        and "browser_click" not in names, names)
typed = " ".join(str(c.get("args", {}).get("text")) for c in K.calls(t) if c.get("name") == "browser_type")
K.check("the typed fields: name, date of loss, amount, reason — no card number", "LUCAS LU" in typed and LOSS in typed and "1753.60" in typed
        and "4321" not in typed, typed)
K.hand_back()
t = K.wait_end(t["id"])
names = K.names(t)
res = t.get("result") or ""
K.check("after the hand-back the task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
after = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.check("a reminder schedule for the documents deadline was created", any("claim" in n.lower() and D90 in n for n in after - before), after - before)
mem = [c for c in K.calls(t) if c.get("name") == "memory_remember"]
K.check("the claim was noted in memory with the deadlines, no card number", len(mem) == 1 and D20 in str(mem[0]["args"]) and D90 in str(mem[0]["args"])
        and "4321" not in str(mem[0]["args"]), mem)
K.check("the answer names the benefit and both deadlines as dates", "Trip Cancellation" in res and D20 in res and D90 in res, res[:400])
K.check("the answer gives the amount, how it was computed, the administrator and the phone number",
        "1,753.60" in res and "1,840.00" in res and "86.40" in res and "chasecardbenefits.com" in res and "1-800-350-1697" in res, res[:800])
K.check("no diagnosis in the answer", "food poisoning" not in res.lower() and "medically unable" in res, res[:400])
K.check("the answer is honest about the decision and the exclusions", "administrator decides" in res and "60 days" in res, res[-300:])
K.finish()
