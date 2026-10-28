"""How fast is each web search source? The search APIs (Brave, Tavily — called directly with the keys in the e2e env
file), Exa's free MCP (through Sentinel's web_search, the head of the chain) and the browser's own engines (through
web_search with the search APIs switched off, real DuckDuckGo / Bing): three travel-style queries each, wall seconds
per search, and a median. Informational: it prints TIMING lines and fails only when a source returns nothing at all.

Not in run_in_docker.sh's default list (real network, a few searches billed against the free tiers):
    OMUSE_E2E_SEARCH_ENGINES= OMUSE_E2E_EXA_MCP_EP=https://mcp.exa.ai/mcp OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env \\
    bash tests/run_in_docker.sh search_timing_real"""
import json
import os
import statistics
import sys
import time

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

if os.environ.get("OMUSE_E2E_REAL") != "1":
    print("search_timing_real needs the network (OMUSE_E2E_REAL=1)")
    raise SystemExit(0)
B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
QUERIES = ["cheapest flights Singapore to Tokyo December", "4 star hotels near Ginza Tokyo", "best ramen restaurants Shinjuku"]
# SEARCH_TIMING_BURST=n: only the browser engines, n different queries back to back (does it stay fast and unblocked?)
BURST = int(os.environ.get("SEARCH_TIMING_BURST", "0") or 0)
if BURST:
    places = ["Tokyo", "Osaka", "Seoul", "Bangkok", "Taipei", "Hong Kong", "Hanoi", "Bali", "Sydney", "London", "Paris", "Rome", "Madrid", "Berlin", "Vienna", "Prague"]
    QUERIES = [f"things to do in {places[i % len(places)]} {2026 + i // len(places)}" for i in range(BURST)]
h = httpx.Client(timeout=60, trust_env=False)
times = {}


def timed(name, fn):
    out = []
    for q in QUERIES:
        t0 = time.time()
        try:
            n = fn(q)
        except Exception as e:   # noqa: BLE001 — a failed search is reported, not fatal
            n = -1
            print(f"INFO {name} error: {str(e)[:120]}")
        dt = time.time() - t0
        out.append((dt, n))
        print(f"INFO {name:16} {dt:6.2f} s  {n} results  {q}")
        if name == "brave":
            time.sleep(1.2)   # the free tier allows one request a second
    times[name] = out


def brave(q):
    r = h.get("https://api.search.brave.com/res/v1/web/search", params={"q": q, "count": 8},
              headers={"X-Subscription-Token": os.environ["OMUSE_E2E_BRAVE_KEY"], "Accept": "application/json"})
    r.raise_for_status()
    return len(r.json().get("web", {}).get("results", []))


def tavily(q):
    r = h.post("https://api.tavily.com/search", json={"api_key": os.environ["OMUSE_E2E_TAVILY_KEY"], "query": q, "max_results": 8})
    r.raise_for_status()
    return len(r.json().get("results", []))


def sentinel_web(q):
    r = c.post(B + "/internal/act", headers=RT, json={"task_id": "st-1", "call_id": "c" + str(time.time()), "tool": "web_search",
                                                       "args": {"query": q, "max_results": 8}}, timeout=120).json()
    res = r.get("result") or {}
    if r.get("status") != "ok":
        print(f"INFO   error: {str(r.get('error') or r)[:160]}")
    print(f"INFO   source: {str(res.get('source', ''))[:60]}  note: {str(res.get('note', ''))[:80]}")
    return len(res.get("results") or [])


K.setup(language="en")
if not BURST:
    if os.environ.get("OMUSE_E2E_BRAVE_KEY"):
        timed("brave", brave)
    if os.environ.get("OMUSE_E2E_TAVILY_KEY"):
        timed("tavily", tavily)
    # the head of the real chain (Exa's free MCP when OMUSE_E2E_EXA_MCP_EP points at it), through Sentinel
    timed("web_search (api)", sentinel_web)
# the browser's own engines: the search connection off, so web_search goes to the browser
c.put(B + "/sentinel/api/connections/search", json={"enabled": False}, headers=H)
timed("browser engines", sentinel_web)
c.put(B + "/sentinel/api/connections/search", json={"enabled": True}, headers=H)
for name, rows in times.items():
    ok = [d for d, n in rows if n > 0]
    med = statistics.median(ok) if ok else None
    print("TIMING " + json.dumps({"suite": "search_timing_real", "source": name, "median_s": round(med, 2) if med else None,
                                  "runs_s": [round(d, 2) for d, _ in rows], "results": [n for _, n in rows]}))
    K.check(f"{name}: returned results", bool(ok), rows)
K.finish()
