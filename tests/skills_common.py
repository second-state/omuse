"""Shared driver for the travel-skill suites (trip_today_e2e.py, sgac_e2e.py, …): the scripted model (tests/fake_llm.py)
plays the agent, the stack's own services play the world — mail in the local IMAP server (Dovecot on 1143, as
mail_generic_e2e.py), an iCal calendar and the travel data connector from tests/fake_apps.py / fake_travel.py, pages
under tests/pages (http://shop.test:8099/…), uploads through /api/upload. Each suite checks which tools the skill made
the agent use and the rules it must keep (no browser where a data source exists, secrets only through the vault, …)."""
from __future__ import annotations

import email.utils
import imaplib
import json
import sys
import time

import httpx
import os

B = "http://127.0.0.1:8080"
S = B + "/sentinel/api"
TRAVEL = "http://127.0.0.1:8097"
PAGES = "http://shop.test:8099"
H = {"X-Persona-UI": "1"}
USER, PW = "tester@example.com", "secretpass123"
c = httpx.Client(timeout=90, trust_env=False)
fails: list[str] = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200], flush=True)
    if not cond:
        fails.append(name)


def finish():
    print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"), flush=True)
    sys.exit(1 if fails else 0)


# ------------------------------------------------------------------ the world
def seed_mail(messages: list[tuple[str, str, str, float | None]]) -> None:
    """(from, subject, body, age_days) → INBOX, replacing whatever an earlier suite left there."""
    m = imaplib.IMAP4("127.0.0.1", 1143)
    m.login(USER, PW)
    m.select("INBOX")
    try:
        m.store("1:*", "+FLAGS", "(\\Deleted)")
        m.expunge()
    except Exception:
        pass
    for i, (frm, subject, body, age) in enumerate(messages):
        when = time.time() - (age or 0) * 86400
        raw = (f"From: {frm}\r\nTo: {USER}\r\nSubject: {subject}\r\nDate: {email.utils.formatdate(when, localtime=True)}\r\n"
               f"Message-ID: <skill{i}{int(when)}@example.com>\r\nContent-Type: text/plain; charset=utf-8\r\n"
               f"Content-Transfer-Encoding: 8bit\r\n\r\n{body}\r\n")
        m.append("INBOX", None, imaplib.Time2Internaldate(when), raw.encode("utf-8"))
    m.logout()


def connect_mail() -> None:
    accs = c.get(S + "/connections", headers=H).json()["connections"]
    gm = next(x for x in accs if x["name"] == "gmail")
    if any(a.get("email") == USER and a.get("ready") for a in gm.get("accounts", [])):
        return
    body = {"provider": "custom", "email": USER, "app_password": PW, "display_name": "Tester",
            "imap_host": "127.0.0.1", "imap_port": 1143, "imap_security": "none",
            "smtp_host": "127.0.0.1", "smtp_port": 1025, "smtp_security": "none"}
    r = c.post(S + "/connections/gmail/credential", json=body, headers=H)
    check("mailbox connected (local IMAP)", r.status_code == 200, r.text[:300])


def connect_calendar() -> None:
    r = c.post(S + "/connections/calendar/ical", json={"url": "http://127.0.0.1:8094/ics/private-ok/basic.ics",
                                                         "time_zone": "Asia/Singapore"}, headers=H)
    check("calendar connected (iCal, read-only)", r.status_code == 200 and r.json().get("ok"), r.text[:300])


def connect_travel() -> None:
    c.post(TRAVEL + "/_reset")
    for s in c.get(S + "/mcp/servers", headers=H).json().get("servers", []):
        if s.get("id") == "travel":
            c.delete(S + "/mcp/servers/travel", headers=H)
    r = c.post(S + "/mcp/servers", headers=H, json={"name": "Travel", "url": TRAVEL + "/mcp", "auth_type": "bearer",
                                                     "token": "test-travel-token", "data_class": "PUBLIC"})
    check("travel data connector added", r.status_code == 200 and len(r.json().get("tools", [])) == 6, r.text[:300])


def set_profile(**fields) -> None:
    for k, v in fields.items():
        c.put(B + "/api/profile", json={"key": k, "value": v}, headers=H)


