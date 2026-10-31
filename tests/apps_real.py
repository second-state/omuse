"""Notion and Slack with a real model, from the user's point of view (tests/user_common.py, tests/conn_common.py). The
user's Notion ("Lucas's Notion": a "Q4 Plan" page whose Goals list "Ship OMuse 0.2", and a "Tasks" database with "Write
launch post" (Doing) and "Fix login bug" (Done)) and their Slack (team Acme: #general with John's "old message"; the
OMuse bot is not in #sales) are connected (tests/fake_apps.py stands in for both). In Main's chat:
  1. notion page — "What are the goals in my Q4 Plan page in Notion?": the answer names "Ship OMuse 0.2".
  2. notion tasks — "Which tasks in my Notion Tasks database are still in progress?": "Write launch post", and
     "Fix login bug" is not called in progress.
  3. notion add — "Add a task 'Book the team offsite' … status Doing, due 15 November 2026": the user's Tasks database
     in Notion now has that row with that status and date.
  4. slack read — "What's the latest message in #general on Slack?": John's message.
  5. slack sales — "… the latest in #sales?": the answer tells the user to invite the bot to the channel.
  6. notion → slack — "Post the goals from my Q4 Plan in Notion to #general on Slack": an approval dialog shows the
     message before it goes out; the user approves; #general in the user's Slack has OMuse's message with the goal.
  7. denied — "Post 'Lunch is on me today!' in #general": the dialog shows that text, the user denies; nothing new in
     #general and the answer does not say it was posted.
  8. archive denied — "Archive my Q4 Plan page in Notion": the dialog asks first, the user denies; the page is still
     there in Notion.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh apps_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

POSTED = re.compile(r"\b(I('ve| have)? posted|has been posted|was posted|posted (it|the message|to))\b", re.I)


def answered(label: str, a: U.Answer) -> None:
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])


async def main():
    async with U.User() as u:
        # 1. a Notion page
        a = await C.ask(u, "What are the goals in my Q4 Plan page in Notion?", label="notion page")
        answered("notion page", a)
        check("notion page: the answer names the goal on the page (Ship OMuse 0.2)", a.mentions("Ship OMuse 0.2", "OMuse 0.2"), a.text[:500])

        # 2. a Notion database, filtered
        a = await C.ask(u, "Which tasks in my Notion Tasks database are still in progress?", label="notion tasks")
        answered("notion tasks", a)
        check("notion tasks: the answer names 'Write launch post'", a.mentions("Write launch post"), a.text[:500])
        fix = re.search(r"Fix login bug[^\n]*", a.text, re.I)
        check("notion tasks: 'Fix login bug' is not called in progress (it is done)", not fix or re.search(r"done|complete|finished", fix.group(0), re.I)
              or re.search(r"Fix login bug.{0,80}\b(done|complete)", a.text, re.I | re.S), a.text[:500])

        # 3. a row added to the database
        a = await C.ask(u, "Add a task 'Book the team offsite' to my Notion Tasks database, status Doing, due 15 November 2026.", label="notion add")
        answered("notion add", a)
        rows = C.notion_tasks()
        print(f"INFO Notion Tasks: {rows}")
        new = [r for r in rows if "offsite" in r["name"].lower()]
        check("notion add: the Tasks database in Notion has the new task", bool(new), rows)
        if new:
            check("notion add: … with status Doing and due 2026-11-15", new[-1]["status"] == "Doing" and new[-1]["due"].startswith("2026-11-15"), new[-1])

        # 4. Slack, a channel the bot is in
        a = await C.ask(u, "What's the latest message in #general on Slack?", label="slack read")
        answered("slack read", a)
        check("slack read: the answer quotes John's message ('old message')", a.mentions("old message"), a.text[:400])

        # 5. a channel the bot is not in
        a = await C.ask(u, "And what's the latest in #sales on Slack?", label="slack sales")
        answered("slack sales", a)
        check("slack sales: the answer says the bot must be invited to #sales (/invite or 'add the app')",
              a.mentions("/invite", "invite", "add the app", "add the bot", "not a member", "not in the channel", "isn't in", "is not in"), a.text[:400])

        # 6. read Notion, post to Slack — the user approves the message
        before = C.slack_channel()
        a = await C.ask(u, "Post the goals from my Q4 Plan in Notion to #general on Slack.", label="notion to slack")
        answered("notion to slack", a)
        check("notion to slack: an approval dialog showed the message before it was posted", any("0.2" in x or "Ship" in x for x in a.approvals), a.approvals)
        after = C.slack_channel()
        mine = [m for m in after[len(before):] if m["bot"]]
        print(f"INFO #general: {after}")
        check("notion to slack: #general has OMuse's new message with the goal (Ship OMuse 0.2)", any("0.2" in m["text"] for m in mine), after[-3:])

        # 7. the user denies a post
        before = C.slack_channel()
        a = await C.ask(u, "Post 'Lunch is on me today!' in #general on Slack.", label="slack denied", approve=False)
        answered("slack denied", a)
        check("slack denied: the approval dialog showed the message", any("Lunch is on me" in x for x in a.approvals), a.approvals)
        after = C.slack_channel()
        check("slack denied: nothing new in #general", len(after) == len(before), after[len(before):])
        check("slack denied: the answer does not say it was posted", not POSTED.search(a.text) or a.mentions("not posted", "didn't post", "did not post", "denied", "declined", "cancel"),
              a.text[:400])

        # 8. archiving a page asks first; the user denies
        a = await C.ask(u, "Archive my Q4 Plan page in Notion.", label="archive denied", approve=False)
        answered("archive denied", a)
        check("archive denied: the user was asked before the page was archived", bool(a.approvals), a.approvals)
        p = C.notion_page()
        check("archive denied: the Q4 Plan page is still there in Notion (not archived)", p["title"] == "Q4 Plan" and not p["archived"], p)


U.setup(vault=False, auto_approve=False)  # this suite checks the approval dialogs: the user turned "Auto-approve actions" off (the default approves everything)
C.connect_notion_and_slack()
U.run(main)
