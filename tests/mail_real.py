"""Mail with a real model, from the user's point of view (tests/user_common.py). The landlord's rent-increase mail sits
in the user's inbox (the local IMAP account) among other mail. In Main's chat the user asks whether the landlord wrote
about the rent increase and for a polite reply as a draft. What the user sees: the answer gives the mail's facts (the
old and the new rent); in their mailbox's Drafts folder there is a reply to the landlord asking to keep the current rent;
nothing new is in Sent (they asked for a draft) and the answer does not say it was sent. One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh mail_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

LANDLORD = "daniel.tan@hollandprops.example"
MAIL = [
    ("Boss <boss@acme.com>", "Quarterly plan", "Please review the quarterly plan by Friday.", 1),
    (f"Daniel Tan <{LANDLORD}>", "Rent for 12 Holland Road #05-03 from 1 January",
     "Hi Lucas,\n\nI hope you are well. As your lease renews on 1 January 2027, the monthly rent for 12 Holland Road #05-03 "
     "will go up from S$3,800 to S$4,300, in line with the market. Please let me know by 15 November whether you would like "
     "to renew on these terms.\n\nBest regards,\nDaniel Tan", 3),
    ("SP Group <billing@spgroup.example>", "Your October utilities bill", "Your bill of S$142.60 is due on 20 November.", 2),
    ("Shop <news@shop.com>", "Big sale today", "50% off everything.", 0),
]


async def main():
    drafts0, sent0 = len(U.mailbox("Drafts")), len(U.mailbox("Sent"))
    async with U.User() as u:
        a = await u.ask("Did my landlord email me about the rent increase? Draft a polite reply asking to keep it at the current amount.",
                        label="rent")
        check("rent: the answer gives the mail's facts (S$3,800 → S$4,300)", a.mentions("4,300", "4300") and a.mentions("3,800", "3800"), a.text[:800])
        check("rent: the answer does not say the reply was sent", not re.search(r"\b(I('ve| have)? sent|has been sent|was sent)\b", a.text, re.I),
              a.text[:600])
    drafts = U.mailbox("Drafts")
    new = drafts[drafts0:]
    print(f"INFO Drafts: {[(d['to'], d['subject'], ' '.join(d['body'].split())[:300]) for d in new]}")
    mine = [d for d in new if LANDLORD in d["to"].lower()]
    check("rent: a new draft to the landlord is in the user's Drafts folder", bool(mine), [(d["to"], d["subject"]) for d in drafts])
    if mine:
        d = mine[-1]
        check("rent: the draft is a reply about the rent (its subject)", re.search(r"rent|holland", d["subject"], re.I), d["subject"])
        check("rent: the draft asks to keep the current rent (S$3,800 or the current amount)",
              re.search(r"3,?800|current (rent|amount|rate)", d["body"], re.I), d["body"][:600])
        check("rent: the draft is polite (thanks or regards)", re.search(r"thank|regards|appreciate|grateful", d["body"], re.I), d["body"][:600])
    sent = U.mailbox("Sent")
    check("rent: nothing new in the user's Sent folder", len(sent) == sent0, [(m["to"], m["subject"]) for m in sent[sent0:]])


U.setup(mail=MAIL)
U.run(main)
