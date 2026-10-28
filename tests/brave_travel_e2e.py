"""Brave key, no Flights & hotels key, and the browser's own engines blocked (a challenge page): the agent's instructions
name the web search services and the Kiwi rule, and a flight + hotel + car request is searched with flights_search
(Kiwi.com data, information only) and three web_search calls in one turn — the browser's engines are asked first, meet the
challenge page, and the Brave stand-in answers each search; no browser tool (navigate, read) is ever used."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

B, H, c = K.B, K.H, K.c
FAKE = "http://127.0.0.1:8094"
LLM = "http://127.0.0.1:8090"
BRAVE = "brave-test-key-0123456789"
K.setup(language="en")
c.delete(B + "/sentinel/api/connections/travel/credential", headers=H)
c.post(FAKE + "/search/_reset")
c.post(FAKE + "/ddg/_down", json={"on": True})       # the browser's engines meet a challenge page: the search APIs answer
c.post(FAKE + "/exa-mcp/_down", json={"on": True})   # and the free Exa MCP is out too: the Brave key answers
r = c.post(B + "/sentinel/api/connections/search/credential", headers=H, json={"provider": "brave", "token": BRAVE})
K.check("Brave connected", r.status_code == 200 and r.json()["provider"] == "brave", r.text[:200])
hits0 = c.get(FAKE + "/ddg/_hits").json()["n"]

t = K.run("BRAVETRAVEL cheapest flight Singapore to Tokyo on 20 Nov 2026, a 4-star hotel near Ginza for 3 nights, and a compact rental car at Narita")
K.trace(t)
names = K.names(t)
K.check("the task completed with results from the search API", t["status"] == "COMPLETED" and "SEARCHED VIA ['brave']" in (t.get("result") or ""), (t["status"], (t.get("result") or "")[:200]))
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "BRAVETRAVEL"}).json()]
K.check("the agent's instructions name the Web search API (brave) and the no-browser rule for flights, hotels and cars",
        sysp and "Web search (web_search, always available): search services connected (2 services" in sysp[0] and "car rental" in sysp[0] and "do NOT open Google Flights" in sysp[0]
        and "brave" not in sysp[0].split("Web search (web_search")[1][:200].lower(), sysp[0][:200] if sysp else None)
K.check("skill, then flights_search (Kiwi.com data) and three web_search calls in ONE turn — flights, hotel, car — and no browser tool at all",
        names == ["load_skill", "flights_search", "web_search", "web_search", "web_search"] and any(b.count("web_search") == 3 and "flights_search" in b for b in K.batches(t)), (names, K.batches(t)))
K.check("the answer uses the Kiwi.com flight data for information only and books on the airline's site", "Kiwi.com data (information only)" in (t.get("result") or "")
        and "never through Kiwi" in (t.get("result") or "") and "kiwi.com/u" not in (t.get("result") or "").lower(), (t.get("result") or "")[:300])
log = c.get(FAKE + "/search/_log").json()
K.check("Brave received the three focused queries (concurrent, any order)", sorted(x["q"] for x in [x for x in log if x["provider"] == "brave"][-3:]) == sorted(["SIN to NRT flights 20 Nov 2026 cheapest",
        "Ginza Tokyo hotel 20-23 Nov 2026 4 star price", "Tokyo Narita airport car rental 20 Nov 2026 compact"]), log[-3:])
K.check("the browser's engines were asked first (once per search) and no page was opened with a browser tool", c.get(FAKE + "/ddg/_hits").json()["n"] == hits0 + 3
        and "browser_navigate" not in names and "browser_read" not in names, (names, c.get(FAKE + "/ddg/_hits").json()))
res = K.results(t, "web_search")
K.check("each result came back marked as the Brave search with snippets", len(res) == 3 and all("web search (brave)" in str(r.get("preview")) and "Brave snippet" in str(r.get("preview")) for r in res),
        [str(r.get("preview"))[:120] for r in res])
c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
c.post(FAKE + "/exa-mcp/_down", json={"on": False})
c.post(FAKE + "/ddg/_down", json={"on": False})
K.finish()
