"""Shared driver for the travel e2e suites (travel_flight_e2e.py, travel_hotel_e2e.py, travel_michelin_e2e.py).

Two modes:
* default: the scripted fake model (tests/fake_llm.py, magic words FLIGHTBOOK / HOTELBOOK / MICHELINBOOK) — deterministic.
* OMUSE_E2E_REAL=1: a real model through Settings → Models → "use my own model" (endpoint OMUSE_E2E_MODEL_URL, key
  OMUSE_E2E_MODEL_KEY, executor OMUSE_E2E_EXECUTOR, planner OMUSE_E2E_PLANNER; OMUSE_E2E_MODELS=alt picks
  OMUSE_E2E_ALT_EXECUTOR / OMUSE_E2E_ALT_PLANNER) with a natural-language request and looser checks.

In both modes the app talks to the model through tests/model_proxy.py (port 8098; started here when it is not running)
so model time can be taken out of the wall time (tests/exec_time.py). Each suite prints PASS/FAIL lines, a TIMING line
and exits 1 on a failure. The model override is always switched off at the end. After the run, the task / conversation
ids are written to /tmp/claude-0/travel_last.json for tests/screenshots_real.py.
"""
from __future__ import annotations

import datetime as dt
import json
import os
import re
import subprocess
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
TRAVEL = "http://127.0.0.1:8097"
PROXY = "http://127.0.0.1:8098"
FAKE_LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
REAL = os.environ.get("OMUSE_E2E_REAL", "") not in ("", "0", "false")
c = httpx.Client(timeout=120, trust_env=False)
fails: list[str] = []
LAST = "/tmp/claude-0/travel_last.json"
BROWSER_TOOLS = ("browser_",)
CARD = {"label": "Visa Lucas", "kind": "card", "values": {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123",
                                                         "holder": "LUCAS LU"}}
PASSPORT = {"label": "Passport Lucas", "kind": "id_document", "values": {"number": "E1234567X", "name": "LU LUCAS",
                                                                         "expiry": "2031-05-01", "country": "SG"}}


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1500], flush=True)
    if not cond:
        fails.append(name)


def day(n: int) -> str:
    return str(dt.date.today() + dt.timedelta(days=n))


# ------------------------------------------------------------------ stack setup
def _up(url: str) -> bool:
    try:
        c.get(url, timeout=3)
        return True
    except httpx.HTTPError:
        return False


def _wait_up(url: str, secs: float = 20) -> bool:
    t0 = time.time()
    while time.time() - t0 < secs:
        if _up(url):
            return True
        time.sleep(0.3)
    return False


def models() -> tuple[str, str]:
    alt = os.environ.get("OMUSE_E2E_MODELS", "") == "alt"
    ex = os.environ.get("OMUSE_E2E_ALT_EXECUTOR" if alt else "OMUSE_E2E_EXECUTOR", "")
    pl = os.environ.get("OMUSE_E2E_ALT_PLANNER" if alt else "OMUSE_E2E_PLANNER", "")
    return ex, pl


def start_proxy() -> None:
    """model_proxy.py on 8098 with the right upstream: the real endpoint (and key) or the fake model."""
    want = os.environ.get("OMUSE_E2E_MODEL_URL", "").rstrip("/") if REAL else FAKE_LLM
    if _up(PROXY + "/_info"):
        if c.get(PROXY + "/_info").json().get("upstream", "").rstrip("/") == want:
            c.post(PROXY + "/_reset")
            return
        subprocess.run(["bash", "-c", "fuser -k 8098/tcp >/dev/null 2>&1; pkill -f 'model_proxy:app' >/dev/null 2>&1"], check=False)
        time.sleep(1)
    env = {**os.environ, "MODEL_PROXY_UPSTREAM": want, "MODEL_PROXY_KEY": os.environ.get("OMUSE_E2E_MODEL_KEY", "") if REAL else ""}
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    log = open("/tmp/claude-0/model_proxy.log" if os.path.isdir("/tmp/claude-0") else os.devnull, "a")
    subprocess.Popen([sys.executable, "-m", "uvicorn", "tests.model_proxy:app", "--port", "8098", "--log-level", "warning"],
                     cwd=root, env={**env, "PYTHONPATH": root}, stdout=log, stderr=log, start_new_session=True)
    if not _wait_up(PROXY + "/_info"):
        raise SystemExit("model_proxy did not start on 8098")


