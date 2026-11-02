"""Scripted (fake-model) integration checks against the locally running services (see run_local.sh): only what a
real-model run cannot do.

The user-facing scenarios moved to real-model suites: memory (remembered, learned on the way, recalled), a scheduled
job with Run now, and a site the user signs in to during a takeover are in tests/assistant_real.py; a risky browser
action approved in a dialog in tests/browser_auto_real.py. What stays here needs a model
that misbehaves on purpose, a forced condition, or checks an internal guarantee the user cannot see: a plain chat makes
no plan (no planner call); an action that asks waits for the user's approval, resumes after it and hands the page over; the approval API
refuses a request without the UI header (CSRF); a <tool_call> written as
text is still parsed; the user's own navigation to a private address is blocked; the audit chain verifies and records
model calls, tool calls and approvals. (The page-driven cases — snapshot redaction, typing outside
browser-use refused, a prompt injection flagged — went with the test pages they needed.)"""
import sys, time, httpx

B = "http://127.0.0.1:8080"
LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []

def chat(msg):
    r = c.post(B + "/api/chat", json={"message": msg}, headers=H); r.raise_for_status(); return r.json()["task_id"]

def wait(tid, states, timeout=60):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in states: return t
        time.sleep(0.5)
    return t

def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else info)
    if not cond: fails.append(name)

# 1 plain
t = wait(chat("你好"), {"COMPLETED", "FAILED"})
check("plain chat completes", t["status"] == "COMPLETED" and "普通回答" in t["result"] and "<think>" not in t["result"], t)
check("a plain chat makes no plan (the executor answered it directly, no planner call)",
      not (t.get("plan") or {}).get("steps") and not any(e["type"] == "plan" for e in t["events"]) and
      c.get(LLM + "/calls_for", params={"marker": "你好"}).json(), (t.get("plan"), [e["type"] for e in t["events"]]))

# 2 an action that asks -> approval in the UI -> resumes (BROWSE: a purchase confirmation, which always asks)
tid = chat("BROWSE the shop")
t = wait(tid, {"WAITING_APPROVAL", "COMPLETED", "FAILED"})
check("an action that asks waits for approval", t["status"] == "WAITING_APPROVAL", t.get("error") or t["events"][-3:])
aps = c.get(B + "/sentinel/api/approvals?status=pending").json()["approvals"]
ap = [a for a in aps if a["task_id"] == tid]
check("approval visible to UI", len(ap) == 1, aps)
if ap:
    c.post(f"{B}/sentinel/api/approvals/{ap[0]['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
    # an approved purchase hands the page over for the final click (auto_handover); the user hands it back
    t = wait(tid, {"WAITING_EXTERNAL", "COMPLETED", "FAILED"})
    check("after the approval the task resumes and hands the page over", t["status"] == "WAITING_EXTERNAL"
          and any(e["type"] == "auto_handover" for e in t["events"]), (t["status"], [e["type"] for e in t["events"]][-8:]))
    c.post(B + "/sentinel/api/browser/release", json={}, headers=H)
    t = wait(tid, {"COMPLETED", "FAILED"})
    check("handed back, the task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
# approval without UI header is rejected
r = c.post(f"{B}/sentinel/api/approvals/x/resolve", json={"decision": "approve"})
check("CSRF guard (no UI header) rejected", r.status_code == 403, r.status_code)

# 8 xml tool call fallback + file write
t = wait(chat("XMLTOOL write"), {"COMPLETED", "FAILED"})
check("<tool_call> text fallback parsed", t["status"] == "COMPLETED" and "written" in t["result"], t["result"])

# 9 private address blocked
r = c.post(B + "/sentinel/api/browser/takeover", json={}, headers=H)
r = c.post(B + "/sentinel/api/browser/input", json={"type": "navigate", "url": "http://127.0.0.1:8080/"}, headers=H)
check("private address blocked even for user nav", r.status_code == 403, r.status_code)
c.post(B + "/sentinel/api/browser/release", json={}, headers=H)

# 10 audit chain
v = c.get(B + "/sentinel/api/audit/verify").json()
check("audit chain verifies", v["ok"] and v["checked"] > 3, v)
ev = c.get(B + "/sentinel/api/audit?limit=500").json()["events"]
acts = {e["action"] for e in ev}
check("audit has model calls, tool calls, approvals", {"model.executor", "files_write", "approval.approve"} <= acts, sorted(acts)[:40])
print("\n%d failures" % len(fails), fails)
sys.exit(1 if fails else 0)
