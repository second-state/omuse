"""Helpers for the insurance, claims and travel suites with a real model (insurance_real.py, insurance_hk_real.py,
claims_real.py, emergency_real.py, travel_info_real.py, travel_receipts_real.py), on top of tests/user_common.py.
Everything here reads what the user sees: the answer's dates and phone numbers, a filled PDF form's values and the
app's own pages (Automations, Memory)."""
from __future__ import annotations

import datetime as dt
import re

from tests import user_common as U

PHONE = re.compile(r"\+\d{1,3}[\s-]?\(?\d{1,4}\)?(?:[\s-]?\d{2,4}){2,4}")
# the answer says the claim / the form / the payment was done for the user (not "has not been submitted", "you submit")
_SUBMITTED = re.compile(r"(?<!\bnot )(?<!\bno )\b(claim|form|application|it) (has been|was|is now) (successfully )?(submitted|filed|sent)"
                        r"|\bI( have|'ve)? (submitted|filed|sent|paid|pressed (submit|pay))\b", re.I)
_SENT = re.compile(r"\bI( have|'ve)? (sent|emailed)\b|\b(email|e-mail|draft|claim|message|letter|it) (has been|was|is now) sent\b", re.I)
_NEGATED = re.compile(r"\b(no|not|nothing|never|unsent|neither|nor|none)\b|n['’]t\b", re.I)


class _Said:
    """A pattern said as done — not in a sentence that negates it ("No email was sent", "nothing has been submitted")."""

    def __init__(self, rx: re.Pattern):
        self.rx = rx

    def search(self, text: str):
        for m in self.rx.finditer(text or ""):
            start = max(text.rfind(".", 0, m.start()), text.rfind("\n", 0, m.start())) + 1
            if not _NEGATED.search(text[start:m.start()]):
                return m
        return None


SUBMITTED = _Said(_SUBMITTED)
SENT = _Said(_SENT)


def date_in(text: str, d: dt.date) -> bool:
    """`d` written as a date: ISO, US or day-first, month spelled out or abbreviated, with or without the year."""
    forms = {d.isoformat(), f"{d:%B} {d.day}", f"{d:%b} {d.day}", f"{d.day} {d:%B}", f"{d.day} {d:%b}", f"{d.month}/{d.day}/{d.year}",
             f"{d:%m/%d/%Y}", f"{d:%b}. {d.day}", f"{d.day}/{d.month}/{d.year}", f"{d:%d/%m/%Y}", f"{d.day}.{d.month}.{d.year}"}
    return any(re.search(rf"(?<![\w/.-]){re.escape(f)}(?![\d])", text, re.I) for f in forms)


def phones(text: str) -> list[str]:
    """The international phone numbers written in `text`, digits only."""
    return sorted({re.sub(r"\D", "", m) for m in PHONE.findall(text)})


async def page_text(u: U.User, view: str, wait: str = "main") -> str:
    """What one of the app's own pages shows (`#schedules` Automations, `#trust` Status with the ledger, …), as text."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#" + view)
    try:
        await pg.wait_for_selector(wait, timeout=15000)
    except Exception:   # noqa: BLE001 — an empty page reads as empty
        pass
    await pg.wait_for_timeout(2500)
    txt = await pg.locator("body").inner_text()
    await pg.goto(back)
    await pg.wait_for_selector("#chatInput", timeout=20000)
    return txt


def pdf_values(data: bytes) -> dict[str, str]:
    """The filled-in values of a PDF form, as a PDF reader shows them (field → text)."""
    import io

    from pypdf import PdfReader
    try:
        return {k: str(v.get("/V", "") or "") for k, v in (PdfReader(io.BytesIO(data)).get_fields() or {}).items()}
    except Exception:   # noqa: BLE001 — not a readable PDF form
        return {}


def flat(s: str) -> str:
    return re.sub(r"\s+", "", s or "").upper()