def use_proxy_model() -> None:
    """Settings → Models → use my own model, pointed at the proxy (the endpoint the app sees in both modes)."""
    ex, pl = models() if REAL else ("fake", "")
    body = {"model_override": True, "model_base_url": PROXY + "/v1", "model_api_key": "via-proxy", "model_name": ex,
            "planner_model": pl}
    r = c.put(B + "/api/settings", json=body, headers=H)
    check("Settings → Models: own model through the timing proxy", r.status_code == 200 and r.json()["settings"]["model_override"] is True,
          (r.status_code, r.text[:300]))


def model_off() -> None:
    r = c.put(B + "/api/settings", json={"model_override": False}, headers=H)
    check("own-model override switched off again", r.status_code == 200 and r.json()["settings"]["model_override"] is False, r.text[:200])


def connect_travel() -> None:
    if not _up(TRAVEL + "/_log"):
        raise SystemExit("fake_travel is not running on 8097 (tests/run_local.sh starts it)")
    c.post(TRAVEL + "/_reset")
    for s in c.get(B + "/sentinel/api/mcp/servers", headers=H).json().get("servers", []):
        if s.get("id") == "travel":
            c.delete(B + "/sentinel/api/mcp/servers/travel", headers=H)
    r = c.post(B + "/sentinel/api/mcp/servers", headers=H, json={"name": "Travel", "url": TRAVEL + "/mcp", "auth_type": "bearer",
                                                                  "token": "test-travel-token", "data_class": "PUBLIC"})
    ok = r.status_code == 200
    tools = {t["name"]: t for t in r.json().get("tools", [])} if ok else {}
    check("travel data connector added (MCP, 6 read-only tools on auto)",
          ok and len(tools) == 6 and all(t.get("mode") == "auto" and t.get("kind") == "read" for t in tools.values()), r.text[:600])
    names = [t["function"]["name"] for t in c.get(B + "/internal/catalog", headers={"X-Persona-Runtime": "rt-test"}).json()["tools"]]
    check("connector tools published as mcp_travel__*", {"mcp_travel__flights_search", "mcp_travel__hotel_rooms",
                                                         "mcp_travel__restaurant_availability"} <= set(names),
          [n for n in names if n.startswith("mcp_")])


def seed_vault() -> dict:
    have = {i["label"]: i["id"] for i in c.get(B + "/sentinel/api/vault", headers=H).json().get("items", [])}
    out = {}
    for item in (CARD, PASSPORT):
        if item["label"] not in have:
            r = c.post(B + "/sentinel/api/vault", json=item, headers=H)
            have[item["label"]] = (r.json().get("item") or r.json()).get("id")
        out[item["kind"]] = have[item["label"]]
    return out


def setup() -> None:
    os.makedirs("/tmp/claude-0", exist_ok=True)
    c.put(B + "/api/settings", json={"language": "en"}, headers=H)
    start_proxy()
    connect_travel()
    seed_vault()
    use_proxy_model()


# ------------------------------------------------------------------ running a task
def task(tid: str) -> dict:
    t = c.get(f"{B}/api/tasks/{tid}").json()
    return t.get("task") or t


