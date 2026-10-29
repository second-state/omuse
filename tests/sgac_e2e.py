"""sg-arrival-card skill on a test copy of the ICA form (tests/pages/sgac.html): flight and hotel from mail, the name and
birthday from the profile, the passport number only through the vault (approved per use, never in chat), the form
filled but NOT submitted until the user says so; the second message submits and quotes the DE number."""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "arrival"   # the Arrival & entry bot has this suite's skills (crate::bots)

ARRIVE = (dt.date.today() + dt.timedelta(days=2)).isoformat()
PASSPORT = "E1234567X"
MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", f"Your e-ticket: SQ 231 SYD-SIN {ARRIVE}",
     f"Booking ref 9QW3PL. SQ 231 departs Sydney (SYD) {ARRIVE} 10:30, arrives Singapore (SIN) {ARRIVE} 16:20.", 1),
    ("Marina Bayfront Hotel <stay@marinabayfront.example>", f"Booking confirmed: Marina Bayfront Hotel {ARRIVE}",
     f"Confirmation MB-55120. Check-in {ARRIVE}, 3 nights. 10 Bayfront Avenue, Singapore 018956.", 1),
]
K.setup(mail=MAIL)
K.set_profile(name_en="LUCAS LU", birthday="1985-04-12", nationality="Australia", email_personal="lucas@example.com", phone="+61 400 000 000")
K.vault_item("Passport Lucas", "id_document", {"number": PASSPORT, "name": "LU LUCAS", "expiry": "2031-05-01", "country": "AU"},
             domains="ica.gov.sg,shop.test")

t = K.run("SGAC fill in my Singapore arrival card for this trip. Health: no symptoms, and no Africa or South America in the last 6 days.")
K.trace(t)
names = K.names(t)
K.check("the task fills the form and hands the browser over for the Submit (takeover card)", t["status"] == "WAITING_EXTERNAL"
        and "Submit" in str((t.get("waiting") or {}).get("reason")), (t["status"], t.get("waiting"), t.get("error")))
K.check("the sg-arrival-card skill was loaded", K.loaded(t, "sg-arrival-card"), names)
K.check("flight and hotel were looked up in mail, profile and vault consulted",
        names.count("gmail_search") == 2 and "profile_get" in names and "vault_list" in names, names)
K.check("the form page was opened and the fields typed", "browser_navigate" in names and names.count("browser_type") >= 5, names)
K.check("the passport number went in through the vault, approved by the user",
        any(a["tool"] == "browser_fill_secret" for a in t["_approvals"]) and "WAITING_APPROVAL" in t["_seen"], t["_approvals"])
K.check("the agent never clicked Submit itself", "browser_click" not in names, names)
cards = K.conv_cards(t["_conv"], "takeover")
K.check("the chat shows the takeover card with the review request", cards and "Submit" in str(cards[-1].get("reason")), cards)
K.hand_back()   # the user looks and hands back without pressing Submit
t = K.wait_end(t["id"])
res = t.get("result") or ""
K.check("after the hand-back the agent reports the form as not submitted, values kept", t["status"] == "COMPLETED" and "NOT SUBMITTED" in res and "LUCAS LU" in res,
        (t["status"], res[:300]))
blob = res + "\n".join(str(e.get("data")) for e in t.get("events", []))
K.check("the passport number appears nowhere in the answer or the trace", PASSPORT not in blob, [l for l in blob.splitlines() if PASSPORT in l][:3])

# the user asked for it to be submitted without a review: the agent clicks Submit once and reads the DE number
t2 = K.run("SGAC fill in my Singapore arrival card and submit it for me without review. Health: no symptoms, no Africa or South America in the last 6 days.")
K.trace(t2)
n2 = K.names(t2)
K.check("submit-for-me: task completes", t2["status"] == "COMPLETED", (t2["status"], t2.get("error")))
clk = K.results(t2, "browser_click")
K.check("submit-for-me: exactly one Submit click, which went through (the user named the action, so no dialog)",
        n2.count("browser_click") == 1 and clk and (clk[0].get("ok") is True or clk[0].get("status") in ("ok", "approved"))
        and "sgac_done" in str(clk[0].get("preview")), (n2, clk))
K.check("submit-for-me: the passport value is masked in the submitted URL", "passport=[VAULT_VALUE]" in str(clk[0].get("preview")) if clk else False, clk)
res2 = t2.get("result") or ""
K.check("submit-for-me: the DE number from the confirmation page is quoted", "DE202611020123" in res2, res2[:400])
K.check("submit-for-me: the filing is remembered for later", "memory_remember" in n2, n2)
K.check("submit-for-me: still no passport number anywhere", PASSPORT not in res2 + "\n".join(str(e.get("data")) for e in t2.get("events", [])), res2[:300])
# ---- the user's own address in Singapore (profile): one → used without asking; two → asked to choose in the chat
K.seed_mail(MAIL[:1])   # the flight only: no hotel booking this time
K.set_profile(**{"address_home": "12 Harbour St, Sydney NSW 2000, Australia", "address:Singapore flat": "10 Bayfront Avenue, Singapore 018956"})
t3 = K.run("SGAC fill in my Singapore arrival card for this trip. Health: no symptoms, no Africa or South America in the last 6 days.")
K.trace(t3)
typed = [str(x["args"].get("text")) for x in K.calls(t3) if x["name"] == "browser_type"]
K.check("own address: the profile's addresses were read (address:*)",
        any(x["name"] == "profile_get" and "address:*" in str(x["args"]) for x in K.calls(t3)), K.names(t3))
K.check("own address: the one Singapore address is used as the accommodation, nothing asked",
        t3["status"] == "WAITING_EXTERNAL" and "10 Bayfront Avenue, Singapore 018956" in typed
        and not any("12 Harbour St" in v for v in typed), (t3["status"], typed))
K.hand_back()
t3 = K.wait_end(t3["id"])
K.check("own address: the answer says the profile address was used", "profile address in Singapore" in (t3.get("result") or ""), (t3.get("result") or "")[:300])
K.set_profile(**{"address:Parents in Singapore": "5 Jalan Kayu, Singapore 799487"})
t4 = K.run("SGAC fill in my Singapore arrival card for this trip. Health: no symptoms, no Africa or South America in the last 6 days.")
res4 = t4.get("result") or ""
K.check("two addresses: the agent asks in the chat which one, before any browser",
        t4["status"] == "COMPLETED" and "Which one" in res4 and "Singapore flat" in res4 and "Parents in Singapore" in res4
        and not any(n.startswith("browser_") for n in K.names(t4)), (t4["status"], res4[:300], K.names(t4)))
t5 = K.run("SGACPICK the Parents in Singapore one, please.", conv=t4["_conv"])
typed5 = [str(x["args"].get("text")) for x in K.calls(t5) if x["name"] == "browser_type"]
K.check("two addresses: the chosen one is filled in", t5["status"] == "WAITING_EXTERNAL" and "5 Jalan Kayu, Singapore 799487" in typed5, (t5["status"], typed5))
K.hand_back()
K.wait_end(t5["id"])
K.c.put(K.B + "/api/profile", json={"key": "address:Parents in Singapore", "value": ""}, headers=K.H)
K.c.put(K.B + "/api/profile", json={"key": "address:Singapore flat", "value": ""}, headers=K.H)
K.finish()
