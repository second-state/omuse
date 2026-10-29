"""Credit cards with a real model (see tests/real_common.py): two plain requests sent to the main bot, each handed to
the Credit cards bot.
  1. picker — which of the user's own cards (profile: Chase Sapphire Reserve; Citi Double Cash) to use for a flight:
     the credit-card-picker skill, an answer that names one of those cards with its value and the date the catalog's
     terms were verified, and no card number anywhere.
  2. claim — a missed United flight paid with the Sapphire Reserve (booking, charge and refund in mail): the
     csr-trip-claim skill (the Credit cards and Travel insurance bots both have it), the amount to claim = paid minus
     refunded, the 20-day and 90-day deadlines as dates, and Submit on the portal never pressed for the user (a
     takeover is handed back without pressing anything, and the task runs on to its answer).
One TIMING line per request (model time excluded).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_PLANNER=xai/grok-4.7 OMUSE_E2E_EXECUTOR=openai/gpt-6-luna \\
        tests/run_in_docker.sh cards_real"""
import datetime as dt
import json
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import real_common as R  # noqa: E402
from tests import skills_common as K  # noqa: E402

LOSS = dt.date.today() - dt.timedelta(days=3)
BOOKED = (dt.date.today() - dt.timedelta(days=40)).isoformat()
D20, D90 = LOSS + dt.timedelta(days=20), LOSS + dt.timedelta(days=90)
MAIL = [
    ("United Airlines <noreply@united.example>", f"Your eTicket receipt — Confirmation ABC123 — UA 837 SFO-NRT {LOSS}",
     f"Passenger LUCAS LU. UA 837 San Francisco (SFO) {LOSS} 11:05 → Tokyo Narita (NRT) 14:30 (+1). Fare USD 1,840.00, Basic Economy, "
     f"non-refundable. Paid with Visa ending 4321 on {BOOKED}.", 40),
    ("Chase <no.reply.alerts@chase.example>", "Your $1,840.00 transaction with UNITED AIRLINES",
     f"A $1,840.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on {BOOKED}.", 40),
    ("United Refunds <refunds@united.example>", "Refund processed — confirmation ABC123",
     "Taxes and fees of USD 86.40 have been refunded to Visa ending 4321. The Basic Economy fare of USD 1,753.60 is non-refundable "
     "under the fare rules.", 2),
]
CARD_NO = re.compile(r"\b(?:\d[ -]?){13,19}\b")


def loaded(info: dict, skill: str) -> bool:
    return any(x["name"] == "load_skill" and (x.get("args") or {}).get("name") == skill for x in info["calls"])


def date_in(text: str, d: dt.date) -> bool:
    """`d` written as a date: ISO, US or day-first, month spelled out or abbreviated."""
    forms = {d.isoformat(), f"{d:%B} {d.day}, {d.year}", f"{d:%b} {d.day}, {d.year}", f"{d.day} {d:%B} {d.year}",
             f"{d.day} {d:%b} {d.year}", f"{d.month}/{d.day}/{d.year}", f"{d:%m/%d/%Y}", f"{d:%b}. {d.day}, {d.year}"}
    return any(f.lower() in text.lower() for f in forms)


def resume(info: dict, rounds: int = 3, timeout: float = 900) -> dict:
    """The user hands the browser back after a takeover without pressing anything; the task runs on (approvals approved
    as in R.ask) to its end or the next takeover. Refreshes the tasks, calls and answer in `info`."""
    for _ in range(rounds):
        if not info["takeover"]:
            break
        print(f"INFO takeover: {json.dumps(R.task(info['takeover']).get('waiting'), ensure_ascii=False)[:300]}")
        info["takeover"] = None
        K.hand_back()
        tid, deadline = info["tasks"][0], time.time() + timeout
        while time.time() < deadline:
            t = R.task(tid)
            st = t.get("status")
            kid = (t.get("waiting") or {}).get("task_id") if st == "WAITING_BOT" else None
            kst = R.task(kid).get("status") if kid else None
            if "WAITING_APPROVAL" in (st, kst):
                R._approve({tid} | ({kid} if kid else set()), info["approvals"], info["excluded"])
            elif "WAITING_EXTERNAL" in (st, kst):
                info["takeover"] = kid if kst == "WAITING_EXTERNAL" else tid
                break
            elif st in R.TERMINAL:
                break
            time.sleep(0.4)
    info["t1"] = time.monotonic()
    info["main"] = R.task(info["tasks"][0])
    info["kid"] = R.task(info["tasks"][1]) if len(info["tasks"]) > 1 else {}
    info["calls"] = [dict(e["data"], task=x) for x in info["tasks"] for e in R.task(x).get("events", []) if e["type"] == "tool_call"]
    info["names"] = [x["name"] for x in info["calls"] if x["name"] != "ask_bot"]
    info["result"] = str(info["kid"].get("result") or info["main"].get("result") or "")
    print(f"INFO after hand-back ({info['bot']}): {info['result'][:700]!r}")
    return info


