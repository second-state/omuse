"""Settings → "auto-approve browser actions" (the browser connection's `auto_approve`; `[browser] auto_approve` in
config.toml is the default, false in the test stack). On: a risky browser click runs without the approval dialog, but
is still recorded as an approval decided by "auto" and shown in the chat and the task timeline like a manual one.
Vault fills and takeover requests still stop for the user. Off again: the click waits for approval as before."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200])
    if not cond:
        fails.append(name)


def run(msg, stop=("COMPLETED", "FAILED", "CANCELLED", "WAITING_APPROVAL", "WAITING_EXTERNAL"), timeout=90):
    t0 = time.time()
    r = c.post(B + "/api/chat", json={"message": msg}, headers=H).json()
    seen, t = set(), {}
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{r['task_id']}").json()
        seen.add(t["status"])
        if t["status"] in stop:
            break
        time.sleep(0.3)
    t["_seen"] = seen
    return t


def wait_end(tid, timeout=60):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.3)
    return t


def browser_conn():
    return next(x for x in c.get(B + "/sentinel/api/connections", headers=H).json()["connections"] if x["name"] == "browser")


c.put(B + "/api/settings", json={"language": "en"}, headers=H)
check("the test stack starts with the setting off (config default)", browser_conn()["config"].get("auto_approve") is False, browser_conn()["config"])
r = c.put(B + "/sentinel/api/connections/browser", json={"config": {"auto_approve": True}}, headers=H)
check("the setting is saved on the browser connection", r.status_code == 200 and browser_conn()["config"].get("auto_approve") is True, r.text[:300])

# 1. on: the risky click is approved by "auto", recorded and shown, and the task never waits
t = run("BROWSE the shop with auto approval")
ev = t.get("events", [])
check("on: the task completes without waiting for an approval", t["status"] == "COMPLETED" and "WAITING_APPROVAL" not in t["_seen"],
      (t["status"], t["_seen"], t.get("error")))
auto = [e["data"] for e in ev if e["type"] == "approval_auto"]
check("on: the timeline shows the auto-approval of the click", len(auto) == 1 and auto[0]["tool"] == "browser_click" and auto[0].get("approval_id"), auto)
res = [e["data"] for e in ev if e["type"] == "tool_result" and e["data"]["name"] == "browser_click"]
check("on: the click's result is marked approved by auto", res and res[0].get("approved") == "auto" and res[0].get("ok") is True, res)
aid = auto[0]["approval_id"] if auto else None
hist = c.get(B + "/sentinel/api/approvals?status=resolved&limit=20", headers=H).json()["approvals"]
mine = [a for a in hist if a["id"] == aid]
check("on: the approval record exists, approved, decided by auto, scope ONCE",
      mine and mine[0]["status"] == "approved" and mine[0].get("decided_by") == "auto" and mine[0].get("scope") == "ONCE"
      and mine[0]["tool"] == "browser_click" and mine[0]["task_id"] == t["id"], mine or hist[:3])
msgs = c.get(B + "/api/conversations/" + t["conv_id"]).json()["messages"]
cards = [m for m in msgs if m["role"] == "system" and '"approval_auto"' in m["content"]]
check("on: the chat carries the same card as a manual approval would", len(cards) == 1 and aid in cards[0]["content"], [m["content"][:200] for m in msgs if m["role"] == "system"])
pend = c.get(B + "/sentinel/api/approvals?status=pending", headers=H).json()["approvals"]
check("on: nothing is left pending", not [a for a in pend if a["task_id"] == t["id"]], pend)
flat = str(c.get(B + "/sentinel/api/audit?limit=60", headers=H).json()["events"])
check("on: the audit trail records the automatic approval", "approval_id" in flat and ("'auto': True" in flat or '"auto": true' in flat or '"auto":true' in flat), flat[:600])

# 2. on: a vault fill still stops for the user (the scripted scenario fills the KrisFlyer membership number)
if not any(i.get("label") == "KrisFlyer" for i in c.get(B + "/sentinel/api/vault", headers=H).json().get("items", [])):
    c.post(B + "/sentinel/api/vault", headers=H, json={"kind": "membership", "label": "KrisFlyer", "domains": "shop.test",
                                                       "values": {"number": "8800123456"}})
t2 = run("VAULTFILL my KrisFlyer number on the checkout page")
check("on: filling a vault value still waits for approval", t2["status"] == "WAITING_APPROVAL"
      and (t2.get("waiting") or {}).get("tool") == "browser_fill_secret", (t2["status"], t2.get("waiting")))
for a in c.get(B + "/sentinel/api/approvals?status=pending", headers=H).json()["approvals"]:
    if a["task_id"] == t2["id"]:
        c.post(f"{B}/sentinel/api/approvals/{a['id']}/resolve", json={"decision": "deny"}, headers=H)
wait_end(t2["id"])

# 3. on: a takeover request still comes to the user
t3 = run("TAKEOVER needed for the shop login")
check("on: a login takeover still stops for the user", t3["status"] == "WAITING_EXTERNAL", (t3["status"], t3.get("waiting")))
c.post(B + "/sentinel/api/browser/release", json={}, headers=H)
wait_end(t3["id"])

# 4. off: the same click waits for approval again
c.put(B + "/sentinel/api/connections/browser", json={"config": {"auto_approve": False}}, headers=H)
t4 = run("BROWSE the shop without auto approval")
check("off: the risky click waits for approval again", t4["status"] == "WAITING_APPROVAL" and (t4.get("waiting") or {}).get("tool") == "browser_click",
      (t4["status"], t4.get("waiting")))
for a in c.get(B + "/sentinel/api/approvals?status=pending", headers=H).json()["approvals"]:
    if a["task_id"] == t4["id"]:
        c.post(f"{B}/sentinel/api/approvals/{a['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
t4 = wait_end(t4["id"])
check("off: approved by hand it completes, with no auto event", t4["status"] == "COMPLETED" and not [e for e in t4["events"] if e["type"] == "approval_auto"],
      (t4["status"], [e["type"] for e in t4["events"]]))

print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
