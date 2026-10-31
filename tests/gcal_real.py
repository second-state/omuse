"""Google Calendar with a real model, from the user's point of view (tests/user_common.py, tests/shop_common.py). The
user's Google account (the fake Google of tests/fake_apps.py stands in for it) has a "Board meeting" from 14:00 to
15:00 (Singapore time) next Tuesday; OMuse ships its one-click Google sign-in.

1. connect — Connections → Google Calendar → "Sign in with Google": the user signs in on Google's page and lands back
   in OMuse. Checked: the page says the calendar is connected with the user's Google account (lucas@example.com).
2. invite — "find a free hour on Tuesday afternoon for a catch-up with Eva and invite her". Adding an event with a guest
   needs the user's approval: the dialog shows the guest and the proposed time, which must be free (not inside the
   board meeting); the user moves it to 15:30 in the dialog (Start and End, keeping its length) and approves. Checked: the user's calendar then
   has the catch-up on that Tuesday at 15:30 with Eva invited, the board meeting is untouched, the agent does not try to
   move the event again (the user would deny that), and the answer gives the time as the one the user chose (not as
   a mistake of the calendar's).
One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh gcal_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TODAY = dt.date.today()
TUE = TODAY + dt.timedelta(days=(1 - TODAY.weekday()) % 7 or 7)
EVA = "eva@example.com"
# the answer treats the user's change in the dialog as a fault: "created it at 3:30 pm rather than the 1:00 pm slot I
# initially selected", "although I requested 3:00 pm", "not the hour you asked for … I tried to correct it"
MISTAKE = re.compile(r"(although|but|though|rather than|instead of|not the|than)\b[^.]{0,60}\bI (had )?(initially |originally )?"
                     r"(requested|selected|chose|picked|proposed|asked for)|tried to (correct|fix)|(calendar|it) (created|saved|moved|set|shows)"
                     r"[^.]{0,80}(rather than|instead of|later than|not the)", re.I)


def minutes(hhmm: str) -> int:
    h, m = hhmm.split(":")
    return int(h) * 60 + int(m)


async def main():
    async with U.User() as u:
        landed = await S.connect_google_calendar(u)
        card = " ".join((await S.app_page(u, "connections")).split())
        conn = re.search(r"Google Calendar.{0,200}", card)
        print(f"INFO Connections: {conn.group(0) if conn else card[:400]!r}")
        check("connect: after signing in with Google the calendar shows as connected with the user's account",
              bool(conn) and "lucas@example.com" in conn.group(0) and re.search(r"Connected", conn.group(0)), (landed[:300], card[:600]))

        seen = {"start": "", "text": "", "later": []}

        async def review(modal, info) -> bool:
            """The user reads the dialog and moves the catch-up to 15:30, keeping its length (Start and End); having
            decided the time, they deny any later dialog that would change the event again."""
            if seen["start"]:
                seen["later"].append(info["text"][:160])
                return False
            seen["text"] = info["text"] + " " + str(info["values"])
            keys = list(info["values"])
            ks, ke = next((k for k in keys if "Start" in k), ""), next((k for k in keys if "End" in k), "")
            hs = re.search(r"\d{1,2}:\d\d", info["values"].get(ks, ""))
            if hs and not seen["start"]:
                seen["start"] = info["values"][ks]
                await modal.locator("dd input").nth(keys.index(ks)).fill(seen["start"].replace(hs.group(0), "15:30", 1))
                he = re.search(r"\d{1,2}:\d\d", info["values"].get(ke, ""))
                if he:
                    end = 15 * 60 + 30 + max(30, minutes(he.group(0)) - minutes(hs.group(0)))
                    await modal.locator("dd input").nth(keys.index(ke)).fill(info["values"][ke].replace(he.group(0), f"{end // 60}:{end % 60:02d}", 1))
            return True

        a = await S.ask(u, f"Find a free hour on Tuesday afternoon ({TUE:%-d %B}) in my Google Calendar for a catch-up with Eva ({EVA}), "
                           "then add it and invite her.", label="invite", on_approval=review)
        print(f"INFO proposed start {seen['start']!r}; dialog {seen['text'][:400]!r}")
        check("invite: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("invite: adding the event with a guest asked for the user's approval, showing Eva", EVA in seen["text"], a.approvals)
        hm = re.search(r"(\d{1,2}):(\d\d)", seen["start"])
        start = minutes(hm.group(0)) if hm else -1
        check("invite: the proposed time is on that Tuesday, in the afternoon, and free (not during the 14:00–15:00 board meeting)",
              TUE.isoformat() in seen["start"] and 12 * 60 <= start < 18 * 60 and (start + 60 <= 14 * 60 or start >= 15 * 60), seen["start"])
        evs = S.calendar_events()
        print(f"INFO the user's calendar: {[(e.get('summary'), e['start'].get('dateTime'), [x.get('email') for x in e.get('attendees', [])]) for e in evs]}")
        mine = [e for e in evs if "board" not in (e.get("summary") or "").lower()]
        check("invite: the user's calendar has the catch-up on that Tuesday at 15:30 (the time the user set)",
              any((e["start"].get("dateTime") or "").startswith(f"{TUE.isoformat()}T15:30") for e in mine), mine)
        check("invite: … only once, and the agent did not try to change the user's time afterwards (no second dialog)",
              len(mine) == 1 and not seen["later"], (mine, seen["later"]))
        check("invite: … with Eva invited", any(EVA in [x.get("email") for x in e.get("attendees", [])] for e in mine), mine)
        check("invite: the board meeting is untouched", any("board" in (e.get("summary") or "").lower()
                                                             and e["start"]["dateTime"].startswith(f"{TUE.isoformat()}T14:00") for e in evs), evs)
        check("invite: the answer reports 15:30 as the user's change, not as the calendar's mistake",
              not MISTAKE.search(a.text), a.text[:600])
        check("invite: the answer gives the time (15:30 / 3:30 pm) and Eva", re.search(r"15:30|3:30\s?((–|-|to)\s?\d{1,2}:\d\d\s?)?(pm|p\.m\.)", a.text, re.I)
              and "eva" in a.text.lower(), a.text[:600])


U.setup(mail=[], vault=False, auto_approve=False)  # this suite checks the approval dialogs: the user turned "Auto-approve actions" off (the default approves everything)
S.c.post("http://127.0.0.1:8094/_g/reset")
D.disconnect_all()
D.managed_client(True)
S.calendar_add("Board meeting", f"{TUE.isoformat()}T14:00:00+08:00", f"{TUE.isoformat()}T15:00:00+08:00")
U.run(main)
