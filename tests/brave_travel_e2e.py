"""Brave key, no Flights & hotels key, and the browser's own engines blocked (a challenge page): the agent's instructions
name the web search services and the Kiwi rule, and a flight + hotel + car request is searched with one data tool each, in one turn: flights_search (Kiwi.com data, information
only), hotels_search (no key: it falls back to web search by itself) and web_search for the car only (no data tool for
cars). The web searches ask the browser's engines first, meet the challenge page, and the Brave stand-in answers; no
browser tool (navigate, read) is ever used, and the flight is booked on the airline's site from the offer's book_at."""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

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

D1 = dt.date.today() + dt.timedelta(days=30)
D3 = D1 + dt.timedelta(days=3)
t = K.run(f"BRAVETRAVEL cheapest flight Singapore to Tokyo on {D1.day} {D1:%b %Y}, a 4-star hotel near Ginza for 3 nights, and a compact rental car at Narita DATE={D1}")
K.trace(t)
names = K.names(t)
K.check("the task completed with results from the search API", t["status"] == "COMPLETED" and "SEARCHED VIA ['brave']" in (t.get("result") or ""), (t["status"], (t.get("result") or "")[:200]))
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "BRAVETRAVEL"}).json()]
K.check("the agent's instructions name the Web search API (brave) and the no-browser rule for flights, hotels and cars",
        sysp and "Web search (web_search, always available): search services connected (2 services" in sysp[0] and "car rental" in sysp[0] and "do NOT open Google Flights" in sysp[0]
        and "brave" not in sysp[0].split("Web search (web_search")[1][:200].lower(), sysp[0][:200] if sysp else None)
K.check("skill, then one data tool each in ONE turn — flights_search, hotels_search, web_search for the car — and no browser tool at all",
        names == ["load_skill", "flights_search", "hotels_search", "web_search"] and any(sorted(b) == ["flights_search", "hotels_search", "web_search"] for b in K.batches(t)), (names, K.batches(t)))
K.check("the answer uses the Kiwi.com flight data for information only and books on the airline's site", "Kiwi.com data (information only)" in (t.get("result") or "")
        and "never through Kiwi" in (t.get("result") or "") and "kiwi.com/u" not in (t.get("result") or "").lower(), (t.get("result") or "")[:300])
log = c.get(FAKE + "/search/_log").json()
K.check("Brave received the hotel fallback and the car query (concurrent, any order), never a flight query", sorted(x["q"] for x in [x for x in log if x["provider"] == "brave"][-2:])
        == sorted([f"Tokyo Narita airport car rental {D1.day} {D1:%b %Y} compact", f"4 star hotels near Ginza Tokyo {D1} to {D3} price"])
        and not any("flights" in x["q"] for x in log if x["provider"] == "brave"), log[-3:])
K.check("the browser's engines were asked first (once per web search) and no page was opened with a browser tool", c.get(FAKE + "/ddg/_hits").json()["n"] == hits0 + 2
        and "browser_navigate" not in names and "browser_read" not in names, (names, c.get(FAKE + "/ddg/_hits").json()))
res = K.results(t, "web_search") + K.results(t, "hotels_search")
K.check("the car and the hotel came back from the Brave search with snippets", len(res) == 2 and all("web search (brave)" in str(r.get("preview")) and "Brave snippet" in str(r.get("preview")) for r in res),
        [str(r.get("preview"))[:120] for r in res])
K.check("the answer books the flight on the airline's own site from the offer (book_at), never through Kiwi", "Book the flight on singaporeair.com" in (t.get("result") or ""), (t.get("result") or "")[-300:])
c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
c.post(FAKE + "/exa-mcp/_down", json={"on": False})
c.post(FAKE + "/ddg/_down", json={"on": False})
K.finish()
