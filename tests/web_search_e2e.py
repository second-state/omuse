"""Web search: the one web_search tool (always in the catalog) goes through the search services — Exa's keyless MCP first,
then the keys on hand in the order Tavily, Brave, Exa (the SerpApi key is for flights, hotels and restaurants only); the first that answers is used and remembered, so
it goes first next time; when it fails the next one answers (the result says so); with the connection off the browser's
own search engines answer. Keys are per provider: the test stack ships a server default for Brave (tests/run_local.sh),
the user's own key overrides it and removing the own key falls back to it; a refused key is reported at save time; one
key can be removed alone. Results keep the browser's shape (title, url, snippet) marked untrusted with the engine in the
source, no provider is ever a tool of its own, and the browser's search page is never opened while a service answers."""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
FAKE = "http://127.0.0.1:8094"
KEYS = {"brave": "brave-test-key-0123456789", "tavily": "tvly-test-key-0123456789", "exa": "exa-test-key-0123456789"}


def act(tool, args, task="ws-t1"):
    return c.post(B + "/internal/act", headers=RT, json={"task_id": task, "call_id": "c" + str(time.time()), "tool": tool, "args": args}).json()


def conn():
    return next(x for x in c.get(B + "/sentinel/api/connections", headers=H).json()["connections"] if x["name"] == "search")


def sources():
    return [(p["id"], p["source"]) for p in conn()["providers"]]


def engine(r):
    src = (r.get("result") or {}).get("source", "")
    return src.split("(", 1)[1].rsplit("):", 1)[0] if "(" in src else ""


def add(provider, token=None):
    return c.post(B + "/sentinel/api/connections/search/credential", headers=H, json={"provider": provider, "token": token or KEYS[provider]})


def rm(provider):
    return c.delete(B + f"/sentinel/api/connections/search/credential/{provider}", headers=H)


def on(flag):
    c.put(B + "/sentinel/api/connections/search", headers=H, json={"enabled": flag})


def down(which, flag):
    c.post(FAKE + f"/{which}/_down", json={"on": flag})


def ddg_hits():
    return c.get(FAKE + "/ddg/_hits").json()["n"]


def cat_names():
    return [t["function"]["name"] for t in c.get(B + "/internal/catalog", headers=RT).json()["tools"]]


K.setup(language="en")
c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
c.post(FAKE + "/search/_reset"); c.post(FAKE + "/serpapi/_reset")
down("tavily", False); down("exa-mcp", False); on(True)

# fresh: the free Exa MCP first, the server's default Brave key beside it
names = cat_names()
K.check("web_search is the one search tool in the catalog", "web_search" in names and "browser_search" not in names, [n for n in names if "search" in n])
K.check("no provider is ever a tool of its own", not [n for n in names if any(p in n.lower() for p in ("tavily", "brave", "exa", "serpapi", "kiwi"))], names)
cv = conn()
K.check("the connection starts on with the free Exa MCP and the server's default Brave key, in try order",
        cv["enabled"] and cv["config"]["providers"] == ["exa-mcp", "brave"] and cv["default_providers"] == ["brave"]
        and sources() == [("exa-mcp", "free"), ("tavily", ""), ("brave", "server"), ("exa", "")], (cv["config"], sources()))
r = act("web_search", {"query": "omuse test free", "max_results": 3})
res = r.get("result") or {}
K.check("web_search answers from the free Exa MCP, no key: same shape, untrusted, engine in the source", r.get("status") == "ok" and engine(r) == "exa (free)" and len(res["results"]) == 3
        and res["results"][0]["title"] == "Exa free result 1 for omuse test free" and res["results"][0]["snippet"].startswith("Exa free highlight 1") and res.get("trust") == "untrusted", str(r)[:400])
K.check("the free Exa MCP is remembered", conn()["config"]["last_ok"] == "exa-mcp", conn()["config"])
hits0 = ddg_hits()

# the free one down: the server's default Brave key answers, and the note says what was skipped
down("exa-mcp", True)
r = act("web_search", {"query": "omuse test one", "max_results": 3})
res = r.get("result") or {}
K.check("the free one down: the server's default Brave key answers, the note says so, the rows are intact", r.get("status") == "ok" and engine(r) == "brave"
        and res["results"][0]["title"] == "Brave result 1 for omuse test one" and res["results"][0]["age"] == "3 days ago" and "rate limit" in res.get("note", "") and "next one answered" in res.get("note", ""), str(r)[:400])
