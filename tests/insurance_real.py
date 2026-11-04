"""FWD travel insurance in Singapore with a real model, on FWD Singapore's REAL site, from the user's point of view
(tests/user_common.py, tests/claims_common.py). The test stops at the first step that is the user's and never presses
it: nothing is paid, declared or submitted. The profile has the user's name, birthday and contact; the Vault has their
NRIC (for fwd.com.sg; clearly fake test values: S1234567D is the well-known dummy NRIC). "Auto-approve browser actions"
is on (a fresh install).

1. buy — the user asks the short example ("Buy me FWD travel insurance for my Tokyo trip.") and answers
   the model's question with the trip (SQ 636 in 3 weeks, back a week later) and the rule (the cheapest with at least
   SGD 10,000 trip cancellation). On the real site code fills the quote, reads the plans, picks one by the rule and
   saves the quote with FWD (name, email, mobile from the profile, the NRIC from the Vault). Checked from the user's
   side: the Money bot answers; no takeover; the block lists the plans FWD quoted (real prices) and the
   chosen one is the cheapest of those with at least SGD 10,000 trip cancellation; the block links FWD's saved quote,
   and on another device (a fresh browser) that link and the mobile number open FWD's application at its declaration
   (nothing confirmed or paid); the NRIC never shows in the chat.
2. claim — the user asks the short example ("My flight was delayed. Please claim it on my FWD travel insurance.") and
   answers the model's question with the delay (SQ 636 two days ago, 10:30 → 17:45), the trip and the policy number
   (PNTR2012-34567890, FWD's format; fake). Code fills every step of FWD's real online claim and asks the user's
   approval to submit; the user DECLINES (a real insurer never gets a test claim), and the form is closed. Checked: no
   takeover; the approval dialog names the policy, the delay, the amount and FWD's declaration; the block gives the
   delay (7 h 15 min), the amount (SGD 100), FWD's 30-day deadline as a date and that nothing was sent;
   the NRIC never shows in the chat.
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

    # the Money bot's example in the UI (once in bots/insurance.md), word for word: a short request — the model asks for
    # what the tool's JSON needs, and the user answers once, in the same chat
    a = await u.ask("Buy me FWD travel insurance for my Tokyo trip.",
                    label="buy", on_takeover=look_then_hand_back, timeout=1200)
    answers = [a]
    for reply in ["Just me: SQ 636 to Tokyo Haneda in 3 weeks, back a week later. Find the cheapest plan with at least SGD 10,000 trip cancellation.",
                  "Everything else is in my profile (name, birthday, contact). Only the flights are paid so far. Please go ahead."]:
        if "Plans quoted" in a.block or a.timed_out:
            break
        a = await u.ask(reply, label="buy follow-up", new_chat=False, on_takeover=look_then_hand_back, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("buy: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("buy: the Money bot did it (the link to continue in it)", any("Money" in x.bot_link for x in answers),
          [x.bot_link for x in answers])
    # no takeover (the user, 2026-11-02): the quote is saved with FWD and the block links FWD's own page for it
    check("buy: the user was not asked to take over the server's browser", not takeovers, takeovers)
    block = "\n".join(x.block for x in answers)
    links = [x for y in answers for x in y.block_links if "/travel-insurance/emailRedirect.html?hash=" in x]
    check("buy: the block links the saved quote on FWD's site", bool(links), [y.block_links for y in answers])
    if links:
        # the user opens it on their own device (a browser that never saw the quote), types their mobile number and
        # lands in FWD's application — its eligibility statements first; nothing is confirmed or paid here
        other = await u._br.new_context(viewport={"width": 1280, "height": 900})
        pg = await other.new_page()
        await pg.goto(links[0], wait_until="domcontentloaded")
        await pg.wait_for_selector("#mobile-input", timeout=60000)
        await pg.fill("#mobile-input", "91234567")
        await pg.click("#load-quote-page-button")
        await pg.wait_for_timeout(12000)
        text = await pg.locator("body").inner_text()
        print(f"INFO buy: the saved quote on another device: {pg.url} {' '.join(text.split())[:300]}")
        if OUT:
            await pg.screenshot(path=os.path.join(OUT, "insurance_saved_quote.png"), full_page=True)
        check("buy: on another device the link and the mobile number open FWD's application at its declaration",
              "/travel-insurance/quote" in pg.url and "Declaration" in text and "Please confirm the following" in text,
              (pg.url, text[:300]))
        await other.close()
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


async def claim(u: U.User) -> None:
    # the user, 2026-11-03: code fills every step of FWD's online claim and submits it after the user's approval in the
    # app. Here the user DECLINES: a real insurer is never sent a test claim; the form is closed unsubmitted.
    seen: dict = {"dialogs": []}

    def decline_submit(dialog: str) -> bool:
        if U.tool_of(dialog) == "submit_confirm":
            seen["dialogs"].append(dialog)
            return False
        return True

    # the Money bot's example in the UI (once in bots/insurance.md), word for word: a short request — the model asks for
    # the flight, the times, the trip and the policy; the user answers in the same chat ("two days ago" = INC)
    a = await u.ask("My flight was delayed. Please claim it on my FWD travel insurance.", label="claim",
                    approve=decline_submit, timeout=1200)
    answers = [a]
    # the documents FWD requires (the itinerary, the boarding pass) and the airline's letter: clearly fake specimens,
    # printed by the test's own browser — they go into FWD's page only, never submitted
    docs = []
    for name, title in [("itinerary.pdf", "E-TICKET ITINERARY"), ("boarding_pass.pdf", "BOARDING PASS SQ 636"),
                        ("delay_letter.pdf", "FLIGHT DELAY CERTIFICATE SQ 636")]:
        pg = await u.ctx.new_page()
        await pg.set_content(f"<h1>SPECIMEN — {title}</h1><p>TEST DOCUMENT, NOT A REAL CLAIM. TAN WEI MING.</p>")
        path = os.path.join("/tmp", name)
        await pg.pdf(path=path)
        await pg.close()
        docs.append(path)
    # the trip has ended (FWD's online claim takes it only then: its return date's calendar has no future day)
    for i, reply in enumerate([f"It was SQ 636 to Tokyo two days ago, scheduled at 10:30, and it left at 17:45. I left Singapore on "
                  f"{INC} and came back yesterday, {TODAY - dt.timedelta(days=1)}; the trip was to Japan. My policy is "
                  "PNTR2012-34567890. Attached are my itinerary, my boarding pass and the airline's delay letter. "
                  "Please file the claim for me.",
                  "All the other details are in my profile; nothing else to add. Please go ahead and file the claim."]):
        if seen["dialogs"] or a.timed_out:
            break
        if i == 0:
            chips = await u.attach(*docs)
            check("claim: the documents are attached", len(chips) == 3 and not any(c["bad"] for c in chips), chips)
        a = await u.ask(reply, label="claim follow-up", new_chat=False, approve=decline_submit, timeout=1200)
        answers.append(a)
    takeovers = [t for x in answers for t in x.takeovers]
    check("claim: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("claim: the Money bot did it", any("Money" in x.bot_link for x in answers), [x.bot_link for x in answers])
    check("claim: no takeover — code fills FWD's claim form itself", not takeovers, takeovers)
    dialog = seen["dialogs"][0] if seen["dialogs"] else ""
    print(f"INFO claim: approval dialog {' '.join(dialog.split())[:600]!r}")
    check("claim: the user is asked to approve the submission, with the policy, the delay, the amount and FWD's declaration",
          "PNTR2012-34567890" in dialog and "7 h 15 min" in dialog and "SGD 100.00" in dialog and "true, complete and accurate" in dialog,
          dialog[:600])
    block = "\n".join(x.block for x in answers)
    check("claim: the block code appends after the answer gives the delay (7 h 15 min) and the amount (SGD 100)",
          "delayed 7 h 15 min" in block and "Claim amount: SGD 100.00" in block, (block, a.text[:300]))
    check("claim: the block code appends after the answer gives FWD's 30-day deadline as a date",
          C.date_in(block, D30), (D30, block))
    check("claim: declined, so nothing was sent to FWD (the block says so)", "nothing was sent to FWD" in block, block)
    # (no browser-view check: the view shows one tab of several — the purchase's — and the approval dialog above, with
    # every fact of the filled form, already shows from the user's side that code reached FWD's declaration)
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
