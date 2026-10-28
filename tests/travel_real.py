"""Flights, hotels and restaurants against the real SerpApi (OMUSE_E2E_REAL=1, OMUSE_E2E_SERPAPI_KEY in the e2e env file,
OMUSE_E2E_SERPAPI_EP=https://serpapi.com): the key connects with the account's plan, flights_search answers with Google
Flights data (offer ids), flight_offer brings booking options, hotels_search answers with Google Hotels data (hotel ids)
hotel_rooms the rates per seller, places_search restaurants near Ginza and place_details one of them. Seven searches
are billed against the plan.

Not in run_in_docker.sh's default list:
    OMUSE_E2E_SERPAPI_EP=https://serpapi.com OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_real"""
import datetime as dt
import json
import os
import sys
import time

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

if os.environ.get("OMUSE_E2E_REAL") != "1" or not os.environ.get("OMUSE_E2E_SERPAPI_KEY"):
    print("travel_real needs the real SerpApi (OMUSE_E2E_REAL=1 and OMUSE_E2E_SERPAPI_KEY in the e2e env file)")
    raise SystemExit(0)
B, H, c = K.B, K.H, K.c
RT = {"X-Persona-Runtime": "rt-test"}
KEY = os.environ["OMUSE_E2E_SERPAPI_KEY"]
D1 = (dt.date.today() + dt.timedelta(days=30)).isoformat()
D2 = (dt.date.today() + dt.timedelta(days=33)).isoformat()


def act(tool, args, task="tr-t1"):
    r = c.post(B + "/internal/act", headers=RT, json={"task_id": task, "call_id": "c" + str(time.time()), "tool": tool, "args": args}, timeout=120).json()
    return r, ((r.get("result") or {}).get("content") or {})


K.setup(language="en")
r = c.post(B + "/sentinel/api/connections/travel/credential", headers=H, json={"token": KEY}, timeout=60)
K.check("the real SerpApi key connects with the account's plan", r.status_code == 200 and r.json().get("plan"), r.text[:200])
print(f"INFO plan {r.json().get('plan')}, searches left {r.json().get('searches_left')}" if r.status_code == 200 else "INFO connect failed")
t0 = time.time()
# flights ask Kiwi.com first; switch the local Kiwi stand-in off so this real-SerpApi suite exercises the Google fallback
httpx.post("http://127.0.0.1:8094/kiwi-mcp/_down", json={"on": True}, timeout=10)
r, res = act("flights_search", {"origin": "SIN", "destination": "Tokyo", "date": D1, "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"})
offers = res.get("offers") or []
print(f"INFO flights {time.time() - t0:.1f} s, {len(offers)} offers: " + " | ".join(f"{o['airlines']} {o['route'][:40]} {o['price']} {o['currency']} stops {o['stops']}" for o in offers[:3]))
K.check("Google Flights: offers with airlines, legs, local times, price and an offer id", r.get("status") == "ok" and "Google Flights" in (r.get("result") or {}).get("source", "")
        and len(offers) >= 3 and all(o["airlines"] and o["legs"] and o["price"] and o["offer_id"].startswith("fl_") for o in offers), json.dumps(r)[:400])
t0 = time.time()
r, bo = act("flight_offer", {"offer_id": offers[0]["offer_id"], "currency": "SGD"}) if offers else ({}, {})
opts = bo.get("options") or []
print(f"INFO booking options {time.time() - t0:.1f} s: " + " | ".join(f"{o['book_with']} {o['price']}" for o in opts[:3]))
K.check("flight_offer: who sells the cheapest offer and at what price", r.get("status") == "ok" and len(opts) >= 1 and all(o["book_with"] for o in opts), json.dumps(r)[:400])
t0 = time.time()
r, hres = act("hotels_search", {"city": "Tokyo", "area": "Ginza", "check_in": D1, "check_out": D2, "guests": 2, "min_stars": 4, "currency": "SGD"})
hotels = hres.get("hotels") or []
print(f"INFO hotels {time.time() - t0:.1f} s, {len(hotels)}: " + " | ".join(f"{h['name'][:30]} {h['stars']}★ {h['rating']} {h['price_per_night']} {h['currency']}" for h in hotels[:3]))
K.check("Google Hotels: hotels with name, stars, rating, price per night and a hotel id", r.get("status") == "ok" and len(hotels) >= 3
        and all(h["name"] and h["hotel_id"].startswith("ht_") for h in hotels) and any(h["price_per_night"] for h in hotels), json.dumps(r)[:400])
t0 = time.time()
r, rooms = act("hotel_rooms", {"hotel_id": hotels[0]["hotel_id"], "check_in": D1, "check_out": D2, "guests": 2, "currency": "SGD"}) if hotels else ({}, {})
print(f"INFO rooms {time.time() - t0:.1f} s: {rooms.get('name')} — " + " | ".join(f"{o['source']} {o['rate_per_night']}" for o in (rooms.get('offers') or [])[:3]))
K.check("hotel_rooms: the property's details and rates per seller", r.get("status") == "ok" and rooms.get("name") and len(rooms.get("offers") or []) >= 1, json.dumps(r)[:400])
t0 = time.time()
r, pl = act("places_search", {"query": "ramen", "city": "Tokyo", "area": "Ginza", "min_rating": 4.0})
places = pl.get("places") or []
print(f"INFO places {time.time() - t0:.1f} s, {len(places)}: " + " | ".join(f"{p['name'][:28]} {p['rating']} ({p['reviews']}) {p['price']} {p['open_state'][:20]}" for p in places[:3]))
K.check("Google Maps: restaurants with name, rating, reviews, address and a place id", r.get("status") == "ok" and len(places) >= 3
        and all(p["name"] and p["place_id"] and p["address"] for p in places) and any(p["rating"] for p in places), json.dumps(r)[:400])
t0 = time.time()
r, pd = act("place_details", {"place_id": places[0]["place_id"]}) if places else ({}, {})
print(f"INFO place {time.time() - t0:.1f} s: {pd.get('name')} {pd.get('phone')} {pd.get('website')} hours {len(pd.get('hours') or [])}")
K.check("place_details: the place's hours, phone or website", r.get("status") == "ok" and pd.get("name") and (pd.get("hours") or pd.get("phone") or pd.get("website")), json.dumps(r)[:400])
c.delete(B + "/sentinel/api/connections/travel/credential", headers=H)
K.finish()
