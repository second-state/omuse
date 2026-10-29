"""Web research with a real model (see tests/real_common.py): a plain research question sent to the main bot, which
keeps it (research is general work). web_search goes to the test stack's stand-in engines (tests/fake_apps.py: the
browser's engines → /ddg, "Result N for <query>" pointing at http://shop.test:8094/slow?…&name=PageN; the Brave API
stand-in behind them), so the content is placeholder text: the checks are that the model searched (web_search, the
stand-in engine saw the queries) and kept to a sane number of searches and calls. The stack has network, and a model that
sees only placeholder results may read well-known pages itself (browser_read): the answer must cite what the agent
actually found — a link from a search result or from a page it read — never a source it did not open. The browser only
reads (no click, typing or form). One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_PLANNER=xai/grok-4.7 OMUSE_E2E_EXECUTOR=openai-codex/gpt-6-luna \\
        tests/run_in_docker.sh research_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import real_common as R  # noqa: E402

FAKE = "http://127.0.0.1:8094"
MAX_SEARCHES, MAX_CALLS = 8, 20
URL = re.compile(r"https?://[^\s)\]>\"'`,]+")
READS = ("browser_read", "browser_navigate", "browser_snapshot", "browser_get_text", "browser_screenshot")


def norm(u: str) -> str:
    return u.rstrip(".;:/").lower()


def results(info: dict, name: str) -> list[dict]:
    return [e["data"] for x in info["tasks"] for e in R.task(x).get("events", []) if e["type"] == "tool_result"
            and e["data"].get("name") == name]


R.setup()
hits0 = R.c.get(FAKE + "/ddg/_hits").json()["n"]
apis0 = len(R.c.get(FAKE + "/search/_log").json())
info = R.ask("Research the pros and cons of heat pumps versus gas boilers for heating a small house in the UK. "
             "Search the web and give me a short summary with links to your sources.")
R.routed("heatpump", info, "main")
R.finished("heatpump", info)
res = info["result"]
searches = [x for x in info["calls"] if x["name"] in ("web_search", "news_search")]
engine = R.c.get(FAKE + "/ddg/_hits").json()["n"] - hits0 + len(R.c.get(FAKE + "/search/_log").json()) - apis0
print(f"INFO searches: {[(x.get('args') or {}).get('query') for x in searches]}; stand-in engines hit {engine} times")
R.check("heatpump: the model searched the web (web_search)", R.used(info, "web_search"), info["names"])
R.check("heatpump: the searches reached the stand-in engines", engine >= 1, engine)
R.check(f"heatpump: a sane number of searches ({len(searches)} <= {MAX_SEARCHES})", 1 <= len(searches) <= MAX_SEARCHES,
        [(x.get("args") or {}).get("query") for x in searches])
R.check(f"heatpump: a sane number of tool calls ({len(info['names'])} <= {MAX_CALLS})", len(info["names"]) <= MAX_CALLS, info["names"])
found = set()   # every URL the agent got from a search or read: result links, the pages browser_read was given and returned
for r in results(info, "web_search") + results(info, "news_search") + results(info, "browser_read"):
    found |= {norm(u) for u in URL.findall(str(r.get("preview") or ""))}
for x in info["calls"]:
    if x["name"] in ("browser_read", "browser_navigate"):
        a = x.get("args") or {}
        found |= {norm(u) for u in list(a.get("urls") or []) + [a.get("url") or ""] if u}
cited = {norm(u) for u in URL.findall(res)}
print(f"INFO cited {sorted(cited)}")
R.check("heatpump: the answer cites its sources, and every one was found or read by the agent",
        bool(cited) and cited <= found, (sorted(cited - found), sorted(found)[:20]))
R.check("heatpump: the answer covers the question (heat pumps and gas boilers)",
        R.mentions(res, "heat pump") and R.mentions(res, "boiler"), res[:600])
acts = [n for n in info["names"] if n.startswith("browser_") and n not in READS]
R.check("heatpump: the browser (if used at all) only read pages", not acts, acts)
R.timing("heatpump", info)
R.finish()
