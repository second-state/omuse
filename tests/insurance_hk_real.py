"""FWD Hong Kong travel insurance claim with a real model, from the user's point of view (tests/user_common.py,
tests/claims_common.py). SKELETON awaiting the real claim form on FWD Hong Kong's site (TODO line below). FWD Hong Kong's
travel plans are underwritten by bolttech, whose claim form is a fillable PDF on FWD's site that goes by email to
claims.hk@bolttechinsurance.com. The user gives the policy (MyTravel MT-HK-2026-00881, HKD 300 for every 6 full hours of
delay) and the delay (CX 543 two days ago, 09:05 → 15:20) in the request; the profile has their name, email, phone and
address. No mailbox: the draft to bolttech is not checked here.

The user asks Main's chat to prepare the delay claim: the form filled, not sent (they sign and send it themselves).
Checked from the user's side:
* the Insurance & claims bot answers (its "Continue in" link);
* the filled claim form is in the chat as a file card; downloaded and opened, it has the policy number, the user's name,
  CX 543, the hours delayed and HKD 300 (the values the user gave) — and the HKID number, the bank account and the
  signature are left empty;
* the block code appends says bolttech decides the claim, gives the amount (HKD 300) and what the user still adds
  (HKID, signature).
One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh insurance_hk_real"""
# TODO real URL: https://www.fwd.com.hk/ (the travel insurance claim form, travel-insurance-clm-form-fillable.pdf)
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


def no_send(dialog: str) -> bool:
    """The user asked for the form only: they deny a dialog about sending mail, by the tool it names."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


async def main():
    async with U.User() as u:
        a = await u.ask(f"My Cathay Pacific flight CX 543 to Tokyo on {INC} was scheduled at 09:05 and left at 15:20. Please prepare the delay "
                        f"claim on my FWD MyTravel policy {POLICY} (HKD 300 for every 6 full hours of delay): fill in the claim form. "
                        "Don't send anything — I'll sign the form and send it myself.", label="hk claim", approve=no_send, timeout=1200)
        answers = [a]
        if not a.files and not a.timed_out:
            a = await u.ask("Everything else is in my profile. Please go ahead and fill in the form.",
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
        # TODO real URL: the field names are the real form's (checked against the PDF on FWD's site)
        check("hk: the downloaded form has the policy number and the user's name (as given)",
              C.flat(vals.get("fill_17", "")) == POLICY and "CHANTAIMAN" in C.flat(vals.get("Full name", "")), vals)
        check("hk: … the flight, the hours delayed and the amount (CX 543, 6 hours, HKD 300)",
              "CX543" in C.flat(vals.get("fill_14_2", "") + vals.get("fill_18_2", "")) and re.search(r"\b6\b", vals.get("fill_1", ""))
              and "300" in vals.get("fill_7", ""), vals)
        check("hk: … and leaves the HKID number, the bank account and the signature to the user",
              not vals.get("HKID Card no", "").strip() and not vals.get("fill_4_5", "").strip() and not vals.get("Claimants signature", "").strip(), vals)
        block = "\n".join(x.block for x in answers)
        check("hk: the block code appends after the answer says bolttech handles the claim and gives the amount (HKD 300)",
              "Handled by: bolttech" in block and "Claim amount: HKD 300.00" in block, (block, text[:300]))
        check("hk: the block code appends after the answer says what the user still adds (HKID, signature)",
              "You still add:" in block and "HKID" in block and "signature" in block, block)


U.setup(vault=False, profile={"name_en": "CHAN TAI MAN", "email_personal": "taiman@example.com", "phone": "+852 9123 4567",
                              "address_home": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong"})
U.run(main)