R.setup(mail=MAIL, profile={"custom:credit cards": "Chase Sapphire Reserve; Citi Double Cash", "custom:card market": "US",
                            "name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100",
                            "address_home": "12 Mission St, San Francisco, CA 94105"})

# 1. which card for a flight
info = R.ask("Which of my credit cards should I use for a US$1,200 flight booking directly on united.com?")
R.routed("picker", info, "cards")
R.finished("picker", info)
res = info["result"]
R.check("picker: the credit-card-picker skill was loaded", loaded(info, "credit-card-picker"), info["names"])
R.check("picker: the user's cards came from the profile", "profile_get" in info["names"] or "Sapphire Reserve" in json.dumps(info["calls"]), info["names"])
R.check("picker: the pick is one of the user's own cards", R.mentions(res, "Sapphire Reserve", "Double Cash"), res[:600])
R.check("picker: the answer gives the value per card (a dollar amount or points)", bool(re.search(r"(\$|USD|US\$)\s?\d|\d[\d,]*\s*(points|pts)", res)), res[:600])
R.check("picker: the answer carries the date the catalog terms were verified (2026-08)",
        bool(re.search(r"2026-08|Aug(ust)?\.? (\d{1,2}, )?2026|\d{1,2} Aug(ust)? 2026", res)), res[:900])
R.check("picker: no card number anywhere", not CARD_NO.search(res) and "4111" not in res, res[:300])
R.no_browser("picker", info)
R.timing("picker", info)

# 2. a Sapphire Reserve trip cancellation claim (the doctor's visit given up front, as the skill asks for it once)
info = R.ask(f"I came down with food poisoning and couldn't take my United flight to Tokyo on {LOSS}. I paid for it with my "
             f"Chase Sapphire Reserve — can you claim the card's trip cancellation insurance for me? I saw Dr. Amy Chen at "
             f"Mission Bay Clinic on {LOSS} and she gave me a letter saying I was unable to travel.", timeout=1500)
bot = info["bot"]
print(f"INFO claim handled by the {bot} bot")
R.check("claim: routed to the cards bot (or insurance, which shares csr-trip-claim)", bot in ("cards", "insurance"), (bot, info["route"]))
if bot in ("cards", "insurance"):
    R.routed("claim", info, bot)
went_takeover = info["takeover"] is not None
resume(info)
R.finished("claim", info, takeover=True)
R.timing("claim", info)
infos = [info]
if not (date_in(info["result"], D20) and date_in(info["result"], D90)) and info["main"].get("status") == "COMPLETED":
    # the bot stopped for a question: the user answers once in the same Main conversation
    info2 = R.ask("I don't have anything else to add — please go ahead with the claim as far as you can.", conv=info["conv_id"], timeout=1500)
    R.check("claim follow-up: back to the same bot conversation", info2["bot"] == bot and info2["kid_conv"] == info["kid_conv"],
            (info2["bot"], info2["kid_conv"], info["kid_conv"]))
    went_takeover = went_takeover or info2["takeover"] is not None
    resume(info2)
    R.finished("claim follow-up", info2, takeover=True)
    R.timing("claim-followup", info2)
    infos.append(info2)
res = "\n".join(i["result"] for i in infos)
calls = [x for i in infos for x in i["calls"]]
names = [n for i in infos for n in i["names"]]
approvals = [a for i in infos for a in i["approvals"]]
R.check("claim: the csr-trip-claim skill was loaded", any(x["name"] == "load_skill" and (x.get("args") or {}).get("name") == "csr-trip-claim"
                                                        for x in calls), names)
R.check("claim: booking, charge and refund were read from mail", any(n.endswith("gmail_search") for n in names), names)
R.check("claim: the amount to claim is paid minus refunded (1,753.60)", R.mentions(res, "1,753.60", "1753.60", "1,753.6", "1753.6"), res[:800])
R.check("claim: the 20-day notice deadline as a date", date_in(res, D20), (D20, res[-1200:]))
R.check("claim: the 90-day documents deadline as a date", date_in(res, D90), (D90, res[-1200:]))
clicks = [a for a in approvals if a["tool"] == "browser_click"]
entered = [x for x in calls if x["name"] == "browser_type" and (x.get("args") or {}).get("submit")]
R.check("claim: Submit was never pressed on the portal (no approved submit-like click, no form sent with Enter)", not clicks and not entered,
        (clicks, entered))
R.check("claim: no card number in the answer", not CARD_NO.search(res) and "4111" not in res, res[:300])
print(f"INFO claim: takeover requested: {went_takeover}; browser calls: {[n for n in names if n.startswith('browser_')]}")
R.finish()
