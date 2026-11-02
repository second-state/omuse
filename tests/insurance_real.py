"""FWD travel insurance in Singapore with a real model, on FWD Singapore's REAL site, from the user's point of view
(tests/user_common.py, tests/claims_common.py). The test stops at the first step that is the user's and never presses
it: nothing is paid, declared or submitted. The profile has the user's name, birthday and contact; the Vault has their
NRIC (for fwd.com.sg; clearly fake test values: S1234567D is the well-known dummy NRIC). "Auto-approve browser actions"
is on (a fresh install).

1. buy — the user asks Main's chat for the cheapest FWD plan with at least SGD 10,000 trip cancellation for next month's
   Tokyo trip (the flights in the request); Pay is theirs. On the real site code fills the quote (Vuetify selects by
   label, the date pickers), reads the plans from the summary, picks one by the rule and presses its button; FWD then
   asks its eligibility statements — the user's. Checked from the user's side: the Insurance & claims bot answers; the
   user is asked to take over on FWD's quote page; the block code appends lists the plans FWD quoted (real prices) and the
   chosen one is the cheapest of those with at least SGD 10,000 trip cancellation (checked against the same block's
   figures, which are the live page's); the NRIC never shows in the chat; afterwards the browser view is still on FWD's
   site (Pay never pressed).
2. claim — the user gives the policy number (PNTR20…, FWD's format; fake) and the delay (SQ 636 two days ago, 10:30 →
   17:45) and asks to claim it. On the real site code fills step 1 of FWD's online claim (no sign-in) and hands it over:
   "next" checks the policy on FWD's server, so the rest is the user's. Checked: the takeover on FWD's claim page
   (nothing submitted; no per-field value checks on the handed-over page); the block gives the delay (7 h 15 min), the amount (SGD 100) and FWD's 30-day deadline as
   a date; the NRIC never shows in the chat.
One TIMING line per request; the live pictures go to OMUSE_E2E_OUT when it is set. In run_in_docker.sh's REAL_SITES
(one at a time).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh insurance_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
D1, D2 = (TODAY + dt.timedelta(days=24)).isoformat(), (TODAY + dt.timedelta(days=31)).isoformat()
INC = TODAY - dt.timedelta(days=2)
D30 = INC + dt.timedelta(days=30)
POLICY = "PNTR20123456"
NRIC = "S1234567D"
OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
FWD = "fwd.com.sg"


def quoted_plans(block: str) -> dict[str, tuple[float, float]]:
    """The plans the block code appended lists ("Plans quoted: Premium — SGD 42.52 (medical SGD 200,000, cancellation
    SGD 7,500, …); …"): plan → (premium, trip cancellation)."""
    line = next((ln for ln in block.splitlines() if "Plans quoted:" in ln), "")
    out = {}
    for part in line.split("Plans quoted:", 1)[-1].split(";"):
        m = re.search(r"(\w+) — SGD ([\d,]+\.\d\d)", part)
        c = re.search(r"cancellation SGD ([\d,]+)", part)
        if m and c:
            out[m.group(1)] = (float(m.group(2).replace(",", "")), float(c.group(1).replace(",", "")))
    return out


