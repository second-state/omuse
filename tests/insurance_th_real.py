"""FWD Thailand travel insurance (Dhipaya's TIP Insure) with a real model, on the REAL site, from the user's point of
view (tests/user_common.py). The user lives in Bangkok (clearly fake profile: SOMCHAI JAIDEE, a dummy passport in the
Vault for tipinsure.com). They ask for FWD travel insurance for a week in Tokyo with at least THB 15,000 travel-delay
cover; the model asks what the purchase needs (the dates, the title, the name in parts; the address parts it reads from
the profile), code quotes Dhipaya's packages, picks by the rule, fills the application and asks the user's approval in
the app (the package, the premium and Dhipaya's conditions). Here the user DECLINES: a test never accepts the conditions
or makes an order at a real insurer.

Checked from the user's side:
* no takeover; the Money bot answers;
* the block lists the packages quoted (THB premiums) and the one chosen;
* the approval dialog names the package, its THB premium and Dhipaya's conditions (Thai residents);
* declined: the block says no order was made.
One TIMING line per request. In run_in_docker.sh's REAL_SITES (one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh insurance_th_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
PASSPORT = "AA1234567"   # a clearly fake test value


async def main():
    async with U.User() as u:
        seen: dict = {"dialogs": []}

        def decline_purchase(dialog: str) -> bool:
            if U.tool_of(dialog) == "submit_confirm":
                seen["dialogs"].append(dialog)
                return False
            return True

        dep = TODAY + dt.timedelta(days=30)
        a = await u.ask("Buy me FWD travel insurance for my Tokyo trip, the cheapest plan with at least THB 15,000 "
                        "travel delay cover.", label="th buy", approve=decline_purchase, timeout=1500)
        answers = [a]
        for reply in [f"Just me, from {dep} to {dep + dt.timedelta(days=6)}. I'm Mr.; my name and address are in my "
                      "profile. Please go ahead.",
                      "Everything else is in my profile. Please go ahead."]:
            if seen["dialogs"] or a.timed_out:
                break
            a = await u.ask(reply, label="th buy follow-up", new_chat=False, approve=decline_purchase, timeout=1500)
            answers.append(a)
        block = "\n".join(x.block for x in answers)
        check("th buy: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("th buy: no takeover", not any(x.takeovers for x in answers), [x.takeovers for x in answers])
        check("th buy: the Money bot did it", any("Money" in x.bot_link for x in answers),
              [x.bot_link for x in answers])
        check("th buy: the block lists the packages quoted and the one chosen (THB)", "Plans quoted:" in block
              and re.search(r"Chosen: Backpack[\w ]*: THB [\d,]+\.\d\d", block) is not None, block[:600])
        dialog = seen["dialogs"][0] if seen["dialogs"] else ""
        print(f"INFO th buy: approval dialog {' '.join(dialog.split())[:600]!r}")
        check("th buy: the user is asked to approve the purchase — the package, its THB premium and Dhipaya's conditions",
              "Backpack" in dialog and "THB" in dialog and "lives in Thailand" in dialog, dialog[:600])
        check("th buy: declined, so no order was made (the block says so)", "no order was made" in block, block[:600])


U.setup(vault=False, profile={"name_en": "SOMCHAI JAIDEE", "email_personal": "somchai.test@example.com",
                              "phone": "+66 81 234 5678", "birthday": "1990-01-01", "home_country": "TH",
                              "nationality": "Thailand",
                              "address_home": "12 Sukhumvit Road, Khlong Toei Nuea, Watthana, Bangkok 10110"})
U.vault_item("Passport Somchai", "id_document", {"number": PASSPORT, "name": "SOMCHAI JAIDEE", "country": "TH"},
             domains="tipinsure.com")
U.run(main)
