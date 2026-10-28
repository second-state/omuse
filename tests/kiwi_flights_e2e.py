"""No Flights & hotels key: flights_search still works, with Kiwi.com's public MCP (tests/fake_apps.py's stand-in) behind
it — the flight data only (airlines, flight numbers, local times, stops, price, baggage), no booking link anywhere — and the
flight-booking skill calls it together with web_search in ONE turn; the answer sends the user to the airline's site or an
agency, never to Kiwi. The hotel tools say they need the key. With the key, flights switch to Google data (travel_search_e2e)."""
import datetime as dt
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
FAKE = "http://127.0.0.1:8094"
DATE = (dt.date.today() + dt.timedelta(days=30)).isoformat()


def act(tool, args, task="kiwi-t1"):
    return c.post(B + "/internal/act", headers=RT, json={"task_id": task, "call_id": "c" + str(time.time()), "tool": tool, "args": args}).json()


def cat_names():
    return [t["function"]["name"] for t in c.get(B + "/internal/catalog", headers=RT).json()["tools"]]


K.setup(language="en")
c.delete(B + "/sentinel/api/connections/travel/credential", headers=H)
LLM = "http://127.0.0.1:8090"
names = cat_names()
K.check("without a key the flight tools are in the catalog", "flights_search" in names and "flight_offer" in names, [n for n in names if "flight" in n or "hotel" in n])
cv = next(x for x in c.get(B + "/sentinel/api/connections", headers=H).json()["connections"] if x["name"] == "travel")
K.check("the connection view says flights come from Kiwi.com", cv.get("flights_source") == "kiwi" and not cv["has_credential"], cv)
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"})
env = r.get("result") or {}
res = env.get("content") or {}
txt = str(r)
K.check("flights_search answers from Kiwi.com data: compact offers with airlines, flight numbers, times, stops, price, baggage", r.get("status") == "ok"
        and "Kiwi.com" in env.get("source", "") and res.get("count") == 2 and res["offers"][0]["airlines"] == "Singapore Airlines" and res["offers"][0]["legs"][0]["flight_number"] == "SQ638"
        and res["offers"][0]["price"] == 620 and res["offers"][0]["stops"] == 0 and res["offers"][1]["layovers"] == ["HKG"] and res["offers"][0]["baggage"]["checkedBag"] == 1, txt[:500])
K.check("no Kiwi booking link anywhere in the result, and the booking guidance names the airline or an agency", "kiwi.com/u/" not in txt.lower() and "bookingUrl" not in txt
        and "never through Kiwi" in res["offers"][0]["book"] and "airline's own site" in res["book"], txt[:300])
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("Kiwi was asked with the route, date, cabin and currency", kl and kl[-1]["flyFrom"] == "Singapore" and kl[-1]["flyTo"] == "Tokyo" and kl[-1]["departureDate"] == DATE
        and kl[-1]["cabinClass"] == "ECONOMY" and kl[-1]["currency"] == "SGD" and kl[-1]["sort"] == "price", kl[-1:])
oid = res["offers"][0]["offer_id"]
r = act("flight_offer", {"offer_id": oid})
d = (r.get("result") or {}).get("content") or {}
K.check("flight_offer brings the offer's details back without a link", r.get("status") == "ok" and d.get("offer_id") == oid and d["airlines"] == "Singapore Airlines"
        and d["booking_options"][0]["link"] == "" and "information only" in d["booking_options"][0]["note"], str(r)[:300])
r = act("hotels_search", {"city": "Tokyo", "check_in": DATE, "check_out": DATE, "guests": 2})
K.check("hotels_search without the key is refused with the way to connect, and says flights work without it", r.get("status") in ("denied", "error") and "SerpApi" in str(r) and "flights work without it" in str(r), str(r)[:300])

t = K.run(f"KIWIFLIGHTS cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}")
K.trace(t)
names = K.names(t)
K.check("the task completed and sends the user to the airline's site or an agency, with no Kiwi link", t["status"] == "COMPLETED" and "singaporeair.com" in (t.get("result") or "")
        and "kiwi.com/u" not in (t.get("result") or "").lower() and "information only" in (t.get("result") or ""), (t["status"], (t.get("result") or "")[:300]))
K.check("skill, then flights_search and web_search in ONE turn, no browser", names == ["load_skill", "flights_search", "web_search"] and any(sorted(b) == ["flights_search", "web_search"] for b in K.batches(t)), (names, K.batches(t)))
K.check("an ordinary skill is followed from its sheet: no plan, no planner call, never PLANNING", not any(e["type"] == "plan" for e in t["events"])
        and not (t.get("plan") or {}).get("steps") and "PLANNING" not in t["_seen"], ([e["type"] for e in t["events"]], t["_seen"]))
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "KIWIFLIGHTS"}).json()]
K.check("the agent's instructions carry the rule: Kiwi.com data for information only, book on the airline's site or an agency, together with web_search",
        sysp and "Kiwi.com data" in sysp[0] and "INFORMATION ONLY" in sysp[0] and "never book through Kiwi" in sysp[0] and "together with web_search" in sysp[0], sysp[0][:200] if sysp else None)
K.finish()
