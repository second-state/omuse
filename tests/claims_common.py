"""Helpers for the insurance, claims and refunds suites with a real model (insurance_real.py, insurance_hk_real.py,
claims_real.py, emergency_real.py, refunds_real.py), on top of tests/user_common.py. Everything here reads what the
user sees: their mailbox as their mail app shows it (with the attachments), the answer's dates and phone numbers, the
app's own pages (Automations, Status → Ledger) and the browser view at a takeover."""
from __future__ import annotations

import datetime as dt
import email
import email.policy
import imaplib
import re

from tests import skills_common as K
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


def mail_full(folder: str = "INBOX") -> list[dict]:
    """The messages in a folder of the user's mailbox with their attachments, as the user's mail app shows them:
    {"from", "to", "subject", "body", "attachments": [(filename, bytes)]}."""
    m = imaplib.IMAP4("127.0.0.1", 1143)
    m.login(K.USER, K.PW)
    out = []
    try:
        if m.select(f'"{folder}"', readonly=True)[0] == "OK":
            _, ids = m.search(None, "ALL")
            for i in (ids[0] or b"").split():
                _, d = m.fetch(i, "(RFC822)")
                msg = email.message_from_bytes(d[0][1], policy=email.policy.default)
                part = msg.get_body(preferencelist=("plain", "html"))
                atts = [(a.get_filename() or "", a.get_payload(decode=True) or b"") for a in msg.iter_attachments()]
                out.append({"from": str(msg["From"] or ""), "to": str(msg["To"] or ""), "subject": str(msg["Subject"] or ""),
                            "body": part.get_content() if part else "", "attachments": atts})
    finally:
        m.logout()
    return out


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


async def numbered_steps(u: U.User) -> int:
    """How many numbered steps the last answer shows: the items of its numbered lists (the numbers are drawn by the
    browser, not in the text), or lines that start with "1." "2." … when written out."""
    n = await u.page.evaluate("() => { const b = [...document.querySelectorAll('.msg.assistant:not(.live) .bubble')].pop();"
                              " return b ? b.querySelectorAll('ol > li').length : 0; }")
    txt = await u.page.evaluate("() => { const b = [...document.querySelectorAll('.msg.assistant:not(.live) .bubble')].pop(); return b ? b.innerText : ''; }")
    return max(n, len(re.findall(r"(?m)^\s*\d+[.)]\s", txt)))


async def ledger_rows(u: U.User) -> list[str]:
    """The rows of Status → Ledger as the user reads them (date, merchant, order number, amount, card, status, proof)."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#trust")
    card = pg.locator(".card", has_text="Ledger")
    rows: list[str] = []
    try:
        await card.locator("table.data, .muted").first.wait_for(timeout=20000)
        rows = [" ".join(t.split()) for t in await card.locator("tbody tr").all_inner_texts()]
    except Exception:   # noqa: BLE001 — no ledger card: no rows
        pass
    print(f"INFO ledger rows: {rows}")
    await pg.goto(back)
    await pg.wait_for_selector("#chatInput", timeout=20000)
    return rows
