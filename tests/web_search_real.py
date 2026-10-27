"""Web search through the real Brave Search and Tavily APIs (OMUSE_E2E_REAL=1; the keys OMUSE_E2E_BRAVE_KEY /
OMUSE_E2E_TAVILY_KEY in the e2e env file, the real endpoints via OMUSE_E2E_BRAVE_EP / OMUSE_E2E_TAVILY_EP): each
provider connects with one small search, web_search answers from it with real URLs and snippets — a plain query and
a flight-fare query — and the browser's search page is never opened. A few searches are billed against the free tiers.

Not in run_in_docker.sh's default list:
    OMUSE_E2E_BRAVE_EP=https://api.search.brave.com/res/v1 OMUSE_E2E_TAVILY_EP=https://api.tavily.com \\
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh web_search_real"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

if os.environ.get("OMUSE_E2E_REAL") != "1":
    print("web_search_real needs the real APIs (OMUSE_E2E_REAL=1 and the keys in the e2e env file)")
    raise SystemExit(0)
B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
FAKE = "http://127.0.0.1:8094"
KEYS = {"brave": os.environ.get("OMUSE_E2E_BRAVE_KEY", ""), "tavily": os.environ.get("OMUSE_E2E_TAVILY_KEY", "")}


def act(tool, args, task="wsr-t1"):
    return c.post(B + "/internal/act", headers=RT, json={"task_id": task, "call_id": "c" + str(time.time()), "tool": tool, "args": args}).json()


K.setup(language="en")
hits0 = c.get(FAKE + "/ddg/_hits").json()["n"]
# the free Exa MCP is the first choice: one search through it first (real, keyless)
r = act("web_search", {"query": "OMuse personal AI agent second state github", "max_results": 5})
res = r.get("result") or {}
rows = res.get("results") or []
K.check("the free Exa MCP answers web_search with real results, no key", r.get("status") == "ok" and res.get("source", "").startswith("web search (exa (free)") and len(rows) >= 2
        and all(x["url"].startswith("http") for x in rows), str(r)[:300])
print("INFO exa-mcp: " + " | ".join(f"{x['title'][:50]} <{x['url'][:40]}>" for x in rows[:3]))
for provider, key in KEYS.items():
    if not key:
        print(f"INFO {provider}: no key in the env file, skipped")
        continue
    r = c.post(B + "/sentinel/api/connections/search/credential", headers=H, json={"provider": provider, "token": key})
    K.check(f"{provider}: the real key connects with one small search", r.status_code == 200 and r.json()["provider"] == provider and r.json()["count"] >= 1, r.text[:200])
    for q in ("OMuse personal AI agent second state github", "SIN to NRT flights 20 Nov 2026 cheapest fare"):
        time.sleep(1.2)   # Brave's free plan: one request a second (the client also retries a 429 once)
        t0 = time.time()
        r = act("web_search", {"query": q, "max_results": 5})
        res = r.get("result") or {}
        rows = res.get("results") or []
        K.check(f"{provider}: web_search answers from a real API — {q[:28]}…", r.get("status") == "ok" and (res.get("source", "").startswith(f"web search ({provider}") or res.get("source", "").startswith("web search (exa (free)"))
                and len(rows) >= 3 and all(x["url"].startswith("http") and x["title"] for x in rows), str(r)[:300])
        print(f"INFO {provider} {time.time() - t0:.1f} s: " + " | ".join(f"{x['title'][:50]} <{x['url'][:40]}>" for x in rows[:3]))
        if rows and "flights" in q:
            print(f"INFO {provider} fare snippet: {rows[0].get('snippet', '')[:160]!r}")
    time.sleep(1.2)
    time.sleep(1.2)
    r = c.post(B + "/sentinel/api/connections/search/test", headers=H)
    K.check(f"{provider}: Test all reports it working", r.status_code == 200 and any(x["provider"] == provider and x["ok"] for x in r.json()["results"]), r.text[:300])
    c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
K.check("the browser's search page was never opened", c.get(FAKE + "/ddg/_hits").json()["n"] == hits0)
K.finish()
