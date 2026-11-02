"""Stop a task that never ends, from the chat: the task card's opened Activity panel has a quiet Stop button (hidden
while the panel is closed); a click stops the task — CANCELLED, the card shows it stopped, the reply bubble goes, and no
model or tool call happens after it. The next message in the same chat starts a new task normally. English UI by
default (the button says Stop, no Chinese), Chinese only when set. The Tasks page keeps its Cancel button.
LOOPFOREVER (tests/fake_llm.py): a model that writes a new note every turn, forever."""
import asyncio
import re
import sys
import time

import httpx
from playwright.async_api import async_playwright

URL = "http://127.0.0.1:8080/"
LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=30, trust_env=False)
fails = []
CJK = re.compile(r"[㐀-鿿]")


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else info)
    if not cond:
        fails.append(name)


def task(tid):
    return c.get(URL + f"api/tasks/{tid}").json()


def tool_calls(t):
    return [e for e in t.get("events", []) if e["type"] == "tool_call"]


def model_calls(marker):
    return len(c.get(LLM + "/calls_for", params={"marker": marker}).json())


def wait(pred, secs=30, step=0.25):
    t0 = time.time()
    while time.time() - t0 < secs:
        v = pred()
        if v:
            return v
        time.sleep(step)
    return None


def conv_task(url):
    """The conversation in the page's address and its newest task id."""
    cid = (url.split("#chat/") + [""])[1]
    if not cid:
        return "", ""
    tasks = c.get(URL + f"api/conversations/{cid}").json().get("tasks") or {}
    newest = max(tasks.values(), key=lambda t: t.get("created_at") or 0, default={})
    return cid, newest.get("id", "")


async def stop_from_chat(p, lang):
    marker = f"LOOPFOREVER{lang.upper()}"
    c.put(URL + "api/settings", json={"language": lang, "ui_language": lang}, headers=H).raise_for_status()
    b = await p.chromium.launch()
    pg = await b.new_page(viewport={"width": 1280, "height": 860})
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    await pg.goto(URL + "#chat")
    await pg.wait_for_selector("#chatInput")
    await pg.click("button.newchat")
    await pg.fill("#chatInput", f"{marker} keep writing notes, never stop")
    await pg.keyboard.press("Enter")
    await pg.wait_for_selector(".taskcard", timeout=30000)
    tid = ""
    for _ in range(60):
        cid, tid = conv_task(pg.url)
        if tid:
            break
        await pg.wait_for_timeout(250)
    check(f"{lang}: the task started", bool(tid))
    looping = wait(lambda: len(tool_calls(task(tid))) >= 3, 40)
    check(f"{lang}: the task keeps calling tools (it would never end)", looping, task(tid).get("status"))
    card = pg.locator(f"#tc-{tid}")
    stop = card.locator("button.stop-task")
    check(f"{lang}: the Stop button hides while the task panel is closed", await stop.count() == 1 and not await stop.is_visible(),
          await stop.count())
    await card.locator("details.trace > summary").click()
    await stop.wait_for(state="visible", timeout=5000)
    label = (await stop.text_content() or "").strip()
    if lang == "en":
        check("en: the button says Stop, in English only", "Stop" in label and not CJK.search(label), label)
    else:
        check("zh: the button is in Chinese only", "停止" in label and "Stop" not in label, label)
    check(f"{lang}: the reply bubble is up while the task works", await pg.locator(f"#live-{tid}").count() == 1)
    await stop.click()
    st = wait(lambda: task(tid)["status"] == "CANCELLED" and task(tid), 10)
    check(f"{lang}: Stop -> the task is CANCELLED", bool(st), task(tid)["status"])
    t_at = task(tid)
    n_tools, n_model = len(tool_calls(t_at)), model_calls(marker)
    await pg.wait_for_timeout(500)
    pill = card.locator(".pill").first
    check(f"{lang}: the card shows the stopped state", "st-CANCELLED" in (await pill.get_attribute("class") or ""),
          await pill.get_attribute("class"))
    check(f"{lang}: the live bubble ended", await pg.locator(f"#live-{tid}").count() == 0)
    check(f"{lang}: no Stop button on a stopped task", await pg.locator(f"#tc-{tid} button.stop-task").count() == 0)
    await pg.wait_for_timeout(3000)   # several of the model's turns' worth of time
    t2 = task(tid)
    check(f"{lang}: no tool call after the stop", len(tool_calls(t2)) == n_tools, (n_tools, len(tool_calls(t2))))
    check(f"{lang}: no model call after the stop", model_calls(marker) == n_model, (n_model, model_calls(marker)))
    check(f"{lang}: the status stays CANCELLED, no answer posted", t2["status"] == "CANCELLED" and not t2.get("result")
          and not any(e["type"] == "final" for e in t2["events"]), (t2["status"], t2.get("result")))
    check(f"{lang}: the stop is in the task's activity", any(e["type"] == "cancelled" for e in t2["events"]))
    # the next message in the same chat is a new task, run normally
    await pg.fill("#chatInput", "hello again")
    await pg.keyboard.press("Enter")
    nxt = wait(lambda: (lambda x: x if x[1] and x[1] != tid else None)(conv_task(pg.url)), 20)
    new_tid = nxt[1] if nxt else ""
    done = wait(lambda: task(new_tid)["status"] in ("COMPLETED", "FAILED") and task(new_tid), 60) if new_tid else None
    check(f"{lang}: a later message starts a new task that completes", bool(done) and done["status"] == "COMPLETED",
          done and done["status"])
    check(f"{lang}: the stopped task stays CANCELLED", task(tid)["status"] == "CANCELLED")
    check(f"{lang}: no page errors", not errs, errs)
    await b.close()


async def stop_from_tasks_page(p):
    """The Tasks page's own Cancel button still stops a task."""
    c.put(URL + "api/settings", json={"language": "en", "ui_language": "en"}, headers=H).raise_for_status()
    tid = c.post(URL + "api/chat", json={"message": "LOOPFOREVERTASKS keep going"}, headers=H).json()["task_id"]
    check("tasks page: the task loops", wait(lambda: len(tool_calls(task(tid))) >= 2, 40), task(tid).get("status"))
    b = await p.chromium.launch()
    pg = await b.new_page(viewport={"width": 1280, "height": 860})
    await pg.goto(URL + f"#tasks/{tid}")
    btn = pg.locator("#taskDetail .btn.danger")
    await btn.wait_for(state="visible", timeout=10000)
    await btn.click()
    check("tasks page: Cancel -> CANCELLED", bool(wait(lambda: task(tid)["status"] == "CANCELLED", 10)), task(tid)["status"])
    n = len(tool_calls(task(tid)))
    await pg.wait_for_timeout(2000)
    check("tasks page: no tool call after it", len(tool_calls(task(tid))) == n)
    await b.close()


async def main():
    s0 = c.get(URL + "api/settings", headers=H).json()["settings"]
    c.put(URL + "api/settings", json={"max_steps": 200}, headers=H).raise_for_status()
    try:
        async with async_playwright() as p:
            await stop_from_chat(p, "en")
            await stop_from_chat(p, "zh")
            await stop_from_tasks_page(p)
    finally:
        c.put(URL + "api/settings", json={k: s0.get(k) for k in ("max_steps", "language", "ui_language") if k in s0}, headers=H)
    print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
    sys.exit(1 if fails else 0)


asyncio.run(main())
