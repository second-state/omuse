"""Filling a PDF form from an email with a real model, from the user's point of view (tests/user_common.py,
tests/shop_common.py). The user's inbox holds the school's mail about the class 4B aquarium trip with the permission
slip attached (tests/pages/permission_slip.pdf: student name, class, parent, emergency phone, consent, lunch regular /
vegetarian, T-shirt S–XL); the profile has the parent's name and phone; the user's mailbox sends through its provider's
SMTP server (here a local one that records what the school receives).

1. fill — "fill in Able's permission slip from the school's email: Able Lu, class 4B, I give permission, vegetarian
   lunch, T-shirt L; my details are in my profile". Checked: a PDF file card in the chat; the PDF the user downloads
   is the school's form with those values (student, class, parent from the profile, emergency phone from the profile,
   consent ticked, vegetarian, L); nothing was sent to the school yet.
2. send — "looks good, reply to the school with it". The user approves the reply in the dialog. Checked: the school
   receives one reply with the filled slip attached, and its values are the ones the user checked.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh pdf_forms_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SCHOOL = "office@lincolnschool.edu.sg"
SLIP = open(os.path.join(S.PAGES, "permission_slip.pdf"), "rb").read()
MAIL = [
    ("Boss <boss@acme.com>", "Quarterly plan", "Please review the quarterly plan by Friday.", 1),
    (f"Lincoln School Office <{SCHOOL}>", "Class 4B aquarium trip on 13 November — permission slip",
     "Dear parents of class 4B,\n\nOur class trip to the S.E.A. Aquarium is on Friday 13 November 2026. Please fill in the attached "
     "permission slip — choose a regular or vegetarian lunch and your child's trip T-shirt size — and reply to this email with it by "
     "6 November.\n\nThank you,\nMs Rachel Ong\nLincoln School Office", 2, [("permission_slip.pdf", SLIP)]),
    ("Shop <news@shop.com>", "Big sale today", "50% off everything.", 0),
]


def slip_ok(v: dict) -> dict:
    """Which of the user's answers the filled slip carries."""
    return {"student": "able" in v.get("student_name", "").lower(), "class": "4b" in v.get("class", "").lower().replace(" ", ""),
            "parent": "lu" in v.get("parent_name", "").lower(), "phone": "91234567" in re.sub(r"\D", "", v.get("emergency_phone", "")),
            "consent": v.get("consent", "").lstrip("/") == "Yes", "lunch": v.get("lunch", "").lstrip("/") == "vegetarian",
            "tshirt": v.get("tshirt", "") == "L"}


async def main():
    school = S.SchoolMail()
    school.start()
    async with U.User() as u:
        a = await u.ask("Able's school emailed a permission slip for the aquarium trip. Please fill it in for me: Able Lu, class 4B, "
                        "I give permission, vegetarian lunch, T-shirt size L. My name and phone are in my profile.", label="fill")
        check("fill: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        pdfs = [f for f in a.files if ".pdf" in (f["name"] + f["href"]).lower()]
        check("fill: a PDF file card is in the chat", bool(pdfs), (a.files, a.text[:500]))
        vals = C.pdf_values(await u.download(pdfs[-1]["href"])) if pdfs else {}
        print(f"INFO the filled slip: {vals}")
        ok = slip_ok(vals)
        check("fill: the downloaded PDF is the school's form, filled in", bool(vals) and "student_name" in vals, vals)
        check("fill: … student Able Lu, class 4B", ok["student"] and ok["class"], vals)
        check("fill: … the parent and the emergency phone from the profile", ok["parent"] and ok["phone"], vals)
        check("fill: … permission given, vegetarian lunch, T-shirt L", ok["consent"] and ok["lunch"] and ok["tshirt"], vals)
        check("fill: nothing was sent to the school yet", not school.received, [m["subject"] for m in school.received])

        b = await S.ask(u, "Looks good — please reply to the school with the filled slip attached.", label="send", new_chat=False)
        await u.page.wait_for_timeout(3000)
        to_school = [m for m in school.received if any(SCHOOL in r for r in m["to"])]
        print(f"INFO the school received: {[(m['to'], m['subject'], [n for n, _ in m['attachments']]) for m in school.received]}")
        check("send: the answer came (no time-out)", bool(b.text.strip()) and not b.timed_out, b.text[:300])
        # "Auto-approve actions" is on by default (2026-10-31): the reply goes out without a dialog (still recorded)
        check("send: the reply went out without asking (auto-approve is the default)", not b.approvals, b.approvals)
        check("send: the school received one reply", len(to_school) == 1, [(m["to"], m["subject"]) for m in school.received])
        att = [d for m in to_school for n, d in m["attachments"] if n.lower().endswith(".pdf")]
        check("send: … with the filled slip attached", bool(att), [[n for n, _ in m["attachments"]] for m in to_school])
        sent_vals = C.pdf_values(att[0]) if att else {}
        check("send: … carrying the values the user checked", bool(sent_vals) and all(slip_ok(sent_vals).values()), sent_vals)


U.setup(mail=MAIL, vault=False, profile={"name_en": "LUCAS LU", "phone": "+65 9123 4567", "email_personal": "lucas@example.com"})
U.run(main)
