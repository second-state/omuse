"""0.2.22: an agent stuck on one source changes strategy instead of looping (the 2026-10-02 HSBC run: one Google Finance
link opened 70 times, 22 re-plans). Copies of a call in one turn are skipped, a host that keeps failing is blocked (the
`dead_host` reason code, and the host in the live state's `dead_ends`), and a task that is still stuck after a re-plan
stops: the tool list is withdrawn and it answers with what it has. Every check is on code-side state (events, tool
lists, the JSON live state, reason codes), never on words written to the model."""
import json
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:900])
    if not cond:
        fails.append(name)


def run(goal, timeout=120):
    r = c.post(B + "/api/chat", json={"message": goal}, headers=H)
    r.raise_for_status()
    tid = r.json()["task_id"]
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.5)
    return t


s0 = c.get(B + "/api/settings", headers=H).json()["settings"]
c.put(B + "/api/settings", json={"max_steps": 40}, headers=H).raise_for_status()

# ---------------------------------------------------------------- 1. the HSBC loop
t = run("STUCKLOOP 做一下 HSBC 过去 5 年的走势")
ev = t["events"]
navs = [e["data"] for e in ev if e["type"] == "tool_call" and e["data"]["name"] == "browser_navigate"]
res = [e["data"] for e in ev if e["type"] == "tool_result" and e["data"]["name"] == "browser_navigate"]
ran = [r for r in res if r.get("ok") and not r.get("skipped")]
skipped = [r for r in res if r.get("skipped")]
replans = [e for e in ev if e["type"] == "replanning"]
gave = [e for e in ev if e["type"] == "gave_up"]
thinking = sum(1 for e in ev if e["type"] == "thinking")
check("stops on its own instead of looping to the step limit", t["status"] == "FAILED" and thinking <= 8,
      (t["status"], t.get("error"), thinking))
check("failure reason says it stopped retrying", "停止重试" in (t.get("error") or ""), t.get("error"))
check("the page itself was opened once", len(ran) == 1, [r.get("preview", "")[:80] for r in res])
check("copies of a call in the same turn are skipped, not run", len(skipped) >= 4 and "Skipped: identical" in skipped[0]["preview"],
      [r.get("preview", "")[:80] for r in skipped][:3])
check("at most one re-plan, and it is told the dead end", len(replans) == 1 and "shop.test" in replans[0]["data"].get("dead_ends", ""),
      [e["data"] for e in replans])
check("gave_up event recorded", len(gave) == 1, gave)
stuck_calls = httpx.get("http://127.0.0.1:8090/calls_for", params={"marker": "STUCKLOOP"}, timeout=10).json()
stuck_calls = [x for x in stuck_calls if "focused sub-agent" not in str(x.get("system", ""))]
check("after giving up the tool list is withdrawn and the final answer is written without tools",
      "STUCK SUMMARY" in (t.get("result") or "") and stuck_calls and stuck_calls[-1]["has_tools"] is False,
      (t.get("result"), [x["has_tools"] for x in stuck_calls]))
check("far fewer calls than the 74 of the real run", len(navs) <= 15, len(navs))

# ---------------------------------------------------------------- 2. a source that keeps failing -> switch site
t = run("SWITCHSRC 找 HSBC 的历史股价")
ev = t["events"]
res = [e["data"] for e in ev if e["type"] == "tool_result" and e["data"]["name"] == "browser_navigate"]
check("switching source finishes the task", t["status"] == "COMPLETED", (t["status"], t.get("error")))
check("first three failures on the bad host really ran", all(not r.get("ok") for r in res[:3]) and len(res) >= 5,
      [r.get("preview", "")[:80] for r in res])
check("4th try on the same host is refused by code (the dead_host reason code, not run)",
      len(res) > 3 and res[3]["preview"].startswith("ERROR: [dead_host]") and "nosuch.test" in res[3]["preview"] and res[3].get("ok") is False,
      res[3]["preview"][:300] if len(res) > 3 else res)
check("the other site then works", res[4].get("ok") is True, res[4] if len(res) > 4 else res)
check("the live state (JSON) lists the dead end", "dead_ends_in_prompt=True" in (t.get("result") or ""), t.get("result"))
check("no give-up when the switch made progress", not any(e["type"] == "gave_up" for e in ev))

