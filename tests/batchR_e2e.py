"""Batch R (browser robustness, no detection evasion) end to end, local stack:
1. a bot-wall page is detected → the site is refused by code for the rest of the task (the SITE BLOCKED reason code),
   the task completes on another site without re-opening the blocked one
2. the blocked site is remembered as a site habit (to confirm), so next time OMuse goes another way
3. the Trust page browser metric counts the block
"""
import sys
import time

import httpx

B, LLM = "http://127.0.0.1:8080", "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=90, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:500], flush=True)
    if not cond:
        fails.append(name)


def wait_task(tid, timeout=90):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(1)
    return t


tid = c.post(B + "/api/chat", json={"message": "BLOCKSITE open the shop"}, headers=H).json()["task_id"]
t = wait_task(tid)
ev = t["events"]
check("the bot wall is detected (site_blocked event)", any(e["type"] == "site_blocked" for e in ev), [e["type"] for e in ev])
navs = {e["data"]["call_id"]: e["data"]["args"].get("url", "") for e in ev if e["type"] == "tool_call" and e["data"]["name"] == "browser_navigate"}
res = [(navs.get(e["data"]["call_id"], ""), e["data"]) for e in ev if e["type"] == "tool_result" and e["data"]["name"] == "browser_navigate"]
wall = [r for u, r in res if "opentable.test" in u]
check("the wall page is marked with the SITE BLOCKED reason code; the next visit to the site is refused by code, not run",
      len(wall) == 2 and wall[0]["preview"].startswith("[SITE BLOCKED site=opentable]")
      and wall[1]["preview"].startswith("ERROR: [SITE BLOCKED site=opentable]") and wall[1].get("ok") is False,
      [(u, r.get("ok"), r.get("preview", "")[:80]) for u, r in res])
check("the task completes on another site (the blocked one is not opened again)",
      t["status"] == "COMPLETED" and any("shop.test" in u and r.get("ok") for u, r in res)
      and sum(1 for e in ev if e["type"] == "site_blocked") == 1, (t["status"], [(u, r.get("ok")) for u, r in res]))

mem = c.get(B + "/api/memory", headers=H).json()
site = [f for f in mem["facts"] if f["domain"] == "site" and "opentable" in f["fact"] and f["status"] == "pending"]
check("the blocked site is saved as a site habit to confirm", site,
      [f["fact"][:60] for f in mem["facts"] if f["domain"] == "site"])

m = c.get(B + "/api/metrics?days=7", headers=H).json()["metrics"]["browser"]
check("the Trust page browser metric counts the block", m["blocked"] >= 1 and any(r["blocked"] for r in m["sites"]), m)

print()
print(("%d FAILED: %s" % (len(fails), fails)) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
