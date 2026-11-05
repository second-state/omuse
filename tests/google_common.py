"""The user's Google accounts for the real-Google suites (tests/google_real.py), from the e2e env file:
OMUSE_E2E_GOOGLE_CLIENT_ID / _CLIENT_SECRET / _PICKER_KEY / _PROJECT_NUMBER (the OAuth client of Second State's Cloud
project) and the refresh tokens of the test user's one-time consent, OMUSE_E2E_GOOGLE_CALENDAR_REFRESH (calendar scopes)
and OMUSE_E2E_GOOGLE_DRIVE_REFRESH (drive.file). Google blocks automated sign-ins, so the suite does not sign in: it
saves the client on Connections as the user would (Connections → own Google client) and puts the two accounts in the
stack's vault exactly as a finished sign-in stores them (omuse_sentinel_core::gaccounts: `cred_calendar_1` /
`cred_drive_1`, encrypted with the stack's vault key; the connection's accounts list).

THE USER'S RULES for this account (2026-11-05): nothing is ever added to the calendar — the calendar is connected
with its write permission OFF (Sentinel refuses every create, update and delete) and the suites only read it; files
may be added to Drive but every file a suite adds is permanently deleted at the end (`delete_drive_files`)."""
from __future__ import annotations

import json
import os
import sqlite3
import time
import urllib.parse
import urllib.request

from cryptography.fernet import Fernet

from tests import user_common as U

SDATA = "/tmp/claude-0/persona-test/sdata"   # the stack's Sentinel data (tests/run_local.sh [data] sentinel)
E = {k: os.environ.get(f"OMUSE_E2E_GOOGLE_{k}", "") for k in
     ("CLIENT_ID", "CLIENT_SECRET", "PICKER_KEY", "PROJECT_NUMBER", "CALENDAR_REFRESH", "DRIVE_REFRESH")}
READY = all(E.values())
EMAIL = os.environ.get("OMUSE_E2E_GOOGLE_EMAIL", "michael@secondstate.io")


def save_client() -> None:
    """Connections → Google Calendar → own Google client: the client, the secret, the Picker key and the project number
    (the API the card's form posts to)."""
    r = U.c.post(U.SENTINEL + "/connections/calendar/google/client", headers=U.H,
                 json={"client_id": E["CLIENT_ID"], "client_secret": E["CLIENT_SECRET"],
                       "picker_api_key": E["PICKER_KEY"], "project_number": E["PROJECT_NUMBER"]})
    r.raise_for_status()


def _put_secret(db: sqlite3.Connection, f: Fernet, handle: str, connector: str, payload: dict) -> None:
    blob = f.encrypt(json.dumps(payload, ensure_ascii=True).encode())
    db.execute("INSERT OR REPLACE INTO secrets(handle, connector, blob, created_at) VALUES (?,?,?,?)",
               (handle, connector, blob, time.time()))


def _put_connection(db: sqlite3.Connection, name: str, config: dict, permissions: dict) -> None:
    db.execute("INSERT OR REPLACE INTO connections(name, config, permissions, enabled, updated_at) VALUES (?,?,?,?,?)",
               (name, json.dumps(config), json.dumps(permissions), 1, time.time()))


def connect_accounts(tz: str = "America/Chicago") -> None:
    """The test user's calendar (account c1, write permission OFF) and drive (d1), as a finished sign-in stores them."""
    f = Fernet(open(os.path.join(SDATA, "vault.key"), "rb").read().strip())
    with sqlite3.connect(os.path.join(SDATA, "sentinel.db")) as db:
        rec = lambda rt: {"client_id": E["CLIENT_ID"], "managed": True, "refresh_token": rt}  # noqa: E731
        _put_secret(db, f, "cred_calendar_1", "calendar", rec(E["CALENDAR_REFRESH"]))
        _put_secret(db, f, "cred_drive_1", "drive", rec(E["DRIVE_REFRESH"]))
        acc = {"id": "c1", "email": EMAIL, "mode": "google", "time_zone": tz, "calendar_name": ""}
        _put_connection(db, "calendar", {"accounts": [acc], "default": "c1", "email": EMAIL, "mode": "google",
                                         "time_zone": tz, "calendar_name": ""},
                        {"read": True, "write": False})   # the user's rule: nothing is ever added to the calendar
        dacc = {"id": "d1", "email": EMAIL, "mode": "google", "time_zone": "", "calendar_name": ""}
        _put_connection(db, "drive", {"accounts": [dacc], "default": "d1", "email": EMAIL, "mode": "google"},
                        {"read": True, "upload": True, "write": True})


