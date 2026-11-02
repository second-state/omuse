"""What the credit-card real suites (tests/cards_*_real.py) share, all from the user's point of view
(tests/user_common.py): the Vault card on the Memory page as the user reads it, adding a card there by its name, and
the patterns the checks read in an answer or a block (a card number, a date, an amount, the catalog's date)."""
from __future__ import annotations

import datetime as dt
import re

from tests import user_common as U

# a card number (13–19 digits, spaces or dashes allowed) anywhere in what the user sees
CARD_NO = re.compile(r"\b(?:\d[ -]?){13,19}\b")
# the date the card terms were verified, as the catalog gives it (August 2026), in any common form
CATALOG_DATE = re.compile(r"2026-08|Aug(ust)?\.? (\d{1,2},? )?2026|\d{1,2} Aug(ust)? 2026|08/2026", re.I)


def date_in(text: str, d: dt.date) -> bool:
    """`d` written as a date: ISO, US or day-first, month spelled out or abbreviated, with or without the year."""
    forms = {d.isoformat(), f"{d:%B} {d.day}", f"{d:%b} {d.day}", f"{d.day} {d:%B}", f"{d.day} {d:%b}", f"{d.month}/{d.day}/{d.year}",
             f"{d:%m/%d/%Y}", f"{d:%b}. {d.day}"}
    return any(re.search(rf"(?<![\w]){re.escape(f)}(?!\d)", text, re.I) for f in forms)


def money_in(text: str, amount: str) -> bool:
    """`amount` ("1753.60") written in the text with or without the thousands comma and the trailing zero."""
    whole, cents = amount.split(".")
    w = re.escape(whole) if len(whole) <= 3 else re.escape(whole[:-3]) + ",?" + re.escape(whole[-3:])
    c = cents.rstrip("0") or "0"
    return bool(re.search(rf"(?<![\d,.]){w}\.{c}0?(?!\d)" if cents != "00" else rf"(?<![\d,.]){w}(\.00?)?(?![\d,])", text))


def norm(name: str) -> str:
    """A card's name as the Vault compares it: case and spacing aside."""
    return " ".join(name.lower().split())


async def _back(u: U.User, url: str) -> None:
    await u.page.goto(url)
    if "#chat" in url or "#" not in url:
        await u.page.wait_for_selector("#chatInput", timeout=20000)
    await u.page.wait_for_timeout(800)


async def vault_cards(u: U.User) -> list[dict]:
    """The credit cards on the Memory page's Vault card, as the user reads them: {"name", "country", "line" (the whole
    row's text), "name_only" (the "name only: add the number…" mark)}. The user then goes back to the chat they were in."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("#memProfileVault", timeout=20000)
    await pg.wait_for_timeout(1000)
    cards = await pg.evaluate("""() => [...document.querySelectorAll('#memProfileVault .vaultitem[data-kind="card"]')].map(r => ({
        name: (r.querySelector('b') || {}).innerText || '',
        country: ((r.querySelector('.cardcountry') || {}).innerText || '').replace(/^\\s*·\\s*/, ''),
        line: r.innerText.replace(/\\s+/g, ' '),
        name_only: !!r.querySelector('.namedcard')}))""")
    print(f"INFO Memory → Vault shows the cards: {[(x['name'], x['country'], x['name_only']) for x in cards]}")
    await _back(u, back)
    return cards


async def memory_page_text(u: U.User) -> str:
    """Everything the Memory page shows (pending changes, profile, Vault, what was learned), as text."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("#memProfileVault", timeout=20000)
    await pg.wait_for_timeout(1000)
    text = await pg.locator("#view, main").first.inner_text()
    await _back(u, back)
    return text


async def add_card_on_vault(u: U.User, name: str, country: str = "") -> None:
    """The user adds a card on the Memory page's Vault card by its name (and the country when given): "+ Add credit
    card", the name, Save — the number, expiry and CVC left empty."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("#addCardBtn", timeout=20000)
    await pg.click("#addCardBtn")
    country_box = pg.locator("#memProfileVault input.cardcountry")
    await country_box.wait_for(timeout=10000)
    await country_box.locator("xpath=preceding-sibling::input[1]").fill(name)
    if country:
        await country_box.fill(country)
    await pg.locator("#memProfileVault button.btn.primary").last.click()
    await pg.wait_for_function("n => [...document.querySelectorAll('#memProfileVault .vaultitem b')].some(b => b.innerText === n)",
                               arg=name, timeout=15000)
    print(f"INFO the user added {name!r} ({country or 'no country'}) on the Vault card")
    await _back(u, back)


def find(cards: list[dict], *words: str) -> list[dict]:
    """The Vault's cards whose name has every one of `words` (case aside)."""
    return [c for c in cards if all(w.lower() in c["name"].lower() for w in words)]
