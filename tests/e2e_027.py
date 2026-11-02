"""Scripted (fake-model) remainder of the 0.2.7 suite: only what a real-model run cannot do.

What stays here needs a forced condition a real model cannot be made to produce:
* step budget: a model that would browse forever (scripted) still gets the last steps kept for the PDF, so the task
  completes with the file instead of failing at the limit — a real model cannot be made to ignore the budget.
(The bot-wall case is batchR_e2e's; the Google Calendar OAuth case went with the fake Google server.)"""
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

print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
