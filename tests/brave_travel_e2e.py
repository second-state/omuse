"""Kept as a scripted suite for what a real run cannot force or observe: the browser's own engines blocked (a challenge
page) and the free Exa MCP down, so the Brave key must answer; exactly ONE travel search per message for a flight + hotel +
car request (flights_search, then hotels_search after the user says yes, then cars_search), Brave's exact queries, the
engines asked once per web search, no browser tool, and the agent's instructions naming the search services. What the user
sees with a Brave key — real flights, hotel and car with prices and booking links that open — is tests/travel_brave_real.py."""
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
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "BRAVETRAVEL"}).json()]
K.check("the agent's instructions name the Web search API (brave) and the no-browser rule for flights, hotels and cars",
        sysp and "Web search (web_search, always available): search services connected (2 services" in sysp[0] and "car rental" in sysp[0] and "do NOT open Google Flights" in sysp[0]
        and "brave" not in sysp[0].split("Web search (web_search")[1][:200].lower(), sysp[0][:200] if sysp else None)
K.check("a flight, a hotel and a car in one message: ONE travel search — the flight — and the hotel offered next",
        t["status"] == "COMPLETED" and names == ["load_skill", "flights_search"] and "Kiwi.com data: SQ638" in (t.get("result") or "")
        and "Shall I search the hotel" in (t.get("result") or ""), (names, (t.get("result") or "")[:300]))
K.check("the answer gives the offer's Kiwi.com booking link and the airline's own site (book_at)",
        "Book the flight at https://kiwi.com/u/abc123 (Kiwi.com) or on singaporeair.com" in (t.get("result") or ""), (t.get("result") or "")[-300:])
th = K.run(f"BRAVEHOTEL yes, the hotel DATE={D1}", conv=t["_conv"])
tc = K.run(f"BRAVECAR and now the car DATE={D1}", conv=t["_conv"])
K.trace(th)
K.trace(tc)
K.check("the next two messages search the hotel, then the car — one travel search each", K.names(th) == ["hotels_search"] and K.names(tc) == ["cars_search"]
        and "SEARCHED VIA ['brave']" in (th.get("result") or "") and "SEARCHED VIA ['brave']" in (tc.get("result") or ""), (K.names(th), K.names(tc), th.get("result"), tc.get("result")))
log = c.get(FAKE + "/search/_log").json()
K.check("Brave received the hotel fallback and the car query, never a flight query", [x["q"] for x in log if x["provider"] == "brave"][-2:]
        == [f"4 star hotels near Ginza Tokyo {D1} to {D3} price", "small car rental companies Tokyo NRT airport prices"]
        and not any("flights" in x["q"] for x in log if x["provider"] == "brave"), log[-3:])
allnames = names + K.names(th) + K.names(tc)
K.check("the browser's engines were asked first (once per web search) and no page was opened with a browser tool", c.get(FAKE + "/ddg/_hits").json()["n"] == hits0 + 2
        and "browser_navigate" not in allnames and "browser_read" not in allnames, (allnames, c.get(FAKE + "/ddg/_hits").json()))
res = K.results(tc, "cars_search") + K.results(th, "hotels_search")
K.check("the car and the hotel came back from the Brave search with snippets", len(res) == 2 and all("web search (brave)" in str(r.get("preview")) and "Brave snippet" in str(r.get("preview")) for r in res),
        [str(r.get("preview"))[:120] for r in res])
K.check("the car's booking link is Kayak's page for exactly this rental (NRT, the dates at 10:00, small cars), and the answer gives it",
        f"http://127.0.0.1:8094/kayak/cars/NRT/{D1}-10h/{D3}-10h?fs=carclass=small" in (tc.get("result") or ""), (tc.get("result") or "")[-400:])
c.delete(B + "/sentinel/api/connections/search/credential", headers=H)
c.post(FAKE + "/exa-mcp/_down", json={"on": False})
c.post(FAKE + "/ddg/_down", json={"on": False})
K.finish()