def run(message: str, *, follow_up: str | None = None, timeout: float = 120) -> dict:
    """Send `message`, approve every approval the task raises (purchase_confirm, vault fills, the booking click) and wait
    for the end. In real mode, a task that stops after showing choices gets one follow-up in the same conversation.
    Returns {tasks, approvals, t0, t1, excluded, conv_id}."""
    approvals, excluded, tasks = [], [], []
    t0 = time.monotonic()
    r = c.post(B + "/api/chat", json={"message": message}, headers=H).json()
    conv, tid = r.get("conversation_id"), r["task_id"]
    tasks.append(tid)
    sent_follow = False
    deadline = time.time() + timeout
    while time.time() < deadline:
        t = task(tid)
        st = t.get("status")
        if st == "WAITING_APPROVAL":
            seen = time.monotonic()
            pend = [a for a in c.get(B + "/sentinel/api/approvals?status=pending", headers=H).json()["approvals"] if a["task_id"] == tid]
            for a in pend:
                rr = c.post(f"{B}/sentinel/api/approvals/{a['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
                done = time.monotonic()
                created = a.get("created_at")
                start = seen
                if isinstance(created, (int, float)):   # wall clock → monotonic: the wait began when the approval was created
                    start = min(seen, done - max(0.0, time.time() - float(created)))
                excluded.append((start, done))
                approvals.append({"id": a["id"], "tool": a["tool"], "args": a.get("args"), "reason": a.get("reason", ""),
                                  "status": rr.json().get("status") if rr.status_code == 200 else rr.status_code})
            time.sleep(0.2)
            continue
        if st == "WAITING_EXTERNAL":
            check("the task did not stop for a takeover", False, json.dumps(t.get("events", [])[-4:])[:800])
            break
        if st in ("COMPLETED", "FAILED", "CANCELLED"):
            if REAL and follow_up and not sent_follow and not booked() and st == "COMPLETED":
                sent_follow = True
                r = c.post(B + "/api/chat", json={"message": follow_up, "conversation_id": conv}, headers=H).json()
                tid = r["task_id"]
                tasks.append(tid)
                continue
            break
        time.sleep(0.2)
    t1 = time.monotonic()
    return {"tasks": tasks, "approvals": approvals, "t0": t0, "t1": t1, "excluded": excluded, "conv_id": conv}


def ok(res: dict) -> bool:
    """A tool_result event succeeded (results resumed after an approval carry status instead of ok)."""
    return res.get("ok") is True or (res.get("ok") is None and res.get("status") in ("ok", "approved"))


def booked() -> list[dict]:
    return c.get(TRAVEL + "/_log").json()["bookings"]


def calls_of(tasks: list[str]) -> list[dict]:
    out = []
    for tid in tasks:
        for e in task(tid).get("events", []):
            if e["type"] == "tool_call" and not (e["data"] or {}).get("sub"):
                out.append({"name": e["data"]["name"], "args": e["data"].get("args") or {}, "task": tid})
    return out


def results_of(tasks: list[str], name: str) -> list[dict]:
    return [e["data"] for tid in tasks for e in task(tid).get("events", []) if e["type"] == "tool_result" and e["data"]["name"] == name]


# ------------------------------------------------------------------ the shared checks
def verify(suite: str, kind: str, skill: str, data_tools: tuple[str, ...], run_info: dict, *, max_browser: int,
           expect: dict | None = None) -> None:
    """kind: flight / hotel / restaurant (the booking page kind in fake_travel)."""
    tasks = run_info["tasks"]
    final = task(tasks[-1])
    calls = calls_of(tasks)
    names = [x["name"] for x in calls]
    for tid in tasks:   # the run's trace, for reading a failure
        for e in task(tid).get("events", []):
            d = e.get("data") or {}
            if e["type"] == "tool_call":
                print(f"INFO call {d.get('name')} {json.dumps(d.get('args'), ensure_ascii=False)[:160]}")
            elif e["type"] == "tool_result":
                print(f"INFO  -> ok={ok(d)} {str(d.get('preview') or d.get('error') or '')[:160]!r}")
            elif e["type"] in ("waiting", "error", "retry", "choices", "choices_rejected"):
                print(f"INFO {e['type']} {json.dumps(d, ensure_ascii=False)[:300]}")
    check("the task completed", final.get("status") == "COMPLETED", (final.get("status"), final.get("error"), final.get("result")))
    loads = [x for x in calls if x["name"] == "load_skill"]
    check(f"the {skill} skill was loaded", any(skill in json.dumps(x["args"]) for x in loads), [x["args"] for x in loads])
    mcp = [i for i, n in enumerate(names) if n.startswith("mcp_travel__")]
    used = {n.split("__", 1)[1] for n in names if n.startswith("mcp_travel__")}
    check("travel data tools were used: " + ", ".join(data_tools), set(data_tools) <= used, names)
    br = [i for i, n in enumerate(names) if n.startswith(BROWSER_TOOLS)]
    check("all data tools ran before the first browser call", mcp and (not br or max(mcp) < min(br)), names)
    check("no web search / page reading: the data came from the connector",
          not any(n in ("browser_search", "browser_read") for n in names), names)
    navs = [x["args"].get("url", "") for x in calls if x["name"] == "browser_navigate"]
    check("the browser only opened the provider's booking page", navs and all("shop.test:8097/book/" in u for u in navs), navs)
    check(f"few browser calls ({len(br)} <= {max_browser})", 0 < len(br) <= max_browser, [names[i] for i in br])
    tl = c.get(TRAVEL + "/_log").json()
    bk = [b for b in tl["bookings"] if b["kind"] == kind]
    check("the booking reached the provider", len(bk) == 1, tl["bookings"] or tl.get("rejected"))
    b = bk[-1] if bk else {}
    nav_ids = [u.rsplit("/", 1)[-1] for u in navs]
    check("the booked offer is the one whose booking page was opened", b.get("id") in nav_ids, (b.get("id"), nav_ids))
    if expect:
        for k, v in expect.items():
            check(f"booking {k} = {v}", b.get(k) == v, b)
    check("booked under the traveller's name and email", b.get("name", "").lower().replace(" ", "") in ("lucaslu", "lulucas")
          and b.get("email") == "lucas@example.com", b)
    pcs = [a for a in run_info["approvals"] if a["tool"] == "purchase_confirm"]
    if b.get("amount"):
        check("one purchase confirmation, approved, with the booked amount",
              pcs and pcs[0]["status"] == "approved" and abs(float((pcs[-1]["args"] or {}).get("total") or 0) - b["amount"]) < 0.01,
              (run_info["approvals"], b.get("amount")))
    clicks = results_of(tasks, "browser_click")
    check("the booking click went through (approved or covered by the purchase confirmation)",
          any(ok(x) for x in clicks), clicks[-2:])
    res = str(final.get("result") or "")
    check("the final answer quotes the booking reference", b.get("ref") and b["ref"] in res, (b.get("ref"), res[:600]))
    failed = [e["data"] for tid in tasks for e in task(tid).get("events", []) if e["type"] == "tool_result"
              and not ok(e["data"]) and e["data"]["name"] not in ("browser_snapshot",)]
    if REAL:
        check("no more than 2 failed tool calls", len(failed) <= 2, failed[:4])
    else:
        check("no tool call failed", not failed, failed[:4])
    timing(suite, run_info, len(calls), len(br))
    os.makedirs(os.path.dirname(LAST), exist_ok=True)
    json.dump({"suite": suite, "tasks": tasks, "conv_id": run_info["conv_id"], "booking": b}, open(LAST, "w"))


def timing(suite: str, run_info: dict, tool_calls: int, browser_calls: int) -> None:
    from tests.exec_time import execution_time
    iv = c.get(PROXY + "/_intervals").json()
    t = execution_time(run_info["t0"], run_info["t1"], iv["intervals"], exclude=run_info["excluded"], now=iv["now"])
    ex, pl = models() if REAL else ("fake", "")
    line = {"suite": suite, "mode": "real" if REAL else "scripted", "executor": ex, "planner": pl or ex,
            "wall_s": t["wall_s"], "model_s": t["model_s"], "exec_s": t["exec_s"], "approval_wait_s": t["excluded_s"],
            "model_calls": t["model_calls"], "retries": t["retries"], "tool_calls": tool_calls, "browser_calls": browser_calls}
    print("TIMING " + json.dumps(line), flush=True)
    check("model requests went through the timing proxy", t["model_calls"] > 0, t)


def finish() -> None:
    try:
        model_off()
    finally:
        print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"), flush=True)
        sys.exit(1 if fails else 0)


def ref_in(text: str) -> str | None:
    m = re.search(r"TRV-[A-Z0-9]{7}", text or "")
    return m.group(0) if m else None
