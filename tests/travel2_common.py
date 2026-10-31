"""Helpers for the travel skills' real-model suites (trip_hotel_real.py, travel_info_real.py, travel_receipts_real.py,
city_guides2_real.py), on top of tests/user_common.py and tests/claims_common.py. They set up the
user's world (a calendar the user subscribes to, the photos the user takes) and read what the user sees; none of them
looks inside the system."""
from __future__ import annotations

import datetime as dt
import os
import re

from tests import claims_common as C
from tests import skills_common as K
from tests import user_common as U
from tests.user_common import check

PAGES_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
date_in = C.date_in


def day(d: dt.date) -> str:
    return f"{d.day} {d:%B} {d.year}"


# ---------------------------------------------------------------------------------------------------- the user's world
def subscribe_calendar(name: str, events: list[tuple[str, dt.datetime, dt.datetime, str, str]]) -> None:
    """The user's calendar: an iCal feed they subscribe to (Settings → Connections → Calendar → iCal address), with
    `events` = (summary, start, end, IANA time zone, location). The feed is a file next to the test pages, served at
    http://127.0.0.1:8099/<name>.ics."""
    def stamp(t: dt.datetime) -> str:
        return t.strftime("%Y%m%dT%H%M%S")
    body = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Lucas Lu//Calendar//EN", "X-WR-CALNAME:Lucas Lu",
            "X-WR-TIMEZONE:Asia/Singapore"]
    for i, (summary, start, end, tz, where) in enumerate(events):
        body += ["BEGIN:VEVENT", f"UID:{name}-{i}@lucas.example", f"DTSTAMP:{stamp(dt.datetime.utcnow())}Z",
                 f"DTSTART;TZID={tz}:{stamp(start)}", f"DTEND;TZID={tz}:{stamp(end)}", f"SUMMARY:{summary}",
                 f"LOCATION:{where}", "END:VEVENT"]
    body.append("END:VCALENDAR")
    with open(os.path.join(PAGES_DIR, f"{name}.ics"), "w", encoding="utf-8") as f:
        f.write("\r\n".join(body) + "\r\n")
    r = K.c.post(K.S + "/connections/calendar/ical", json={"url": f"http://127.0.0.1:8099/{name}.ics", "time_zone": "Asia/Singapore"},
                 headers=K.H)
    check("setup: the user's calendar is connected (iCal)", r.status_code == 200 and r.json().get("ok"), r.text[:300])


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
    """The user clicks ＋ in the composer and picks files (tests/apdt_common.py's way)."""
    pg = u.page
    await pg.locator(".composer input[type=file]").set_input_files(paths)
    await pg.wait_for_function(f"() => document.querySelectorAll('.composer .chip-att').length >= {len(paths)} && "
                               "[...document.querySelectorAll('.composer .chip-att .small')].every(e => /B$/.test(e.textContent.trim()))",
                               timeout=60000)


# ---------------------------------------------------------------------------------------------------- the user's side
def no_send(dialog: str) -> bool:
    """The user asked for drafts: they deny sending an email and approve anything else."""
    yes = not re.search(r"\bsend|reply|forward", dialog, re.I)
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:300]}")
    return yes


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


def near(values: list[float], target: float, rel: float = 0.01) -> bool:
    return any(abs(v - target) <= rel * abs(target) for v in values)


# ---------------------------------------------------------------------------------------------------- city guides
async def guides(cases: list[tuple[str, str, list[str], list[str]]]) -> None:
    """Each guide answers its own city's question from its content: the user opens the Travel bot, asks, and the answer
    carries facts only that guide holds (two groups of words: at least one of each), without the user being asked to
    approve anything or take over a browser."""
    async with U.User() as u:
        for name, prompt, facts, more in cases:
            await u.open_bot("Travel")
            a = await u.ask(prompt, label=name, new_chat=False)
            check(f"{name}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])
            check(f"{name}: no approval dialog and no request to take over a browser", not a.approvals and not a.takeovers,
                  (a.approvals, a.takeovers))
            text = a.text.replace("\u2019", "'")
            check(f"{name}: the answer carries the guide's facts ({facts[0]} …)",
                  any(re.search(f, text, re.I) for f in facts) and any(re.search(m, text, re.I) for m in more), a.text[:900])
