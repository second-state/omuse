"""Sidebar navigation in four named sections: Bots (Main first, then the specialised ones, each a robot icon with a
badge) · Tools (Browser, Connections, Memory) · Trust (Status, Audit) · Config (Skills, Settings); the Tasks and
Automations views are not listed but #tasks/<id> still opens a task (Audit links to it); the Health section of the Trust page
is in English when the UI language is English (no Chinese in any component detail); the Skills page lists the skills
under each bot."""
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
        BOTS = ["Main", "Travel", "Insurance & claims", "Credit cards", "Photos & videos"]
        check("the sidebar shows the five bots (Main first) in place of Chat, then the seven views in order",
              labels == BOTS + ["Browser", "Connections", "Memory", "Status", "Audit", "Skills", "Settings"], labels)
        seps = await pg.evaluate("() => [...document.querySelectorAll('#navlinks > *')].map(e => e.classList.contains('navhead') ? '[' + e.textContent + ']' : (a => [...a.childNodes].filter(n => !(n.classList && (n.classList.contains('ic') || n.classList.contains('badge')))).map(n => n.textContent).join('').trim())(e)).join(' ')")
        check("four named sections: Bots, Tools, Trust, Config",
              seps == "[Bots] " + " ".join(BOTS) + " [Tools] Browser Connections Memory [Trust] Status Audit [Config] Skills Settings", seps)
        icons = await pg.evaluate("() => [...document.querySelectorAll('#navlinks a')].map(a => [!!a.querySelector('.boticon svg .head'), a.querySelectorAll('.boticon .bb').length])")
        check("each bot has a robot icon with a badge; the tools do not", icons[:5] == [[True, 1]] * 5 and all(not i[0] for i in icons[5:]), icons)
        check("no Tasks or Automations tab", "Tasks" not in labels and "Automations" not in labels, labels)
        await pg.goto(B + "#tasks/" + t["id"])
        await pg.wait_for_selector("#taskDetail", timeout=10000)
        check("a task is still reachable at #tasks/<id> (Audit links there)", await pg.locator("#taskDetail").count() == 1 and not await pg.locator("#navlinks a.active").count())
        await pg.goto(B + "#trust")
        await pg.wait_for_selector("text=Health")
        txt = await pg.locator("#view").inner_text() if await pg.locator("#view").count() else await pg.inner_text("body")
        health = txt[txt.find("Health"):][:600]
        check("the Trust page's Health section has no Chinese", not CJK.search(health), health[:300])
        # the Skills page: the skills under each bot, in the menu's order; a shared skill under every bot that has it
        bots = K.c.get(K.B + "/api/bots", headers=K.H).json()["bots"]
        await pg.goto(B + "#skills")
        await pg.wait_for_selector(".skillgroup")
        groups = await pg.evaluate("() => [...document.querySelectorAll('.skillgroup')].map(g => [g.dataset.bot, [...g.querySelectorAll('.mono')].map(e => e.textContent)])")
        check("the Skills page has one section per bot, in the menu's order", [g[0] for g in groups] == [b["id"] for b in bots], [g[0] for g in groups])
        check("each bot's section lists exactly that bot's skills", all(sorted(g[1]) == sorted(b["skills"]) for g, b in zip(groups, bots)),
              [(g[0], len(g[1]), len(b["skills"])) for g, b in zip(groups, bots)])
        shared = await pg.evaluate("() => { const bs = [...document.querySelectorAll('.skillgroup label')].filter(l => l.querySelector('.mono').textContent === 'pdf-forms').map(l => l.querySelector('input')); bs[0].click(); const r = bs.map(b => b.checked); bs[0].click(); return r; }")
        check("a shared skill (pdf-forms) shows under each of its bots, its checkboxes moving together", len(shared) == 3 and len(set(shared)) == 1, shared)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
finish()
