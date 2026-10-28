"""flights_search with no key against Kiwi.com's real public MCP server (OMUSE_E2E_REAL=1, OMUSE_E2E_KIWI_MCP_EP=https://mcp.kiwi.com/):
one real search Singapore → Tokyo 30 days out comes back as compact offers with airlines, flight numbers, times and prices,
and no Kiwi link anywhere. Real network, nothing billed.

Not in run_in_docker.sh's default list:
    OMUSE_E2E_KIWI_MCP_EP=https://mcp.kiwi.com/ OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh flights_real"""
import datetime as dt
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

if os.environ.get("OMUSE_E2E_REAL") != "1":
    print("flights_real needs the network (OMUSE_E2E_REAL=1)")
    raise SystemExit(0)
B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
DATE = (dt.date.today() + dt.timedelta(days=30)).isoformat()
K.setup(language="en")
c.delete(B + "/sentinel/api/connections/travel/credential", headers=H)
t0 = time.time()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "fr-t1", "call_id": "c1", "tool": "flights_search",
                                                  "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"}}, timeout=120).json()
res = (r.get("result") or {}).get("content") or {}
txt = json.dumps(r)
offers = res.get("offers") or []
print(f"INFO {time.time() - t0:.1f} s, {len(offers)} offers: " + " | ".join(f"{o['airlines']} {o['legs'][0]['flight_number']} {o['price']} {o['currency']} stops {o['stops']}" for o in offers[:3]))
K.check("a real Kiwi.com search returns compact offers with airlines, flight numbers, times and prices", r.get("status") == "ok" and len(offers) >= 3
        and all(o["airlines"] and o["legs"] and o["price"] and o["legs"][0]["depart"] for o in offers), txt[:400])
K.check("no Kiwi link anywhere; the booking guidance names the airline or an agency", "kiwi.com/u" not in txt.lower() and "bookingUrl" not in txt and "never through Kiwi" in txt, txt[:300])
# a city code, business class and nonstop: Kiwi gets the city's name and the cabin, the offers are business fares from either Tokyo airport
t0 = time.time()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "fr-t2", "call_id": "c2", "tool": "flights_search",
                                                  "args": {"origin": "TYO", "destination": "SIN", "date": DATE, "adults": 1, "cabin": "business", "max_stops": 0, "sort": "price", "currency": "SGD"}}, timeout=120).json()
res = (r.get("result") or {}).get("content") or {}
offers = res.get("offers") or []
print(f"INFO {time.time() - t0:.1f} s, {len(offers)} business offers: " + " | ".join(f"{o['airlines']} {o['legs'][0]['flight_number']} {o['legs'][0].get('from', '')} {o['price']} {o['currency']} stops {o['stops']} cabin {o['legs'][0].get('cabin', '')}" for o in offers[:4]))
K.check("TYO + business + nonstop against the real Kiwi: offers come back, all nonstop, from HND or NRT, in business class", r.get("status") == "ok" and len(offers) >= 1
        and all(o["stops"] == 0 for o in offers) and all(o["legs"][0].get("from") in ("HND", "NRT") for o in offers)
        and all("business" in str(o["legs"][0].get("cabin", "")).lower() for o in offers), json.dumps(r)[:500])
t0 = time.time()
d2 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=2)).isoformat()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "fr-t3", "call_id": "c3", "tool": "flights_search",
                                                  "args": {"origin": "SIN", "destination": "TYO", "date": DATE, "date_to": d2, "currency": "SGD"}}, timeout=180).json()
rg = (r.get("result") or {}).get("content") or {}
print(f"INFO range {time.time() - t0:.1f} s: " + ", ".join(f"{b['date']} {b.get('cheapest')} {b.get('airlines', '')}" for b in rg.get("by_date", [])))
K.check("a 3-day range against the real Kiwi: one call, three days with a cheapest each, offers dated and sorted by price",
        r.get("status") == "ok" and [b["date"] for b in rg.get("by_date", [])] == [DATE, (dt.date.fromisoformat(DATE) + dt.timedelta(days=1)).isoformat(), d2]
        and all(b.get("count", 0) > 0 for b in rg["by_date"]) and all(o.get("date") for o in rg["offers"])
        and [o["price"] for o in rg["offers"]] == sorted(o["price"] for o in rg["offers"]), json.dumps(r)[:500])
K.check("the result repeats the search: Tokyo, business, nonstop only", res.get("searched", {}).get("origin") == "Tokyo" and res["searched"]["cabin"] == "business" and res["searched"]["stops"] == "nonstop only", res.get("searched"))
K.finish()
