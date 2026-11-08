"""What's next (the Main bot's whats-next skill) with a real model, from the user's point of view (tests/user_common.py).
A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the test inbox is connected as the user's mailbox (read-only:
OMUSE_E2E_IMAP_*; nothing is sent or deleted), no calendar is connected, the web searches are the real ones. What the
plan holds depends on the inbox, so the checks are smoke checks on what code shows, never on the model's words.

1. ask — the What's next bot's example, asked in Main: "What's next for me today?" Checked: Main hands it to the
   What's next bot; the
   block shows today's list as a table (the date, the Time / What / Where columns) and nothing asks to take over a
   browser or approve anything.
2. add — the user answers the question with a dinner to add ("Please add dinner at Burnt Ends in Singapore at 7pm
   today. Nothing else."). Checked: the block links the report (`/artifacts/whats-next/…`); signed in, the report
   opens and shows today's section with the dinner the user added; no daily report was asked for, so the block offers
   one ("Run it automatically?", agent/longtask.rs).
3. schedule — the user answers the offer: "Yes, every day at 6:30am." Checked: the block says it is scheduled at 06:30;
   the schedule stores the task's JSON (whats_next) and is in this chat. The suite deletes it at once: its run would
   mail the read-only test inbox.
The scheduled run (no question; the report emailed to the user) is unit-tested (agent/whatsnext/tests.rs): the test
inbox never sends. One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh whats_next_real"""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today().isoformat()


async def main():
    async with U.User() as u:
        a = await u.ask("What's next for me today?", label="ask", timeout=1200)
        check("ask: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("ask: Main hands the request to the What's next bot", "What's next" in a.bot_link, a.bot_link)
        check("ask: the block shows today's list as a table (Time / What / Where)",
              all(w in a.block for w in ("Today", "Time", "What", "Where")), a.block[:600])
        check("ask: the report is not made yet — the user is asked first", not any("/artifacts/whats-next/" in x for x in a.block_links),
              a.block_links)
        check("ask: nothing to approve and no takeover", not a.approvals and not a.takeovers, (a.approvals, a.takeovers))

        a = await u.ask("Please add dinner at Burnt Ends in Singapore at 7pm today. Nothing else.", label="add",
                        new_chat=False, timeout=1800)
        check("add: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        links = [x for x in a.block_links if "/artifacts/whats-next/" in x]
        check("add: the block links the report", bool(links), (a.block[:600], a.block_links))
        check("add: no takeover", not a.takeovers, a.takeovers)
        if links:
            r = await u.page.request.get(links[0])
            html = await r.text()
            print(f"INFO add: report {links[0]} → {r.status}, {len(html)} chars")
            check("add: signed in, the report opens with today's section", r.status == 200 and "<h2>Today</h2>" in html,
                  (r.status, html[:300]))
            check("add: the report has the dinner the user added", "Burnt Ends" in html, html[:2000])
        check("add: no daily report asked: the block offers one", a.shows("Run it automatically?"), a.block)

        a = await u.ask("Yes, every day at 6:30am.", label="schedule", new_chat=False, timeout=600)
        check("schedule: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        # the reply to the offer is routed by the planner: the bot answers it (task_schedule → "Scheduled"), or makes the
        # daily report from the words ("Every day: at 06:30") — either way one 06:30 schedule with the task's JSON
        check("schedule: the block says it runs every day at 06:30", (a.shows("Scheduled") or a.shows("Every day")) and "06:30" in a.block,
              a.block)
        mine = [x for x in U.c.get(U.B + "/api/schedules", headers=U.H).json()["schedules"] if "What's next" in x.get("name", "")]
        for x in mine:
            U.c.delete(f"{U.B}/api/schedules/{x['id']}", headers=U.H)   # never run: it would mail the test inbox
        print(f"INFO schedule: {[(x.get('name'), x.get('spec'), x.get('task')) for x in mine]}")
        check("schedule: one daily 06:30 schedule that stores the task's JSON",
              len(mine) == 1 and mine[0].get("spec") == "30 6 * * *" and (mine[0].get("task") or {}).get("tool") == "whats_next", mine)


U.setup(profile={"name_en": "JORDAN LU", "home_city": "Singapore", "home_country": "SG", "email_personal": A.EMAIL})
if not A.connect_mailbox():
    print("SKIP whats_next_real: no test inbox (OMUSE_E2E_IMAP_*) — What's next needs a mailbox")
    raise SystemExit(0)
U.run(main)
