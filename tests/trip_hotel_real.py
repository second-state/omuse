"""Today's trip and a hotel request with a real model, from the user's point of view (tests/user_common.py,
tests/travel2_common.py). Replaces the scripted trip_today_e2e.py and hotel_requests_e2e.py.

The user's world: a Tokyo trip three weeks ahead (D). Their mailbox has the Singapore Airlines e-ticket (SQ 638 SIN → NRT
on D at 09:25, landing 17:15; back on SQ 637 on D+4), the Hotel Ginza Grand booking (confirmation 88812345, D to D+3,
free cancellation until D−2 18:00 JST, the hotel's address reservations@ginzagrand.example), a dinner at Sushi Hoshizora
on D+1 at 19:00 and a shop's newsletter. Their calendar (an iCal feed they subscribed to) has a client meeting in
Marunouchi on D at 16:30 Tokyo time — before the flight lands.

1. trip — in the Travel bot: "what have I booked for my Tokyo trip, is anything missing or clashing?". Checked: the answer
   names the flight, the hotel, the dinner and the calendar's meeting at its Tokyo time (16:30); it calls out the night of D+3 without a hotel and
   the meeting that starts before the flight lands; the newsletter is not in it; nothing asked the user to take over.
2. hotel — "ask the Ginza hotel for an early check-in on D and tell me the cancellation deadline; I'll send it myself".
   Checked: the answer gives the deadline as the mail has it (D−2, 18:00 JST); a new draft
   to reservations@ginzagrand.example about an early check-in with 88812345 is in the user's Drafts; nothing new in Sent.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh trip_hotel_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel2_common as T2  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

D = dt.date.today() + dt.timedelta(days=21)
D1, D3, D4, DL = (D + dt.timedelta(days=n) for n in (1, 3, 4, -2))
f = T2.day
MAIL = [
    ("Singapore Airlines <noreply@singaporeair.example>", f"Your e-ticket: SQ 638 SIN-NRT {f(D)}",
     f"Passenger LU/LUCAS MR. Booking reference 7HK2LM.\nSQ 638 departs Singapore (SIN) {f(D)} 09:25, arrives Tokyo Narita (NRT) "
     f"{f(D)} 17:15 (local time).\nReturn SQ 637 departs Tokyo Narita (NRT) {f(D4)} 11:05, arrives Singapore (SIN) 17:35.\n"
     "Economy, 1 checked bag of 30 kg.", 3),
    ("Hotel Ginza Grand <reservations@ginzagrand.example>", f"Booking confirmed: Hotel Ginza Grand {f(D)} - {f(D3)}",
     f"Dear Lucas Lu,\n\nYour booking is confirmed. Confirmation number 88812345.\nCheck-in {f(D)} from 15:00, check-out {f(D3)} by 11:00.\n"
     "Superior twin, JPY 26,000 per night, pay at the hotel.\n"
     f"Free cancellation until {f(DL)} 18:00 JST; afterwards the first night is charged.\n"
     "Hotel Ginza Grand, 1-1-1 Ginza, Chuo City, Tokyo. Contact: reservations@ginzagrand.example, +81 3 0000 5678.", 2),
    ("Sushi Hoshizora <book@hoshizora.example>", f"Reservation: Sushi Hoshizora {f(D1)} 19:00, 2 guests",
     f"Your table for 2 on {f(D1)} at 19:00 is confirmed. Sushi Hoshizora, 4-2-8 Ginza, Tokyo. Deposit JPY 18,000 charged.", 1),
    ("Shop <news@shop.com>", "Big sale today", "50% off everything this weekend only.", 0),
]


async def trip(u: U.User) -> None:
    await u.open_bot("Travel")
    a = await u.ask("What have I booked for my Tokyo trip? Is anything missing or clashing?", label="trip", new_chat=False)
    T2.answered("trip", a)
    check("trip: the flight (SQ 638), the hotel (Ginza Grand) and the dinner (Sushi Hoshizora) are in the answer",
          re.search(r"SQ\s?638", a.text) and a.mentions("Ginza Grand") and a.mentions("Hoshizora"), a.text[:1200])
    check("trip: the calendar's client meeting is in the answer", re.search(r"client meeting|Marunouchi|Mitsubishi Estate", a.text, re.I), a.text[:1200])
    check("trip: the night without a hotel (the night of D+3, before the D+4 flight home) is called out",
          T2.date_in(a.text, D3) and re.search(r"no hotel|without (a )?(hotel|bed|accommodation)|not covered|gap|missing|no accommodation|"
                                               r"no booking|unbooked|not booked|nowhere to stay", a.text, re.I), a.text[:1500])
    check("trip: the meeting at its Tokyo time (16:30, or 15:30 named as Singapore time)",
          re.search(r"(16:30|4:30\s?p\.?m\.?)\s?(JST|\(?Tokyo( time)?\)?)?\s?(–|-|to|until)", a.text, re.I)
          or re.search(r"15:30[^.\n]{0,25}(SGT|Singapore)", a.text), a.text[:1500])
    check("trip: the meeting that starts before the flight lands is called out",
          re.search(r"16:30|4:30\s?p\.?m", a.text, re.I) and re.search(r"17:15|5:15\s?p\.?m", a.text, re.I)
          and re.search(r"clash|conflict|before (the flight|you|your flight) (lands|land|arrive)|can.?t make|won.?t make|impossible|"
                        r"overlap|lands? after|too late|miss", a.text, re.I), a.text[:1500])
    check("trip: the shop's newsletter is not in the trip", not a.mentions("Big sale", "50% off"), a.text[:800])


async def hotel(u: U.User) -> None:
    d0, s0 = len(U.mailbox("Drafts")), len(U.mailbox("Sent"))
    await u.open_bot("Travel")
    a = await u.ask(f"Please ask the Ginza hotel for an early check-in on {f(D)} — my flight lands in the late afternoon but I'd like the "
                    "room as early as they can — and tell me the cancellation deadline. Just draft the email, I'll send it myself.",
                    label="hotel", new_chat=False, approve=T2.no_send)
    T2.answered("hotel", a)
    check("hotel: the cancellation deadline as the mail has it (D−2, 18:00 JST)",
          T2.date_in(a.text, DL) and re.search(r"18:00|6:00\s?p\.?m|6\s?p\.?m", a.text, re.I) and re.search(r"JST|Japan|Tokyo time", a.text),
          a.text[:900])
    drafts = U.mailbox("Drafts")[d0:]
    print(f"INFO hotel drafts: {[(d['to'], d['subject'], ' '.join(d['body'].split())[:300]) for d in drafts]}")
    check("hotel: a draft to the hotel's own address (reservations@ginzagrand.example) asks for an early check-in with 88812345",
          any("reservations@ginzagrand.example" in d["to"] and "88812345" in d["subject"] + d["body"]
              and re.search(r"earl(y|ier|iest)\b[^.]{0,25}check.?in|check.?in early|check in earl", d["subject"] + d["body"], re.I) for d in drafts),
          [(d["to"], d["subject"]) for d in drafts])
    sent = U.mailbox("Sent")
    check("hotel: nothing new in the user's Sent folder", len(sent) == s0, [(m["to"], m["subject"]) for m in sent[s0:]])


async def main():
    async with U.User() as u:
        await trip(u)
        await hotel(u)


U.setup(mail=MAIL, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com"})
T2.subscribe_calendar("lucas_trip", [
    ("Client meeting — Mitsubishi Estate", dt.datetime.combine(D, dt.time(16, 30)), dt.datetime.combine(D, dt.time(17, 30)), "Asia/Tokyo",
     "Marunouchi Building 21F, Tokyo"),
    ("Weekly sync", dt.datetime.combine(D - dt.timedelta(days=7), dt.time(10, 0)), dt.datetime.combine(D - dt.timedelta(days=7), dt.time(11, 0)),
     "Asia/Singapore", "Zoom"),
])
U.run(main)