# ---------------------------------------------------------------- the test's own reads of the account (checks)

def _token(refresh: str) -> str:
    body = urllib.parse.urlencode({"client_id": E["CLIENT_ID"], "client_secret": E["CLIENT_SECRET"],
                                   "refresh_token": refresh, "grant_type": "refresh_token"}).encode()
    with urllib.request.urlopen(urllib.request.Request("https://oauth2.googleapis.com/token", data=body), timeout=30) as r:
        return json.load(r)["access_token"]


def _get(url: str, token: str) -> dict:
    with urllib.request.urlopen(urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"}), timeout=30) as r:
        return json.load(r)


def events(time_min: str, time_max: str) -> list[dict]:
    """The primary calendar's events in a window (RFC 3339), read-only: [{summary, start}]."""
    q = urllib.parse.urlencode({"timeMin": time_min, "timeMax": time_max, "singleEvents": "true",
                                "orderBy": "startTime", "maxResults": 100})
    d = _get(f"https://www.googleapis.com/calendar/v3/calendars/primary/events?{q}", _token(E["CALENDAR_REFRESH"]))
    return [{"summary": e.get("summary", ""), "start": (e.get("start") or {}).get("dateTime") or (e.get("start") or {}).get("date")}
            for e in d.get("items", []) if e.get("status") != "cancelled"]


def calendar_tz() -> str:
    return _get("https://www.googleapis.com/calendar/v3/users/me/settings/timezone", _token(E["CALENDAR_REFRESH"])).get("value", "UTC")


def drive_files(name: str) -> list[dict]:
    """The files OMuse's client can see (drive.file) with this exact name: [{id, name}]."""
    q = urllib.parse.urlencode({"q": f"name = '{name}' and trashed = false", "fields": "files(id,name)"})
    return _get(f"https://www.googleapis.com/drive/v3/files?{q}", _token(E["DRIVE_REFRESH"])).get("files", [])


def delete_drive_files(name_prefix: str) -> list[str]:
    """Permanently delete every file the suite added (drive.file sees only OMuse's own and picked files; the name prefix
    narrows it to this suite's): the user's rule. Returns the names deleted."""
    t = _token(E["DRIVE_REFRESH"])
    q = urllib.parse.urlencode({"q": f"name contains '{name_prefix}'", "fields": "files(id,name)"})
    gone = []
    for f in _get(f"https://www.googleapis.com/drive/v3/files?{q}", t).get("files", []):
        if not f["name"].startswith(name_prefix):
            continue
        req = urllib.request.Request(f"https://www.googleapis.com/drive/v3/files/{f['id']}", method="DELETE",
                                     headers={"Authorization": f"Bearer {t}"})
        with urllib.request.urlopen(req, timeout=30):
            gone.append(f["name"])
    return gone


def add_second_calendar(email: str) -> None:
    """A second calendar account (c2) on the card, as a second sign-in would add it — the same read-only consent under
    another address, so the card's Make default and Disconnect can be used (Disconnect removes only OMuse's own entry)."""
    f = Fernet(open(os.path.join(SDATA, "vault.key"), "rb").read().strip())
    with sqlite3.connect(os.path.join(SDATA, "sentinel.db")) as db:
        _put_secret(db, f, "cred_calendar_2", "calendar",
                    {"client_id": E["CLIENT_ID"], "managed": True, "refresh_token": E["CALENDAR_REFRESH"]})
        row = db.execute("SELECT config FROM connections WHERE name='calendar'").fetchone()
        cfg = json.loads(row[0])
        cfg["accounts"] = [a for a in cfg.get("accounts", []) if a.get("id") != "c2"] + [
            {"id": "c2", "email": email, "mode": "google", "time_zone": cfg.get("time_zone", ""), "calendar_name": ""}]
        db.execute("UPDATE connections SET config=?, updated_at=? WHERE name='calendar'", (json.dumps(cfg), time.time()))
