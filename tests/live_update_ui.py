"""Chat page keeps up even when the live event stream is broken (dead / dropped), e.g. after laptop sleep or an
Olares session refresh: a new chat's task card must move past "Planning" and show the approval button by itself."""
import asyncio
import re, sys
from playwright.async_api import async_playwright

URL = "http://127.0.0.1:8080/"
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else info)
    if not cond:
        fails.append(name)


async def card_state(pg):
    return await pg.evaluate("""() => [...document.querySelectorAll('.taskcard')].map(c => ({
        status: (c.querySelector('.pill') || {}).textContent || '', approve: !!c.querySelector('.btn.approve')}))""")


def deny_pending():
    """The previous scenario's approval would pop up on page load and cover the page."""
    import httpx
    c = httpx.Client(timeout=30, trust_env=False)
    for a in c.get(URL + "sentinel/api/approvals?status=pending").json()["approvals"]:
        c.post(f"{URL}sentinel/api/approvals/{a['id']}/resolve", json={"decision": "deny"}, headers={"X-Persona-UI": "1"})


async def scenario(p, name, route_stream):
    deny_pending()
    b = await p.chromium.launch()
    pg = await b.new_page(viewport={"width": 1280, "height": 800})
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    if route_stream:
        await pg.route("**/api/stream", route_stream)
    await pg.goto(URL + "#chat")
    await pg.wait_for_selector("#chatInput")
    # every verb the card ever shows, recorded as it changes (the task reaches its approval within a second, faster
    # than polling can see)
    await pg.evaluate("""() => { window._verbs = []; const note = e => { if (e && e.classList && e.classList.contains('verb')) window._verbs.push(e.textContent); };
      new MutationObserver(ms => { for (const m of ms) { const t = m.target.nodeType === 3 ? m.target.parentElement : m.target; note(t);
        if (t && t.querySelectorAll) t.querySelectorAll('.typing.verb').forEach(note); } }).observe(document.body, { subtree: true, childList: true, characterData: true }); }""")
    await pg.click("button.newchat")
    await pg.fill("#chatInput", "BROWSE the shop " + name)
    await pg.keyboard.press("Enter")
    got = None
    verbs, titles, dots = set(), set(), False
    get_verbs = "() => [...document.querySelectorAll('.msg.assistant.live .typing.verb')].map(e => [e.textContent, e.title])"
    for _ in range(80):
        await pg.wait_for_timeout(250)
        seen = await pg.evaluate(get_verbs)
        verbs.update(v for v, _t in seen); titles.update(t for _v, t in seen)
        # the reply bubble under the card dances while the agent works (no answer text yet)
        dots = dots or await pg.evaluate("() => !!document.querySelector('.msg.assistant.live.wait .dance')")
        st = await card_state(pg)
        if st and st[-1]["approve"]:
            got = st[-1]
            break
    check(f"{name}: card shows the approval without a reload", got is not None, await card_state(pg))
    flat = {"规划中", "执行中", "規劃中", "執行中", "Planning", "Running"}
    check(f"{name}: while working the reply bubble shows a fun verb next to the dots, not the flat state", verbs and all(v and v not in flat for v in verbs), verbs)
    head = await pg.evaluate("() => { const c = document.querySelector('.taskcard'); return c ? { timer: (c.querySelector('.timer') || {}).textContent || '', details: c.querySelectorAll('a[href^=\"#tasks/\"]').length, verb: c.querySelectorAll('.verb').length } : null; }")
    check(f"{name}: the card head shows the turn's timer in seconds and no Details link or verb", head and re.fullmatch(r"\d+ s|\d+ min \d+ s", head["timer"]) and head["details"] == 0 and head["verb"] == 0, head)
    check(f"{name}: ...and the real state in its tooltip", titles and titles <= flat, titles)
    if route_stream is None:   # with the event stream up, every model call and tool call brings a new verb (BROWSE makes several)
        verbs |= set(await pg.evaluate("() => window._verbs"))
        check(f"{name}: the verb changes with the task's calls", len(verbs) >= 2, verbs)
    check(f"{name}: the reply bubble shows the dancing dots while the agent works", dots)
    check(f"{name}: the card has no dots of its own any more (the bubble has them)", not await pg.evaluate("() => !!document.querySelector('.taskcard .dots')"))
    check(f"{name}: no page errors", not errs, errs)
    await b.close()


async def main():
    async with async_playwright() as p:
        await scenario(p, "live stream", None)
        await scenario(p, "stream down", lambda route: route.abort())

        async def silent(route):   # connects, then closes without delivering any event (a reconnect that lost the gap)
            await route.fulfill(status=200, headers={"content-type": "text/event-stream"}, body="retry: 60000\n\n")
        await scenario(p, "stream lost events", silent)
    print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
    sys.exit(1 if fails else 0)

asyncio.run(main())
