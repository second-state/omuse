"""Shared driver for the scripted (fake-model) suites that run a skill or a bot (bots_e2e.py, city_guides_e2e.py, the
card UIs, …): the scripted model (tests/fake_llm.py) plays the agent against the app itself; uploads go through
/api/upload. Each suite checks which tools the skill made the agent use and the rules it must keep (no browser where a
data source exists, secrets only through the vault, …). No fake services: nothing here seeds a mailbox, a calendar or
a connector."""
from __future__ import annotations

import json
import sys
import time

import httpx

try:
    from show_common import appended_block, shows, shows_items  # noqa: F401  (shows: for the suites)
except ImportError:  # imported as tests.skills_common
    from tests.show_common import appended_block, shows, shows_items  # noqa: F401

B = "http://127.0.0.1:8080"
S = B + "/sentinel/api"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=90, trust_env=False)
fails: list[str] = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200], flush=True)
    if not cond:
        fails.append(name)


def finish():
    print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"), flush=True)
    sys.exit(1 if fails else 0)


# ------------------------------------------------------------------ the user's data
def set_profile(**fields) -> None:
    for k, v in fields.items():
        c.put(B + "/api/profile", json={"key": k, "value": v}, headers=H)


def vault_item(label: str, kind: str, values: dict, domains: str = "", country: str = "") -> str:
    have = {i["label"]: i["id"] for i in c.get(S + "/vault", headers=H).json().get("items", [])}
    if label in have:
        return have[label]
    r = c.post(S + "/vault", headers=H, json={"label": label, "kind": kind, "values": values, "domains": domains, "country": country})
    return (r.json().get("item") or r.json()).get("id")


def upload(name: str, data: bytes) -> dict:
    r = c.put(f"{B}/api/upload", params={"name": name}, content=data, headers=H)
    r.raise_for_status()
    return r.json()


# ------------------------------------------------------------------ running a task
# the bot a new conversation of run() belongs to (crate::bots): a suite of one bot's skills sets it once, e.g.
# K.BOT = "travel"; "main" is the main bot, which hands specialised requests over with ask_bot
BOT = "main"


def run(message: str, *, conv: str | None = None, attachments: list | None = None, approve: bool = True,
        stop_on_external: bool = True, timeout: float = 150, bot: str | None = None) -> dict:
    """Send `message` (a new conversation goes to `bot`, else BOT); approve approvals as they come (recording them);
    return the finished task with `_approvals` and `_seen` (every status seen)."""
    r = c.post(B + "/api/chat", json={"message": message, "conversation_id": conv, "attachments": attachments or [],
                                      "bot": bot or BOT}, headers=H).json()
    tid = r["task_id"]
    seen, approvals, t0, t = set(), [], time.time(), {}
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        seen.add(t["status"])
        if t["status"] == "WAITING_APPROVAL":
            if not approve:
                break
            for a in c.get(S + "/approvals?status=pending", headers=H).json()["approvals"]:
                if a["task_id"] == tid:
                    rr = c.post(f"{S}/approvals/{a['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
                    approvals.append({"tool": a["tool"], "args": a.get("args"), "ok": rr.status_code == 200})
            time.sleep(0.3)
            continue
        if t["status"] == "WAITING_EXTERNAL" and stop_on_external:
            break
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            break
        time.sleep(0.3)
    t["_approvals"], t["_seen"], t["_conv"] = approvals, seen, r.get("conversation_id")
    return t


def wait_end(tid: str, timeout: float = 90) -> dict:
    t0, t = time.time(), {}
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.3)
    return t


def calls(t: dict, sub: bool = False) -> list[dict]:
    return [e["data"] for e in t.get("events", []) if e["type"] == "tool_call" and bool((e["data"] or {}).get("sub")) == sub]


def names(t: dict) -> list[str]:
    return [x["name"] for x in calls(t)]


WORKFLOW_TOOLS = ("travel_search", "travel_book", "shop_order")


def work(t: dict) -> list[str]:
    """The calls of the run without the skill load and the workflow tool calls: the code machine's steps."""
    return [n for n in names(t) if n != "load_skill" and n not in WORKFLOW_TOOLS]


def workflow(t: dict) -> list[dict]:
    """The workflow tool calls of the run as the model made them (`workflow_call` events: the tool and its result)."""
    return [e["data"] for e in t.get("events", []) if e["type"] == "workflow_call"]


def result(t: dict) -> dict:
    """The JSON result of the run's (last) workflow machine (`workflow_result` event), {} when none ran."""
    r = [e["data"]["result"] for e in t.get("events", []) if e["type"] == "workflow_result"]
    return r[-1] if r else {}


def says_all(t: dict) -> bool:
    """The answer's block that code appends after the reply (tests/show_common.py) carries every `show` item of the
    machine's result — the facts the user must read; the model's own words are not checked."""
    return shows_items(t.get("result") or "", result(t).get("show") or [])


def block(t: dict) -> str:
    """The block code appended after the reply (the "- Label: value" lines), "" when there is none."""
    return appended_block(t.get("result") or "")


def results(t: dict, name: str) -> list[dict]:
    return [e["data"] for e in t.get("events", []) if e["type"] == "tool_result" and e["data"].get("name") == name]


def loaded(t: dict, skill: str) -> bool:
    return any(x["name"] == "load_skill" and skill in json.dumps(x.get("args")) for x in calls(t))


def batches(t: dict) -> list[list[str]]:
    return [[x["name"] for x in e["data"]["calls"]] for e in t.get("events", []) if e["type"] == "parallel_calls"]


def conv_cards(conv: str, kind: str) -> list[dict]:
    out = []
    for m in c.get(f"{B}/api/conversations/{conv}").json()["messages"]:
        if m["role"] == "system":
            try:
                j = json.loads(m["content"])
            except Exception:
                continue
            if j.get("type") == kind:
                out.append(j)
    return out


def trace(t: dict) -> None:
    for e in t.get("events", []):
        d = e.get("data") or {}
        if e["type"] == "tool_call":
            print(f"INFO call {d.get('name')} {json.dumps(d.get('args'), ensure_ascii=False)[:150]}")
        elif e["type"] == "tool_result":
            print(f"INFO  -> ok={d.get('ok')} {str(d.get('preview') or '')[:150]!r}")
        elif e["type"] in ("waiting", "parallel_calls", "approval_auto", "error"):
            print(f"INFO {e['type']} {json.dumps(d, ensure_ascii=False)[:200]}")


def setup(*, language: str = "en") -> None:
    c.put(B + "/api/settings", json={"language": language}, headers=H)
