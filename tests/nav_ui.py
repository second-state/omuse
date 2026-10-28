"""Sidebar navigation: Chat, Browser, Connections, Memory · Trust, Audit · Skills, Settings (two separators); the Tasks and
Automations views are not listed but #tasks/<id> still opens a task (Audit links to it); the Health section of the Trust page
is in English when the UI language is English (no Chinese in any component detail)."""
import asyncio
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
CJK = re.compile(r"[\u4e00-\u9fff]")
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en", "profile_reminder": False}, headers=K.H)
t = K.run("hello there")
st = K.c.post(K.B + "/api/health/check", json={}, headers=K.H).json()
details = {k: v.get("detail", "") for k, v in st.get("components", {}).items()}
check("English UI: no Chinese in any health detail", details and not any(CJK.search(d) for d in details.values()), details)
check("the model service detail says OK", details.get("model", "").endswith(" OK") or "online" in details.get("model", ""), details.get("model"))


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1280, "height": 900})
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#navlinks a")
        labels = await pg.evaluate("() => [...document.querySelectorAll('#navlinks a')].map(a => [...a.childNodes].filter(n => !(n.classList && (n.classList.contains('ic') || n.classList.contains('badge')))).map(n => n.textContent).join('').trim())")
        check("the sidebar shows the eight views in order", labels == ["Chat", "Browser", "Connections", "Memory", "Trust", "Audit", "Skills", "Settings"], labels)
        seps = await pg.evaluate("() => [...document.querySelectorAll('#navlinks > *')].map(e => e.classList.contains('navsep') ? '|' : (a => [...a.childNodes].filter(n => !(n.classList && (n.classList.contains('ic') || n.classList.contains('badge')))).map(n => n.textContent).join('').trim())(e)).join(' ')")
        check("two separators: after Memory and after Audit", seps == "Chat Browser Connections Memory | Trust Audit | Skills Settings", seps)
        check("no Tasks or Automations tab", "Tasks" not in labels and "Automations" not in labels, labels)
        await pg.goto(B + "#tasks/" + t["id"])
        await pg.wait_for_selector("#taskDetail", timeout=10000)
        check("a task is still reachable at #tasks/<id> (Audit links there)", await pg.locator("#taskDetail").count() == 1 and not await pg.locator("#navlinks a.active").count())
        await pg.goto(B + "#trust")
        await pg.wait_for_selector("text=Health")
        txt = await pg.locator("#view").inner_text() if await pg.locator("#view").count() else await pg.inner_text("body")
        health = txt[txt.find("Health"):][:600]
        check("the Trust page's Health section has no Chinese", not CJK.search(health), health[:300])
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
finish()
