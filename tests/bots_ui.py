"""Bots in the web UI: the left menu lists Main first, then the specialised bots; each bot is a chat view of its own
(#bot/<id>/<conv>); a handed-over answer in Main links to the bot's conversation, which links back to Main; a chat
started in a bot's view belongs to that bot."""
import asyncio
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
DATE = (dt.date.today() + dt.timedelta(days=30)).isoformat()
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en", "profile_reminder": False}, headers=K.H)
t = K.run(f"BOTHAND cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}", bot="main")
main_cid = t["_conv"]
h = next((e["data"] for e in t["events"] if e["type"] == "handoff"), {})
kid_cid = h.get("conv_id", "")
check("setup: the main bot handed over and answered", t["status"] == "COMPLETED" and kid_cid, (t["status"], h))


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1280, "height": 900})
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#navlinks a.botlink")
        bots = await pg.evaluate("() => [...document.querySelectorAll('#navlinks a.botlink')].map(a => [a.dataset.bot, a.getAttribute('href'), a.classList.contains('active')])")
        check("the menu lists the seven bots, Main first and active on #chat", [b[0] for b in bots] == ["main", "travel", "cities", "arrival", "insurance", "cards", "photos"]
              and bots[0][1] == "#chat" and bots[1][1] == "#bot/travel" and bots[0][2] and not bots[1][2], bots)
        check("the title is the bot's name", (await pg.inner_text("#viewTitle")).strip() == "Main")
        await pg.goto(B + "#chat/" + main_cid)
        link = pg.locator(".msg.assistant .handoff-link")
        await link.first.wait_for(timeout=10000)
        txt = await link.first.inner_text()
        check("the handed-over answer links to the bot's conversation", "Continue in" in txt and "Travel booking" in txt
              and await link.first.get_attribute("href") == f"#bot/travel/{kid_cid}", txt)
        await link.first.click()
        await pg.wait_for_function(f"() => location.hash === '#bot/travel/{kid_cid}'", timeout=10000)
        await pg.wait_for_selector(".thread-head .from-main", timeout=10000)
        active = await pg.evaluate("() => [...document.querySelectorAll('#navlinks a.botlink.active')].map(a => a.dataset.bot)")
        check("the bot's conversation opens in its view: Travel booking active, its title in the bar", active == ["travel"]
              and (await pg.inner_text("#viewTitle")).strip() == "Travel booking", active)
        back = await pg.locator(".thread-head .from-main").get_attribute("href")
        check("the bot's conversation links back to Main", back == "#chat/" + main_cid, back)
        conv_titles = await pg.evaluate("() => [...document.querySelectorAll('.convlist .conv')].length")
        check("the Travel booking list shows only its own conversation(s)", conv_titles >= 1)
        check("the question shows where it came from", await pg.locator(".msg.user .from-main").count() == 1)
        # a new chat in the bot's view: the bot's intro, no Main suggestions; the message goes to that bot
        await pg.click(".thread-head .btn.primary")
        await pg.wait_for_function("() => location.hash === '#bot/travel'", timeout=5000)
        intro = await pg.locator(".bot-intro").inner_text()
        check("an empty Travel booking chat introduces the bot and its skills, without Main's suggestions",
              "Travel booking" in intro and "flight-booking" in intro and await pg.locator(".suggest").count() == 0, intro[:200])
        await pg.fill("#chatInput", f"BOTKID1 straight to the travel bot DATE={DATE}")
        await pg.keyboard.press("Enter")
        await pg.wait_for_function("() => /^#bot\\/travel\\/conv_/.test(location.hash)", timeout=10000)
        cid = (await pg.evaluate("() => location.hash")).split("/")[2]
        c = K.c.get(f"{K.B}/api/conversations/{cid}", headers=K.H).json()["conversation"]
        check("a chat started in the bot's view belongs to that bot", c["bot_id"] == "travel", c)
        await pg.click("#navlinks a.botlink[data-bot=main]")
        await pg.wait_for_function("() => location.hash === '#chat'", timeout=5000)
        mine = [x["title"] for x in K.c.get(K.B + "/api/conversations?bot=main", headers=K.H).json()["conversations"] if x["kind"] == "chat"]
        js = "want => { const s = [...document.querySelectorAll('.convlist .conv .ct')].map(e => e.textContent).sort(); return JSON.stringify(s) === JSON.stringify(want); }"
        try:
            await pg.wait_for_function(js, arg=sorted(mine), timeout=10000)
        except Exception:
            pass
        shown = await pg.evaluate("() => [...document.querySelectorAll('.convlist .conv .ct')].map(e => e.textContent)")
        check("Main's list shows Main's conversations only", sorted(shown) == sorted(mine), (shown, mine))
        # an old-style link (#chat/<id>) to a bot's conversation opens it in that bot
        await pg.goto(B + "#chat/" + kid_cid)
        await pg.wait_for_function(f"() => location.hash === '#bot/travel/{kid_cid}'", timeout=10000)
        check("#chat/<id> of a bot's conversation lands in that bot's view", await pg.locator(".thread-head .from-main").count() == 1)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
finish()
