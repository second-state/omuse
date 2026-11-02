"""Roadmap batch 1 end to end (local stack + scripted model, tests/run_local.sh): forced outages a real run cannot make.
1. health watch — a missing model shows as down on the status page; a task still completes on a fallback chat model
2. failure streak — 3 tasks failing for the same reason (model unreachable); the next one completes
3. outcome evidence — nothing ran, so the stored outcome is "none" and can be re-assessed
4. metrics — counts and failure causes
(The Telegram alert and recovery messages went with the fake Telegram server.)
"""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=90, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:600], flush=True)
    if not cond:
        fails.append(name)


def wait_for(pred, timeout=60, step=0.5):
    t0 = time.time()
    while time.time() - t0 < timeout:
        if pred():
            return True
        time.sleep(step)
    return False


def task(tid):
    return c.get(f"{B}/api/tasks/{tid}").json()


def wait_task(tid, timeout=120):
    wait_for(lambda: task(tid)["status"] in ("COMPLETED", "FAILED", "CANCELLED"), timeout, 1)
    return task(tid)


def chat(msg):
    return c.post(B + "/api/chat", json={"message": msg}, headers=H).json()["task_id"]


def settings(**kw):
    return c.put(B + "/api/settings", json=kw, headers=H).json()["settings"]


def health_check():
    return c.post(B + "/api/health/check", json={}, headers=H).json()


orig = c.get(B + "/api/settings", headers=H).json()["settings"]

# ---------------------------------------------------------------- 1 health watch
st = health_check()
check("all components healthy at start", all(v["ok"] for v in st["components"].values()) and
      {"model", "browser", "sentinel"} <= set(st["components"]), st)
settings(model_override=True, model_base_url=orig["model_base_url"], model_api_key="k", model_name="missing-model")
health_check()
health_check()
st = c.get(B + "/api/health/status").json()
check("status page shows the model down", st["components"]["model"]["down"] is True, st)
# a task still works: the runtime falls back to a chat model (never a TTS one) for a few minutes
tid = chat("你好")
check("task still completes on a stand-in chat model", wait_task(tid)["status"] == "COMPLETED", task(tid).get("error"))
settings(model_override=False)   # back to the system model
st = health_check()
check("the model is up again after switching back", st["components"]["model"]["ok"], st)

# ---------------------------------------------------------------- 2 failure streak
settings(model_override=True, model_base_url="http://127.0.0.1:9/v1", model_api_key="k", model_name=orig["model_name"])   # nothing listens there
tids = [chat(f"你好 {i}") for i in range(3)]
for t in tids:
    wait_task(t, 180)
check("3 tasks failed (model unreachable)", all(task(t)["status"] == "FAILED" for t in tids), [task(t)["error"] for t in tids])
settings(model_override=False)
tid = chat("你好 again")
check("next task completes", wait_task(tid)["status"] == "COMPLETED")

# ---------------------------------------------------------------- 3 outcome evidence
tid = chat("FAKECLAIM 给 Jennifer 发一封邮件，说明天的会改到下午 3 点")
t = wait_task(tid)
evs = c.get(f"{B}/api/tasks/{tid}").json()["events"]
check("nothing was sent and the task completed", t["status"] == "COMPLETED"
      and not any(e["type"] == "tool_call" and e["data"].get("name") in ("gmail_send", "gmail_reply") for e in evs), (t["status"], [e["type"] for e in evs]))
# the outcome is read from the task's events and pages only (no regex over the request's words): nothing ran, nothing to prove
check("outcome stored on the task (none: no action ran)", (t.get("outcome") or {}).get("status") == "none", t.get("outcome"))
r = c.post(f"{B}/api/outcome/{tid}", json={}, headers=H)
check("outcome can be re-assessed for an older task", r.status_code == 200 and r.json()["outcome"]["status"] in ("none", "unverified"), r.text[:200])

# ---------------------------------------------------------------- 4 metrics
m = c.get(B + "/api/metrics?days=7").json()
mm = m["metrics"]
check("metrics count the tasks", mm["tasks"] >= 6 and mm["failed"] >= 3, mm)
check("failure cause = model", mm["failure_causes"] and mm["failure_causes"][0]["cause"] == "model", mm["failure_causes"])

print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
