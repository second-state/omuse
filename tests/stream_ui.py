"""Chat: the answer appears in a live bubble while the model writes it (answer_delta events), under the running task's
card, with a caret; when the task finishes the live bubble goes and the stored message shows the full text. The scripted
model streams a STREAMSLOW answer at 50 ms a piece, so the page is caught mid-answer."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en", "stream_answers": True}, headers=K.H)


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1200, "height": 900})
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#chatInput")
        await pg.click("button.newchat")
        await pg.fill("#chatInput", "STREAMSLOW tell me a bedtime story")
        await pg.keyboard.press("Enter")
        await pg.wait_for_selector(".msg.assistant.live .bubble", timeout=30000)
        live = pg.locator(".msg.assistant.live .bubble")
        first = await live.inner_text()
        await pg.wait_for_timeout(600)
        second = await live.inner_text()
        tasks = K.c.get(K.B + "/api/tasks", headers=K.H).json()
        running = [t for t in (tasks.get("tasks") if isinstance(tasks, dict) else tasks) if t["status"] in ("RUNNING", "PLANNING")]
        check("the live bubble shows the answer while the task still runs", first and len(second) > len(first) and running, (len(first), len(second), [t["status"] for t in (tasks.get("tasks") if isinstance(tasks, dict) else tasks)][:3]))
        check("the live bubble sits under the task card", await pg.evaluate("() => { const l = document.querySelector('.msg.assistant.live'); const c = l && l.previousElementSibling; return !!(c && c.querySelector('.taskcard, [id^=tc-]')); }"))
        check("no reasoning in the live text", "<think>" not in second and "keep it short" not in second, second[:100])
        await pg.wait_for_selector(".msg.assistant.live", state="detached", timeout=60000)
        await pg.wait_for_function("() => [...document.querySelectorAll('.msg.assistant:not(.live) .bubble')].some(b => b.textContent.includes('THE END.'))", timeout=30000)
        check("when the task finishes the stored message replaces the live bubble", await pg.locator(".msg.assistant.live").count() == 0)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
finish()
