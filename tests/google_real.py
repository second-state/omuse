"""Google Calendar and Google Drive (0.6.7) with a real model and the test user's real Google account, from the user's
point of view (tests/user_common.py, tests/google_common.py). A real-data suite (run_in_docker.sh REAL_DATA_SUITES);
skipped (a SKIP line) without OMUSE_E2E_GOOGLE_* in the env file.

THE USER'S RULES (2026-11-05): nothing is added to the calendar — it is connected with write permission off, every
calendar approval dialog is declined, and the suite checks at the end that the calendar's events are the same as at
the start; a file may be added to Drive, and every file the suite adds is permanently deleted at the end.

1. connections — the Calendar and Drive cards list the account, the default marked; the calendar's write is off.
2. calendar — "What's on my calendar tomorrow?": the answer names at least one of tomorrow's events (the suite reads
   the same calendar itself); "When am I free tomorrow afternoon for an hour?": a time in the answer. No dialogs.
3. what's next — "What's next for me today?": today's list (and, when the calendar has events today, one of them);
   "Nothing to add, thanks.": the report is linked and opens, with today's events.
4. drive — a text file attached in the chat and "Save this file to my Google Drive.": the upload is approved, the file
   is in Drive (the suite looks); the picker's settings answer (key, app id, a token per drive) and the file comes back
   through the picker's fetch; ＋ offers "From Google Drive".
5. accounts — a second calendar account on the card: made the default, then disconnected; one account is left.
6. cleanup — the files the suite added are deleted from Drive; the calendar is unchanged.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh google_real"""
import datetime as dt
import os
import sys
from zoneinfo import ZoneInfo

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import google_common as G  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

if not G.READY:
    print("SKIP google_real: no OMUSE_E2E_GOOGLE_* (client, picker key, project number, refresh tokens) in the env file")
    raise SystemExit(0)

PREFIX = "omuse-e2e-"
STAMP = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
FILE = f"/tmp/{PREFIX}{STAMP}.txt"
CONTENT = f"OMuse e2e test file {STAMP}. Safe to delete."


def no_calendar_writes(dialog: str) -> bool:
    """The user's decision on an approval dialog: never anything on the calendar; the Drive upload is fine."""
    yes = not U.tool_of(dialog).startswith("calendar_")
    print(f"INFO approval dialog ({'approve' if yes else 'DENY'}): {dialog[:200]}")
    return yes


def window(tz: str, days_from: int, days: int) -> tuple[str, str, dt.date]:
    d0 = dt.datetime.now(ZoneInfo(tz)).date() + dt.timedelta(days=days_from)
    a = dt.datetime.combine(d0, dt.time(0), ZoneInfo(tz))
    return a.isoformat(), (a + dt.timedelta(days=days)).isoformat(), d0