# ---------------------------------------------------------------- 3. prompt-cache friendliness (0.2.23)
calls = httpx.get("http://127.0.0.1:8090/calls_for", params={"marker": "SWITCHSRC"}, timeout=10).json()
# the chat-title call and the main bot's router (agent/bots.rs `route`) are not steps
# … nor the small JSON calls that read an answer (the classifier, the answer review) or extract facts
calls = [x for x in calls if not str(x.get("system", "")).startswith(("You name conversations", "## Route", "You classify an assistant",
                                                                      "You check an assistant", "Read the booking emails",
                                                                      "You write the final answer", "你根据一个任务", "Translate the user's text"))]
systems = {x["system"] for x in calls}
check("system prompt identical on every step (model server can reuse its cache)", len(calls) >= 4 and len(systems) == 1,
      (len(calls), len(systems)))


# the live state comes last only when it carries something (a plan, failed sources): a bare clock as the last message got
# answered instead of the user's request (2026-10-30). It is data: one JSON object {"state": {...}}, never in the system prompt.
def state_of(x):
    try:
        return json.loads(x["last"])["state"] if x["last"].startswith('{"state"') else None
    except ValueError:
        return None


check("live state (JSON: dead ends) comes last when there is one, never in the system prompt; the first step ends with the request",
      state_of(calls[0]) is None and "SWITCHSRC" in calls[0]["last"]
      and all("nosuch.test" not in x["system"] for x in calls)
      and any((state_of(x) or {}).get("dead_ends") and state_of(x)["dead_ends"][0]["host"] == "nosuch.test"
              and state_of(x)["dead_ends"][0]["why_code"] == "failed_repeatedly" for x in calls), [x["last"][:160] for x in calls])
# ---------------------------------------------------------------- 4. time budget (0.2.23)
c.put(B + "/api/settings", json={"max_minutes": 0.5}, headers=H).raise_for_status()
t0 = time.time()
t = run("SLOWTASK 查一下很多网页")
took = time.time() - t0
check("time budget ends a slow task with an answer", t["status"] == "FAILED" and "用时上限" in (t.get("error") or "")
      and "SLOW SUMMARY" in (t.get("result") or ""), (t["status"], t.get("error"), t.get("result")))
slow = httpx.get("http://127.0.0.1:8090/calls_for", params={"marker": "SLOWTASK"}, timeout=10).json()
check("the page tools are withdrawn before the limit (70% of the time budget), the finishing tools stay",
      "navigate withdrawn before the limit: True" in (t.get("result") or "") and slow and slow[-1]["has_tools"] is False,
      (t.get("result"), [x["has_tools"] for x in slow]))
check("stopped near the limit", took < 60, took)
c.put(B + "/api/settings", json={"max_minutes": s0.get("max_minutes", 20)}, headers=H)
# ---------------------------------------------------------------- 4b. re-making the same file (0.2.23)
c.put(B + "/api/settings", json={"max_steps": 40}, headers=H).raise_for_status()
t = run("REMAKEXLSX 香港行程导出 Excel")
res = [e["data"] for e in t["events"] if e["type"] == "tool_result" and e["data"]["name"] == "make_xlsx"]
check("2nd remake carries the structured count ({\"made_times\": 2})", len(res) >= 2 and '{"made_times": 2}' in res[1]["preview"]
      and "made_times" not in res[0]["preview"], [r["preview"][:160] for r in res[:2]])
check("5th remake of the same file is refused (the remade reason code)", len(res) == 6 and all(r["ok"] for r in res[:4]) and not res[4]["ok"]
      and res[4]["preview"].startswith("ERROR: [remade]"), [(r["ok"], r["preview"][:80]) for r in res])
# ---------------------------------------------------------------- 5. tool-call markup never becomes the answer (0.2.23)
t = run("MARKUPFINAL 查一下特斯拉的财报")
# a reply made only of (cut-off) tool-call markup: stripped in code, the answer written by the tool-free reply call
# (rescued_final event), and the task completes
check("raw <tool_call> text never becomes the final answer (the task completes with plain text)",
      t["status"] == "COMPLETED" and bool((t.get("result") or "").strip()) and "<tool_call>" not in (t.get("result") or "")
      and "<function=" not in (t.get("result") or "") and any(e["type"] == "rescued_final" for e in t["events"]),
      (t["status"], t.get("result"), [e["type"] for e in t["events"]]))
c.put(B + "/api/settings", json={"max_steps": s0.get("max_steps", 30)}, headers=H)
print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