def vault_item(label: str, kind: str, values: dict, domains: str = "") -> str:
    have = {i["label"]: i["id"] for i in c.get(S + "/vault", headers=H).json().get("items", [])}
    if label in have:
        return have[label]
    r = c.post(S + "/vault", headers=H, json={"label": label, "kind": kind, "values": values, "domains": domains})
    return (r.json().get("item") or r.json()).get("id")


def upload(name: str, data: bytes) -> dict:
    r = c.put(f"{B}/api/upload", params={"name": name}, content=data, headers=H)
    r.raise_for_status()
    return r.json()


PNG = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages", "lipstick.png"), "rb").read()   # any real PNG: the scripted vision model never looks at pixels


# ------------------------------------------------------------------ running a task
def run(message: str, *, conv: str | None = None, attachments: list | None = None, approve: bool = True,
        stop_on_external: bool = True, timeout: float = 150) -> dict:
    """Send `message`; approve approvals as they come (recording them); return the finished task with `_approvals`
    and `_seen` (every status seen)."""
    r = c.post(B + "/api/chat", json={"message": message, "conversation_id": conv, "attachments": attachments or []}, headers=H).json()
    tid = r["task_id"]
    seen, approvals, t0, t = set(), [], time.time(), {}
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        seen.add(t["status"])
        if t["status"] == "WAITING_APPROVAL":
            if not approve:
                break
            for a in c.get(S + "/approvals?status=pending", headers=H).json()["approvals"]:
                if a["task_id"] == tid:
                    rr = c.post(f"{S}/approvals/{a['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
                    approvals.append({"tool": a["tool"], "args": a.get("args"), "ok": rr.status_code == 200})
            time.sleep(0.3)
            continue
        if t["status"] == "WAITING_EXTERNAL" and stop_on_external:
            break
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            break
        time.sleep(0.3)
    t["_approvals"], t["_seen"], t["_conv"] = approvals, seen, r.get("conversation_id")
    return t


def wait_end(tid: str, timeout: float = 90) -> dict:
    t0, t = time.time(), {}
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.3)
    return t


def hand_back() -> None:
    """The user hands the browser back after a takeover (without pressing anything)."""
    c.post(S + "/browser/release", json={}, headers=H)


def calls(t: dict, sub: bool = False) -> list[dict]:
    return [e["data"] for e in t.get("events", []) if e["type"] == "tool_call" and bool((e["data"] or {}).get("sub")) == sub]


def names(t: dict) -> list[str]:
    return [x["name"] for x in calls(t)]


def results(t: dict, name: str) -> list[dict]:
    return [e["data"] for e in t.get("events", []) if e["type"] == "tool_result" and e["data"].get("name") == name]


def loaded(t: dict, skill: str) -> bool:
    return any(x["name"] == "load_skill" and skill in json.dumps(x.get("args")) for x in calls(t))


def batches(t: dict) -> list[list[str]]:
    return [[x["name"] for x in e["data"]["calls"]] for e in t.get("events", []) if e["type"] == "parallel_calls"]


def conv_cards(conv: str, kind: str) -> list[dict]:
    out = []
    for m in c.get(f"{B}/api/conversations/{conv}").json()["messages"]:
        if m["role"] == "system":
            try:
                j = json.loads(m["content"])
            except Exception:
                continue
            if j.get("type") == kind:
                out.append(j)
    return out


def trace(t: dict) -> None:
    for e in t.get("events", []):
        d = e.get("data") or {}
        if e["type"] == "tool_call":
            print(f"INFO call {d.get('name')} {json.dumps(d.get('args'), ensure_ascii=False)[:150]}")
        elif e["type"] == "tool_result":
            print(f"INFO  -> ok={d.get('ok')} {str(d.get('preview') or '')[:150]!r}")
        elif e["type"] in ("waiting", "parallel_calls", "approval_auto", "error"):
            print(f"INFO {e['type']} {json.dumps(d, ensure_ascii=False)[:200]}")


def setup(*, mail: list | None = None, calendar: bool = False, travel: bool = False, language: str = "en") -> None:
    c.put(B + "/api/settings", json={"language": language}, headers=H)
    if mail is not None:
        seed_mail(mail)
        connect_mail()
    if calendar:
        connect_calendar()
    if travel:
        connect_travel()
