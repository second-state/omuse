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
* the Money bot did it (the answer's "Continue in 💳 Money" link); no takeover and no approval dialog;
* the block code appends: eligible (the two flights), the tax to be refunded (HKD 120.00), the last day to apply (the
  departure + 28 days, as a date), the steps left (the documents' upload and the face recognition, on the phone) and a
  link to the official platform (a smoke check: where code stopped on the platform is not checked).
One TIMING line per request. In run_in_docker.sh's
REAL_SITES (one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh apdt_real"""
import datetime as dt
import os
import re
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
            a = await u.ask("I don't have the documents at hand, I'll upload them later. Please go ahead on the official site; refund it to my credit card.",
                            label="apdt follow-up", new_chat=False, timeout=1500)
            answers.append(a)
        block = "\n".join(x.block for x in answers)
        check("apdt: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("apdt: the Money bot did it (the link to continue in Money)", any("Money" in x.bot_link for x in answers),
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


async def with_documents(u: U.User, docs: str, doc_no: str) -> None:
    """The user's own documents (e2e-local/apdt/ on this machine, git-ignored: passport.jpg, boarding_pass.jpg,
    ticket_with_tax.pdf and doc_no.txt; skipped where the folder is missing): they attach them and ask in a short request; code uploads them, the platform saves the application and
    the block gives its reference number. On another device (a fresh browser) the platform's "View Application /
    Continue Application" with the travel document number (doc_no.txt) and that number opens the application,
    not submitted. The arrival into Hong Kong is not given: the block says to add it there."""
    files = [os.path.join(docs, f) for f in ("passport.jpg", "boarding_pass.jpg", "ticket_with_tax.pdf")]
    await u.new_chat()
    chips = await u.attach(*files)
    check("docs: the three documents are attached", len(chips) == 3 and not any(c["bad"] for c in chips), chips)
    a = await u.ask("Please apply for my Hong Kong departure tax refund with these documents, refunded to my credit card.",
                    label="docs", new_chat=False, timeout=1500)
    answers = [a]
    if "Application reference" not in a.block and not a.timed_out:
        a = await u.ask("I came into Hong Kong by land from Shenzhen the day before I flew out. Please go ahead and upload the documents; refund to my credit card.",
                        label="docs follow-up", new_chat=False, timeout=1500)
        answers.append(a)
    block = "\n".join(x.block for x in answers)
    ref = re.search(r"Application reference: \**([A-Z0-9]{6})\**", block)
    print(f"INFO docs: reference {ref.group(1) if ref else None}")
    check("docs: no takeover and no approval dialog", not any(x.takeovers or x.approvals for x in answers),
          [(x.takeovers, x.approvals) for x in answers])
    check("docs: the block gives the application's reference number", bool(ref), block)
    check("docs: the block says how to open it on any device, and the entry by land / sea from the conversation",
          "View Application / Continue Application" in block and "Entered Hong Kong by: land / sea" in block, block)
    check("docs: the block links the official platform", any(SITE in x for z in answers for x in z.block_links),
          [x.block_links for x in answers])
    if not ref:
        return
    # another device: the platform's own "find application" with the travel document number and the reference
    other = await u._br.new_context(viewport={"width": 420, "height": 900}, is_mobile=True)
    pg = await other.new_page()
    await pg.goto("https://app.apdtrefund.hk/hkTaxMobile/?lang=en", wait_until="domcontentloaded")
    await pg.wait_for_selector("span.check-img", timeout=60000)

    async def close_notices():
        for _ in range(4):
            shown = await pg.evaluate("() => [...document.querySelectorAll('.mask')].filter(m => m.getClientRects().length).length")
            if not shown:
                return
            await pg.locator(".mask div.button, .mask button").first.click()
            await pg.wait_for_timeout(800)

    for k in range(2):
        await pg.wait_for_timeout(1200)
        await close_notices()
        await pg.locator("span.check-img").nth(k).click()
    await pg.wait_for_timeout(1200)
    await close_notices()
    await pg.locator("div.button", has_text="Start").first.click()
    await pg.wait_for_selector("div.btn:has-text('View Application')", timeout=20000)
    await pg.wait_for_timeout(1500)
    await close_notices()
    await pg.locator("div.btn", has_text="View Application").first.click()
    await pg.wait_for_selector("input.el-input__inner", timeout=20000)
    await pg.locator("input.el-input__inner").first.fill(doc_no)
    await pg.get_by_text("Use reference number (6 digits & letters)", exact=True).first.click()
    await pg.wait_for_timeout(800)
    await pg.locator("input.el-input__inner").nth(1).fill(ref.group(1))
    await pg.get_by_text("Start Search", exact=True).first.click()
    await pg.wait_for_timeout(8000)
    text = await pg.locator("body").inner_text()
    print(f"INFO docs: on another device {pg.url} {' '.join(text.split())[:400]}")
    if OUT:
        await pg.screenshot(path=os.path.join(OUT, "apdt_other_device.png"), full_page=True)
    check("docs: on another device the reference number opens the application, in progress and not submitted",
          "/refundProcess" in pg.url and "Application in progress" in text
          and re.search(r"not (yet )?been submitted", text) is not None, (pg.url, text[:300]))
    await other.close()


async def all_suites():
    await main()
    # the user's own documents: per machine, never in git — e2e-local/apdt/ (passport.jpg, boarding_pass.jpg,
    # ticket_with_tax.pdf, doc_no.txt with the travel document's number), or OMUSE_E2E_APDT_DOCS / OMUSE_E2E_APDT_DOC_NO
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    docs = os.environ.get("OMUSE_E2E_APDT_DOCS", "") or next(
        (d for d in ("/repo/e2e-local/apdt", os.path.join(repo, "e2e-local", "apdt")) if os.path.isdir(d)), "")
    doc_no = os.environ.get("OMUSE_E2E_APDT_DOC_NO", "")
    if docs and not doc_no and os.path.isfile(os.path.join(docs, "doc_no.txt")):
        doc_no = open(os.path.join(docs, "doc_no.txt")).read().strip()
    if docs and doc_no:
        async with U.User() as u:
            await with_documents(u, docs, doc_no)
    else:
        print("INFO docs: skipped (no e2e-local/apdt/ on this machine: the user's own documents)")


U.setup(vault=False, profile={"name_en": "LUCAS LU", "birthday": "1985-04-12", "nationality": "Australia",
                              "gender": "Male", "email_personal": "lucas@example.com", "phone": "+61 400 000 000"})
U.run(all_suites)
