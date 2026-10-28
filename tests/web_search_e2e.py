"""Web search: the one web_search tool (always in the catalog) asks the browser's own search engines first (free, as fast as
the APIs); when they fail, meet a challenge page or find nothing, the search APIs answer in the order Brave → Exa's keyless
MCP → Exa (the one that worked last goes first among them; Tavily is not in this chain and the SerpApi key never is);
the result's note says what was skipped. news_search asks Tavily's news topic when its key is set, else web_search.
Keys are per provider: the test stack ships a server default for Brave (tests/run_local.sh), the user's own key overrides
it and removing the own key falls back to it; a refused key is reported at save time; one key can be removed alone; Test
all includes the Tavily (news) key. Results keep the browser's shape (title, url, snippet) marked untrusted with the
engine in the source, and no provider is ever a tool of its own."""
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
for w in ("tavily", "exa-mcp", "brave", "ddg"):
    down(w, False)
on(True)


def api_log():
    return c.get(FAKE + "/search/_log").json()


names = cat_names()
K.check("web_search and news_search are the search tools in the catalog", "web_search" in names and "news_search" in names and "browser_search" not in names, [n for n in names if "search" in n])
K.check("no provider is ever a tool of its own", not [n for n in names if any(p in n.lower() for p in ("tavily", "brave", "exa", "serpapi", "kiwi"))], names)
cv = conn()
K.check("the connection starts on; the card lists the web chain in order (Brave, the free Exa MCP, Exa), then Tavily for news",
        cv["enabled"] and sources() == [("brave", "server"), ("exa-mcp", "free"), ("exa", ""), ("tavily", "")]
        and [p["role"] for p in cv["providers"]] == ["web", "web", "web", "news"], (cv["config"], sources()))

# fresh: the browser's engines answer first; no search API is asked
n_api, hits0 = len(api_log()), ddg_hits()
r = act("web_search", {"query": "omuse test first", "max_results": 3})
res = r.get("result") or {}
K.check("web_search answers from the browser's engines first: same shape, untrusted, engine in the source, no note", r.get("status") == "ok" and engine(r) == "duckduckgo"
        and len(res["results"]) >= 3 and res["results"][0]["title"].startswith("Result 1 for omuse test first") and res.get("trust") == "untrusted" and not res.get("note"), str(r)[:400])
K.check("…and no search API was asked", len(api_log()) == n_api and ddg_hits() == hits0 + 1, (api_log()[n_api:], ddg_hits() - hits0))

# the browser's engines meet a challenge page: Brave (the server's default key) answers, the note says why
down("ddg", True)
r = act("web_search", {"query": "omuse test one", "max_results": 3})
res = r.get("result") or {}
K.check("browser engines blocked: Brave answers, the note says the browser's engines found nothing", r.get("status") == "ok" and engine(r) == "brave"
        and res["results"][0]["title"] == "Brave result 1 for omuse test one" and "browser's engines" in res.get("note", ""), str(r)[:400])
K.check("Brave is remembered among the APIs", conn()["config"]["last_ok"] == "brave", conn()["config"])
# Brave down too: the free Exa MCP answers
down("brave", True)
r = act("web_search", {"query": "omuse test two", "max_results": 3})
res = r.get("result") or {}
K.check("Brave down too: the free Exa MCP answers and the note says Brave failed", r.get("status") == "ok" and engine(r) == "exa (free)"
        and "Brave" in res.get("note", "") and "next one answered" in res.get("note", ""), str(r)[:400])
K.check("the free Exa MCP is remembered", conn()["config"]["last_ok"] == "exa-mcp", conn()["config"])
down("brave", False)
r = act("web_search", {"query": "omuse test three", "max_results": 2})
K.check("among the APIs the one that worked last goes first (Exa MCP before Brave now)", engine(r) == "exa (free)", str(r)[:200])
# everything down: an error that names the browser and each API
down("exa-mcp", True); down("brave", True)
r = act("web_search", {"query": "omuse test none", "max_results": 2})
K.check("browser and every API down: an error that names them", r.get("status") != "ok" and "browser" in str(r) and "Brave" in str(r), str(r)[:300])
down("exa-mcp", False); down("brave", False); down("ddg", False)

