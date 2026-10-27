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
K.finish()