K.check("the server default is remembered like any other", conn()["config"]["last_ok"] == "brave", conn()["config"])
K.check("the browser's search page was not opened", ddg_hits() == hits0)
down("exa-mcp", False)
r = act("web_search", {"query": "omuse test two", "max_results": 2})
K.check("Brave stays first after the free one recovers (the same working one next time)", engine(r) == "brave", str(r)[:200])

# the connection off: only the browser's engines, not even the server default
on(False)
r = act("web_search", {"query": "omuse test off", "max_results": 3})
K.check("switched off: the browser's engines answer, and web_search is still the one tool", r.get("status") == "ok" and engine(r) == "duckduckgo" and len(r["result"]["results"]) == 3
        and "web_search" in cat_names(), str(r)[:300])
on(True)

# keys: refused / unknown / the free one takes none
r = add("brave", "wrong-brave-key-000000")
K.check("a refused key is reported when it is saved", r.status_code == 400 and "refused" in r.text, r.text[:200])
r = add("bing", KEYS["brave"])
K.check("an unknown provider is rejected", r.status_code == 400 and "provider" in r.text, r.text[:200])
r = add("exa-mcp", KEYS["brave"])
K.check("the free Exa MCP takes no key", r.status_code == 400, r.text[:200])

# the own Brave key overrides the server default; removing it falls back
r = add("brave")
K.check("the own Brave key is saved and overrides the server default (same provider list, the source changes)", r.status_code == 200 and r.json()["providers"] == ["exa-mcp", "brave"]
        and dict(sources())["brave"] == "user", (r.text[:200], sources()))
r = rm("brave")
K.check("removing the own key falls back to the server's default Brave key", r.status_code == 200 and r.json()["providers"] == ["exa-mcp", "brave"] and dict(sources())["brave"] == "server", (r.text[:200], sources()))
r = act("web_search", {"query": "omuse test three", "max_results": 2})
K.check("Brave keeps answering with the server's key", engine(r) == "brave", str(r)[:200])

# Tavily added: the remembered Brave still first; the view and Test all show every source
r = add("tavily")
K.check("Tavily's key is saved beside the default, in try order", r.status_code == 200 and r.json()["providers"] == ["exa-mcp", "tavily", "brave"], r.text[:200])
K.check("the connection view lists every provider in try order with its key source", sources() == [("exa-mcp", "free"), ("tavily", "user"), ("brave", "server"), ("exa", "")], sources())
r = c.post(B + "/sentinel/api/connections/search/test", headers=H)
K.check("Test all runs every provider, the remembered one first, then the order, each with its key source", r.status_code == 200
        and [(x["provider"], x["source"], x["ok"]) for x in r.json()["results"]] == [("brave", "server", True), ("exa-mcp", "free", True), ("tavily", "user", True)], r.text[:400])
r = act("web_search", {"query": "omuse test four", "max_results": 2})
K.check("Brave (remembered) still answers although Tavily comes earlier in the order", engine(r) == "brave", str(r)[:200])

# the order when nothing is remembered: off and on again clears nothing, so take the free one out and the memory with a fresh provider
r = rm("tavily")
K.check("one provider's key can be removed alone", r.status_code == 200 and r.json()["providers"] == ["exa-mcp", "brave"], r.text[:200])

# Exa (keyed) in its turn: Test all exercises it while the earlier ones are down; a SerpApi key is no web-search provider
add("exa"); add("tavily")
r = add("serpapi", "serp-test-key-0123456789abcdef")
K.check("SerpApi is not a web-search provider (its key is for flights, hotels and restaurants)", r.status_code == 400, r.text[:200])
down("exa-mcp", True); down("tavily", True)
r = c.post(B + "/sentinel/api/connections/search/test", headers=H)
got = {x["provider"]: x["ok"] for x in r.json()["results"]}
K.check("Test all: the free Exa MCP and Tavily down, Brave (server) and Exa working", got == {"brave": True, "exa-mcp": False, "tavily": False, "exa": True}, got)
sq = [q for q in c.get(FAKE + "/serpapi/_log").json() if q.get("engine") == "google"]
K.check("SerpApi was never asked for web results", not sq, sq[-1:])
down("exa-mcp", False); down("tavily", False)

# remove all own keys: the free Exa MCP and the server default remain
r = c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
K.check("remove all own keys: the free Exa MCP and the server default remain", r.status_code == 200 and not conn()["has_credential"] and conn()["config"]["providers"] == ["exa-mcp", "brave"], (r.text[:100], conn()["config"]))
K.finish()
