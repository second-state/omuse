"""Flight disruption and delayed baggage with a real model, from the user's point of view (tests/user_common.py,
tests/claims_common.py). Singapore Airlines' site and WorldTracer are stand-ins at their real addresses
(tests/https_standins.py: https://www.singaporeair.com — flight status, "Delays and cancellations", baggage services;
https://wtrweb.worldtracer.aero — baggage tracing); a web search about them finds those pages (tests/fake_apps.py).
The mailbox has a travel insurance policy from TravelSure (TI-2026-77: SGD 100 for every full 6 hours of delay with the
airline's delay certificate; baggage delay SGD 150 after 6 hours with the PIR and receipts; baggage loss up to SGD 3,000).

1. delay — the user's SQ 638 Singapore → Tokyo Narita today at 09:25 (the e-ticket in mail). The status page says it is
   delayed to 17:10 (7 h 45 min). The user asks whether it is delayed, the options, what insurance and the airline give,
   and whether they can get their money back; drafts only. Checked: the Insurance & claims bot answers; the answer has
   the new time (17:10) next to the old (09:25), the insurance benefit (SGD 100, the delay certificate) and the refund
   right from the airline's page (a refund to the original form of payment); a draft is in the user's Drafts (the
   certificate request to the airline or the claim to the insurer) about SQ 638; nothing in Sent; nothing submitted.
2. baggage — the bag did not arrive in Tokyo on yesterday's SQ 638; the PIR file is NRTSQ12345; the hotel booking is in
   mail; the user bought essentials for JPY 9,850. The user asks what to do, the bag's status, the expenses claim drafted
   (not sent) and a check on the bag. Checked: the Insurance & claims bot answers; the answer has the tracing status from
   WorldTracer ("tracing continues", not located), the 21-day rule, the policy's requirements (PIR, receipts) and SGD 150;
   a draft about NRTSQ12345 is in Drafts; nothing in Sent; the Automations page lists a check on the bag (or the answer
   says one was set up).
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh claims_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
YDAY = TODAY - dt.timedelta(days=1)
POLICY = ("TravelSure <policy@travelsure.example>", "Your travel insurance policy TI-2026-77",
          f"Dear TAN WEI MING,\n\nPolicy TI-2026-77 covers your trip to Japan from {(YDAY - dt.timedelta(days=1)).isoformat()} to "
          f"{(TODAY + dt.timedelta(days=10)).isoformat()}.\n- Travel delay: SGD 100 for every full 6 hours of delay; the airline's written "
          "delay certificate and your boarding pass are required.\n- Baggage delay: SGD 150 after 6 hours, against the airline's Property "
          "Irregularity Report (PIR) and receipts for essentials.\n- Baggage loss: up to SGD 3,000, SGD 500 per item.\n"
          "Claims: claims@travelsure.example. 24-hour assistance: +65 6000 0000.\n\nTravelSure", 6)
DELAY_MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", f"Your e-ticket: SQ 638 SIN-NRT {TODAY}",
     f"Passenger TAN WEI MING. Booking reference 7HK2LM. SQ 638 departs Singapore (SIN) {TODAY} 09:25, arrives Tokyo Narita (NRT) 17:15. "
     "Economy, paid SGD 742.00 with Visa ending 1111.", 12),
    POLICY,
]
BAG_MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", f"Your e-ticket: SQ 638 SIN-NRT {YDAY}",
     f"Passenger TAN WEI MING. Booking reference 7HK2LM. SQ 638 departs Singapore (SIN) {YDAY} 09:25, arrives Tokyo Narita (NRT) 17:15. "
     "1 checked bag, 30 kg.", 12),
    ("Hotel Ginza Grand <stay@ginzagrand.example>", f"Booking confirmed: Hotel Ginza Grand {YDAY}",
     f"Confirmation GG-88120. Check-in {YDAY}, 5 nights. 1-1-1 Ginza, Chuo City, Tokyo.", 20),
    POLICY,
]


def no_send(dialog: str) -> bool:
    """The user asked for drafts: they deny a dialog about sending mail, by the tool it names (auto-approve covers only
    the browser)."""
    yes = U.tool_of(dialog) not in U.MAIL_SEND_TOOLS
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


async def look(pg, why: str) -> None:
    url = await pg.locator(".bview input.url").input_value() if await pg.locator(".bview input.url").count() else ""
    print(f"INFO takeover asked: {why[:200]!r}; the browser shows {url!r}")


async def delay(u: U.User) -> None:
    from tests import skills_common as K
    K.seed_mail(DELAY_MAIL)
    d0, s0 = len(C.mail_full("Drafts")), len(C.mail_full("Sent"))
    a = await u.ask("Is my Singapore Airlines flight SQ 638 to Tokyo today delayed? What are my options, what can I claim from my travel "
                    "insurance and the airline, and can I get my money back instead? Prepare any emails I need as drafts — don't send anything.",
                    label="delay", approve=no_send, on_takeover=look, timeout=1200)
    check("delay: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("delay: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
    check("delay: the new departure time from the airline's status page (17:10) next to the old one (09:25)",
          re.search(r"17:10|5:10\s?p\.?m", a.text, re.I) and re.search(r"0?9:25", a.text), a.text[:900])
    check("delay: the insurance benefit (SGD 100) and the delay certificate it needs",
          re.search(r"(SGD|S\$)\s?100\b", a.text) and re.search(r"certificate", a.text, re.I), a.text[:900])
    check("delay: the refund right from the airline's page (a refund to the original form of payment)",
          re.search(r"refund", a.text, re.I) and re.search(r"original (form of )?payment", a.text, re.I), a.text[:1200])
    said = C.SUBMITTED.search(a.text) or C.SENT.search(a.text)
    check("delay: nothing submitted or sent for the user", not said,
          a.text[max(0, said.start() - 160):said.end() + 80] if said else "")
    drafts = C.mail_full("Drafts")[d0:]
    print(f"INFO delay drafts: {[(d['to'], d['subject'], ' '.join(d['body'].split())[:200]) for d in drafts]}")
    check("delay: a draft about SQ 638 is in the user's Drafts (to the airline or the insurer)",
          any(re.search(r"SQ\s?638|7HK2LM|TI-2026-77", d["subject"] + d["body"]) for d in drafts), [(d["to"], d["subject"]) for d in drafts])
    sent = C.mail_full("Sent")
    check("delay: nothing new in the user's Sent folder", len(sent) == s0, [(m["to"], m["subject"]) for m in sent[s0:]])


async def baggage(u: U.User) -> None:
    from tests import skills_common as K
    K.seed_mail(BAG_MAIL)
    d0, s0 = len(C.mail_full("Drafts")), len(C.mail_full("Sent"))
    a = await u.ask("My checked bag didn't arrive in Tokyo on SQ 638 yesterday. I filed a report at Narita, file reference NRTSQ12345, "
                    "and I've bought toiletries and a change of clothes for JPY 9,850 (I kept the receipts). What should I do? Check where my bag is, "
                    "draft my expenses claim (don't send it), and keep an eye on the bag for me.",
                    label="baggage", approve=no_send, on_takeover=look, timeout=1200)
    check("baggage: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("baggage: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
    check("baggage: the bag's status from the tracing page (tracing continues, not located yet)",
          re.search(r"tracing continues|still being traced|not (yet )?(been )?(located|found)|tracing (is )?(still )?(in progress|ongoing)", a.text, re.I),
          a.text[:900])
    check("baggage: the 21-day rule", re.search(r"21[ -]days?", a.text), a.text[:900])
    check("baggage: the policy's baggage-delay benefit and what it needs (SGD 150, PIR, receipts)",
          re.search(r"(SGD|S\$)\s?150\b", a.text) and re.search(r"PIR|Property Irregularity", a.text) and re.search(r"receipt", a.text, re.I),
          a.text[:900])
    check("baggage: nothing sent for the user", not C.SENT.search(a.text), a.text[:300])
    drafts = C.mail_full("Drafts")[d0:]
    print(f"INFO baggage drafts: {[(d['to'], d['subject'], ' '.join(d['body'].split())[:200]) for d in drafts]}")
    check("baggage: a draft of the expenses claim (file NRTSQ12345) is in the user's Drafts",
          any("NRTSQ12345" in C.flat(d["subject"] + d["body"]) for d in drafts), [(d["to"], d["subject"]) for d in drafts])
    sent = C.mail_full("Sent")
    check("baggage: nothing new in the user's Sent folder", len(sent) == s0, [(m["to"], m["subject"]) for m in sent[s0:]])
    autos = await C.page_text(u, "schedules")
    print(f"INFO Automations page: {' '.join(autos.split())[:600]}")
    check("baggage: a check on the bag is on the Automations page",
          re.search(r"NRTSQ12345|baggage|bag", autos, re.I) is not None, autos[:300])


async def main():
    async with U.User() as u:
        await delay(u)
        await baggage(u)


U.setup(mail=DELAY_MAIL, profile={"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567",
                                  "address_home": "10 Anson Road, Singapore 079903"})
U.run(main)
