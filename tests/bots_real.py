"""Bots (crate::bots) with a real model (OMUSE_E2E_REAL=1, see travel_common.py): requests sent to the main bot are
handed to the right specialised bot with ask_bot (Travel, City guides, Credit cards), the bot's answer comes
back word for word with the link to its conversation, a follow-up goes back to the same bot conversation, and a
general request stays in Main. One TIMING line per request (model time excluded).

Not in run_in_docker.sh's default list (it needs a real model):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh bots_real
    (OMUSE_E2E_EXECUTOR=… OMUSE_E2E_PLANNER=… on the command line pick another model pair)"""
import datetime as dt
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_common as T  # noqa: E402

if not T.REAL:
    print("bots_real needs a real model (OMUSE_E2E_REAL=1)")
    sys.exit(1)

T.BOT = "main"
DATE = (dt.date.today() + dt.timedelta(days=30)).isoformat()


def trace(tids):
    for tid in tids:
        for e in T.task(tid).get("events", []):
            d = e.get("data") or {}
            if e["type"] == "tool_call":
                full = d.get("name", "").endswith(("flights_search", "hotels_search"))
                print(f"INFO [{tid[-6:]}] call {d.get('name')} {json.dumps(d.get('args'), ensure_ascii=False)[:None if full else 160]}")
            elif e["type"] in ("handoff", "handoff_done", "waiting", "error", "plan"):
                print(f"INFO [{tid[-6:]}] {e['type']} {json.dumps(d, ensure_ascii=False)[:240]}")


def conv(cid):
    return T.c.get(f"{T.B}/api/conversations/{cid}", headers=T.H).json()


def handed(label: str, info: dict, bot: str, *, kid_conv: str | None = None) -> dict:
    """The main task handed the request to `bot` and ended with its answer; returns the bot's task."""
    tid = info["tasks"][-1]
    t = T.task(tid)
    names = [x["name"] for x in T.calls_of([tid])]
    asks = [x["args"] for x in T.calls_of([tid]) if x["name"] == "ask_bot"]
    hs = [e["data"] for e in t.get("events", []) if e["type"] == "handoff"]
    kid = T.task(hs[0]["task_id"]) if hs and hs[0].get("task_id") else {}
    trace([tid] + ([kid["id"]] if kid.get("id") else []))
    T.check(f"{label}: the main task completed", t.get("status") == "COMPLETED", (t.get("status"), t.get("error"), str(t.get("result"))[:300]))
    T.check(f"{label}: Main handed the request to the {bot} bot (ask_bot bot={bot})", [a.get("bot") for a in asks][-1:] == [bot], (names, asks))
    T.check(f"{label}: Main did the work through the bot only (no search or skill of its own)",
            not [n for n in names if n not in ("ask_bot", "memory_search", "profile_get", "vault_list")], names)
    if kid_conv:
        T.check(f"{label}: the follow-up went to the same {bot} conversation", hs and hs[-1].get("conv_id") == kid_conv
                and asks and asks[-1].get("conversation_id") == kid_conv, (hs, asks))
    T.check(f"{label}: the {bot} bot's task completed", kid.get("status") == "COMPLETED", (kid.get("status"), kid.get("error")))
    T.check(f"{label}: Main's answer is the bot's answer word for word", bool(t.get("result")) and t.get("result") == kid.get("result"),
            (str(t.get("result"))[:200], str(kid.get("result"))[:200]))
    last = [m for m in conv(info["conv_id"])["messages"] if m["role"] == "assistant"][-1:]
    ho = ((last[0].get("meta") or {}).get("handoff") or {}) if last else {}
    T.check(f"{label}: Main's answer links to the bot's conversation", ho.get("bot") == bot and ho.get("conv_id") == (hs[-1]["conv_id"] if hs else None), ho)
    if hs:
        kc = conv(hs[-1]["conv_id"])["conversation"]
        T.check(f"{label}: the bot conversation belongs to {bot} and points back to Main", kc.get("bot_id") == bot and kc.get("parent_conv") == info["conv_id"], kc)
    kcalls = [x["name"] for x in T.calls_of([kid["id"]])] if kid.get("id") else []
    T.check(f"{label}: no browser in Main or in the bot", not [n for n in names + kcalls if n.startswith("browser_")], (names, kcalls))
    T.timing(f"bots_real/{label}", info, len(names) + len(kcalls), 0)
    return kid


T.setup()
try:
    # 1. a flight question to Main → Travel, which searches
    info = T.run(f"What is the cheapest flight from Singapore to Tokyo on {DATE}?", timeout=600)
    kid = handed("flight", info, "travel")
    kn = [x["name"] for x in T.calls_of([kid["id"]])] if kid.get("id") else []
    T.check("flight: the Travel bot searched flights itself", any(n.endswith("flights_search") for n in kn), kn)
    kid_conv = next((e["data"]["conv_id"] for e in T.task(info["tasks"][-1]).get("events", []) if e["type"] == "handoff"), None)
    main_conv = info["conv_id"]

    # 2. a follow-up in the same main conversation → the same Travel conversation
    r = T.c.post(T.B + "/api/chat", json={"message": "And the cheapest return flight a week later?", "conversation_id": main_conv}, headers=T.H).json()
    t0 = time.monotonic()
    end = T.task(r["task_id"])
    deadline = time.time() + 600
    while end.get("status") not in ("COMPLETED", "FAILED", "CANCELLED") and time.time() < deadline:
        time.sleep(0.5)
        end = T.task(r["task_id"])
    info2 = {"tasks": [r["task_id"]], "approvals": [], "t0": t0, "t1": time.monotonic(), "excluded": [], "conv_id": main_conv}
    handed("follow-up", info2, "travel", kid_conv=kid_conv)
    users = [m["content"] for m in conv(kid_conv)["messages"] if m["role"] == "user"] if kid_conv else []
    T.check("follow-up: the Travel conversation now has both questions", len(users) == 2, [u[:80] for u in users])

    # 3. a city guide question → Travel
    info = T.run("I'm visiting Tokyo for the first time next month. How do I get from Narita airport to Shinjuku, and which transit card should I get?", timeout=600)
    handed("city", info, "travel")

    # 4. which card to pay with → Credit cards
    info = T.run("Which credit card should I use to pay for a S$300 dinner in Singapore? I have a DBS Altitude Visa and a Citi Rewards Mastercard.", timeout=600)
    handed("card", info, "cards")

    # 5. a general request stays in Main
    info = T.run("Rewrite this sentence to sound more polite: 'Send me the report now.'", timeout=300)
    t = T.task(info["tasks"][-1])
    names = [x["name"] for x in T.calls_of(info["tasks"])]
    trace(info["tasks"])
    T.check("general: the task completed in Main without a hand-over", t.get("status") == "COMPLETED" and "ask_bot" not in names, (t.get("status"), names))
    T.check("general: Main answered itself", bool(str(t.get("result") or "").strip()), t.get("result"))
    T.timing("bots_real/general", info, len(names), 0)
finally:
    T.finish()
