"""Helpers for the real-model suites that use the Automations page (assistant_real, news_report_real), on top of
tests/user_common.py: the app's own pages as the user reads them, and the buttons the user presses on an Automations
card."""
from __future__ import annotations

import re

from tests import user_common as U


async def app_page(u: U.User, view: str, wait: str = "main") -> str:
    """What one of the app's own pages shows (`schedules` Automations, `memory`, `connections`, …), as text."""
    pg = u.page
    await pg.goto(U.APP + "#" + view)
    try:
        await pg.wait_for_selector(wait, timeout=15000)
    except Exception:   # noqa: BLE001 — an empty page reads as empty
        pass
    await pg.wait_for_timeout(2500)
    return await pg.locator("#view, main, body").first.inner_text()


def _cards(pg):
    """The Automations page's own cards (a watch, a trigger, a schedule): the innermost cards with a Check now / Run now."""
    return pg.locator(".card.stack:not(:has(.card))", has=pg.locator("button", has_text=re.compile("Check now|Run now")))


async def automation_cards(u: U.User) -> list[str]:
    """Every watch, trigger and schedule card on Automations, as the user reads them."""
    if "#schedules" not in u.page.url:
        await u.page.goto(U.APP + "#schedules")
        await u.page.wait_for_timeout(2500)
    return [" ".join(t.split()) for t in await _cards(u.page).all_inner_texts()]


async def automation_card(u: U.User, words: str) -> str:
    """The Automations card (trigger, watch or schedule) whose text has `words`, as the user reads it ('' if none)."""
    pg = u.page
    await pg.goto(U.APP + "#schedules")
    await pg.wait_for_timeout(2500)
    for t in await automation_cards(u):
        if words.lower() in t.lower():
            return " ".join(t.split())
    return ""


async def press_on_card(u: U.User, words: str, button: str) -> None:
    """The user presses `button` ("Check now", "Run now", "Results") on the Automations card with `words`."""
    pg = u.page
    if "#schedules" not in pg.url:
        await pg.goto(U.APP + "#schedules")
        await pg.wait_for_timeout(2500)
    card = _cards(pg).filter(has_text=words).first
    await card.locator("button", has_text=button).first.click()
    await pg.wait_for_timeout(1500)
