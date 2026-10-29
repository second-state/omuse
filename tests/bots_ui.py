"""Bots in the web UI: the left menu lists Main first, then the specialised bots; each bot is a chat view of its own
(#bot/<id>/<conv>); a handed-over answer in Main links to the bot's conversation, which links back to Main; a chat
started in a bot's view belongs to that bot; while a bot answers for Main, Main's chat shows the dots and a verb, then
the bot's answer streaming in."""
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
# BOTSLOW: the routing and the bot's work take about 2 s each (tests/fake_llm.py), so the two timers differ
t = K.run(f"BOTHAND BOTSLOW cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}", bot="main")
main_cid = t["_conv"]
h = next((e["data"] for e in t["events"] if e["type"] == "handoff"), {})
kid_cid = h.get("conv_id", "")
check("setup: the main bot handed over and answered", t["status"] == "COMPLETED" and kid_cid, (t["status"], h))
kid = K.c.get(f"{K.B}/api/tasks/{h.get('task_id', 'x')}").json()
main_s = (t.get("finished_at") or 0) - (t.get("created_at") or 0)
kid_s = (kid.get("finished_at") or 0) - (kid.get("created_at") or 0)
check("the bot's task is timed from the hand-over to its answer; Main's from the request to the answer, routing included",
      t.get("created_at") < kid.get("created_at") <= kid.get("finished_at") <= t.get("finished_at") and kid_s >= 1.8 and main_s - kid_s >= 1.8,
      (main_s, kid_s, t.get("created_at"), kid.get("created_at"), kid.get("finished_at"), t.get("finished_at")))


