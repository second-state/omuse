"""FWD travel insurance in Singapore with a real model, from the user's point of view (tests/user_common.py,
tests/claims_common.py). FWD Singapore's site is the stand-in served at its real address (tests/https_standins.py:
https://www.fwd.com.sg — quote page with three plans: Premium SGD 34 (SGD 5,000 trip cancellation), Business SGD 47
(SGD 10,000), First SGD 68 (SGD 15,000); the application; the travel claim form). The profile has the user's name,
birthday and contact; the Vault has their Visa card and passport (for fwd.com.sg). "Auto-approve browser actions" is on
(a fresh install): nothing in the browser asks, and the agent hands the final Pay / Submit to the user.

1. buy — the mailbox has the Singapore Airlines booking for next month's Tokyo trip. The user asks Main's chat for the
   cheapest FWD plan with at least SGD 10,000 trip cancellation, filled in with their Vault card; they press Pay
   themselves. Checked from the user's side:
   * the Insurance & claims bot answers (its "Continue in" link);
   * the agent asks the user to take over the browser for Pay: the URL bar is FWD's application page for the Business
     plan, the live picture shows the passport and card fields filled in; the user hands it back without paying;
   * the answer names the plan bought and its amount (Business, SGD 47) and compares the plans (two of SGD 34 / 47 / 68
     in the answer or the chat's cards); afterwards the browser view shows no payment confirmation (Pay never pressed);
   * neither the passport number nor the card number appears in the chat.
2. claim — the mailbox now has FWD's policy email (TI-SG-2026-55123, SGD 100 for every 6 hours of delay) and Singapore
   Airlines' delay certificate for SQ 636 two days ago (10:30 → 17:45, the PDF attached). The user asks to claim the
   delay; the agent fills FWD's claim form and hands it over for Submit. In the browser view the URL bar is FWD's claim
   form (nothing submitted for the user); the user presses "Submit claim" in the live view, and the confirmation page's
   address shows what the form held: the policy number, the incident date, SGD 100, the passport number (from the Vault)
   and the delay certificate attached. The answer gives the delay (7 h 15 min), the amount (SGD 100) and FWD's 30-day
   deadline as a date (or that it was submitted in time); no passport number in the chat.
One TIMING line per request; the live pictures go to OMUSE_E2E_OUT when it is set.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh insurance_real"""
import datetime as dt
import os
import re
import sys
from urllib.parse import parse_qs, urlsplit

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
D1, D2 = (TODAY + dt.timedelta(days=24)).isoformat(), (TODAY + dt.timedelta(days=31)).isoformat()
INC = TODAY - dt.timedelta(days=2)
D30 = INC + dt.timedelta(days=30)
PASSPORT_NO, CARD_NO = "K1234567Z", ("4111 1111 1111 1111", "4111111111111111")
OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
PAGES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
BUY_MAIL = [("Singapore Airlines <noreply@singaporeair.example>", f"Booking confirmed — SQ 636 Singapore to Tokyo, {D1}",
             f"Passenger TAN WEI MING. SQ 636 SIN 10:30 → HND 18:25 on {D1}; SQ 637 HND → SIN on {D2}. Booking reference 7XKQ2P.", 5)]
CLAIM_MAIL = [
    ("FWD Singapore <noreply@fwd.com.sg>", "Your FWD Travel Insurance policy TI-SG-2026-55123",
     f"Dear TAN WEI MING,\n\nThank you for choosing FWD. Policy number: TI-SG-2026-55123. Plan: Business (single trip), Japan. "
     f"Period of insurance: {(INC - dt.timedelta(days=1)).isoformat()} to {(INC + dt.timedelta(days=6)).isoformat()}.\n"
     "Travel delay: SGD 100 for every 6 consecutive hours of delay, up to SGD 1,500. Claims must be notified within 30 days.\n"
     "24-hour emergency assistance overseas: +65 6322 2072.", 20),
    ("Singapore Airlines <noreply@singaporeair.example>", f"Delay certificate — SQ 636 on {INC}",
     f"Dear TAN WEI MING,\n\nSQ 636 Singapore–Tokyo Haneda on {INC} was scheduled to depart at 10:30 and departed at 17:45 "
     "because of the late arrival of the inbound aircraft. Our delay certificate is attached.\n\nSingapore Airlines", 1,
     [("delay_certificate_SQ636.pdf", open(os.path.join(PAGES, "delay_certificate_SQ636.pdf"), "rb").read())]),
]
APPLY = {"name": "#name", "passport": "#passport", "card": "#card", "exp": "#exp", "holder": "#holder"}
CLAIM = {"policy": "#policy", "name": "#name", "idno": "#idno", "date": "#date", "amount": "#amount", "submit": "#submit"}