async def buy(u: U.User) -> None:
    seen = {"url": ""}

    async def look_then_hand_back(pg, request: str) -> None:
        # the user reads the request and looks at FWD's page; they answer no statement and press nothing
        seen["url"] = await u.view_url()
        await u.view_picture(os.path.join(OUT, "insurance_quote.png") if OUT else None)
        print(f"INFO takeover: {request!r} url {seen['url']!r}")

    a = await u.ask(f"I need travel insurance from FWD for my trip to Tokyo next month, just me: SQ 636 Singapore to Tokyo Haneda on {D1}, "
                    f"back on SQ 637 on {D2}. Get me the cheapest plan with at least SGD 10,000 trip cancellation; "
                    "I'll answer FWD's questions and pay myself.",
                    label="buy", on_takeover=look_then_hand_back, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        # the answer asked something before opening the quote page: the user answers once, in the same chat
        a = await u.ask("Everything else is in my profile (name, birthday, contact). Only the flights are paid so far. Please go ahead.",
                        label="buy follow-up", new_chat=False, on_takeover=look_then_hand_back, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("buy: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("buy: the Insurance & claims bot did it (the link to continue in it)", any("nsurance" in x.bot_link for x in answers),
          [x.bot_link for x in answers])
    check("buy: the user was asked to take over the browser on FWD's quote page (the statements and Pay are theirs)",
          bool(takeovers) and f"{FWD}/travel-insurance/online-quote" in seen["url"], (takeovers, seen["url"]))
    block = "\n".join(x.block for x in answers)
    plans = quoted_plans(block)
    chosen = re.search(r"Chosen: (\w+): SGD ([\d,]+\.\d\d)", block)
    print(f"INFO buy: plans {plans}; chosen {chosen.group(0) if chosen else None}")
    check("buy: the block lists the plans FWD quoted, with their prices and trip cancellation", len(plans) >= 2, block)
    ok = [p for p, (_, c) in plans.items() if c >= 10_000]
    best = min(ok, key=lambda p: plans[p][0]) if ok else ""
    check("buy: the chosen plan is the cheapest quoted with at least SGD 10,000 trip cancellation (its price as quoted)",
          bool(chosen) and chosen.group(1) == best and float(chosen.group(2).replace(",", "")) == plans[best][0],
          (chosen.group(0) if chosen else None, plans))
    everything = await u.thread_text()
    check("buy: the NRIC never appears in the chat", NRIC not in everything, "")
    # afterwards the user opens the browser view: still FWD's page, Pay never pressed
    await u.page.goto(U.APP + "#browser")
    await u.page.wait_for_selector(".bview", timeout=20000)
    await u.page.wait_for_timeout(2500)
    now = await u.view_url()
    print(f"INFO browser view afterwards: url {now!r}")
    check("buy: the browser view is still on FWD's site afterwards (Pay never pressed)", FWD in now, now)


async def claim(u: U.User) -> None:
    seen = {"url": ""}

    async def look_then_hand_back(pg, request: str) -> None:
        # the user reads the request and looks at the filled-in step 1; "next" and Submit are theirs, never pressed here
        seen["url"] = await u.view_url()
        await u.view_picture(os.path.join(OUT, "insurance_claim.png") if OUT else None)
        print(f"INFO takeover: {request!r} url {seen['url']!r}")

    a = await u.ask(f"My flight SQ 636 to Tokyo on {INC} was scheduled at 10:30 and left at 17:45. Please claim the delay on my FWD travel "
                    f"insurance, policy {POLICY} (SGD 100 for every 6 hours of delay; claims within 30 days). Fill in FWD's claim form; "
                    "I'll check it and submit it myself.", label="claim", on_takeover=look_then_hand_back, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        a = await u.ask("All the other details are in my profile; nothing else to add. Please go ahead and fill in the claim form.",
                        label="claim follow-up", new_chat=False, on_takeover=look_then_hand_back, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("claim: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("claim: the Insurance & claims bot did it", any("nsurance" in x.bot_link for x in answers), [x.bot_link for x in answers])
    check("claim: the user was asked to take over the browser on FWD's claim page (next and Submit are theirs)",
          bool(takeovers) and f"{FWD}/claim-online/" in seen["url"], (takeovers, seen["url"]))
    block = "\n".join(x.block for x in answers)
    check("claim: the block code appends after the answer gives the delay (7 h 15 min) and the amount (SGD 100)",
          "delayed 7 h 15 min" in block and "Claim amount: SGD 100.00" in block, (block, a.text[:300]))
    check("claim: the block code appends after the answer gives FWD's 30-day deadline as a date",
          C.date_in(block, D30), (D30, block))
    everything = await u.thread_text()
    check("claim: the NRIC never appears in the chat", NRIC not in everything, "")


async def main():
    async with U.User() as u:
        await buy(u)
        await claim(u)


U.setup(vault=False, profile={"name_en": "TAN WEI MING", "birthday": "1985-04-12", "email_personal": "weiming@example.com",
                              "phone": "+65 9123 4567", "address_home": "10 Anson Road, Singapore 079903"})
U.vault_item("NRIC Wei Ming", "id_document", {"number": NRIC, "name": "TAN WEI MING", "country": "SG"}, domains=FWD)
U.run(main)
