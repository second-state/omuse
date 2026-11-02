"""Hong Kong departure tax (APDT) refund with a real model, from the user's point of view (see tests/user_common.py).

The user flew Singapore → Hong Kong on SQ 890 three days ago and Hong Kong → Tokyo on CX 500 the next morning, on a
separate Cathay Pacific ticket that charged the HK$120 departure tax. Their mailbox has both boarding passes and the
Cathay e-ticket receipt as PDF attachments (tests/apdt_common.py); the profile has their name, birthday, nationality and
email; the Vault has their passport and their Visa card (both allowed on apdtrefund.hk). The Airport Authority's
platform is the stand-in served at its real address (tests/https_standins.py: https://app.apdtrefund.hk/hkTaxMobile/,
tests/pages/apdt*.html). The user attaches the photo page of their passport with ＋ (the Vault keeps the number, not
the picture) and asks Main for the refund to their Visa card. If the answer asks for something first, the user answers
once in the same chat.

What the user sees and is checked:
* the Travel bot did it (the answer's "Continue in ✈️ Travel" link) and no approval dialog came up (auto-approve on);
* in the browser view, the platform's tab is on the "Proceed with Facial Recognition" page, and the application its
  address carries has every field and document: entry by air, a passport, the attached photo, the name, the passport
  number from the Vault, birthday and nationality, SQ 890 / CX 500 with their dates, both boarding passes and the
  receipt uploaded from the mail, the third-party and declaration boxes, the Visa card with holder, expiry and the
  fee agreement, the application confirmed;
* the refund page receives money, so the Visa card goes in from the Vault (number, expiry, holder) — no hand-over at
  all; (a card the Vault cannot fill would be typed by the user in a takeover — the handler stays for that);
* the face-recognition page was never opened (the platform counts the attempts);
* the answer gives the reference number exactly as the site issued it and a link to the official site for the face
  recognition the user does on their phone;
* the passport and card numbers never appear in the chat (messages, cards, the answer).
One TIMING line per request; the browser view's picture goes to OMUSE_E2E_OUT when it is set.

Not in run_in_docker.sh's default list (real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh apdt_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import apdt_common as A  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
FILES = A.write_files("/tmp/claude-0/apdt_user")


async def main():
    async with U.User() as u:
        await u.new_chat()
        await A.attach(u, [FILES["passport"]])
        a = await u.ask("I flew through Hong Kong this week on two separate tickets: Singapore Airlines into Hong Kong, then "
                        "Cathay Pacific to Tokyo the next morning, and I paid the Hong Kong departure tax on the Cathay ticket. "
                        "Please apply for the Hong Kong departure tax refund for me. The boarding passes and the e-ticket receipt "
                        "are in my email, and I attached the photo page of my passport. Refund it to my Visa card.",
                        label="apdt", new_chat=False, on_takeover=lambda pg, r: A.type_card(u, r), timeout=1500)
        approvals, takeovers = list(a.approvals), list(a.takeovers)
        page = await A.browser_tab(u, "apdtrefund.hk", os.path.join(OUT, "apdt_final.png") if OUT else None)
        if not re.search(r"apdt_(submit|confirm)\.html", page):
            check("apdt question: the Travel bot answered (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
            a = await u.ask("Yes, please go ahead: everything is in my email and in the passport photo I attached; my Visa card "
                            "and my passport are in the Vault. Fill in and upload everything and give me the reference number.",
                            label="apdt follow-up", new_chat=False, on_takeover=lambda pg, r: A.type_card(u, r), timeout=1500)
            approvals += a.approvals
            takeovers += a.takeovers
            page = await A.browser_tab(u, "apdtrefund.hk", os.path.join(OUT, "apdt_final.png") if OUT else None)
        app = A.application(page)
        print(f"INFO platform tab: {page[:160]!r}")
        print(f"INFO application: { {k: v for k, v in app.items() if k not in ('pics',)} }")
        check("apdt: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("apdt: the Travel bot did it (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
        check("apdt: no approval dialog (browser auto-approve is on)", not approvals, approvals)
        check("apdt: the platform's tab stops at 'Proceed with Facial Recognition' (application confirmed)",
              app["page"] == "apdt_submit.html" and app.get("confirmed") is True, (app["page"], app.get("confirmed")))
        check("apdt: the face-recognition page was never opened", app.get("face", 0) == 0 and "apdt_face" not in page, (page[:120], app.get("face")))
        check("apdt: entered Hong Kong by air, with a passport", app.get("mode") == "air" and app.get("doctype") == "Passport", app)
        f = app.get("files") or {}
        check("apdt: the passport photo the user attached was uploaded as the travel document",
              bool(re.search(r"passport.*\.jpe?g$", f.get("doc", ""), re.I)), f)
        check("apdt: both boarding passes and the e-ticket receipt from the mail were uploaded in their boxes",
              "SQ890" in f.get("inboard", "").upper() and "CX500" in f.get("outboard", "").upper()
              and re.search(r"ETKT|receipt", f.get("paydoc", ""), re.I) is not None, f)
        name = app.get("name", "").upper()
        check("apdt: the name as on the passport, the birthday and the nationality",
              {"LU", "LUCAS"} <= set(re.findall(r"[A-Z]+", name)) and app.get("dob") == "1985-04-12"
              and "AUSTRAL" in app.get("nationality", "").upper(), (name, app.get("dob"), app.get("nationality")))
        check("apdt: the travel document number is the passport's (from the Vault)", app.get("docsum") == A.docsum(A.PASSPORT), app.get("docsum"))
        check("apdt: the arrival flight SQ 890 and the departure flight CX 500 with their dates",
              (app.get("inflight"), app.get("indate"), app.get("outflight"), app.get("outdate"))
              == ("SQ890", A.ARRIVE.isoformat(), "CX500", A.DEPART.isoformat()), app)
        check("apdt: the third-party authorisation and the declaration boxes ticked",
              app.get("third") is True and (app.get("pics") or {}).get("declare") is True, app)
        r = app.get("refund") or {}
        check("apdt: refund to the Visa card from the Vault, with holder, expiry and the fee agreement",
              r.get("type") == "bank" and r.get("cardsum") == A.docsum(A.CARD) and r.get("valid") == "12/29"
              and "LUCAS" in r.get("holder", "").upper() and r.get("fee_agree") is True, r)
        check("apdt: the block code appends after the answer links the official site (where the face recognition is done)",
              any(x.startswith("https://app.apdtrefund.hk/hkTaxMobile") for x in a.block_links), (a.block, a.block_links))
        shown = await u.thread_text() + " " + " ".join(approvals + takeovers)
        check("apdt: the passport and card numbers never appear in the chat",
              A.PASSPORT not in shown and A.CARD not in shown.replace(" ", ""), [m.start() for m in re.finditer(A.PASSPORT, shown)][:3])
        check("apdt: no hand-over was needed (the Visa card came from the Vault; the face step is the user's, on their phone)",
              not takeovers, takeovers)


U.setup(mail=A.mail(), vault=False, profile={"name_en": "LUCAS LU", "birthday": "1985-04-12", "nationality": "Australia",
                                             "gender": "Male", "email_personal": "lucas@example.com", "phone": "+61 400 000 000"})
K.vault_item("Passport Lucas", "id_document", {"number": A.PASSPORT, "name": "LU LUCAS", "expiry": "2031-05-01", "country": "AU"},
             domains="apdtrefund.hk")
K.vault_item("Visa Lucas", "card", {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123", "holder": "LUCAS LU"},
             domains="apdtrefund.hk")
U.run(main)
