"""fwd-travel-insurance skill, three scenarios on stand-in pages (tests/pages/fwd_sg_*.html) and mail:
1. Singapore purchase: trip facts from mail with the profile in one turn, the quote page typed, the plans compared in a
   card, purchase_confirm with the Vault card, the application filled (passport and card only through the Vault) and
   handed over for Pay — never pressed.
2. Singapore claim: the policy and the airline's delay certificate (a PDF attachment) from mail in one turn, the hours
   computed, FWD's claim form filled, the certificate uploaded, handed over for Submit — never pressed; the 30-day date,
   a reminder schedule and a memory note.
3. Hong Kong claim: the plan is bolttech's, so bolttech's PDF claim form is opened (the browser keeps a copy under
   downloads/), filled without the HKID, bank account and signature, shown in the chat and attached with the delay
   letter to an email draft to claims.hk@bolttechinsurance.com — a draft, never sent."""
import datetime as dt
import imaplib
import os
import sys

from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "insurance"   # the Travel insurance & claims bot has this suite's skills (crate::bots)

WS = "/tmp/claude-0/persona-test/workspace"
PAGES_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
D1 = (dt.date.today() + dt.timedelta(days=24)).isoformat()
D2 = (dt.date.today() + dt.timedelta(days=31)).isoformat()
INC = (dt.date.today() - dt.timedelta(days=2)).isoformat()
D30 = (dt.date.fromisoformat(INC) + dt.timedelta(days=30)).isoformat()
CARD = K.vault_item("Visa Wei Ming", "card", {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123", "holder": "TAN WEI MING"},
                    domains="shop.test")
PASSPORT = K.vault_item("Passport Wei Ming", "id_document", {"number": "K1234567Z", "name": "TAN WEI MING", "expiry": "2031-05-01", "country": "SG"},
                        domains="shop.test")


def pdf(name):
    return open(os.path.join(PAGES_DIR, name), "rb").read()


def typed(t):
    return " ".join(str(c.get("args", {}).get("text")) for c in K.calls(t) if c.get("name") == "browser_type")


# ------------------------------------------------------------------ 1. Singapore: buy
K.setup(mail=[("Singapore Airlines <noreply@singaporeair.example>", f"Booking confirmed — SQ 636 Singapore to Tokyo, {D1}",
               f"Passenger TAN WEI MING. SQ 636 SIN 10:30 → HND 18:25 on {D1}; SQ 637 HND → SIN on {D2}. Booking reference 7XKQ2P.", 5)])
K.set_profile(name_en="TAN WEI MING", birthday="1985-04-12", email_personal="weiming@example.com", phone="+65 9123 4567",
              address_home="10 Anson Road, Singapore 079903")
t = K.run(f"FWDBUY Buy FWD travel insurance for my Tokyo trip {D1} to {D2}, just me — the cheapest plan with at least SGD 10,000 trip cancellation.")
K.trace(t)
names = K.names(t)
K.check("SG buy: the fwd-travel-insurance skill was loaded, no plan", K.loaded(t, "fwd-travel-insurance") and "plan_task" not in names, names)
K.check("SG buy: the trip from mail and the profile before the quote", names[1:3] == ["gmail_search", "profile_get"], names)
K.check("SG buy: the quote page typed (destination and both dates) and the quote asked for", D1 in typed(t) and D2 in typed(t)
        and "Japan" in typed(t) and "browser_click" in names, typed(t))
cards = [c for c in K.calls(t) if c.get("name") == "present_choices"]
opts = (cards[0]["args"].get("options") if cards else None) or []
K.check("SG buy: the three plans compared with their premiums from the page", [o["label"] for o in opts] == ["Premium", "Business", "First"]
        and "SGD 47.00" in str(opts[1]["details"]), opts)
pc = [a for a in t["_approvals"] if a["tool"] == "purchase_confirm"]
K.check("SG buy: purchase_confirm for the Business plan, SGD 47.00, with the Vault card", pc and pc[0]["args"].get("total") == 47.0
        and pc[0]["args"].get("currency") == "SGD" and pc[0]["args"].get("card_item_id") == CARD, pc)
fills = [c["args"] for c in K.calls(t) if c.get("name") == "browser_fill_secret"]
K.check("SG buy: passport and card only through the Vault (5 fills, one per turn), never typed", len(fills) == 5
        and {f["item_id"] for f in fills} == {CARD, PASSPORT} and "K1234567Z" not in typed(t) and "4111" not in typed(t)
        and all(b.count("browser_fill_secret") <= 1 for b in K.batches(t)), (fills, typed(t)))
K.check("SG buy: the application handed over for Pay, the button never pressed", t["status"] == "WAITING_EXTERNAL"
        and "Pay" in str((t.get("waiting") or {}).get("reason")) and names.count("browser_click") == 1, (t["status"], t.get("waiting"), names))
K.hand_back()
t = K.wait_end(t["id"])
res = t.get("result") or ""
K.check("SG buy: the answer names the insurer, the plan, the premium, that the user pays, and the emergency line",
        t["status"] == "COMPLETED" and "FWD Singapore" in res and "Business" in res and "SGD 47.00" in res and "Pay now" in res
        and "+65 6322 2072" in res, (t["status"], res[:600]))
K.check("SG buy: no secret in the answer", "K1234567Z" not in res and "4111" not in res, res)

# ------------------------------------------------------------------ 2. Singapore: claim a delay
before = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.setup(mail=[
    ("FWD Singapore <noreply@fwd.com.sg>", "Your FWD Travel Insurance policy TI-SG-2026-55123",
     f"Policyholder TAN WEI MING. Plan: Business (single trip), Japan. Period of insurance: {INC} to {D1}. "
     "Travel delay: SGD 100 for every 6 consecutive hours of delay, up to SGD 1,500. Emergency assistance (24 hours): +65 6322 2072.", 20),
    ("Singapore Airlines <noreply@singaporeair.example>", f"Delay certificate — SQ 636 on {INC}",
     f"Dear TAN WEI MING, SQ 636 Singapore–Tokyo Haneda on {INC} was scheduled to depart at 10:30 and departed at 17:45 "
     "because of the late arrival of the inbound aircraft. The delay certificate is attached.", 1,
     [("delay_certificate_SQ636.pdf", pdf("delay_certificate_SQ636.pdf"))]),
])
t = K.run(f"FWDCLAIM My flight SQ 636 to Tokyo on {INC} was delayed by more than 7 hours. Claim my FWD travel insurance. INC={INC}")
K.trace(t)
names = K.names(t)
K.check("SG claim: the skill was loaded", K.loaded(t, "fwd-travel-insurance"), names)
K.check("SG claim: policy, incident and receipts from mail with the profile, in one turn",
        any(b.count("gmail_search") == 3 for b in K.batches(t)) and "profile_get" in names, (K.batches(t), names))
sv = K.results(t, "gmail_save_attachment")
K.check("SG claim: the airline's delay certificate saved from the mail attachment", sv and sv[0].get("ok")
        and "delay_certificate_SQ636" in str(sv[0].get("preview")), sv)
K.check("SG claim: the delay computed with calculate (7.25 hours)", any("7.25" in str(r.get("preview")) for r in K.results(t, "calculate")),
        K.results(t, "calculate"))
K.check("SG claim: FWD's claim form filled — policy number, date, amount — with the passport from the Vault",
        "TI-SG-2026-55123" in typed(t) and INC in typed(t) and "100.00" in typed(t) and "K1234567Z" not in typed(t)
        and any(c["args"].get("item_id") == PASSPORT for c in K.calls(t) if c.get("name") == "browser_fill_secret"), typed(t))
up = [a for a in t["_approvals"] if a["tool"] == "browser_upload"]
K.check("SG claim: the certificate uploaded to the form (approved)", up and "delay_certificate_SQ636" in str(up[0]["args"]), up)
K.check("SG claim: handed over for Submit, never pressed", t["status"] == "WAITING_EXTERNAL" and "Submit" in str((t.get("waiting") or {}).get("reason"))
        and "browser_click" not in names, (t["status"], t.get("waiting"), names))
K.hand_back()
t = K.wait_end(t["id"])
res = t.get("result") or ""
after = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.check("SG claim: a weekly check for FWD's reply was scheduled", any("FWD claim" in n for n in after - before), after - before)
mem = [c["args"] for c in K.calls(t) if c.get("name") == "memory_remember"]
K.check("SG claim: the claim noted in memory with the deadline, no ID number", mem and D30 in str(mem[0]) and "K1234567Z" not in str(mem[0]), mem)
K.check("SG claim: the answer gives the channel, the 30-day date, the hours, the amount, who presses Submit and the phone numbers",
        t["status"] == "COMPLETED" and D30 in res and "7 hours 15 minutes" in res and "SGD 100" in res and "Submit" in res
        and "+65 6322 2072" in res and "FWD decides" in res, (t["status"], res[:900]))

# ------------------------------------------------------------------ 3. Hong Kong: claim on bolttech's form by email
K.setup(mail=[
    ("FWD Hong Kong <noreply@fwd.com.hk>", "Your MyTravel Insurance policy MT-HK-2026-00881",
     f"Policyholder CHAN TAI MAN. MyTravel Insurance, Premium Plan, underwritten by Bolttech Insurance (Hong Kong) Company Limited. "
     f"Period of insurance: {INC} to {D1}. Travel delay: HKD 300 for every 6 full hours of delay. 24-hour emergency assistance: +852 2000 0100.", 15),
    ("Cathay Pacific <noreply@cathaypacific.example>", f"Delay confirmation — CX 543 on {INC}",
     f"Dear CHAN TAI MAN, CX 543 Hong Kong–Tokyo Narita on {INC} was scheduled at 09:05 and departed at 15:20 after a technical "
     "inspection. Our delay confirmation is attached.", 1, [("delay_letter_CX543.pdf", pdf("delay_letter_CX543.pdf"))]),
])
K.set_profile(name_en="CHAN TAI MAN", email_personal="taiman@example.com", phone="+852 9123 4567",
              address_home="Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong")
t = K.run(f"FWDHKCLAIM My Cathay flight CX 543 to Tokyo on {INC} was delayed 6 hours. Claim on my FWD MyTravel policy. INC={INC}")
K.trace(t)
names = K.names(t)
K.check("HK claim: the skill was loaded, the mail gathered in one turn", K.loaded(t, "fwd-travel-insurance")
        and any(b.count("gmail_search") == 3 for b in K.batches(t)) and "profile_get" in names, (names, K.batches(t)))
nav = K.results(t, "browser_navigate")
K.check("HK claim: opening bolttech's claim form keeps a copy under downloads/", nav and "downloads/travel-insurance-clm-form-fillable" in str(nav[0].get("preview")),
        nav[:1])
fl = K.results(t, "pdf_form_fill")
K.check("HK claim: the form filled into a new copy", fl and fl[0].get("ok") and "-filled.pdf" in str(fl[0].get("preview")), fl)
filled = os.path.join(WS, "downloads", "travel-insurance-clm-form-fillable-filled.pdf")
vals = {k: str(v.get("/V", "") or "") for k, v in (PdfReader(filled).get_fields() or {}).items()} if os.path.exists(filled) else {}
K.check("HK claim: the filled PDF has the policy number, name, flight, hours and amount", vals.get("fill_17") == "MT-HK-2026-00881"
        and vals.get("Full name") == "CHAN TAI MAN" and vals.get("fill_14_2") == "CX 543" and vals.get("fill_1") == "6 hours 15 minutes"
        and vals.get("fill_7") == "HKD 300", vals)
K.check("HK claim: HKID, bank account and signature left for the user", not vals.get("HKID Card no") and not vals.get("fill_4_5")
        and not vals.get("Claimants signature"), vals)
K.check("HK claim: the filled form shown in the chat", any("-filled.pdf" in str(c) for c in K.conv_cards(t["_conv"], "file")),
        K.conv_cards(t["_conv"], "file"))
dr = [c["args"] for c in K.calls(t) if c.get("name") == "gmail_create_draft"]
K.check("HK claim: a draft to bolttech's claims mailbox with the form and the delay letter attached",
        dr and dr[0].get("to") == "claims.hk@bolttechinsurance.com" and len(dr[0].get("attachments") or []) == 2
        and "MT-HK-2026-00881" in dr[0].get("subject", ""), dr)
K.check("HK claim: nothing sent", not any(n in ("gmail_send", "gmail_reply") for n in names)
        and not any(a["tool"] in ("gmail_send", "gmail_reply") for a in t["_approvals"]), names)
m = imaplib.IMAP4("127.0.0.1", 1143)
m.login(K.USER, K.PW)
box = next((b for b in ("Drafts", "INBOX.Drafts") if m.select(b)[0] == "OK"), None)
raw = b""
if box:
    _, ids = m.search(None, "ALL")
    if ids[0]:
        raw = m.fetch(ids[0].split()[-1], "(RFC822)")[1][0][1]
m.logout()
K.check("HK claim: the draft in the mailbox carries both PDFs", b"claims.hk@bolttechinsurance.com" in raw
        and b"travel-insurance-clm-form-fillable-filled.pdf" in raw and b"delay_letter_CX543.pdf" in raw, raw[:400])
res = t.get("result") or ""
K.check("HK claim: the answer says bolttech handles it, the amount, what the user adds, and the phone", t["status"] == "COMPLETED"
        and "bolttech" in res and "HKD 300" in res and "HKID" in res and "signature" in res and "+852 3123 3344" in res, (t["status"], res[:900]))
K.finish()
