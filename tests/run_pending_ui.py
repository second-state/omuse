"""Pending scheduled tasks and /run_pending_tasks, as on a machine that slept through a task's time. The stack runs with
`[server] scheduler_loop = false` (the line below), so nothing but /run_pending_tasks runs a due task.

1. A daily task is put in the database with its time an hour ago (what a suspended machine leaves).
2. Tasks tab: the task shows Pending; 25 s later (longer than the loop's 20 s) it is still pending — not run.
3. GET /run_pending_tasks streams one JSON line per event: pending (this task) → started → finished (a final status)
   → done, then closes the connection.
4. Tasks tab: the task shows its next run (not Pending); its chat has the run; a second call finds nothing pending."""
# run_local: OMUSE_E2E_SCHEDULER_LOOP=false
import asyncio
import json
import os
import sqlite3
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

DB = "/tmp/claude-0/persona-test/data/runtime.db"
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en", "profile_reminder": False}, headers=K.H)
s = K.c.post(K.B + "/api/schedules", json={"name": "Morning mail", "goal": "summarise my new emails", "kind": "cron",
                                           "spec": "0 8 * * *"}, headers=K.H).json()
SID = s["id"]
DUE = time.time() - 3600
db = sqlite3.connect(DB, timeout=10)
db.execute("UPDATE schedules SET next_run=? WHERE id=?", (DUE, SID))
db.commit()
db.close()


def schedule() -> dict:
    return next((x for x in K.c.get(K.B + "/api/schedules").json()["schedules"] if x["id"] == SID), {})


async def card_text(pg) -> str:
    await pg.goto(K.B + "/#timers")
    await pg.wait_for_selector("#view .card, #view p")
    await pg.wait_for_timeout(800)
    card = pg.locator("#view .card", has_text="Morning mail").first
    return " ".join((await card.inner_text()).split()) if await card.count() else ""


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1280, "height": 900})
        t = await card_text(pg)
        check("tasks tab: the task the machine slept through shows Pending", "Pending" in t, t)
        await asyncio.sleep(25)
        check("with the loop off it is still pending 25 s later (not run)", abs((schedule().get("next_run") or 0) - DUE) < 1
              and not schedule().get("last_task"), schedule())

        lines, closed = [], False
        with K.c.stream("GET", K.B + "/run_pending_tasks", timeout=300) as r:
            check("/run_pending_tasks answers NDJSON", r.status_code == 200 and "ndjson" in r.headers.get("content-type", ""),
                  (r.status_code, r.headers.get("content-type")))
            for ln in r.iter_lines():
                if ln.strip():
                    lines.append(json.loads(ln))
            closed = True
        print(f"INFO stream: {lines}")
        ev = [x["event"] for x in lines if x["event"] != "waiting"]
        check("the stream: pending → started → finished → done, then the connection closes",
              ev == ["pending", "started", "finished", "done"] and closed, ev)
        check("pending names the task", lines and lines[0].get("count") == 1 and lines[0]["tasks"][0]["schedule_id"] == SID, lines[:1])
        fin = next((x for x in lines if x["event"] == "finished"), {})
        check("finished: the run ended (a final status)", fin.get("status") in ("COMPLETED", "FAILED", "CANCELLED"), fin)

        t = await card_text(pg)
        check("tasks tab: afterwards the task shows its next run, not Pending", "Next run" in t and "Pending" not in t, t)
        href = await pg.locator("#view .card", has_text="Morning mail").first.locator("a", has_text="Open chat").get_attribute("href")
        await pg.goto(K.B + "/" + href)
        await pg.wait_for_timeout(2500)
        chat = " ".join((await pg.locator(".thread").inner_text()).split())
        check("its chat has the run", "Scheduled run" in chat, chat[:300])
        again = [json.loads(x) for x in K.c.get(K.B + "/run_pending_tasks", timeout=60).text.splitlines() if x.strip()]
        check("a second call finds nothing pending", again and again[0].get("count") == 0 and again[-1]["event"] == "done", again)
        await br.close()


asyncio.run(main())
finish()
