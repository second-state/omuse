"""Arrival cards without a mailbox: no mail connected, so the flight and stay must come from the user. The agent checks
calendar / memory, fills nothing blank, asks ONCE for exactly the flight and the accommodation (and suggests connecting
mail), then: (1) the user answers in chat → the form is filled with those answers, the trip is remembered and the Submit
is handed over; (2) the user uploads an e-ticket instead → it is read with the vision model and used; (3) the user says
"I'll fill it in myself" → the form is opened with the profile / vault values and handed over for the rest."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

PASSPORT = "E1234567X"
K.setup()
for acc in next(x for x in K.c.get(K.S + "/connections", headers=K.H).json()["connections"] if x["name"] == "gmail").get("accounts", []):
    K.c.delete(f"{K.S}/connections/gmail/accounts/{acc['id']}", headers=K.H)
accs = next(x for x in K.c.get(K.S + "/connections", headers=K.H).json()["connections"] if x["name"] == "gmail").get("accounts", [])
K.check("no mailbox is connected", not accs, accs)
K.set_profile(name_en="LUCAS LU", birthday="1985-04-12", nationality="Australia", email_personal="lucas@example.com", phone="+61 400 000 000")
K.vault_item("Passport Lucas", "id_document", {"number": PASSPORT, "name": "LU LUCAS", "expiry": "2031-05-01", "country": "AU"}, domains="shop.test")


def filled_and_handed_over(t, label, flight, hotel, myself=False):
    names = K.names(t)
    reason = str((t.get("waiting") or {}).get("reason"))
    K.check(f"{label}: the form is filled and the Submit handed to the user (takeover card)",
            t["status"] == "WAITING_EXTERNAL" and "Submit" in reason, (t["status"], t.get("waiting"), t.get("error")))
    typed = [str(x["args"].get("text")) for x in K.calls(t) if x["name"] == "browser_type"]
    if myself:
        K.check(f"{label}: profile values typed, flight and stay left to the user (said on the card)",
                "LUCAS LU" in typed and not any(v.startswith("SQ") for v in typed) and "flight and accommodation" in reason, (typed, reason))
    else:
        K.check(f"{label}: the flight and the stay the user gave were typed in", flight in typed and hotel in typed, typed)
        mem = [x["args"] for x in K.calls(t) if x["name"] == "memory_remember"]
        K.check(f"{label}: the trip is remembered for the other travel skills", mem and flight in str(mem) and hotel in str(mem), mem)
    K.check(f"{label}: the passport went in through the vault with the user's approval",
            any(a["tool"] == "browser_fill_secret" for a in t["_approvals"]), t["_approvals"])
    K.check(f"{label}: the agent never clicked Submit itself", "browser_click" not in names, names)
    K.hand_back()
    t = K.wait_end(t["id"])
    res = t.get("result") or ""
    blob = res + "\n".join(str(e.get("data")) for e in t.get("events", []))
    K.check(f"{label}: reported as not submitted, no passport number anywhere",
            t["status"] == "COMPLETED" and "NOT SUBMITTED" in res and PASSPORT not in blob, (t["status"], res[:200]))


# ---- 1. ask once, then the answer in chat
t = K.run("ARRNOMAIL ask fill in my Thailand arrival card. Health and customs: nothing to declare.")
K.trace(t)
names = K.names(t)
res = t.get("result") or ""
K.check("ask: the task ends with a question, no browser opened yet", t["status"] == "COMPLETED"
        and not any(n.startswith("browser_") for n in names), (t["status"], names))
K.check("ask: skill loaded; memory, profile and vault checked before asking; no mail tool offered",
        K.loaded(t, "th-arrival-card") and {"memory_search", "profile_get", "vault_list"} <= set(names)
        and not any(n.startswith("gmail_") for n in names), names)
K.check("ask: one message naming the flight, the accommodation, the e-ticket upload and connecting mail",
        "Which flight" in res and "Where are you staying" in res and "upload the e-ticket" in res and "Connecting a mailbox" in res, res[:400])
t = K.run("ARRNOMAIL answer: SQ 708 on 2026-11-02, staying at Riverside Test Hotel.", conv=t["_conv"])
K.trace(t)
filled_and_handed_over(t, "answer", "SQ 708", "Riverside Test Hotel")

# ---- 2. an uploaded e-ticket instead of an email
up = K.upload("eticket.png", K.PNG)
t = K.run("ARRNOMAIL eticket fill in my Thailand arrival card; my e-ticket is attached. Nothing to declare.", attachments=[up.get("attachment") or up])
K.trace(t)
fl = K.calls(t)
look = [x for x in fl if x["name"] == "file_look"]
K.check("eticket: the upload was read with the vision model", look and "uploads/" in str(look[0]["args"].get("path")), look)
filled_and_handed_over(t, "eticket", "SQ 708", "Riverside Test Hotel")

# ---- 3. "I'll fill it in myself"
t = K.run("ARRNOMAIL myself open my Thailand arrival card, I'll fill in the flight and hotel myself.")
K.trace(t)
filled_and_handed_over(t, "myself", "", "", myself=True)
K.finish()
