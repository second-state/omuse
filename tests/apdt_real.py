"""Hong Kong departure tax (APDT) refund with a real model, on the Airport Authority's REAL platform
(https://app.apdtrefund.hk/hkTaxMobile/?lang=en), from the user's point of view (tests/user_common.py).

The user flew Singapore → Hong Kong on SQ 890 three days ago and Hong Kong → Tokyo on CX 500 the next morning, on a
separate Cathay Pacific ticket that charged the HK$120 departure tax; they give the flights in the request (no mailbox,
no documents attached — a made-up passport or boarding pass is never uploaded to a government site) and ask Main for the
refund to their card. Code checks the rule (left the same or the next day, within 28 days, after departing), opens the
platform and goes through its anti-scam notice, the two statements, Start, New Application and the entry mode (Air) to
the travel document's upload, and stops: the documents' photos, the refund method and the face recognition are the
user's, on their phone — the flow never hands the browser over. If the answer asks something first, the user answers
once in the same chat.

What the user sees and is checked:
* the Travel bot did it (the answer's "Continue in ✈️ Travel" link); no takeover and no approval dialog;
* the block code appends: eligible (the two flights), the tax to be refunded (HKD 120.00), the last day to apply (the
  departure + 28 days, as a date), the steps left (the documents' upload and the face recognition, on the phone) and a
  link to the official platform;
* in the browser view, the platform's tab is at its document upload (`/hkTaxMobile/process/upload`): code reached it on
  the real site, nothing uploaded or submitted.
One TIMING line per request; the browser view's picture goes to OMUSE_E2E_OUT when it is set. In run_in_docker.sh's
REAL_SITES (one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh apdt_real"""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
TODAY = dt.date.today()
ARRIVE = TODAY - dt.timedelta(days=3)    # SQ 890 Singapore → Hong Kong
DEPART = TODAY - dt.timedelta(days=2)    # CX 500 Hong Kong → Tokyo, the next calendar day, on a separate ticket
SITE = "app.apdtrefund.hk"


async def main():
    async with U.User() as u:
        a = await u.ask(f"I flew through Hong Kong this week on two separate tickets: Singapore Airlines SQ 890 from Singapore landed in "
                        f"Hong Kong on {ARRIVE}, and I left on Cathay Pacific CX 500 to Tokyo the next morning, {DEPART}. The Cathay "
                        "ticket charged the HK$120 Hong Kong departure tax. Please apply for the Hong Kong departure tax refund for "
                        "me, refunded to my credit card.", label="apdt", timeout=1500)
        answers = [a]
        if "Next steps (yours)" not in a.block and "Not eligible" not in a.block and not a.timed_out:
            a = await u.ask("Yes, please go ahead with the application on the official site; refund it to my credit card.",
                            label="apdt follow-up", new_chat=False, timeout=1500)
            answers.append(a)
        block = "\n".join(x.block for x in answers)
        check("apdt: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("apdt: the Travel bot did it (the link to continue in Travel)", any("Travel" in x.bot_link for x in answers),
              [x.bot_link for x in answers])
        check("apdt: no takeover and no approval dialog (the documents and the face check are the user's, on their phone)",
              not any(x.takeovers or x.approvals for x in answers), [(x.takeovers, x.approvals) for x in answers])
        check("apdt: the block says the trip qualifies (the two flights) and the tax to be refunded (HKD 120.00)",
              "Eligible:" in block and "SQ 890" in block and "CX 500" in block and "HKD 120.00" in block, block)
        check("apdt: the block gives the last day to apply (the departure + 28 days) as a date",
              "Apply by:" in block and C.date_in(block, DEPART + dt.timedelta(days=28)), block)
        check("apdt: the block says the documents' upload and the face recognition are the user's, on their phone",
              "document upload" in block and "face recognition" in block, block)
        check("apdt: the block links the official platform", any(SITE in x for x in a.block_links + [y for z in answers for y in z.block_links]),
              [x.block_links for x in answers])
        # the user opens the browser view: the platform's tab, at its document upload (reached by code on the real site)
        await u.page.goto(U.APP + "#browser")
        await u.page.wait_for_selector(".bview", timeout=20000)
        await u.page.wait_for_timeout(2500)
        now = await u.view_url()
        await u.view_picture(os.path.join(OUT, "apdt_upload.png") if OUT else None)
        print(f"INFO browser view: url {now!r}")
        check("apdt: the browser view shows the platform at its document upload (nothing uploaded or submitted)",
              SITE in now and "/process/upload" in now, now)


U.setup(vault=False, profile={"name_en": "LUCAS LU", "birthday": "1985-04-12", "nationality": "Australia",
                              "gender": "Male", "email_personal": "lucas@example.com", "phone": "+61 400 000 000"})
U.run(main)
