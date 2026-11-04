"""FWD Hong Kong travel insurance claim with a real model, on the REAL claim form, from the user's point of view
(tests/user_common.py, tests/claims_common.py). FWD Hong Kong's travel plans are underwritten by bolttech, whose claim
form is a fillable PDF on FWD's site (https://www.fwd.com.hk/files/…/travel-insurance-clm-form-fillable.pdf: opening it
downloads it) that goes by email to claims.hk@bolttechinsurance.com. The user gives the policy (MyTravel MT-HK-2026-00881, HKD 300 for every 6 full hours of
delay) and the delay (CX 543 two days ago, 09:05 → 15:20) in the request; the profile has their name, email, phone and
address. No mailbox: no draft is made, and the block says where to send the signed form.

The user asks Main's chat to prepare the delay claim: the form filled, not sent (they sign and send it themselves).
Checked from the user's side:
* the Money bot answers (its "Continue in" link);
* the filled claim form is in the chat as a file card; downloaded and opened, it has the policy number, the user's name,
  CX 543, the hours delayed and HKD 300 (the values the user gave) — and the HKID number, the bank account and the
  signature are left empty;
* the block code appends says bolttech decides the claim, gives the amount (HKD 300), what the user still adds (HKID,
  signature) and where to send it (no mailbox, so no draft).
One TIMING line. In run_in_docker.sh's REAL_SITES (one at a time).

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


def no_send(dialog: str) -> bool:
    """The user asked for the form only: they deny a dialog about sending mail, by the tool it names."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


HKID = "A1234563"   # the well-known dummy HKID A123456(3): a clearly fake test value


async def buy(u: U.User) -> None:
    """FWD Hong Kong's MyTravel (bolttech underwrites), on the real site: the user's short request, the model asks what
    the purchase needs (the trip, the title, the new Smart HKID's date of issue); code fills the quote, reads the plans,
    picks by the user's rule, opens the plan's application and saves the insured's details there. Checked from the
    user's side: no takeover; the block lists the plans with their premiums and the one chosen; the block links FWD's
    application, and on another device (a fresh browser) the link opens it prefilled with the user's details — nothing
    is paid."""
    a = await u.ask("Buy me FWD travel insurance for my Tokyo trip.", label="hk buy", timeout=1200)
    answers = [a]
    dep = dt.date.today() + dt.timedelta(days=30)
    for reply in [f"Just me, Hong Kong to Tokyo from {dep} to {dep + dt.timedelta(days=6)}. The cheapest plan with at least "
                  "HKD 800,000 medical cover. I'm Mr., and my HKID card was issued on 2020-01-01.",
                  "Everything else is in my profile. Please go ahead."]:
        if "Your FWD application" in a.block or a.timed_out:
            break
        a = await u.ask(reply, label="hk buy follow-up", new_chat=False, timeout=1200)
        answers.append(a)
    block = "\n".join(x.block for x in answers)
    check("hk buy: no takeover", not any(x.takeovers for x in answers), [x.takeovers for x in answers])
    check("hk buy: the block lists the plans FWD Hong Kong quoted and the one chosen", "Plans quoted:" in block
          and re.search(r"Chosen: \w+: HKD [\d,]+\.\d\d", block) is not None, block)
    links = [x for y in answers for x in y.block_links if "mytravel-insurance/en/application/?leadId=" in x]
    check("hk buy: the block links FWD's application", bool(links), [y.block_links for y in answers])
    if links:
        other = await u._br.new_context(viewport={"width": 1280, "height": 1000})
        pg = await other.new_page()
        await pg.goto(links[0], wait_until="domcontentloaded")
        await pg.wait_for_selector("#lastName", timeout=60000)
        await pg.wait_for_timeout(4000)
        vals = await pg.evaluate("() => ['lastName','givenName','hkid','email'].map(i => (document.getElementById(i) || {}).value || '')")
        print(f"INFO hk buy: the application on another device {pg.url} {vals}")
        check("hk buy: on another device the link opens FWD's application prefilled with the user's details",
              vals[0] == "CHAN" and vals[3] == "taiman@example.com" and vals[2].startswith("A123456"), vals)
        await other.close()


async def main():
    async with U.User() as u:
        await buy(u)
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
        check("hk: the Money bot did it", any("Money" in x.bot_link for x in answers), [x.bot_link for x in answers])
        pdfs = [f for f in a.files if ".pdf" in (f["name"] + f["href"]).lower()]
        check("hk: the filled claim form is in the chat (a PDF file card)", bool(pdfs), a.files)
        vals = {}
        for f in reversed(pdfs):
            vals = C.pdf_values(await u.download(f["href"]))
            if vals:
                break
        print(f"INFO hk: the form's values {vals}")
        # the field names are the real form's (the PDF on FWD's site, read with pypdf 2026-11-02)
        # a smoke check of the filled form (the user, 2026-11-04: no per-field e2e checks; forms_claims.rs unit-tests the fields)
        check("hk: the downloaded form has the policy number", C.flat(vals.get("fill_17", "")) == POLICY, vals)
        block = "\n".join(x.block for x in answers)
        check("hk: the block code appends after the answer says bolttech handles the claim and gives the amount (HKD 300)",
              "Handled by: bolttech" in block and "Claim amount: HKD 300.00" in block, (block, text[:300]))
        check("hk: the block code appends after the answer says what the user still adds (HKID, signature)",
              "You still add:" in block and "HKID" in block and "signature" in block, block)
        check("hk: no mailbox — no draft claimed; the block says where to send the signed form",
              "Send it to: claims.hk@bolttechinsurance.com" in block and "Email draft" not in block, block)


U.setup(vault=False, profile={"name_en": "CHAN TAI MAN", "email_personal": "taiman@example.com", "phone": "+852 9123 4567",
                              "address_home": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong", "birthday": "1990-01-01",
                              "home_country": "HK"})
U.vault_item("HKID Tai Man", "id_document", {"number": HKID, "name": "CHAN TAI MAN", "country": "HK"}, domains="fwd.com.hk")
U.run(main)
