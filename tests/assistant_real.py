"""Everyday assistant features with a real model, from the user's point of view (tests/user_common.py,
tests/shop_common.py): memory and a scheduled job.

1. remember — "Please remember: I'm vegetarian and I don't drink alcohol." Checked: the answer came and the Memory
   page lists it.
2. learned — in another chat the user mentions in passing their shoe size (EU 42) and how much they run while asking
   about running shoes. Checked: within two minutes the Memory page shows the shoe size that OMuse picked up by itself.
3. schedule — "Every weekday at 8 am, summarise my new emails for me." Checked: Automations lists the job at 08:00 on
   weekdays; the user presses "Run now" and "Results". The Tasks tab lists the job with its next run and a link to its
   chat; deleting that chat removes the job from the Tasks tab. (What the run says is the model's and the mailbox's, not checked.)
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh assistant_real"""
import asyncio
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import arrival_common as A  # noqa: E402
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402



async def memory_page(u: U.User) -> str:
    txt = await S.app_page(u, "memory")
    await u.page.goto(U.APP + "#chat")
    await u.page.wait_for_selector("#chatInput")
    return txt


def facts(page: str) -> str:
    """The Memory page's remembered things (from the learned / facts cards on), without the profile form above."""
    i = min([x for x in (page.find("Learned"), page.find("Facts"), page.find("Long-term")) if x >= 0] or [0])
    return " ".join(page[i:].split())[:1500]


async def schedule_results(u: U.User, timeout: float = 600) -> str:
    """After Run now: the user opens the job's Results and waits for the run's answer."""
    pg = u.page
    await S.press_on_card(u, "8", "Results")
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        n = await pg.locator(".msg.assistant:not(.live) .bubble").count()
        live = await pg.locator(".msg.assistant.live").count()
        if n and not live:
            return await pg.locator(".msg.assistant:not(.live) .bubble").last.inner_text()
        await pg.wait_for_timeout(3000)
        if not n:   # the conversation fills in when the run writes to it: look again
            await pg.goto(U.APP + "#schedules")
            await pg.wait_for_timeout(1500)
            await S.press_on_card(u, "8", "Results")
    return ""


async def main():
    async with U.User() as u:
        # ---- 1. remember
        a = await u.ask("Please remember: I'm vegetarian and I don't drink alcohol.", label="remember")
        mem = await memory_page(u)
        check("remember: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("remember: the Memory page lists that the user is vegetarian", "vegetarian" in mem.lower(), mem[:1500])

        # ---- 2. learned on the way
        a = await u.ask("I wear EU size 42 and run about 20 km a week. What should I look for in my next pair of running shoes?", label="learned")
        mem = ""
        for _ in range(12):
            mem = await memory_page(u)
            if re.search(r"\b42\b", mem):
                break
            await u.page.wait_for_timeout(10000)
        print(f"INFO Memory page (facts): {facts(mem)!r}")
        check("learned: the Memory page shows the shoe size (42), picked up without being asked", re.search(r"\b42\b", mem), facts(mem))

        # ---- 3. a scheduled job — on the user's own inbox (the test inbox, read-only: the run summarises in the chat,
        # nothing is sent; without a mailbox the model may rightly decline)
        if not A.connect_mailbox():
            print("SKIP the scheduled job: no test inbox (OMUSE_E2E_IMAP_* in the env file)")
            return
        a = await u.ask("Every weekday at 8 am, summarise my new emails for me.", label="schedule")
        card = await S.automation_card(u, "8")
        print(f"INFO Automations card: {card!r}")
        check("schedule: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("schedule: Automations lists the job at 08:00", bool(card) and re.search(r"\b0?8:00\b|\b8\s?(am|a\.m\.)", card, re.I), card)
        check("schedule: … on weekdays", re.search(r"weekday|Mon(day)?\s?(–|-|to)\s?Fri|1-5", card, re.I), card)
        if card:
            await S.press_on_card(u, "8", "Run now")
            await schedule_results(u)
            await u.page.goto(U.APP + "#chat")
            await u.page.wait_for_selector("#chatInput")

        # ---- 3b. the Tasks tab: the job, its next run and its chat; deleting the chat removes the job
        if card:
            pg = u.page
            accept = lambda d: asyncio.ensure_future(d.accept())   # noqa: E731 — the user confirms the browser's dialog
            await pg.goto(U.APP + "#chat")
            await pg.locator("#navlinks a[href='#timers']").click()   # Tasks, in the menu
            await pg.wait_for_timeout(2500)
            tasks = " ".join((await pg.locator("#view").inner_text()).split())
            print(f"INFO Tasks tab: {tasks[:600]!r}")
            check("tasks tab: the job is listed with its next run and what it does",
                  re.search(r"Next run", tasks) and re.search(r"email", tasks, re.I), tasks[:600])
            row = pg.locator("#view .card", has_text=re.compile("email", re.I)).first
            href = await row.locator("a", has_text="Open chat").first.get_attribute("href") or ""
            cid = href.rsplit("/", 1)[-1]
            check("tasks tab: a link to the job's chat", bool(cid) and href.startswith("#chat/"), href)
            await row.locator("a", has_text="Open chat").first.click()
            await pg.wait_for_timeout(2500)
            chat = " ".join((await pg.locator(".thread").inner_text()).split())
            check("tasks tab: the link opens the job's chat with the run in it", re.search(r"Scheduled run", chat), chat[:400])
            # the user deletes that chat (🗑 in its header): the job goes with it — stopped and gone from the Tasks tab
            # (0.6.1, the user's rule)
            pg.once("dialog", accept)
            await pg.locator(".thread button", has_text="🗑").first.click()
            await pg.wait_for_timeout(2500)
            await pg.goto(U.APP + "#timers")
            await pg.wait_for_timeout(2500)
            tasks = " ".join((await pg.locator("#view").inner_text()).split())
            left = await pg.locator("#view .card", has_text=re.compile("email", re.I)).count()
            check("tasks tab: deleting the job's chat removes the job", left == 0, tasks[:400])
            await pg.goto(U.APP + "#chat")
            await pg.wait_for_selector("#chatInput")


U.setup(vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com"})
U.run(main)