def leaks(text: str) -> list[str]:
    f = text.replace(" ", "")
    return [x for x in (PASSPORT_NO, CARD_NO[1]) if x in f]


async def buy(u: U.User) -> None:
    seen = {"url": "", "ink": {}}

    async def look_then_hand_back(pg, request: str) -> None:
        # the user reads the request and looks at the application in the browser view; they do not press Pay
        seen["url"] = await u.view_url()
        await u.take_over()
        await u.view_picture(os.path.join(OUT, "insurance_application.png") if OUT else None)
        where = await u.layout("https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business&premium=47.00", APPLY)
        seen["ink"] = await u.view_ink(where)
        print(f"INFO takeover: {request!r} url {seen['url']!r} ink {seen['ink']}")

    a = await u.ask("I need travel insurance from FWD for my trip next month, just me — the flight booking is in my email. "
                    "Get me the cheapest plan with at least SGD 10,000 trip cancellation "
                    "and fill in the application with my card from the Vault; I'll press Pay myself.",
                    label="buy", on_takeover=look_then_hand_back, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        # the answer asked something before opening the quote page: the user answers once, in the same chat
        a = await u.ask("Everything is in my email (the Singapore Airlines booking) and my profile (name, birthday, contact). "
                        "Only the flights are paid so far, no pre-existing conditions. Please go ahead.",
                        label="buy follow-up", new_chat=False, on_takeover=look_then_hand_back, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("buy: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("buy: the Insurance & claims bot did it (the link to continue in it)", any("nsurance" in x.bot_link for x in answers),
          [x.bot_link for x in answers])
    check("buy: the user was asked to take over the browser for Pay", any(re.search(r"\bpay", t, re.I) for t in takeovers), takeovers)
    q = {k: v[0] for k, v in parse_qs(urlsplit(seen["url"]).query).items()}
    check("buy: the page handed over is FWD's application for the Business plan, not a payment confirmation",
          "fwd.com.sg" in seen["url"] and "fwd_sg_apply" in seen["url"] and q.get("plan") == "Business", seen["url"])
    ink = seen["ink"]
    check("buy: the live picture shows the passport and the card filled in",
          bool(ink) and ink.get("passport", 0) > 0.01 and ink.get("card", 0) > 0.01, ink)
    cards = await u.thread_cards()
    print(f"INFO buy: chat cards {[x[:200] for x in cards]}")
    check("buy: the answer names the plan and the amount to pay (Business, SGD 47)",
          re.search(r"business", a.text, re.I) and re.search(r"(SGD|S\$|\$)\s?47(\.00)?\b", a.text), a.text[:900])
    shown = await u.thread_text()   # everything the chat shows: the answers, the plan cards, the hand-over
    prem = [p for p in ("34", "47", "68") if re.search(rf"(SGD|S\$|\$)\s?{p}(\.00)?\b", shown)]
    check("buy: the plans are compared with their premiums (at least two of SGD 34 / 47 / 68)", len(prem) >= 2, (prem, shown[:600]))
    check("buy: the answer does not say it paid", not C.SUBMITTED.search(a.text), (C.SUBMITTED.search(a.text) or [""])[0])
    everything = await u.thread_text()
    check("buy: neither the passport nor the card number appears in the chat", not leaks(everything), leaks(everything))
    # afterwards the user opens the browser view: the page is still not a payment confirmation
    await u.page.goto(U.APP + "#browser")
    await u.page.wait_for_selector(".bview", timeout=20000)
    await u.page.wait_for_timeout(2500)
    tabs = await u.page.evaluate("() => [...document.querySelectorAll('.bview .btab')].map(b => b.title)")
    now = await u.view_url()
    print(f"INFO browser view afterwards: url {now!r} tabs {tabs}")
    check("buy: the browser view shows no payment confirmation afterwards (Pay never pressed)",
          "fwd_sg_done" not in now and not any("fwd_sg_done" in t for t in tabs), (now, tabs))


async def claim(u: U.User) -> None:
    seen = {"url": "", "ink": {}, "after": ""}

    async def check_and_submit(pg, request: str) -> None:
        # the user reads the request, looks at the filled-in claim form, and presses Submit in the live view themselves
        seen["url"] = await u.view_url()
        await u.take_over()
        await u.view_picture(os.path.join(OUT, "insurance_claim.png") if OUT else None)
        where = await u.layout("https://www.fwd.com.sg/claim-online/travel.html", CLAIM)
        seen["ink"] = await u.view_ink({k: v for k, v in where.items() if k != "submit"})
        print(f"INFO takeover: {request!r} url {seen['url']!r} ink {seen['ink']}")
        if "travel.html" in seen["url"] and "submit" in where:
            x, y, w, h = where["submit"]
            await u.click_view(x + w / 2, y + h / 2)
            for _ in range(10):
                seen["after"] = await u.view_url()
                if "claim_done" in seen["after"]:
                    break
                await pg.wait_for_timeout(1000)
            print(f"INFO after Submit the URL bar shows {seen['after']!r}")

    K.seed_mail(CLAIM_MAIL)
    a = await u.ask(f"My flight SQ 636 to Tokyo on {INC} was delayed by more than 7 hours. Please claim it on my FWD travel insurance — "
                    "the policy and the airline's delay certificate are in my email. Fill in FWD's claim form; I'll check it and submit it myself.",
                    label="claim", on_takeover=check_and_submit, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        a = await u.ask("All the details are in my email and my profile; nothing else to add. Please go ahead and fill in the claim form.",
                        label="claim follow-up", new_chat=False, on_takeover=check_and_submit, timeout=1200)
        answers.append(a)
    text = "\n".join(x.text for x in answers)
    takeovers = [t for x in answers for t in x.takeovers]
    check("claim: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("claim: the Insurance & claims bot did it", any("nsurance" in x.bot_link for x in answers), [x.bot_link for x in answers])
    check("claim: the user was asked to take over the browser to check and submit",
          any(re.search(r"submit|check|review", t, re.I) for t in takeovers), takeovers)
    check("claim: the page handed over is FWD's claim form, not its confirmation (nothing submitted for the user)",
          "fwd.com.sg" in seen["url"] and "travel.html" in seen["url"] and "claim_done" not in seen["url"], seen["url"])
    ink = seen["ink"]
    check("claim: the live picture shows the policy number, name, passport, date and amount filled in",
          bool(ink) and all(ink.get(k, 0) > 0.003 for k in ("policy", "name", "idno", "date", "amount")), ink)
    q = {k: v[0] for k, v in parse_qs(urlsplit(seen["after"]).query).items()}
    check("claim: the user's Submit reaches FWD's confirmation page", "claim_done" in seen["after"], seen["after"])
    if "claim_done" in seen["after"]:
        check("claim: the form held the policy number and the incident date from the mail",
              C.flat(q.get("policy", "")) == "TI-SG-2026-55123" and q.get("date", "").strip() == INC.isoformat(), q)
        check("claim: the form held the amount (SGD 100) and the passport number from the Vault",
              re.fullmatch(r"(SGD|S\$)?\s?100(\.00)?", q.get("amount", "").strip(), re.I) and C.flat(q.get("idno", "")) == PASSPORT_NO, q)
        check("claim: the delay certificate was attached to the form", "delay_certificate" in q.get("docs", ""), q)
    check("claim: the answer gives the delay (7 h 15 min) and the amount (SGD 100)",
          re.search(r"7\s?(h|hours?|hrs?)\s?(and\s)?15|7\.25|7:15", text, re.I) and re.search(r"(SGD|S\$)\s?100\b", text), text[:900])
    check("claim: the answer gives FWD's 30-day deadline as a date, or says the claim went in",
          C.date_in(text, D30) or re.search(r"submitted|CL-TR-2026-118204", text, re.I), (D30, text[:900]))
    everything = await u.thread_text()
    check("claim: the passport number never appears in the chat", PASSPORT_NO not in everything.replace(" ", ""), "")


async def main():
    async with U.User() as u:
        await buy(u)
        await claim(u)


U.setup(mail=BUY_MAIL, vault=False, profile={"name_en": "TAN WEI MING", "birthday": "1985-04-12", "email_personal": "weiming@example.com",
                                             "phone": "+65 9123 4567", "address_home": "10 Anson Road, Singapore 079903"})
K.vault_item("Visa Wei Ming", "card", {"number": CARD_NO[0], "expiry": "12/29", "cvc": "123", "holder": "TAN WEI MING"}, domains="fwd.com.sg")
K.vault_item("Passport Wei Ming", "id_document", {"number": PASSPORT_NO, "name": "TAN WEI MING", "expiry": "2031-05-01", "country": "SG"},
             domains="fwd.com.sg")
U.run(main)