async def main():
    tz = G.calendar_tz()
    U.c.put(U.B + "/api/settings", json={"timezone": tz}, headers=U.H)   # the user's own time zone (world, not a check)
    w0, w1, _ = window(tz, 0, 4)
    before = G.events(w0, w1)
    print(f"INFO calendar {tz}: {len(before)} events today … +3 days")
    try:
        async with U.User() as u:
            pg = u.page
            # 1. the cards
            conns = U.c.get(U.SENTINEL + "/connections", headers=U.H).json()
            conns = conns.get("connections", conns)
            cal = next((c for c in conns if c.get("name") == "calendar"), {})
            check("connections: the calendar's write permission is off (the user's rule)", cal.get("permissions", {}).get("write") is False,
                  cal.get("permissions"))
            await pg.goto(U.APP + "#connections")
            await pg.wait_for_timeout(2500)
            body = await pg.inner_text("body")
            check("connections: the Calendar and Drive cards list the account, the default marked",
                  body.count(G.EMAIL) >= 2 and "Default" in body, body[:400])

            # 2. reading the calendar
            t0, t1, tomorrow = window(tz, 1, 1)
            tomorrows = [e["summary"] for e in G.events(t0, t1) if e["summary"]]
            a = await u.ask("What's on my calendar tomorrow?", label="tomorrow", approve=no_calendar_writes, timeout=600)
            check("calendar: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
            check("calendar: no dialogs, no takeover (reading only)", not a.approvals and not a.takeovers, (a.approvals, a.takeovers))
            if tomorrows:
                check("calendar: the answer names at least one of tomorrow's events (as the calendar has them)",
                      any(s.lower()[:30] in a.text.lower() for s in tomorrows), (tomorrows[:5], a.text[:600]))
            else:
                print("INFO calendar: no events tomorrow")
            a = await u.ask("When am I free tomorrow afternoon for an hour?", label="free", approve=no_calendar_writes, timeout=600)
            check("free: the answer gives a time", any(ch.isdigit() for ch in a.text) and ":" in a.text, a.text[:400])
            check("free: no dialogs, no takeover", not a.approvals and not a.takeovers, (a.approvals, a.takeovers))

            # 3. What's next, with nothing added
            d0, d1, today = window(tz, 0, 1)
            todays = [e["summary"] for e in G.events(d0, d1) if e["summary"]]
            a = await u.ask("What's next for me today?", label="whats-next", approve=no_calendar_writes, timeout=1200)
            check("whats-next: today's list (Time / What / Where)", all(x in a.block for x in ("Today", "Time", "What", "Where")),
                  a.block[:600])
            if todays:
                check("whats-next: the list has one of today's calendar events",
                      any(s.lower()[:30] in a.block.lower() for s in todays), (todays[:5], a.block[:600]))
            a = await u.ask("Nothing to add, thanks.", label="whats-next report", new_chat=False, approve=no_calendar_writes,
                            timeout=1800)
            links = [x for x in a.block_links if "/artifacts/whats-next/" in x]
            check("whats-next: the report is linked", bool(links), (a.block[:600], a.block_links))
            if links:
                r = await pg.request.get(links[0])
                html = await r.text()
                check("whats-next: the report opens with today's section", r.status == 200 and "<h2>Today</h2>" in html, r.status)
                if todays:
                    check("whats-next: the report has one of today's events",
                          any(s.lower()[:30] in html.lower() for s in todays), todays[:5])

            # 4. Drive: an upload the user approves, then the picker's side
            with open(FILE, "w") as f:
                f.write(CONTENT)
            chips = await u.attach(FILE)
            check("drive: the file is attached", len(chips) == 1 and not chips[0]["bad"], chips)
            a = await u.ask("Save this file to my Google Drive.", label="drive upload", new_chat=False,
                            approve=no_calendar_writes, timeout=900)
            name = os.path.basename(FILE)
            found = G.drive_files(name)
            check("drive: the file is in the user's Drive (the suite looks)", len(found) >= 1, (found, a.text[:300]))
            pick = U.c.get(U.SENTINEL + "/connections/drive/picker", headers=U.H).json()
            check("drive: the picker's settings answer (API key, app id, a token for the drive)",
                  pick.get("ok") and pick.get("api_key") and pick.get("app_id") == G.E["PROJECT_NUMBER"]
                  and any(d.get("token") for d in pick.get("drives", [])),
                  {k: (v if k != "drives" else len(v)) for k, v in pick.items() if k != "api_key"})
            if found:
                r = U.c.get(U.SENTINEL + "/connections/drive/file", params={"id": found[0]["id"]}, headers=U.H)
                check("drive: a picked file comes back through the picker's fetch (its bytes and its name)",
                      r.status_code == 200 and r.text == CONTENT and name in r.headers.get("x-file-name", ""),
                      (r.status_code, r.text[:80]))
            await pg.locator("button.attach").click()
            menu = await pg.locator(".attach-menu").inner_text()
            check("drive: ＋ offers this device and Google Drive", "From this device" in menu and "From Google Drive" in menu, menu)
            await pg.keyboard.press("Escape")

            # 5. a second calendar account on the card: made the default, then disconnected (the local entry only)
            G.add_second_calendar("second.calendar@example.test")
            await pg.goto(U.APP + "#connections")
            await pg.wait_for_selector("text=second.calendar@example.test", timeout=15000)
            row = pg.locator(".row", has_text="second.calendar@example.test").first
            await row.locator("button", has_text="Make default").click()
            await pg.wait_for_timeout(1500)
            cal = next(c for c in U.c.get(U.SENTINEL + "/connections", headers=U.H).json().get("connections", []) if c.get("name") == "calendar")
            check("accounts: the second calendar became the default", cal.get("default") == "c2", cal.get("default"))
            pg.once("dialog", lambda d: d.accept())
            row = pg.locator(".row", has_text="second.calendar@example.test").first
            await row.locator("button", has_text="Disconnect").click()
            await pg.wait_for_timeout(1500)
            cal = next(c for c in U.c.get(U.SENTINEL + "/connections", headers=U.H).json().get("connections", []) if c.get("name") == "calendar")
            emails = [x.get("email") for x in cal.get("accounts", [])]
            check("accounts: disconnected; the user's own calendar is left, and the default again",
                  emails == [G.EMAIL] and cal.get("default") == "c1", (emails, cal.get("default")))
    finally:
        # 6. the user's rules: the files the suite added are deleted, the calendar is as it was
        gone = G.delete_drive_files(PREFIX)
        print(f"INFO cleanup: deleted from Drive {gone}")
        check("cleanup: no file the suite added is left in Drive", not G.drive_files(os.path.basename(FILE)), gone)
        after = G.events(w0, w1)
        added = [e for e in after if e not in before]
        check("cleanup: nothing was added to the calendar (today … +3 days, before vs after)", not added, added[:5])


U.setup(profile={"name_en": "MICHAEL YUAN", "email_work": G.EMAIL})
G.save_client()
G.connect_accounts(G.calendar_tz())
U.run(main)
