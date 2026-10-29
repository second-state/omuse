"""The per-country arrival-card skills (Thailand TDAC, Malaysia MDAC, Korea e-Arrival, Visit Japan Web, China's online
card, Taiwan, Indonesia, the Philippines eTravel, Cambodia, the UK ETA) on the generic test form: each gathers from mail,
profile and vault, fills the form, puts the passport number in only through the vault (approved), and stops before
Submit; the EU skill reads the official status page and files nothing while ETIAS is not live. The Singapore card has
its own suite (sgac_e2e.py); the overview skill is checked for routing."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "arrival"   # the Arrival & entry bot has this suite's skills (crate::bots)

PASSPORT = "E1234567X"
MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", "Your e-ticket: SQ 231 flights for the trip",
     "Booking ref 9QW3PL. SQ 231 departs 2026-11-02 10:30, arrives 16:20.", 1),
    ("Test Hotel <stay@testhotel.example>", "Booking confirmed: Test Hotel", "Confirmation TH-1. Check-in 2026-11-02, 3 nights.", 1),
]
K.setup(mail=MAIL)
K.set_profile(name_en="LUCAS LU", birthday="1985-04-12", nationality="Australia", email_personal="lucas@example.com", phone="+61 400 000 000")
K.vault_item("Passport Lucas", "id_document", {"number": PASSPORT, "name": "LU LUCAS", "expiry": "2031-05-01", "country": "AU"}, domains="shop.test")

FORM_SKILLS = ["th-arrival-card", "my-arrival-card", "kr-arrival-card", "jp-visit-japan-web", "cn-arrival-card", "tw-arrival-card",
               "id-arrival-card", "ph-etravel", "kh-arrival-card", "uk-eta"]
for skill in FORM_SKILLS:
    t = K.run(f"ARRCARD {skill} fill in my arrival card for this trip. Health and customs: nothing to declare.")
    names = K.names(t)
    waiting = t["status"] == "WAITING_EXTERNAL" and "Submit" in str((t.get("waiting") or {}).get("reason"))
    if not waiting:
        K.trace(t)
    K.check(f"{skill}: the form is filled and the Submit handed to the user (takeover card)", waiting, (t["status"], t.get("waiting"), t.get("error")))
    K.check(f"{skill}: skill loaded, mail / profile / vault consulted, form filled",
            K.loaded(t, skill) and names.count("gmail_search") == 2 and "profile_get" in names and "vault_list" in names
            and "browser_navigate" in names and names.count("browser_type") >= 4, names)
    K.check(f"{skill}: the passport number went in through the vault with the user's approval",
            any(a["tool"] == "browser_fill_secret" for a in t["_approvals"]), t["_approvals"])
    K.check(f"{skill}: the agent never clicked Submit itself", "browser_click" not in names, names)
    if skill == "uk-eta":
        K.check("uk-eta: the photo and the fee are left to the user", "pay the fee" in str((t.get("waiting") or {}).get("reason")), t.get("waiting"))
    K.hand_back()
    t = K.wait_end(t["id"])
    res = t.get("result") or ""
    blob = res + "\n".join(str(e.get("data")) for e in t.get("events", []))
    K.check(f"{skill}: after the hand-back it reports 'not submitted' and the passport number appears nowhere",
            t["status"] == "COMPLETED" and "NOT SUBMITTED" in res and PASSPORT not in blob, (t["status"], res[:200]))

t = K.run("EUETIAS do I need to fill anything before my trip to Paris next month?")
names = K.names(t)
K.check("eu-etias: task completes with no form and no browsing beyond the status page", t["status"] == "COMPLETED" and K.loaded(t, "eu-etias")
        and names.count("browser_read") == 1 and "browser_navigate" not in names, (t["status"], names))
res = t.get("result") or ""
K.check("eu-etias: the answer says ETIAS is not in operation, warns about paid sites, explains EES",
        "not in operation" in res and "scam" in res and "EES" in res and "official page read" in res, res[:400])

skills = {s["name"] for s in K.c.get(K.B + "/api/skills").json().get("skills", [])}
K.check("the overview skill and all per-country skills are installed and enabled",
        {"arrival-cards", "sg-arrival-card", "eu-etias", *FORM_SKILLS} <= skills, sorted(skills))
K.finish()
