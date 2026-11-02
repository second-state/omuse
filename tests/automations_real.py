"""Goals with a real model, from the user's point of view (tests/user_common.py).
  With the test inbox connected (read-only; the goals are not run):
  1. goal — "Keep an eye on my inbox until John confirms the contract; check once a day, until 20 November."
     Automations lists the goal, Active.
  2. pause — a second goal ("…until the landlord replies about the rent"); the user presses "Pause": it shows Paused.
(Triggers on Slack / Notion and a goal achieved by a mail need the user's real accounts: not tested here.)
One TIMING line per chat request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh automations_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


async def main():
    async with U.User() as u:
        # the user's own mailbox is connected (a goal that watches the inbox needs one; the model rightly declines
        # without it): the test inbox, read-only — the goals are not run here
        if not A.connect_mailbox():
            print("SKIP automations_real: no test inbox (OMUSE_E2E_IMAP_* in the env file)")
            return
        # ---- 1. a goal
        await u.page.goto(U.APP + "#chat")
        a = await u.ask("Keep an eye on my inbox until John confirms the contract for the Q4 pilot. Check once a day, until 20 November 2026.",
                        label="goal")
        check("goal: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        await u.page.goto(U.APP + "#schedules")
        await u.page.wait_for_timeout(3000)
        goals = [" ".join(t.split()) for t in await u.page.locator(".goal-card").all_inner_texts()]
        print(f"INFO goals: {goals}")
        goal = next((g for g in goals if re.search(r"John|contract", g, re.I)), "")
        check("goal: Automations lists the goal about John's contract, Active", bool(goal) and "Active" in goal, goals)

        # ---- 2. pause a goal
        await u.page.goto(U.APP + "#chat")
        a = await u.ask("Keep an eye on my inbox until my landlord replies about the rent renewal. Check once a day.", label="goal 2")
        await u.page.goto(U.APP + "#schedules")
        await u.page.wait_for_timeout(3000)
        g = u.page.locator(".goal-card", has_text=re.compile("landlord|rent", re.I)).first
        check("pause: Automations lists the landlord goal", await g.count() > 0, [" ".join(t.split())[:120] for t in await u.page.locator(".goal-card").all_inner_texts()])
        if await g.count():
            await g.locator("button", has_text="Pause").click()
            await u.page.wait_for_timeout(2500)
            g = u.page.locator(".goal-card", has_text=re.compile("landlord|rent", re.I)).first
            txt = " ".join((await g.inner_text()).split())
            check("pause: the goal shows Paused", "Paused" in txt, txt)


U.setup(vault=False)
U.run(main)
