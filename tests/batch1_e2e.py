"""Roadmap batch 1 end to end (local stack + fake Telegram, tests/run_local.sh):
1. health watch — a missing model raises ONE Telegram alert, then a recovery message
2. failure streak — 3 tasks failing for the same reason raise an alert; the next completed task clears it
3. outcome evidence — an answer that claims "sent" with nothing sent is caught and rewritten
4. metrics — counts and failure causes
"""
import sys
import time

import httpx

B, TG = "http://127.0.0.1:8080", "http://127.0.0.1:8091"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=90, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:600], flush=True)
    if not cond:
        fails.append(name)


def tg_texts():
    return [m["text"] for m in c.get(TG + "/_log").json()["sent"]]


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


# telegram so alerts have somewhere to go
c.post(TG + "/_push", json={"message": {"message_id": 1, "chat": {"id": 555, "type": "private", "first_name": "Lucas"}, "text": "/start"}})
c.post(B + "/sentinel/api/connections/telegram/credential", json={"bot_token": "123:ABC", "chat_id": "555"}, headers=H)
orig = c.get(B + "/api/settings", headers=H).json()["settings"]

# ---------------------------------------------------------------- 1 health watch
st = health_check()
check("all components healthy at start", all(v["ok"] for v in st["components"].values()) and
      {"model", "browser", "sentinel"} <= set(st["components"]), st)
n0 = len(tg_texts())
settings(model_override=True, model_base_url=orig["model_base_url"], model_api_key="k", model_name="missing-model")
health_check()
check("one failed check is not an alert yet (router restarts happen)", not [t for t in tg_texts()[n0:] if "出问题" in t], tg_texts()[n0:])
health_check()
alerts = [t for t in tg_texts()[n0:] if "模型服务出问题" in t]
check("second failed check -> one Telegram alert naming the missing model", len(alerts) == 1 and "missing-model" in alerts[0], tg_texts()[n0:])
health_check()
check("no repeated alert while still down", len([t for t in tg_texts()[n0:] if "模型服务出问题" in t]) == 1)
st = c.get(B + "/api/health/status").json()
check("status page shows the model down", st["components"]["model"]["down"] is True, st)
# a task still works: the runtime falls back to a chat model (never a TTS one) for a few minutes
tid = chat("你好")
check("task still completes on a stand-in chat model", wait_task(tid)["status"] == "COMPLETED", task(tid).get("error"))
settings(model_override=False)   # back to the system model
health_check()
check("recovery message sent", wait_for(lambda: any("模型服务已恢复" in t for t in tg_texts()[n0:]), 10), tg_texts()[n0:])

# ---------------------------------------------------------------- 2 failure streak
n1 = len(tg_texts())
settings(model_override=True, model_base_url="http://127.0.0.1:9/v1", model_api_key="k", model_name=orig["model_name"])   # nothing listens there
tids = [chat(f"你好 {i}") for i in range(3)]
for t in tids:
    wait_task(t, 180)
check("3 tasks failed (model unreachable)", all(task(t)["status"] == "FAILED" for t in tids), [task(t)["error"] for t in tids])
check("failure streak -> Telegram alert about task runs",
      wait_for(lambda: any("任务执行出问题" in t and "模型" in t for t in tg_texts()[n1:]), 15), tg_texts()[n1:])
settings(model_override=False)
tid = chat("你好 again")
check("next task completes", wait_task(tid)["status"] == "COMPLETED")
check("streak alert cleared with a recovery message", wait_for(lambda: any("任务执行已恢复" in t for t in tg_texts()[n1:]), 15),
      tg_texts()[n1:])

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
