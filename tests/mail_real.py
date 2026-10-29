"""Mail with a real model (see tests/real_common.py): a plain request sent to the main bot, which keeps it (mail is
general work, no specialised bot). The landlord's rent-increase mail sits in the local IMAP inbox among other mail; the
model must find and read it (gmail_* tools), give its facts (the old and the new rent, the date), and write a polite
reply as a draft (gmail_create_draft, to the landlord, asking to keep the current rent) — saved in Drafts, never sent
(no gmail_send / gmail_reply: the user asked for a draft). No browser. One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_PLANNER=xai/grok-4.7 OMUSE_E2E_EXECUTOR=openai-codex/gpt-6-luna \\
        tests/run_in_docker.sh mail_real"""
import imaplib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import real_common as R  # noqa: E402
from tests import skills_common as K  # noqa: E402

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


def results(info: dict, name: str) -> list[dict]:
    return [e["data"] for x in info["tasks"] for e in R.task(x).get("events", []) if e["type"] == "tool_result"
            and e["data"].get("name") == name]


def drafts() -> list[str]:
    """The raw messages in the Drafts folder of the local IMAP mailbox."""
    m = imaplib.IMAP4("127.0.0.1", 1143)
    m.login(K.USER, K.PW)
    out = []
    try:
        if m.select("Drafts", readonly=True)[0] == "OK":
            _, ids = m.search(None, "ALL")
            for i in (ids[0] or b"").split():
                _, d = m.fetch(i, "(RFC822)")
                out.append(d[0][1].decode("utf-8", "replace"))
    finally:
        m.logout()
    return out


R.setup(mail=MAIL)
info = R.ask("Did my landlord email me about the rent increase? Draft a polite reply asking to keep it at the current amount.")
R.routed("rent", info, "main")
R.finished("rent", info)
res = info["result"]
R.check("rent: the mailbox was searched or read (gmail_* tools)",
        any(n in info["names"] for n in ("gmail_search", "gmail_get_message", "gmail_get_thread")), info["names"])
R.check("rent: the answer gives the mail's facts (S$3,800 → S$4,300)",
        R.mentions(res, "4,300", "4300") and R.mentions(res, "3,800", "3800"), res[:800])
mk = [x.get("args") or {} for x in info["calls"] if x["name"] == "gmail_create_draft"]
ok = [r for r in results(info, "gmail_create_draft") if R.T.ok(r)]
R.check("rent: a draft was created (gmail_create_draft succeeded)", bool(mk) and bool(ok), (mk, results(info, "gmail_create_draft")))
d = mk[-1] if mk else {}
R.check("rent: the draft replies to the landlord (to him, or a reply to his mail)",
        LANDLORD in str(d.get("to", "")).lower() or bool(d.get("reply_to_message_id")), d)
R.check("rent: the draft asks to keep the current rent (mentions S$3,800 or the current amount)",
        R.mentions(str(d.get("body", "")), "3,800", "3800", "current"), str(d.get("body", ""))[:600])
R.check("rent: nothing was sent (no gmail_send / gmail_reply / gmail_forward)",
        not [n for n in info["names"] if n in ("gmail_send", "gmail_reply", "gmail_forward")], info["names"])
saved = [m for m in drafts() if LANDLORD in m.lower()]
R.check("rent: the draft is in the mailbox's Drafts folder", bool(saved), [m[:300] for m in drafts()])
R.no_browser("rent", info)
R.timing("rent", info)
R.finish()
