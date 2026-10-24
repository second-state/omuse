"""Screenshots of the web UI for comparing the Rust port with the Python original.

Seeds the same state through the API (language, a chat with a plain answer, a memory fact, a schedule, a vault item,
a pending approval), then photographs every view at the same viewport. Run against a local stack (run_local.sh):

    python3 tests/screenshots.py <output-dir> [base-url]

Compare two directories with tests/compare_screenshots.py.
"""
import asyncio
import os
import sys
import time

import httpx
from playwright.async_api import async_playwright

OUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/claude-0/shots"
B = (sys.argv[2] if len(sys.argv) > 2 else "http://127.0.0.1:8080").rstrip("/")
H = {"X-Persona-UI": "1"}
VIEWS = ["chat", "tasks", "approvals", "goals", "schedules", "memory", "library", "skills", "connections", "vault", "audit",
         "browser", "settings", "metrics"]
os.makedirs(OUT, exist_ok=True)
c = httpx.Client(timeout=60, trust_env=False)


def wait(tid, states=("COMPLETED", "FAILED", "WAITING_APPROVAL", "WAITING_EXTERNAL"), timeout=60):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in states:
            return t
        time.sleep(0.3)
    return t


def seed():
    c.put(B + "/api/settings", json={"language": "zh", "ui_language": "zh", "theme": "paper", "instance_name": "OMuse",
                                      "user_name": "Lucas", "model_override": False}, headers=H)
    r = c.post(B + "/api/chat", json={"message": "你好"}, headers=H).json()
    wait(r["task_id"])
    r = c.post(B + "/api/chat", json={"message": "REMEMBER I like quiet hotels"}, headers=H).json()
    wait(r["task_id"])
    r = c.post(B + "/api/chat", json={"message": "SCHEDULE daily"}, headers=H).json()
    wait(r["task_id"])
    c.post(B + "/sentinel/api/vault", json={"label": "KrisFlyer", "kind": "membership",
                                            "values": {"number": "8812345678", "name": "Lucas"}}, headers=H)
    r = c.post(B + "/api/chat", json={"message": "BROWSE the shop"}, headers=H).json()
    wait(r["task_id"])   # ends in WAITING_APPROVAL: the approval drawer has an item
    return r["task_id"]


async def main():
    tid = seed()
    async with async_playwright() as p:
        b = await p.chromium.launch()
        ctx = await b.new_context(viewport={"width": 1400, "height": 1000}, locale="zh-CN", device_scale_factor=1,
                                  reduced_motion="reduce")
        pg = await ctx.new_page()
        await pg.goto(B + "/#chat")
        await pg.wait_for_selector("#chatInput")
        await pg.wait_for_timeout(1500)
        await pg.screenshot(path=os.path.join(OUT, "approval_modal.png"))   # the pending approval pops up by itself
        print("shot approval_modal")

        async def dismiss():   # click the modal's backdrop (the app's own way to close it)
            if await pg.locator(".modal-back").count():
                await pg.locator(".modal-back").click(position={"x": 5, "y": 5})
                await pg.wait_for_timeout(300)

        for v in VIEWS:
            await pg.goto(B + "/#" + v)
            await pg.wait_for_timeout(1200)
            await dismiss()
            # hide clocks / relative times that differ between runs
            await pg.add_style_tag(content="time, .ts, .time, .muted.small.ts { visibility: hidden !important }")
            await pg.screenshot(path=os.path.join(OUT, f"{v}.png"), full_page=False)
            print("shot", v)
        await pg.goto(B + "/#chat")
        await pg.wait_for_selector("#approvalBell")
        await pg.wait_for_timeout(1200)
        await dismiss()
        await pg.click("#approvalBell")
        await pg.wait_for_timeout(800)
        await pg.screenshot(path=os.path.join(OUT, "approval_drawer.png"))
        await pg.goto(B + f"/#browser/{tid}")
        await pg.wait_for_timeout(1500)
        await dismiss()
        await pg.screenshot(path=os.path.join(OUT, "browser_task.png"))
        mob = await b.new_context(viewport={"width": 390, "height": 844}, locale="en-US", device_scale_factor=1)
        mp = await mob.new_page()
        await mp.goto(B + "/#chat")
        await mp.wait_for_selector("#chatInput")
        await mp.wait_for_timeout(1500)
        if await mp.locator(".modal-back").count():
            await mp.locator(".modal-back").click(position={"x": 3, "y": 3})
            await mp.wait_for_timeout(300)
        await mp.screenshot(path=os.path.join(OUT, "mobile_chat.png"))
        await b.close()
    print("screenshots in", OUT)


asyncio.run(main())
