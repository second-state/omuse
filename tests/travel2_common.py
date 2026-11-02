"""Helpers for the travel skills' real-model suites (travel_info_real.py, travel_receipts_real.py), on top of
tests/user_common.py: the photos the user takes, attaching them, and what the user sees; none of them looks inside the
system."""
from __future__ import annotations

import datetime as dt
import os
import re

from tests import user_common as U
from tests.user_common import check


def day(d: dt.date) -> str:
    return f"{d.day} {d:%B} {d.year}"


# ---------------------------------------------------------------------------------------------------- the user's things
async def photo(u: U.User, html: str, path: str, width: int = 480) -> str:
    """A photo the user took of a paper document (a menu, a receipt): `html` drawn in a browser at phone width and
    saved as a PNG at `path`."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    ctx = await u._br.new_context(viewport={"width": width, "height": 400}, device_scale_factor=2)
    try:
        p = await ctx.new_page()
        await p.set_content(html, wait_until="load")
        await p.screenshot(path=path, full_page=True)
    finally:
        await ctx.close()
    return path


async def attach(u: U.User, paths: list[str]) -> None:
    """The user clicks ＋ in the composer and picks files."""
    pg = u.page
    await pg.locator(".composer input[type=file]").set_input_files(paths)
    await pg.wait_for_function(f"() => document.querySelectorAll('.composer .chip-att').length >= {len(paths)} && "
                               "[...document.querySelectorAll('.composer .chip-att .small')].every(e => /B$/.test(e.textContent.trim()))",
                               timeout=60000)


# ---------------------------------------------------------------------------------------------------- the user's side
def answered(label: str, a: U.Answer, *, browser_ok: bool = True) -> None:
    """The answer came, and the user was never asked to take over a browser (nothing here needs the user's hands)."""
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check(f"{label}: the user was never asked to take over a browser", not a.takeovers, a.takeovers)
    if not browser_ok:
        check(f"{label}: no approval dialog", not a.approvals, a.approvals)


def numbers(text: str) -> list[float]:
    """Every number written in `text` (thousands separators dropped)."""
    out = []
    for m in re.findall(r"\d[\d,]*(?:\.\d+)?", text):
        try:
            out.append(float(m.replace(",", "")))
        except ValueError:
            pass
    return out
