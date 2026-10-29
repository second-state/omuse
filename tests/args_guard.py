"""Run after EVERY e2e suite (tests/run_in_docker.sh): no search may run with a number that makes no sense.

A model filling in defaults sends `max_price: 0` ("nothing is affordable") or `adults: 0`; Sentinel and the runtime
replace them (omuse_common::util::sanitize_search_args: a price cap ≤ 0 → no limit, fewer than 1 traveller/guest → 1).
This guard checks what really ran, whatever the suite did:
  * every tool call the runtime recorded (events of type tool_call in runtime.db, sub-agents included);
  * every request the fake providers received: Kiwi.com MCP (price_to, adults), SerpApi (max_price, adults), the fake
    travel connector (its tool calls' arguments).
It also checks the bots (crate::bots): a load_skill refused because the skill is another bot's means the suite talked to
the wrong bot (set K.BOT / travel_common.BOT) — only bots_e2e's BOTSKILL case asks for that refusal on purpose.
Exit 1 with one GUARD line per violation; prints "ARGS GUARD OK" otherwise. Missing services or an empty database are
fine (a suite that never started the stack)."""
import json
import os
import sqlite3
import sys
import urllib.request

DB = "/tmp/claude-0/persona-test/data/runtime.db"
PRICE = {"max_price", "max_price_per_night", "price_to", "price_max", "max_budget", "budget", "max_total"}
PEOPLE = {"adults", "guests", "travelers", "travellers", "passengers", "party_size", "people", "pax", "rooms"}
ZERO_OK = {"children", "infants", "kids"}


def num(v):
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return float(v)
    try:
        return float(str(v).strip())
    except ValueError:
        return None


def bad_values(obj, where, out):
    if isinstance(obj, dict):
        for k, v in obj.items():
            n = num(v)
            if k in PRICE and (v is None or isinstance(v, bool) or (isinstance(v, str) and not v.strip()) or (n is not None and n <= 0)):
                out.append(f"{where}: {k}={v!r} (a price cap must be a positive number or absent = unlimited)")
            elif k in PEOPLE and (n is None or n < 1):
                out.append(f"{where}: {k}={v!r} (at least 1)")
            elif k in ZERO_OK and (n is None or n < 0):
                out.append(f"{where}: {k}={v!r} (never negative)")
            elif isinstance(v, (dict, list)):
                bad_values(v, where, out)
    elif isinstance(obj, list):
        for v in obj:
            bad_values(v, where, out)


def get(url):
    try:
        with urllib.request.urlopen(url, timeout=5) as r:
            return json.loads(r.read().decode())
    except Exception:
        return None


def main():
    out = []
    if os.path.exists(DB):
        con = sqlite3.connect(DB)
        for tid, data in con.execute("SELECT task_id, data FROM events WHERE type='tool_call'"):
            try:
                d = json.loads(data or "{}")
            except ValueError:
                continue
            bad_values(d.get("args") or {}, f"tool_call {d.get('name')} in {tid}", out)
        goals = dict(con.execute("SELECT id, goal FROM tasks"))
        for tid, data in con.execute("SELECT task_id, data FROM events WHERE type='tool_result'"):
            try:
                d = json.loads(data or "{}")
            except ValueError:
                continue
            pv = str(d.get("preview") or "")
            if d.get("name") == "load_skill" and ("belongs to the bot(s)" in pv or "is not one of the" in pv) \
                    and "BOTSKILL" not in (goals.get(tid) or ""):
                out.append(f"load_skill refused in {tid} (wrong bot for this suite): {pv[:160]}")
    for i, args in enumerate(get("http://127.0.0.1:8094/kiwi-mcp/_log") or []):
        bad_values(args, f"Kiwi.com request #{i}", out)
    for i, q in enumerate(get("http://127.0.0.1:8094/serpapi/_log") or []):
        if q.get("engine") in ("google_flights", "google_hotels"):
            bad_values(q, f"SerpApi {q.get('engine')} request #{i}", out)
    for c in (get("http://127.0.0.1:8097/_log") or {}).get("calls", []):
        bad_values(c.get("args") or {}, f"travel connector {c.get('name')}", out)
    for line in out:
        print("GUARD", line)
    print("ARGS GUARD OK" if not out else f"ARGS GUARD FAILED: {len(out)}")
    return 1 if out else 0


if __name__ == "__main__":
    sys.exit(main())
