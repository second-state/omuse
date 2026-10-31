"""Everyday assistant features with a real model, from the user's point of view (tests/user_common.py,
tests/shop_common.py): memory, a scheduled job and a site the user has to sign in to.

1. remember — "Please remember: I'm vegetarian and I don't drink alcohol." Checked: the answer confirms it and the Memory
   page lists it.
2. learned — in another chat the user mentions in passing their shoe size (EU 42) and how much they run while asking
   about running shoes. Checked: the answer is about running shoes; within two minutes the Memory page shows the shoe
   size that OMuse picked up by itself.
3. recall — in a new chat: "What do you know about my food and drink preferences?" Checked: vegetarian, no alcohol.
4. schedule — "Every weekday at 8 am, summarise my new emails for me." Checked: Automations lists the job at 08:00;
   the user presses "Run now" and then "Results": the run's result — its answer, or the notification it sends —
   summarises the mail in the inbox (the landlord's rent increase from S$3,800 to S$4,300), and the answer does not
   contradict the notification.
   The Tasks tab lists the job with its next run and a link to its chat; after the user deletes that chat, the next
   run's message is in a chat again at the same link; Delete on the Tasks tab removes the job.
5. sign in — "How many Orchid Rewards points do I have?" with the member site's address. The site needs the user's
   password, which the agent never types: it asks the user to take over. The user signs in in the live view (email,
   password, Sign in) and hands back. Checked: the takeover was on the sign-in page; the answer gives the balance
   (12,480 points) and the 1,200 points expiring on 31 December 2026; the password never appears in the chat.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh assistant_real"""
import asyncio
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

