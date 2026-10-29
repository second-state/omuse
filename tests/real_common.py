"""Shared driver for the real-model suites (`tests/run_in_docker.sh real`): plain requests sent to the main bot, which
routes them (crate::bots) — so every suite also checks that the request reached the right bot. The world is the same as
the scripted suites' (mail in the local IMAP server, the iCal calendar, the travel connector, the fake Drive, pages under
tests/pages); the model is the real one from the e2e env file, through tests/model_proxy.py for the timing.

    tests/run_in_docker.sh real          # the env file's models: planner xai/grok-4.7, executor openai/gpt-6-luna
    OMUSE_E2E_EXECUTOR=… OMUSE_E2E_PLANNER=… tests/run_in_docker.sh real   # another pair

ask() approves every approval as it comes (the main task's and the bot's), records it, and stops at a takeover request
(the user's turn: a sign-in, a Pay or Submit button) instead of failing; the checks are looser than the scripted suites'
— what the model must get right, not the exact calls."""
from __future__ import annotations

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
from tests import travel_common as T  # noqa: E402

B, S, H, c = T.B, T.B + "/sentinel/api", T.H, T.c
check, task = T.check, T.task
TERMINAL = ("COMPLETED", "FAILED", "CANCELLED")
SUITE = os.path.splitext(os.path.basename(sys.argv[0]))[0]
_tasks: list[str] = []

if not T.REAL:
    print(f"{SUITE} needs a real model (OMUSE_E2E_REAL=1, see tests/real_common.py)")
    sys.exit(1)


def setup(*, mail: list | None = None, calendar: bool = False, travel: bool = False, vault: bool = True,
          profile: dict | None = None) -> None:
    """The world first (mail, calendar, travel connector, Vault, profile), then the real model through the proxy."""
    os.makedirs("/tmp/claude-0", exist_ok=True)
    K.setup(mail=mail, calendar=calendar)
    T.start_proxy()
    if travel:
        T.connect_travel()
    if vault:
        T.seed_vault()
    for k, v in (profile or {}).items():
        c.put(B + "/api/profile", json={"key": k, "value": v}, headers=H)
    T.use_proxy_model()


