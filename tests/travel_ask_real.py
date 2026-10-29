"""Two research-only travel questions with a real model (OMUSE_E2E_REAL=1, see travel_common.py): the standard skills
(flight-booking, michelin-dining), the travel data connector for the facts, and — the point of the check — no browser,
no approval and no takeover, so nothing ever opens for the user. One TIMING line per question (model time excluded).

Not in run_in_docker.sh's default list (it needs a real model):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_MODELS=alt tests/run_in_docker.sh travel_ask_real"""
import datetime as dt
import json
import os
import re
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
    "business": "What is the cheapest business class direct flight from Tokyo to Singapore on {date}?",
    "range": "What is the cheapest flight from Singapore to Tokyo on any day between {d1} and {d2}?",
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
    # the connector's mcp_travel__* tools, or the built-in ones with the same names (flights_search is always there)
    used = {n.split("__", 1)[1] for n in names if n.startswith("mcp_travel__")} | {n for n in names if n in data_tools}
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
    T.check("flight: a flight question uses the flight search only — no web_search beside it", "web_search" not in names, names)
    if "mcp_travel__flights_search" in names:
        first_no = last["flight_no"].split(" + ")[0]
        T.check(f"flight: the answer names the last same-day arrival ({last['title']}, {last['schedule']})",
                first_no.replace(" ", "") in res.replace(" ", "") or last["title"] in res, res[:800])
        T.check("flight: the answer does not pick a flight that lands after midnight",
                not any(o["flight_no"].split(" + ")[0].replace(" ", "") in res.replace(" ", "") for o in offers if "(+" in o["schedule"]), res[:800])
    else:
        # the built-in tool's stand-in (tests/fake_apps.py): the CX pair lands the next day and must not be the answer (the model may add
        # web_search facts, so the exact flight is not pinned here)
        # the answer may name the CX pair to explain why it does not qualify; what it picks is the first flight it names
        first = re.search(r"\b([A-Z][A-Z0-9])\s?(\d{2,4})\b", res)
        T.check("flight: the answer does not pick the pair that lands the next day (CX710 / CX500)",
                bool(first) and first.group(1) + first.group(2) not in ("CX710", "CX500"), res[:800])

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

    # 3. business class, direct, from a city with two airports: the request's cabin and stop limit go into ONE search
    #    (built-in flights_search or the connector's), the city as a whole (TYO / Tokyo / HND,NRT), no plan, and the answer is business class
    bday = (dt.date.today() + dt.timedelta(days=30)).isoformat()
    T.c.put(T.B + "/api/profile", json={"key": "preferred_airline", "value": "ANA"}, headers=T.H)
    info = T.run(QUESTIONS["business"].format(date=bday), timeout=600)
    all_tasks += info["tasks"]
    final = T.task(info["tasks"][-1])
    calls = T.calls_of(info["tasks"])
    trace(info["tasks"])
    fs = [x for x in calls if x["name"].endswith("flights_search") and x.get("args")]
    args = fs[0]["args"] if fs else {}
    stops_ok = args.get("stops") == "nonstop" or args.get("max_stops") == 0 or args.get("nonstop_only") is True
    T.check("business: the task completed", final.get("status") == "COMPLETED", (final.get("status"), final.get("error"), str(final.get("result"))[:300]))
    # one search per source (the built-in tool, and a travel connector when one is configured — the skill allows both), never one per
    # airport or a second cabin: every search is business, nonstop, Tokyo as one city
    T.check("business: one flights search per source (no search per airport, no second cabin)", fs and len({x["name"] for x in fs}) == len(fs)
            and all(str(x["args"].get("cabin", "")).lower() == "business" and (x["args"].get("stops") == "nonstop" or x["args"].get("max_stops") == 0 or x["args"].get("nonstop_only") is True)
                    and str(x["args"].get("origin", "")).lower().replace(" ", "") in ("tyo", "tokyo", "hnd,nrt", "nrt,hnd") for x in fs), [(x["name"], x["args"]) for x in fs])
    T.check("business: the search asked for business class", str(args.get("cabin", "")).lower() == "business", args)
    T.check("business: the search asked for nonstop (max_stops 0 / nonstop_only)", stops_ok, args)
    T.check("business: Tokyo searched as one city (TYO, Tokyo or HND,NRT), on the date asked", str(args.get("origin", "")).lower().replace(" ", "") in ("tyo", "tokyo", "hnd,nrt", "nrt,hnd")
            and args.get("date") == bday, args)
    T.check("business: no plan was made for a one-search question", not any(e["type"] == "plan" for e in final.get("events", [])), [e["type"] for e in final.get("events", [])])
    res = str(final.get("result") or "")
    # a business itinerary may carry a short economy connection (cabin_mix) and the answer then names that leg; what must not
    # happen is an economy fare offered as the answer
    T.check("business: the answer is about business class and never offers an economy fare instead", "business" in res.lower()
            and not re.search(r"economy[- ](class )?(fare|ticket|option|seat)s?\b|cheapest economy|in economy for", res, re.I), res[:600])
    # 4. a range of dates: one flights search with date + date_to, no plan, the answer names the cheapest day
    r1 = (dt.date.today() + dt.timedelta(days=40)).isoformat(); r2 = (dt.date.today() + dt.timedelta(days=44)).isoformat()
    rinfo = T.run(QUESTIONS["range"].format(d1=r1, d2=r2), timeout=600)
    all_tasks += rinfo["tasks"]
    rfinal = T.task(rinfo["tasks"][-1])
    rcalls = T.calls_of(rinfo["tasks"])
    trace(rinfo["tasks"])
    # a call whose arguments did not parse (the model once wrote max_price as 0.000… until it was cut off) was refused
    # and retried: only the calls that ran count
    rfs = [x for x in rcalls if x["name"] == "flights_search" and "_unparsed" not in x["args"]]
    T.check("range: the task completed", rfinal.get("status") == "COMPLETED", (rfinal.get("status"), rfinal.get("error")))
    # never one search per day: every flights_search covers the whole range (a model may repeat the same call to double-check)
    T.check("range: the built-in flights_search covers the whole range (date + date_to) in each call, never one call per day", bool(rfs)
            and all(x["args"].get("date") == r1 and x["args"].get("date_to") == r2 for x in rfs), [x["args"] for x in rfs])
    T.check("range: no stop limit invented (the question said nothing about stops)", bool(rfs) and str(rfs[0]["args"].get("stops", "any")) in ("any", "")
            and rfs[0]["args"].get("max_stops") is None and not rfs[0]["args"].get("nonstop_only"), [x["args"] for x in rfs])
    T.check("range: no plan", not any(e["type"] == "plan" for e in rfinal.get("events", [])), [e["type"] for e in rfinal.get("events", [])][:12])
    # ISO, "8 Dec" or "Dec 8" (also the start of a span such as "Dec 8–12")
    T.check("range: the answer names a date in the range", any(str(dt.date.fromisoformat(r1) + dt.timedelta(days=i)) in str(rfinal.get("result"))
            or any((dt.date.fromisoformat(r1) + dt.timedelta(days=i)).strftime(f) in str(rfinal.get("result")) for f in ("%-d %b", "%b %-d"))
            for i in range(5)), str(rfinal.get("result"))[:400])
    T.check("business: no web_search beside the flight search", not [x for x in calls if x["name"] == "web_search"], [x["name"] for x in calls])
    T.check("business: the browser was never used", not [x for x in calls if x["name"].startswith("browser_")], [x["name"] for x in calls])
    pref = str(args.get("airlines", "")).lower()
    T.check("business: the profile's preferred airline (ANA) went into the search, or the answer says ANA has no such offer",
            "ana" in pref or "nh" in pref.split() or "ana" in res.lower(), (args.get("airlines"), res[:400]))
    os.makedirs(os.path.dirname(T.LAST), exist_ok=True)
    json.dump({"suite": "travel_ask_real", "tasks": all_tasks, "conv_id": conv}, open(T.LAST, "w"))
finally:
    T.finish()
