"""Scripted (fake-model) remainder of the 0.2.7 suite: only what a real-model run cannot do.

The use cases are tested from the user's side by real-model suites: a research brief PDF within a low step limit, the
same data as an Excel sheet and a site behind a bot wall in tests/brief_real.py; signing in to Google Calendar, a free
slot and an invitation the user approves (and edits) in tests/gcal_real.py. What stays here needs a forced condition
or checks an internal guarantee the user cannot see:
* step budget: a model that would browse forever (scripted) still gets the last steps kept for the PDF, so the task
  completes with the file instead of failing at the limit — a real model cannot be made to ignore the budget;
* bot wall: after the first wall the second URL on that site is not even opened and a site_blocked event is recorded
  (internal: the user only sees the answer);
* Google Calendar OAuth security: a malformed client id is rejected, a forged or replayed callback (wrong state) is
  refused, the refresh token never reaches the UI, the Test button works, Disconnect removes the credential."""
import os
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
T = "/tmp/claude-0/persona-test"
WS = T + "/workspace"
c = httpx.Client(timeout=90, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1500])
    if not cond:
        fails.append(name)


def wait(tid, timeout=120, until=("COMPLETED", "FAILED", "CANCELLED")):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in until:
            return t
        time.sleep(0.5)
    return t


def chat(msg):
    return c.post(B + "/api/chat", json={"message": msg}, headers=H).json()


# ------------------------------------------------------------------ 1. step budget
old = c.get(B + "/api/settings").json()
c.put(B + "/api/settings", json={"max_steps": 8}, headers=H)
r = chat("BUDGETPDF 调研日本餐厅并生成 PDF 简报发给我")
t = wait(r["task_id"])
navs = [e for e in t["events"] if e["type"] == "tool_call" and e["data"]["name"] == "browser_navigate"]
sent = [e for e in t["events"] if e["type"] == "tool_call" and e["data"]["name"] == "send_file"]
check("budget: task completed (not FAILED at the limit)", t["status"] == "COMPLETED", (t["status"], t.get("error"), t.get("result")))
# the PDF is sent by code right after it is made (no model send_file to count): the file and its card are the proof
check("budget: stopped browsing and delivered the PDF", os.path.isfile(WS + "/reports/brief.pdf") and 2 <= len(navs) <= 4,
      (len(navs), len(sent)))
conv = c.get(f"{B}/api/conversations/{r['conversation_id']}").json()
check("budget: PDF card in the chat", any('"type": "file"' in m["content"] and "brief.pdf" in m["content"] for m in conv["messages"]))
c.put(B + "/api/settings", json={"max_steps": old.get("max_steps", 40)}, headers=H)

# ------------------------------------------------------------------ 2. blocked site
r = chat("BLOCKSITE 在 OpenTable 查餐厅")
t = wait(r["task_id"])
res = t.get("result") or ""
parts = res.split("=====")
check("blocked: wall detected on first visit", len(parts) == 3 and "[SITE BLOCKED site=opentable]" in parts[0], res[:600])
check("blocked: second URL on the same site not even opened", "ERROR: [SITE BLOCKED site=opentable]" in parts[1], res[:900])
check("blocked: other sites still work", "Kuriya" in parts[2] and "SITE BLOCKED" not in parts[2], parts[2:] and parts[2][:300])
ev = [e for e in t["events"] if e["type"] == "site_blocked"]
check("blocked: site_blocked event recorded", len(ev) == 1 and ev[0]["data"]["kind"] in ("akamai", "http-403"), ev)
bnav = [e for e in t["events"] if e["type"] == "tool_result" and e["data"]["name"] == "browser_navigate"]
check("blocked: 3 navigate results", len(bnav) == 3, len(bnav))

# ------------------------------------------------------------------ 4. Google Calendar
c.post("http://127.0.0.1:8094/_g/reset")
cat = c.get(B + "/sentinel/api/connections", headers=H).json()
cal = next(x for x in cat["connections"] if x["name"] == "calendar")
check("calendar: listed, not connected yet", not cal["has_credential"] and not cal["enabled"], cal)
bad = c.post(B + "/sentinel/api/connections/calendar/start", json={"client_id": "nope", "client_secret": "x"}, headers=H)
check("calendar: rejects a malformed client id", bad.status_code == 400, bad.text)
st = c.post(B + "/sentinel/api/connections/calendar/start", headers=H,
            json={"client_id": "1234567890-abcdef.apps.googleusercontent.com", "client_secret": "GOCSPX-test-secret-123"}).json()
check("calendar: consent URL points at Google auth with offline access", st["auth_url"].startswith("http://127.0.0.1:8094/g/auth?")
      and "access_type=offline" in st["auth_url"] and st["redirect_uri"].endswith("/sentinel/api/connections/calendar/callback"), st)
# forged callback with the wrong state is refused
forged = c.get(B + "/sentinel/api/connections/calendar/callback", params={"code": "x", "state": "evil"})
check("calendar: callback with a wrong state is refused", "state mismatch" in forged.text and forged.status_code == 200, forged.text[:200])
# the user's browser follows Google's redirect back to OMuse
g = c.get(st["auth_url"], follow_redirects=False)
loc = g.headers["location"]
cb = c.get(loc.replace(st["redirect_uri"].split("/sentinel/")[0], B))
check("calendar: callback stores the token and shows success", "lucas@example.com" in cb.text and "✓" in cb.text, cb.text[:300])
replay = c.get(loc.replace(st["redirect_uri"].split("/sentinel/")[0], B))
check("calendar: the same callback can't be replayed", "state mismatch" in replay.text, replay.text[:200])
cal = next(x for x in c.get(B + "/sentinel/api/connections", headers=H).json()["connections"] if x["name"] == "calendar")
check("calendar: connected, time zone read from Google", cal["has_credential"] and cal["enabled"] and cal["config"]["time_zone"] == "Asia/Singapore"
      and "refresh_token" not in str(cal), cal)
tst = c.post(B + "/sentinel/api/connections/calendar/test", json={}, headers=H).json()
check("calendar: test button works", tst.get("ok") and tst.get("upcoming") == 1, tst)

d = c.delete(B + "/sentinel/api/connections/calendar/credential", headers=H)
cal = next(x for x in c.get(B + "/sentinel/api/connections", headers=H).json()["connections"] if x["name"] == "calendar")
check("calendar: disconnect removes the credential", d.status_code == 200 and not cal["has_credential"] and not cal["enabled"], cal)

print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
