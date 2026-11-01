"""FWD Hong Kong travel insurance claim with a real model, from the user's point of view (tests/user_common.py,
tests/claims_common.py). FWD Hong Kong's travel plans are underwritten by bolttech, whose claim form is a fillable PDF on
FWD's site (the stand-in at its real address, tests/https_standins.py: https://www.fwd.com.hk/…/travel-insurance-clm-
form-fillable.pdf) that goes by email to claims.hk@bolttechinsurance.com. The mailbox has FWD's policy email (MyTravel
MT-HK-2026-00881, HKD 300 for every 6 full hours of delay) and Cathay Pacific's delay confirmation for CX 543 two days
ago (09:05 → 15:20, the letter attached as a PDF); the profile has the user's name, email, phone and address.

The user asks Main's chat to prepare the delay claim: the form filled and put in an email draft with the delay letter,
not sent (they sign and send it themselves). Checked from the user's side:
* the Insurance & claims bot answers (its "Continue in" link);
* the filled claim form is in the chat as a file card; downloaded and opened, it has the policy number, the user's name,
  CX 543, the hours delayed and HKD 300 — and the HKID number, the bank account and the signature are left empty;
* the user's Drafts folder has a new email to claims.hk@bolttechinsurance.com about policy MT-HK-2026-00881 with the
  filled form and Cathay's delay letter attached; nothing new in Sent, and the answer does not say it was sent;
* the answer says bolttech decides the claim, gives the amount (HKD 300) and what the user still adds (HKID, signature).
One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh insurance_hk_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

INC = dt.date.today() - dt.timedelta(days=2)
POLICY = "MT-HK-2026-00881"
BOLTTECH = "claims.hk@bolttechinsurance.com"
PAGES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
MAIL = [
    ("FWD Hong Kong <noreply@fwd.com.hk>", f"Your MyTravel Insurance policy {POLICY}",
     f"Dear CHAN TAI MAN,\n\nYour MyTravel Insurance policy {POLICY} (Premium Plan, single trip to Japan) is underwritten by bolttech "
     f"Insurance (Hong Kong) Company Limited. Period of insurance: {(INC - dt.timedelta(days=1)).isoformat()} to "
     f"{(INC + dt.timedelta(days=5)).isoformat()}.\nTravel delay: HKD 300 for every 6 full hours of delay, up to HKD 1,500. "
     "Claims must be submitted within 90 days of the incident.\n24-hour emergency assistance: +852 2000 0100.\n\nFWD Financial Limited", 15),
    ("Cathay Pacific <noreply@cathaypacific.example>", f"Delay confirmation — CX 543 on {INC}",
     f"Dear CHAN TAI MAN,\n\nCX 543 Hong Kong–Tokyo Narita on {INC} was scheduled to depart at 09:05 and departed at 15:20 after a "
     "technical inspection of the aircraft. Our delay confirmation is attached (reference CX-DLY-55821).\n\nCathay Pacific", 1,
     [("delay_letter_CX543.pdf", open(os.path.join(PAGES, "delay_letter_CX543.pdf"), "rb").read())]),
]


def no_send(dialog: str) -> bool:
    """The user asked for a draft: they deny a dialog about sending mail, by the tool it names (auto-approve covers only
    the browser)."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


async def main():
    drafts0, sent0 = len(C.mail_full("Drafts")), len(C.mail_full("Sent"))
    async with U.User() as u:
        a = await u.ask(f"My Cathay Pacific flight CX 543 to Tokyo on {INC} was delayed more than 6 hours. Please prepare the delay claim on my "
                        "FWD MyTravel policy: fill in the claim form and put it in an email draft with the airline's delay letter. "
                        "Don't send it — I'll sign the form and send it myself.", label="hk claim", approve=no_send, timeout=1200)
        answers = [a]
        if not a.files and not a.timed_out:
            a = await u.ask("Everything is in my email and my profile. Please go ahead: fill in the form and leave the email as a draft.",
                            label="hk follow-up", new_chat=False, approve=no_send, timeout=1200)
            answers.append(a)
        text = "\n".join(x.text for x in answers)
        check("hk: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("hk: the Insurance & claims bot did it", any("nsurance" in x.bot_link for x in answers), [x.bot_link for x in answers])
        pdfs = [f for f in a.files if ".pdf" in (f["name"] + f["href"]).lower()]
        check("hk: the filled claim form is in the chat (a PDF file card)", bool(pdfs), a.files)
        vals = {}
        for f in reversed(pdfs):
            vals = C.pdf_values(await u.download(f["href"]))
            if vals:
                break
        print(f"INFO hk: the form's values {vals}")
        check("hk: the downloaded form has the policy number and the user's name",
              C.flat(vals.get("fill_17", "")) == POLICY and "CHANTAIMAN" in C.flat(vals.get("Full name", "")), vals)
        check("hk: … the flight, the hours delayed and the amount (CX 543, 6 hours, HKD 300)",
              "CX543" in C.flat(vals.get("fill_14_2", "") + vals.get("fill_18_2", "")) and re.search(r"\b6\b", vals.get("fill_1", ""))
              and "300" in vals.get("fill_7", ""), vals)
        check("hk: … and leaves the HKID number, the bank account and the signature to the user",
              not vals.get("HKID Card no", "").strip() and not vals.get("fill_4_5", "").strip() and not vals.get("Claimants signature", "").strip(), vals)
        check("hk: the answer says bolttech handles the claim and gives the amount (HKD 300)",
              re.search(r"bolttech", text, re.I) and re.search(r"(HKD|HK\$)\s?300\b", text), text[:900])
        check("hk: the answer says what the user still adds (HKID, signature)", re.search(r"HKID|identity card", text, re.I)
              and re.search(r"sign", text, re.I), text[:900])
        check("hk: the answer does not say the email was sent", not C.SENT.search(text), (C.SENT.search(text) or [""])[0])
    drafts = C.mail_full("Drafts")[drafts0:]
    print(f"INFO hk drafts: {[(d['to'], d['subject'], [n for n, _ in d['attachments']]) for d in drafts]}")
    mine = [d for d in drafts if BOLTTECH in d["to"].lower()]
    check("hk: a new draft to bolttech's claims mailbox is in the user's Drafts", bool(mine), [(d["to"], d["subject"]) for d in drafts])
    if mine:
        d = mine[-1]
        check("hk: the draft is about policy MT-HK-2026-00881", POLICY in d["subject"] + d["body"], (d["subject"], d["body"][:400]))
        forms = [n for n, data in d["attachments"] if n.lower().endswith(".pdf") and C.flat(C.pdf_values(data).get("fill_17", "")) == POLICY]
        check("hk: the filled claim form is attached to the draft", bool(forms), [n for n, _ in d["attachments"]])
        check("hk: Cathay's delay letter is attached to the draft", any("CX543" in C.flat(n) for n, _ in d["attachments"]),
              [n for n, _ in d["attachments"]])
    sent = C.mail_full("Sent")
    check("hk: nothing new in the user's Sent folder", len(sent) == sent0, [(m["to"], m["subject"]) for m in sent[sent0:]])


U.setup(mail=MAIL, vault=False, profile={"name_en": "CHAN TAI MAN", "email_personal": "taiman@example.com", "phone": "+852 9123 4567",
                                         "address_home": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong"})
U.run(main)