def fmt(s):
    s = max(0, round(s))
    return f"{s} s" if s < 60 else f"{s // 60} min {s % 60} s"


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1280, "height": 900})
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#navlinks a.botlink")
        bots = await pg.evaluate("() => [...document.querySelectorAll('#navlinks a.botlink')].map(a => [a.dataset.bot, a.getAttribute('href'), a.classList.contains('active')])")
        check("the menu lists the five bots, Main first and active on #chat", [b[0] for b in bots] == ["main", "travel", "insurance", "cards", "photos"]
              and bots[0][1] == "#chat" and bots[1][1] == "#bot/travel" and bots[0][2] and not bots[1][2], bots)
        check("the title is the bot's name", (await pg.inner_text("#viewTitle")).strip() == "Main")
        mb = next(b for b in K.c.get(K.B + "/api/bots", headers=K.H).json()["bots"] if b["id"] == "main")
        await pg.wait_for_selector(".composer .suggest button", timeout=5000)
        ex = await pg.evaluate("() => [...document.querySelectorAll('.composer .suggest button')].map(b => b.textContent)")
        check("an empty Main chat shows Main's examples above the input", ex == mb["examples"] and len(ex) >= 3, ex)
        await pg.goto(B + "#chat/" + main_cid)
        link = pg.locator(".msg.assistant .handoff-link")
        await link.first.wait_for(timeout=10000)
        txt = await link.first.inner_text()
        tm = pg.locator(f"#tc-{t['id']} .timer")
        check("Main's timer shows the whole answer time, routing and the bot's work included", await tm.count() == 1
              and (await tm.inner_text()).strip() == fmt(main_s) and float(await tm.get_attribute("data-start")) == t["created_at"],
              (await tm.count(), await tm.all_inner_texts(), fmt(main_s)))
        check("the handed-over answer links to the bot's conversation", "Continue in" in txt and "Travel" in txt
              and await link.first.get_attribute("href") == f"#bot/travel/{kid_cid}", txt)
        await link.first.click()
        await pg.wait_for_function(f"() => location.hash === '#bot/travel/{kid_cid}'", timeout=10000)
        await pg.wait_for_selector(".thread-head .from-main", timeout=10000)
        active = await pg.evaluate("() => [...document.querySelectorAll('#navlinks a.botlink.active')].map(a => a.dataset.bot)")
        check("the bot's conversation opens in its view: Travel active, its title in the bar", active == ["travel"]
              and (await pg.inner_text("#viewTitle")).strip() == "Travel", active)
        back = await pg.locator(".thread-head .from-main").get_attribute("href")
        check("the bot's conversation links back to Main", back == "#chat/" + main_cid, back)
        km = pg.locator(f"#tc-{kid.get('id')} .timer")
        await km.first.wait_for(timeout=10000)
        check("the bot's conversation has its own timer: the time spent inside the bot", (await km.first.inner_text()).strip() == fmt(kid_s)
              and fmt(kid_s) != fmt(main_s), (await km.all_inner_texts(), fmt(kid_s), fmt(main_s)))
        conv_titles = await pg.evaluate("() => [...document.querySelectorAll('.convlist .conv')].length")
        check("the Travel list shows only its own conversation(s)", conv_titles >= 1)
        check("the question shows where it came from", await pg.locator(".msg.user .from-main").count() == 1)
        # a new chat in the bot's view: the bot's intro, no Main suggestions; the message goes to that bot
        await pg.click(".thread-head .btn.primary")
        await pg.wait_for_function("() => location.hash === '#bot/travel'", timeout=5000)
        intro = await pg.locator(".bot-intro").inner_text()
        ex = await pg.evaluate("() => [...document.querySelectorAll('.composer .suggest button')].map(b => b.textContent)")
        tb = next(b for b in K.c.get(K.B + "/api/bots", headers=K.H).json()["bots"] if b["id"] == "travel")
        check("an empty Travel chat introduces the bot and its skills, and shows its own examples above the input (not Main's)",
              "Travel" in intro and "flight-booking" in intro and ex == tb["examples"] and len(ex) >= 3, (intro[:200], ex))
        await pg.locator(".composer .suggest button").first.click()
        check("an example fills the input", await pg.input_value("#chatInput") == tb["examples"][0], await pg.input_value("#chatInput"))
        await pg.fill("#chatInput", "")
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
        try:   # the redirect changes the hash first; the thread head follows once the conversation has loaded
            await pg.wait_for_selector(".thread-head .from-main", timeout=10000)
        except Exception:
            pass
        check("#chat/<id> of a bot's conversation lands in that bot's view", await pg.locator(".thread-head .from-main").count() == 1)
        # a hand-over seen live from Main: the dancing dots and a verb while the bot works, then its answer streaming in
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#chatInput")
        await pg.fill("#chatInput", f"BOTHAND BOTSLOW BOTSTREAM cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}")
        await pg.keyboard.press("Enter")
        await pg.wait_for_selector(".taskcard .handoff", timeout=15000)
        dots = await pg.evaluate("() => { const l = document.querySelector('.thread .msg.assistant.live'); return l ? [l.classList.contains('wait'), !!l.querySelector('.dance'), (l.querySelector('.verb') || {}).textContent || ''] : null; }")
        check("while the bot works, Main's chat shows the dancing dots and a verb", dots and dots[0] and dots[1] and dots[2].strip(), dots)
        await pg.wait_for_function("() => { const l = document.querySelector('.thread .msg.assistant.live:not(.wait)'); return l && l.textContent.includes('TRAVELBOT ANSWER'); }", timeout=20000)
        part = await pg.evaluate("() => document.querySelector('.thread .msg.assistant.live').textContent")
        check("then the bot's answer streams into Main's chat", "TRAVELBOT ANSWER" in part, part[:200])
        await pg.wait_for_selector(".msg.assistant:not(.live) .handoff-link >> nth=0", timeout=20000)
        await pg.wait_for_function("() => !document.querySelector('.thread .msg.assistant.live')", timeout=10000)
        final = await pg.evaluate("() => [...document.querySelectorAll('.thread .msg.assistant .bubble')].map(b => b.textContent).pop()")
        check("the stored answer replaces the live bubble, with the link to the bot", "TRAVELBOT ANSWER" in final and "Continue in" in final, final[:200])
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
finish()
