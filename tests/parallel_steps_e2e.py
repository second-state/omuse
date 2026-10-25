"""Parallel plan steps. The planner labels independent steps with one `parallel` group, the status note tells the
executor to do that stage in one turn, and the executor runs the turn's delegate calls at the same time: one sub-agent
per step, each with its own browser tab (lane), marking its step done. The batch form delegate(tasks=[…]) does the same
in one call. Scripted model: PARALLEL / PARALLELBATCH goals in tests/fake_llm.py; the pages are tests/fake_apps.py's
/slow (2 s each), so three of them in well under 6 s proves they ran side by side."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
BR = "http://127.0.0.1:8082"
LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
HB = {"X-Browser-Token": "bt-test"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200])
    if not cond:
        fails.append(name)


def run(msg):
    t0 = time.time()
    r = c.post(B + "/api/chat", json={"message": msg}, headers=H).json()
    t = {}
    while time.time() - t0 < 120:
        t = c.get(f"{B}/api/tasks/{r['task_id']}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            break
        time.sleep(0.3)
    return t


c.put(B + "/api/settings", json={"language": "en"}, headers=H)
for label, goal in (("three delegate calls in one turn", "PARALLEL compare three hotels"),
                    ("one delegate call with tasks=[…]", "PARALLELBATCH compare three hotels")):
    t = run(goal)
    ev = t.get("events", [])
    types = [e["type"] for e in ev]
    check(f"{label}: task completes", t.get("status") == "COMPLETED", (t.get("status"), t.get("error"), types))
    plan = next((e["data"] for e in ev if e["type"] == "plan"), {})
    check(f"{label}: the plan carries the parallel group",
          [s.get("parallel") for s in plan.get("steps", [])] == ["p1", "p1", "p1", None], plan)
    calls = c.get(LLM + "/calls_for", params={"marker": goal}).json()
    check(f"{label}: the status note names the parallel stage and how to run it",
          any("Next: steps s1, s2, s3 (parallel group p1)" in x["last"] and "ONE turn" in x["last"] for x in calls),
          [x["last"][-400:] for x in calls][:2])
    starts = [e for e in ev if e["type"] == "subagent_start"]
    dones = [e for e in ev if e["type"] == "subagent_done"]
    check(f"{label}: three sub-agents, one per step",
          [s["data"].get("step") for s in starts] == ["s1", "s2", "s3"] and len(dones) == 3, [s["data"] for s in starts])
    first_done = types.index("subagent_done") if "subagent_done" in types else -1
    check(f"{label}: all three started before any of them finished",
          first_done > 0 and types[:first_done].count("subagent_start") == 3, types)
    # concurrency proper: the sub-agents' page loads (2 s each) overlap in time instead of following each other
    navs_t = {}
    for e in ev:
        d = e.get("data") or {}
        if d.get("name") == "browser_navigate" and d.get("lane"):
            navs_t.setdefault(d["lane"], [None, None])[0 if e["type"] == "tool_call" else 1] = e["ts"]
    iv = sorted(v for v in navs_t.values() if None not in v)
    overlaps = sum(1 for i in range(len(iv)) for j in range(i + 1, len(iv)) if iv[j][0] < iv[i][1])
    span = (max(d["ts"] for d in dones) - min(s["ts"] for s in starts)) if starts and dones else 99
    check(f"{label}: the three page loads overlapped in time ({overlaps} of 3 pairs; whole stage {span:.1f} s)",
          len(iv) == 3 and overlaps >= 2 and span < 9, (iv, span))
    check(f"{label}: each sub-agent in its own lane", sorted(s["data"].get("lane") for s in starts) == ["sub1", "sub2", "sub3"],
          [s["data"] for s in starts])
    navs = [e["data"] for e in ev if e["type"] == "tool_call" and e["data"]["name"] == "browser_navigate"]
    check(f"{label}: the sub-agents' browser calls carry their lane",
          sorted(n.get("lane") for n in navs) == ["sub1", "sub2", "sub3"], navs)
    pages = [x.get("task_id") for x in c.get(BR + "/state", headers=HB).json().get("tasks", [])]
    check(f"{label}: the browser opened one page per lane (no shared tab)",
          all(f"{t['id']}~sub{i}" in pages for i in (1, 2, 3)), pages)
    res = t.get("result") or ""
    check(f"{label}: the answer has all three reports", all(f"Hotel{x}" in res for x in "ABC"), res[:800])
    steps = {s["id"]: s["status"] for s in (t.get("plan") or {}).get("steps", [])}
    check(f"{label}: the steps were marked done by their sub-agents",
          steps == {"s1": "done", "s2": "done", "s3": "done", "s4": "done"}, steps)
    secs = [d["data"].get("secs") for d in dones]
    check(f"{label}: every report carries ok and its duration", all(d["data"].get("ok") is True for d in dones)
          and all(isinstance(x, (int, float)) and x >= 2 for x in secs), [d["data"] for d in dones])
    if "BATCH" not in goal:
        par = [e["data"] for e in ev if e["type"] == "parallel_calls"]
        check(f"{label}: the turn's calls were announced as one parallel batch", len(par) == 1 and len(par[0]["calls"]) == 3, par)
        batch = [e["data"].get("batch") for e in ev if e["type"] == "tool_call" and e["data"]["name"] == "delegate"]
        check(f"{label}: each delegate call is marked as part of the batch", batch == [True, True, True], batch)

print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
