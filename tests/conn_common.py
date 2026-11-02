"""Helpers for the real-model suites of the everyday chat features (memory_real, news_report_real), on top of
tests/user_common.py and tests/shop_common.py: OMuse's own pages as the user reads them (Memory, Automations, the
other views). Nothing here reads OMuse's insides (no tool calls, events, audit or internal APIs)."""
from __future__ import annotations

import re
import time

from tests import user_common as U


# ================================================================================================ Memory and the other views
async def memory_view(u: U.User) -> None:
    pg = u.page
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("table.memtable, .card", timeout=20000)
    await pg.wait_for_timeout(2000)


async def memory_card(u: U.User, heading: str) -> str:
    """The text of the Memory page's card whose heading has `heading` ("Pending", "Learned", "Long-term", "Entities",
    "Recent", "Vault", "Profile", …)."""
    await memory_view(u)
    card = u.page.locator(".card", has=u.page.locator("h3", has_text=heading)).first
    if not await card.count():
        return ""
    return " ".join((await card.inner_text()).split())


async def memory_domain(u: U.User, domain: str) -> list[str]:
    """Memory → Long-term: the user clicks the domain chip `domain` ("Preferences", "People", "Sites", "Rules", …) and
    reads the facts the table then shows."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Long-term")).first
    chip = card.locator("button.chip", has_text=re.compile("^" + re.escape(domain), re.I)).first
    if not await chip.count():
        return []
    await chip.click()
    await pg.wait_for_timeout(600)
    rows = [" ".join(t.split()) for t in await card.locator("table.memtable tbody tr").all_inner_texts()]
    await card.locator("button.chip", has_text=re.compile("^All", re.I)).first.click()
    return rows


async def memory_chips(u: U.User) -> list[str]:
    await memory_view(u)
    card = u.page.locator(".card", has=u.page.locator("h3", has_text="Long-term")).first
    return [" ".join(t.split()) for t in await card.locator("button.chip").all_inner_texts()]


async def memory_try(u: U.User, request: str) -> str:
    """Memory → "Try it": the user types a request and presses Preview; returns what the card then lists."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Try it")).first
    await card.locator("input").fill(request)
    await card.locator("button.btn.primary").click()
    # the model reads the request first: wait, as the user does, until the card stops showing that it is working
    await pg.wait_for_timeout(500)
    for _ in range(60):
        if not await card.locator("[data-loading]").count():
            break
        await pg.wait_for_timeout(500)
    return " ".join((await card.inner_text()).split())


async def memory_pending(u: U.User) -> str:
    """Memory → Pending: what the suggested changes say, with the value in each one's box."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Pending")).first
    if not await card.count():
        return ""
    vals = [await x.input_value() for x in await card.locator("input").all()]
    return " ".join((await card.inner_text()).split()) + " " + " ".join(vals)


async def memory_accept_pending(u: U.User, words: str) -> bool:
    """Memory → Pending: the user presses Accept on the suggested change whose text has `words`."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Pending")).first
    rows = card.locator(".stack", has=pg.locator("button.btn.approve"))
    for i in range(await rows.count()):
        r = rows.nth(i)
        txt = (await r.inner_text()) + " " + " ".join([await x.input_value() for x in await r.locator("input").all()])
        if words.lower() in txt.lower():
            await r.locator("button.btn.approve").first.click()
            await pg.wait_for_timeout(1500)
            return True
    return False


async def profile_value(u: U.User, label: str) -> str:
    """Memory → Profile: the value in the field labelled `label` ("Mobile", "Phone", …)."""
    await memory_view(u)
    pg = u.page
    f = pg.locator("#profile label.field", has_text=label).first
    if not await f.count():
        f = pg.locator("label.field", has_text=label).first
    if not await f.count():
        return ""
    el = f.locator("input, select").first
    return await el.input_value() if await el.count() else ""


async def page_text(u: U.User, view: str) -> str:
    """What one of the app's pages shows (`connections`, `schedules`, `memory`, …), as text."""
    pg = u.page
    await pg.goto(U.APP + "#" + view)
    await pg.wait_for_timeout(3000)
    return " ".join((await pg.locator("#view").inner_text()).split())


# ================================================================================================ Automations
async def answers_in(u: U.User, words: str, button: str = "Results") -> int:
    """How many answers the card's run conversation (`button`: "Results" / "View runs") shows now."""
    from tests import shop_common as SC
    await SC.press_on_card(u, words, button)
    await u.page.wait_for_timeout(2000)
    n = await u.page.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").count()
    await u.page.goto(U.APP + "#schedules")
    await u.page.wait_for_timeout(1500)
    return n


async def run_results(u: U.User, words: str, button: str = "Results", prev: int = 0, timeout: float = 600) -> tuple[str, list[str]]:
    """After a run started from Automations: the user opens the card's `button` ("Results" / "View runs") and waits
    for the run's answer. Any approval the run asks for is denied (the user asked for a summary, not for something
    to be sent). `prev`: the answers the conversation already had (an earlier run's). Returns (the new answer, the
    approval dialogs' texts)."""
    from tests import shop_common as SC
    pg = u.page
    denied: list[str] = []
    await SC.press_on_card(u, words, button)
    end = time.monotonic() + timeout
    seen = 0
    while time.monotonic() < end:
        modal = pg.locator(".modal-back .approval")
        if await modal.count() and not await modal.first.locator("button.btn.approve:disabled").count():
            denied.append(" ".join((await modal.first.inner_text()).split()))
            await modal.first.locator("button.btn.danger").first.click()
            await pg.wait_for_timeout(1000)
            continue
        review = pg.locator(".sys-card button.btn.approve")
        if await review.count() and len(denied) < 3:
            await review.first.click()
            await pg.wait_for_timeout(800)
            continue
        n = await pg.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").count()
        live = await pg.locator(".msg.assistant.live").count()
        busy = await pg.locator(".taskcard:not(.folded)").count()
        if n > prev and not live and not busy:
            return await pg.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").last.inner_text(), denied
        await pg.wait_for_timeout(3000)
        seen += 1
        if n <= prev and seen % 5 == 0:   # the conversation fills in when the run writes to it: look again
            await pg.goto(U.APP + "#schedules")
            await pg.wait_for_timeout(1500)
            await SC.press_on_card(u, words, button)
    return "", denied