# the connection off: only the browser's engines
on(False)
r = act("web_search", {"query": "omuse test off", "max_results": 3})
K.check("switched off: the browser's engines answer, and web_search is still the one web tool", r.get("status") == "ok" and engine(r) == "duckduckgo" and "web_search" in cat_names(), str(r)[:300])
down("ddg", True)
n_api = len(api_log())
r = act("web_search", {"query": "omuse test off blocked", "max_results": 3})
K.check("switched off and the browser blocked: no API is asked, the empty result says why", r.get("status") == "ok" and not (r.get("result") or {}).get("results")
        and "browser" in str((r.get("result") or {}).get("note", "")) and len(api_log()) == n_api, str(r)[:300])
down("ddg", False)
on(True)

# keys: refused / unknown / the free one takes none / SerpApi is no web provider
r = add("brave", "wrong-brave-key-000000")
K.check("a refused key is reported when it is saved", r.status_code == 400 and "refused" in r.text, r.text[:200])
r = add("bing", KEYS["brave"])
K.check("an unknown provider is rejected", r.status_code == 400 and "provider" in r.text, r.text[:200])
r = add("exa-mcp", KEYS["brave"])
K.check("the free Exa MCP takes no key", r.status_code == 400, r.text[:200])
r = add("serpapi", "serp-test-key-0123456789abcdef")
K.check("SerpApi is not a web-search provider (its key is for flights, hotels and restaurants)", r.status_code == 400, r.text[:200])

# the own Brave key overrides the server default; removing it falls back
r = add("brave")
K.check("the own Brave key is saved and overrides the server default", r.status_code == 200 and dict(sources())["brave"] == "user", (r.text[:200], sources()))
r = rm("brave")
K.check("removing the own key falls back to the server's default Brave key", r.status_code == 200 and dict(sources())["brave"] == "server", (r.text[:200], sources()))

# news: without a Tavily key, news_search is a web search for "<query> news <period>"
r = act("news_search", {"query": "OMuse release", "days": 3})
res = r.get("result") or {}
K.check("news_search without a Tavily key: web search for the news of the period, with the reason", r.get("status") == "ok" and engine(r) == "duckduckgo"
        and "OMuse release news last 3 days" in res.get("source", "") and "no Tavily key" in res.get("news_fallback", ""), str(r)[:400])
# with the Tavily key: Tavily's news topic, the days passed on, and Tavily stays out of web_search
r = add("tavily")
K.check("Tavily's key is saved (listed last, as the news source)", r.status_code == 200 and dict(sources())["tavily"] == "user", r.text[:200])
n_api = len(api_log())
r = act("news_search", {"query": "OMuse release", "days": 3, "max_results": 4})
res = r.get("result") or {}
tl = [x for x in api_log()[n_api:] if x["provider"] == "tavily"]
K.check("news_search with the key: Tavily's news topic for the last 3 days, dated rows, no fallback note", r.get("status") == "ok" and engine(r) == "tavily news"
        and tl and tl[-1]["topic"] == "news" and tl[-1]["days"] == 3 and len(res["results"]) == 4 and "news_fallback" not in res, (str(r)[:300], tl))
down("ddg", True)
n_api = len(api_log())
r = act("web_search", {"query": "omuse test no tavily", "max_results": 2})
K.check("web_search never asks Tavily, even with its key and the browser blocked", engine(r) != "tavily" and not [x for x in api_log()[n_api:] if x["provider"] == "tavily"], api_log()[n_api:])
down("ddg", False)
down("tavily", True)
r = act("news_search", {"query": "OMuse launch", "days": 1})
res = r.get("result") or {}
K.check("Tavily down: news_search falls back to web search for today's news and says why", r.get("status") == "ok" and "OMuse launch news today" in res.get("source", "")
        and "Tavily failed" in res.get("news_fallback", ""), str(r)[:300])
down("tavily", False)
r = c.post(B + "/sentinel/api/connections/search/test", headers=H)
got = {x["provider"]: x["ok"] for x in r.json().get("results", [])}
K.check("Test all covers every key, the Tavily (news) one included", got.get("brave") and got.get("exa-mcp") and got.get("tavily"), r.text[:300])
r = rm("tavily")
K.check("one provider's key can be removed alone", r.status_code == 200 and dict(sources())["tavily"] == "", sources())
sq = [q for q in c.get(FAKE + "/serpapi/_log").json() if q.get("engine") == "google"]
K.check("SerpApi was never asked for web results", not sq, sq[-1:])

# remove all own keys: the free Exa MCP and the server default remain
add("exa")
r = c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
K.check("remove all own keys: the free Exa MCP and the server default remain", r.status_code == 200 and not conn()["has_credential"]
        and dict(sources()) == {"brave": "server", "exa-mcp": "free", "exa": "", "tavily": ""}, (r.text[:100], sources()))
K.finish()