PASSWORD = "Orchid#2026"
MAIL = [
    ("Boss <boss@acme.com>", "Quarterly plan", "Please review the quarterly plan by Friday.", 0),
    ("Daniel Tan <daniel.tan@hollandprops.example>", "Rent for 12 Holland Road #05-03 from 1 January",
     "Hi Lucas,\n\nAs your lease renews on 1 January 2027, the monthly rent will go up from S$3,800 to S$4,300. Please let me know by "
     "15 November whether you would like to renew.\n\nBest regards,\nDaniel Tan", 0),
]


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
        check("remember: the answer confirms what will be remembered", a.mentions("vegetarian", "alcohol"), a.text[:300])
        check("remember: the Memory page lists that the user is vegetarian", "vegetarian" in mem.lower(), mem[:1500])

        # ---- 2. learned on the way
        a = await u.ask("I wear EU size 42 and run about 20 km a week. What should I look for in my next pair of running shoes?", label="learned")
        check("learned: the answer is about running shoes", re.search(r"shoe", a.text, re.I), a.text[:400])
        mem = ""
        for _ in range(12):
            mem = await memory_page(u)
            if re.search(r"\b42\b", mem):
                break
            await u.page.wait_for_timeout(10000)
        print(f"INFO Memory page (facts): {facts(mem)!r}")
        check("learned: the Memory page shows the shoe size (42), picked up without being asked", re.search(r"\b42\b", mem), facts(mem))

        # ---- 3. recall
        a = await u.ask("What do you know about my food and drink preferences?", label="recall")
        check("recall: vegetarian, no alcohol", a.mentions("vegetarian") and a.mentions("alcohol"), a.text[:400])

        # ---- 4. a scheduled job
        a = await u.ask("Every weekday at 8 am, summarise my new emails for me.", label="schedule")
        card = await S.automation_card(u, "8")
        print(f"INFO Automations card: {card!r}")
        check("schedule: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("schedule: Automations lists the job at 08:00", bool(card) and re.search(r"\b0?8:00\b|\b8\s?(am|a\.m\.)", card, re.I), card)
        check("schedule: … on weekdays", re.search(r"weekday|Mon(day)?\s?(–|-|to)\s?Fri|1-5", card, re.I), card)
        if card:
            await S.watch_toasts(u)
            await S.press_on_card(u, "8", "Run now")
            out = await schedule_results(u)
            bells = [x for x in await S.toasts(u) if "🔔" in x]
            print(f"INFO the run's answer: {' '.join(out.split())[:600]!r}; notifications: {bells}")
            said = out + "\n" + "\n".join(bells)
            check("schedule: Run now → the run's result (Results or its notification) summarises the new mail (rent S$3,800 → S$4,300)",
                  re.search(r"4,?300", said) and re.search(r"rent|landlord|Holland", said, re.I), said[:900])
            check("schedule: … and the Results answer does not contradict it (no 'no new emails' after summarising them)",
                  not (re.search(r"no new e-?mails", out, re.I) and bells), (out[:300], bells))
            await u.page.goto(U.APP + "#chat")
            await u.page.wait_for_selector("#chatInput")

        # ---- 4b. the Tasks tab: the job, its next run and its chat; the chat deleted comes back with the next run;
        # Delete removes the job
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
            # the user deletes that chat (🗑 in its header), then the job runs again
            pg.once("dialog", accept)
            await pg.locator(".thread button", has_text="🗑").first.click()
            await pg.wait_for_timeout(1500)
            await S.press_on_card(u, "8", "Run now")
            await pg.wait_for_timeout(3000)
            await pg.goto(U.APP + "#chat/" + cid)
            await pg.wait_for_timeout(3000)
            chat = " ".join((await pg.locator(".thread").inner_text()).split())
            check("tasks tab: after the chat was deleted, the next run's message is in a chat again (same link)",
                  re.search(r"Scheduled run", chat), chat[:400])
            # Delete on the Tasks tab
            await pg.goto(U.APP + "#timers")
            await pg.wait_for_timeout(2500)
            pg.once("dialog", accept)
            await pg.locator("#view .card", has_text=re.compile("email", re.I)).first.locator("button", has_text="Delete").click()
            await pg.wait_for_timeout(2500)
            tasks = " ".join((await pg.locator("#view").inner_text()).split())
            left = await pg.locator("#view .card", has_text=re.compile("email", re.I)).count()
            check("tasks tab: Delete removes the job", left == 0, tasks[:400])
            await pg.goto(U.APP + "#chat")
            await pg.wait_for_selector("#chatInput")

        # ---- 5. a site that needs the user's password
        seen = {"url": "", "after": ""}

        async def sign_in(pg, request: str) -> None:
            seen["url"] = await u.view_url()
            print(f"INFO takeover: {request!r} url {seen['url']!r}")
            if "orchid_login" not in seen["url"]:
                return
            await u.take_over()
            box = await u.layout(f"{S.SHOP}/orchid_login.html", {"em": "#em", "pw": "#pw", "go": "#go"})
            await S.type_in_view(u, box["em"][0] + 40, box["em"][1] + box["em"][3] / 2, "lucas@example.com")
            await S.type_in_view(u, box["pw"][0] + 40, box["pw"][1] + box["pw"][3] / 2, PASSWORD)
            await u.click_view(box["go"][0] + box["go"][2] / 2, box["go"][1] + box["go"][3] / 2)
            for _ in range(10):
                seen["after"] = await u.view_url()
                if "orchid_account" in seen["after"]:
                    break
                await pg.wait_for_timeout(1000)
            print(f"INFO after Sign in: {seen['after']!r}")

        a = await S.ask_takeover(u, f"How many Orchid Rewards points do I have? The member site is {S.SHOP}/orchid_login.html — I'll sign in "
                                    "for you.", label="sign in", on_takeover=sign_in)
        check("sign in: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("sign in: the user was asked to take over on the sign-in page", bool(a.takeovers) and "orchid_login" in seen["url"],
              (a.takeovers, seen["url"]))
        check("sign in: the user's sign-in in the live view reached the account page", "orchid_account" in seen["after"], seen["after"])
        check("sign in: the answer gives the balance (12,480 points)", re.search(r"12,?480", a.text), a.text[:500])
        check("sign in: … and the 1,200 points expiring on 31 December 2026", re.search(r"1,?200", a.text)
              and re.search(r"31 Dec(ember)?|Dec(ember)? 31|2026-12-31", a.text, re.I), a.text[:500])
        check("sign in: the password never appears in the chat", PASSWORD not in await u.thread_text(), "")


U.setup(mail=MAIL, vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com"})
U.run(main)
