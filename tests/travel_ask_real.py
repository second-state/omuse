"""Two research-only travel questions with a real model (OMUSE_E2E_REAL=1, see travel_common.py): the standard skills
(flight-booking, michelin-dining), the travel data connector for the facts, and — the point of the check — no browser,
no approval and no takeover, so nothing ever opens for the user. One TIMING line per question (model time excluded).

Not in run_in_docker.sh's default list (it needs a real model):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_MODELS=alt tests/run_in_docker.sh travel_ask_real"""
import datetime as dt
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import fake_travel  # noqa: E402
from tests import travel_common as T  # noqa: E402

if not T.REAL:
    print("travel_ask_real needs a real model (OMUSE_E2E_REAL=1)")
    sys.exit(1)

QUESTIONS = {
    "flight": "What is today's last flight from Hong Kong to Singapore that arrives before mid-night?",
    "michelin": "I need to book a Michelin 3 star restaurant in Tokyo next Friday. What are my options?",
}


def next_friday() -> dt.date:
    today = dt.date.today()
    return today + dt.timedelta(days=(4 - today.weekday()) % 7 or 7)


def trace(tasks):
    for tid in tasks:
        for e in T.task(tid).get("events", []):
            d = e.get("data") or {}
            if e["type"] == "tool_call":
                print(f"INFO call {d.get('name')} {json.dumps(d.get('args'), ensure_ascii=False)[:160]}")
            elif e["type"] == "tool_result":
                print(f"INFO  -> ok={T.ok(d)} {str(d.get('preview') or d.get('error') or '')[:160]!r}")
            elif e["type"] in ("plan", "waiting", "error", "retry", "choices", "choices_rejected", "parallel_calls"):
                print(f"INFO {e['type']} {json.dumps(d, ensure_ascii=False)[:300]}")


def common(label: str, info: dict, skill: str, data_tools: tuple) -> tuple:
    tasks = info["tasks"]
    final = T.task(tasks[-1])
    calls = T.calls_of(tasks)
    names = [x["name"] for x in calls]
    trace(tasks)
    T.check(f"{label}: the task completed", final.get("status") == "COMPLETED",
            (final.get("status"), final.get("error"), str(final.get("result"))[:300]))
    loads = [x for x in calls if x["name"] == "load_skill"]
    loaded = any(skill in json.dumps(x["args"]) for x in loads)
    used = {n.split("__", 1)[1] for n in names if n.startswith("mcp_travel__")}
    # a plain lookup may skip the booking skill as long as it still goes straight to the data connector
    T.check(f"{label}: the {skill} skill was loaded, or the connector used without it" + ("" if loaded else " (skill not loaded)"),
            loaded or set(data_tools) <= used, [x["args"] for x in loads])
    T.check(f"{label}: the facts came from the travel data connector ({', '.join(data_tools)})", set(data_tools) <= used, names)
    br = [n for n in names if n.startswith("browser_")]
    T.check(f"{label}: the browser was never used", not br, br)
    T.check(f"{label}: no approval was asked", not info["approvals"], info["approvals"])
    waits = [e["data"] for tid in tasks for e in T.task(tid).get("events", []) if e["type"] == "waiting"]
    T.check(f"{label}: the task never waited for the user (no approval dialog, no takeover)", not waits, waits)
    T.timing(f"travel_ask_real/{label}", info, len(calls), len(br))
    return final, names


all_tasks, conv = [], None
T.setup()
try:
    # 1. the flight: the data connector's last same-day arrival is the expected answer
    today = dt.date.today()
    offers = fake_travel.flights_search({"origin": "HKG", "destination": "SIN", "date": str(today), "adults": 1})["offers"]
    same_day = [o for o in offers if "(+" not in o["schedule"]]
    last = max(same_day, key=lambda o: o["schedule"].split(" - ")[1].split()[1])
    print(f"INFO expected: {last['title']} {last['schedule']} (of {len(offers)} offers, {len(same_day)} land the same day)")
    info = T.run(QUESTIONS["flight"], timeout=600)
    all_tasks += info["tasks"]
    final, names = common("flight", info, "flight-booking", ("flights_search",))
    res = str(final.get("result") or "")
    first_no = last["flight_no"].split(" + ")[0]
    T.check(f"flight: the answer names the last same-day arrival ({last['title']}, {last['schedule']})",
            first_no in res or last["title"] in res, res[:800])
    T.check("flight: the answer does not pick a flight that lands after midnight",
            not any(o["flight_no"].split(" + ")[0] in res for o in offers if "(+" in o["schedule"]), res[:800])

    # 2. the restaurants: options only, nothing booked
    rs = fake_travel.michelin_search({"city": "Tokyo", "distinction": "3 Stars"})["restaurants"]
    info = T.run(QUESTIONS["michelin"], timeout=600)
    all_tasks += info["tasks"]
    conv = info["conv_id"]
    final, names = common("michelin", info, "michelin-dining", ("michelin_search",))
    res = str(final.get("result") or "")
    named = [r["name"] for r in rs if r["name"] in res]
    T.check(f"michelin: the answer lists 3-star restaurants from the data ({len(named)} of {len(rs)})", len(named) >= 1,
            ([r["name"] for r in rs], res[:800]))
    T.check("michelin: the options are for next Friday " + str(next_friday()),
            str(next_friday()) in res or next_friday().strftime("%-d %B") in res or next_friday().strftime("%B %-d") in res, res[:800])
    T.check("michelin: nothing was booked", not T.booked(), T.booked())
    os.makedirs(os.path.dirname(T.LAST), exist_ok=True)
    json.dump({"suite": "travel_ask_real", "tasks": all_tasks, "conv_id": conv}, open(T.LAST, "w"))
finally:
    T.finish()