def _approve(tids: set[str], out: list[dict], excluded: list) -> None:
    for a in c.get(S + "/approvals?status=pending", headers=H).json()["approvals"]:
        if a["task_id"] in tids:
            t0 = time.monotonic()
            rr = c.post(f"{S}/approvals/{a['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
            excluded.append((t0, time.monotonic()))
            out.append({"tool": a["tool"], "args": a.get("args"), "task": a["task_id"],
                        "status": rr.json().get("status") if rr.status_code == 200 else rr.status_code})


def ask(message: str, *, conv: str | None = None, attachments: list | None = None, timeout: float = 900) -> dict:
    """Send `message` to Main (or to `conv`) and wait for the end or a takeover request. Returns
    {conv_id, main, kid, bot, kid_conv, tasks, calls, names, result, approvals, takeover, route, t0, t1, excluded}:
    `kid` is the bot's task when Main handed over (else {}); `calls` are both tasks' tool calls (sub-agents included,
    marked sub); `takeover` is the task that waits for the user, if any."""
    body = {"message": message, "conversation_id": conv, "attachments": attachments or [], "bot": "main"}
    t0 = time.monotonic()
    r = c.post(B + "/api/chat", json=body, headers=H).json()
    tid, cid = r["task_id"], r.get("conversation_id")
    approvals, excluded, takeover = [], [], None
    deadline = time.time() + timeout
    t = {}
    while time.time() < deadline:
        t = task(tid)
        st = t.get("status")
        kid = (t.get("waiting") or {}).get("task_id") if st == "WAITING_BOT" else None
        kst = task(kid).get("status") if kid else None
        if st == "WAITING_APPROVAL" or kst == "WAITING_APPROVAL":
            _approve({tid} | ({kid} if kid else set()), approvals, excluded)
        elif st == "WAITING_EXTERNAL" or kst == "WAITING_EXTERNAL":
            takeover = kid if kst == "WAITING_EXTERNAL" else tid
            break
        elif st in TERMINAL:
            break
        time.sleep(0.4)
    t1 = time.monotonic()
    t = task(tid)
    hs = [e["data"] for e in t.get("events", []) if e["type"] == "handoff"]
    kid = task(hs[-1]["task_id"]) if hs else {}
    tasks = [tid] + ([kid["id"]] if kid.get("id") else [])
    _tasks.extend(tasks)
    calls = [dict(e["data"], task=x) for x in tasks for e in task(x).get("events", []) if e["type"] == "tool_call"]
    route = next((e["data"] for e in t.get("events", []) if e["type"] == "route"), {})
    info = {"conv_id": cid, "main": t, "kid": kid, "bot": hs[-1]["bot"] if hs else "main",
            "kid_conv": hs[-1]["conv_id"] if hs else None, "tasks": tasks, "calls": calls,
            "names": [x["name"] for x in calls if x["name"] != "ask_bot"],
            "result": str((kid or t).get("result") or t.get("result") or ""), "approvals": approvals,
            "takeover": takeover, "route": route, "t0": t0, "t1": t1, "excluded": excluded}
    trace(info)
    return info


def trace(info: dict) -> None:
    print(f"INFO route {json.dumps(info['route'], ensure_ascii=False)[:300]}")
    for x in info["calls"]:
        print(f"INFO [{x['task'][-6:]}] call {x['name']}{' (sub)' if x.get('sub') else ''} "
              f"{json.dumps(x.get('args'), ensure_ascii=False)[:200]}")
    for a in info["approvals"]:
        print(f"INFO approval {a['tool']} → {a['status']}")
    print(f"INFO answer ({info['bot']}): {info['result'][:700]!r}")


def routed(label: str, info: dict, bot: str) -> None:
    """The request went to `bot` ("main": Main answered itself), and Main's answer is that bot's, word for word."""
    check(f"{label}: routed to the {bot} bot", info["bot"] == bot, (info["bot"], info["route"]))
    if bot != "main" and info["kid"]:
        done = info["main"].get("status") in TERMINAL
        check(f"{label}: Main shows the {bot} bot's answer word for word", not done or info["main"].get("result") == info["kid"].get("result"),
              (str(info["main"].get("result"))[:200], str(info["kid"].get("result"))[:200]))


def finished(label: str, info: dict, *, takeover: bool = False) -> None:
    """Completed — or, when `takeover`, stopped for the user's turn (sign-in, Pay, Submit) with nothing pressed for them."""
    end = info["kid"] or info["main"]
    if takeover:
        check(f"{label}: stopped for the user's turn (takeover requested) or completed", info["takeover"] is not None
              or end.get("status") == "COMPLETED", (end.get("status"), end.get("error")))
    else:
        check(f"{label}: the task completed", end.get("status") == "COMPLETED" and info["main"].get("status") == "COMPLETED",
              (info["main"].get("status"), end.get("status"), end.get("error")))


def used(info: dict, *names: str) -> bool:
    """Every one of `names` was called (a connector's mcp_<server>__<name> counts as <name>)."""
    have = {n.split("__", 1)[-1] for n in info["names"]}
    return all(n in have for n in names)


def no_browser(label: str, info: dict) -> None:
    br = [n for n in info["names"] if n.startswith("browser_")]
    check(f"{label}: no browser", not br, br)


def mentions(text: str, *words: str) -> bool:
    low = text.lower()
    return any(w.lower() in low for w in words)


def timing(label: str, info: dict) -> None:
    T.timing(f"{SUITE}/{label}", info, len(info["names"]), sum(1 for n in info["names"] if n.startswith("browser_")))


def finish() -> None:
    os.makedirs(os.path.dirname(T.LAST), exist_ok=True)
    json.dump({"suite": SUITE, "tasks": _tasks}, open(T.LAST, "w"))
    T.finish()
