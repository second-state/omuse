"""Flights & hotels connection (Google Flights / Google Hotels / Google Maps through SerpApi, tests/fake_apps.py's stand-in): a bad key is
refused, a good key is accepted with the plan and searches left, the four tools appear in the catalog, the flight-booking
skill searches flights (one call, IATA codes resolved from city names, SGD), fetches the booking options of the cheapest
direct offer, searches hotels near Ginza (4+ stars) and one hotel's rates — compact results with offer and hotel ids — all
without the browser; the card shows the plan; disconnecting removes the tools and the policy explains how to connect."""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel booking bot has this suite's skills (crate::bots)

B, H = K.B, K.H
RT = {"X-Persona-Runtime": "rt-test"}
FAKE = "http://127.0.0.1:8094"
KEY = "serp-test-key-0123456789abcdef"
DATE = (dt.date.today() + dt.timedelta(days=30)).isoformat()
OUT = (dt.date.today() + dt.timedelta(days=32)).isoformat()   # two nights
c = K.c


def cat_names():
    return [t["function"]["name"] for t in c.get(B + "/internal/catalog", headers=RT).json()["tools"]]


def conn():
    return next(x for x in c.get(B + "/sentinel/api/connections", headers=H).json()["connections"] if x["name"] == "travel")


K.setup(language="en")
c.post(FAKE + "/serpapi/_reset")
# flights ask Kiwi.com first; this suite is about the Google path, so Kiwi is down until the Kiwi-first check at the end
c.post(FAKE + "/kiwi-mcp/_down", json={"on": True})
r = c.post(B + "/sentinel/api/connections/travel/credential", headers=H, json={"token": "wrong-key-0123456789abcdef"})
K.check("a refused key is reported", r.status_code == 400 and "refused" in r.text, r.text[:200])
r = c.post(B + "/sentinel/api/connections/travel/credential", headers=H, json={"token": "short"})
K.check("a malformed key is rejected before any call", r.status_code == 400 and "API key" in r.text, r.text[:200])
r = c.post(B + "/sentinel/api/connections/travel/credential", headers=H, json={"token": KEY})
K.check("a good key connects with the plan and the searches left", r.status_code == 200 and r.json()["plan"] == "Free Plan" and r.json()["searches_left"] == 238, r.text[:300])
cv = conn()
K.check("the connection view carries the plan and a credential", cv["has_credential"] and cv["enabled"] and cv["config"]["plan"] == "Free Plan"
        and cv["config"]["searches_left"] == 238, cv)
names = cat_names()
K.check("the six travel tools are in the catalog", all(x in names for x in ("flights_search", "flight_offer", "hotels_search", "hotel_rooms", "places_search", "place_details")), names)
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "travel-p1", "call_id": "p1", "tool": "places_search", "args": {"query": "ramen", "city": "Tokyo", "area": "Ginza", "min_rating": 4.0, "open_now": True}}).json()
pl = (r.get("result") or {}).get("content") or {}
K.check("places_search: Google Maps places with type, rating, price, address, hours, phone, website and a place_id", r.get("status") == "ok" and pl.get("count") == 2
        and pl["places"][0]["name"] == "Kyushu Jangara Ginza" and pl["places"][0]["rating"] == 4.4 and pl["places"][0]["website"].startswith("https://kyushujangara")
        and pl["places"][0]["place_id"] == "ChIJ7YW0VOaLGGARhGuFEiGbqS0" and pl["places"][0]["service_options"] == ["dine_in", "takeout"] and "restaurant's own site" in pl["book"], str(r)[:400])
mq = [q for q in c.get(FAKE + "/serpapi/_log").json() if q.get("engine") == "google_maps" and q.get("type") == "search"]
K.check("the query named the dish, area and city, the rating floor and open now", mq and mq[-1]["q"] == "ramen near Ginza, Tokyo" and mq[-1]["min_rating"] == "4.0" and mq[-1]["open_state"] == "now", mq[-1:])
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "travel-p1", "call_id": "p2", "tool": "place_details", "args": {"place_id": "ChIJ7YW0VOaLGGARhGuFEiGbqS0"}}).json()
pd = (r.get("result") or {}).get("content") or {}
K.check("place_details: hours per day, phone, website, the rating breakdown and review snippets", r.get("status") == "ok" and pd.get("hours") == ["wednesday: 11 AM–10:30 PM", "thursday: 11 AM–10:30 PM"]
        and pd["phone"] == "+81 3-3572-3025" and pd["rating_summary"][0] == "5★ 1500" and pd["review_snippets"][0].startswith("Great tonkotsu"), str(r)[:400])
K.check("with the key the connection view says flights come from Kiwi.com with Google as the fallback", conn().get("flights_source") == "kiwi" and conn().get("flights_fallback") == "google", conn())
r = c.post(B + "/sentinel/api/connections/travel/test", headers=H)
K.check("Test reads the account again", r.status_code == 200 and r.json()["plan"] == "Free Plan", r.text[:200])

t = K.run(f"TRAVELSEARCH cheapest flight Singapore to Tokyo on {DATE} and a 4-star hotel near Ginza for 3 nights DATE={DATE}")
K.trace(t)
names = K.names(t)
K.check("the task completed with the flights and the hotel", t["status"] == "COMPLETED" and "SQ 638" in (t.get("result") or "") and "Hotel Ginza Grand" in (t.get("result") or ""),
        (t["status"], t.get("error"), (t.get("result") or "")[:300]))
K.check("skill, flights, booking options, hotels, rooms — no browser",
        names == ["load_skill", "flights_search", "flight_offer", "hotels_search", "hotel_rooms"], names)
log = c.get(FAKE + "/serpapi/_log").json()
fl = [q for q in log if q.get("engine") == "google_flights" and not q.get("booking_token")]
K.check("flights: one Google Flights search, city names resolved to IATA codes, one way, sorted by price, in SGD",
        len(fl) == 1 and fl[0]["departure_id"] == "SIN" and fl[0]["arrival_id"] == "HND,NRT" and fl[0]["outbound_date"] == DATE
        and fl[0]["type"] == "2" and fl[0]["sort_by"] == "2" and fl[0]["currency"] == "SGD" and fl[0]["adults"] == "1", fl)
bo = [q for q in log if q.get("booking_token")]
K.check("the booking options were fetched with the offer's token", len(bo) == 1 and bo[0]["booking_token"].startswith("WlRCdz"), bo)
ht = [q for q in log if q.get("engine") == "google_hotels" and not q.get("property_token")]
K.check("hotels: one Google Hotels search near Ginza, the dates, 2 guests, 4 and 5 stars",
        len(ht) == 1 and ht[0]["q"] == "hotels near Ginza Tokyo" and ht[0]["check_in_date"] == DATE and ht[0]["adults"] == "2" and ht[0]["hotel_class"] == "4,5", ht)
pt = [q for q in log if q.get("property_token")]
K.check("the hotel's rates were fetched with its property token", len(pt) == 1 and pt[0]["property_token"].startswith("ChkI"), pt)
res = {r["name"]: r for r in K.results(t, "flights_search")}
prev = (K.results(t, "flights_search") or [{}])[0].get("preview", "")
K.check("the flights result is compact: airline, route with local times, price, stops, an offer id",
        "Singapore Airlines" in prev and "SQ 638" in prev and '"price": 620' in prev and "fl_" in prev, prev[:400])
prev = (K.results(t, "hotels_search") or [{}])[0].get("preview", "")
K.check("the hotels result is compact: name, stars, rating, price per night, a hotel id",
        "Hotel Ginza Grand" in prev and '"stars": 4' in prev and '"price_per_night": 210' in prev and "ht_" in prev, prev[:400])
K.check("no approval was needed (reads only)", not t["_approvals"], t["_approvals"])
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-biz", "call_id": "c-biz", "tool": "flights_search",
                                                   "args": {"origin": "TYO", "destination": "London", "date": DATE, "cabin": "business", "max_stops": 1, "adults": 2, "currency": "GBP"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights" and not x.get("booking_token")][-1]
K.check("city codes and names become every airport of the city in ONE Google search; business class, at most one stop and 2 adults go through as asked",
        q["departure_id"] == "HND,NRT" and q["arrival_id"] == "LHR,LGW,STN,LTN,LCY" and q["travel_class"] == "3" and q["stops"] == "2" and q["adults"] == "2" and q["currency"] == "GBP", q)
sr = ((r.get("result") or {}).get("content") or {}).get("searched") or {}
K.check("the Google result repeats the search: business, at most one stop, the airports used", sr.get("cabin") == "business" and sr.get("stops") == "at most one stop"
        and sr.get("origin") == "HND,NRT" and sr.get("adults") == 2, sr)
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-pref", "call_id": "c-pref", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "airlines": ["SQ", "NH"], "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights" and not x.get("booking_token")][-1]
fo = ((r.get("result") or {}).get("content") or {})
K.check("preferred airlines as codes narrow the Google search (include_airlines) and flag the SQ and ANA offers; the CX offer stays", q.get("include_airlines") == "SQ,NH"
        and fo["offers"][0].get("preferred") is True and "preferred" not in fo["offers"][1] and fo["offers"][2].get("preferred") is True and fo["searched"]["preferred_found"] == 2,
        (q.get("include_airlines"), [(o.get("airlines"), o.get("preferred")) for o in fo.get("offers", [])], {k: v for k, v in fo.get("searched", {}).items() if k != "rule"}))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-brand", "call_id": "c-brand", "tool": "hotels_search",
                                                   "args": {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": OUT, "guests": 2, "brands": ["Hilton"], "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_hotels" and not x.get("property_token")][-1]
ho = ((r.get("result") or {}).get("content") or {})
K.check("a preferred hotel group narrows Google Hotels with its brand filter in ONE search (Hilton → brands=28, plain query): the Conrad, marked preferred",
        q["q"] == "hotels near Ginza Tokyo" and q.get("brands") == "28" and [h["name"] for h in ho.get("hotels", [])] == ["Conrad Tokyo"]
        and ho["hotels"][0].get("preferred") is True and ho.get("searched", {}).get("brands") == ["Hilton"] and ho["searched"]["preferred_found"] == 1
        and "only these brands were searched" in str(ho["searched"].get("brand_filter")) and "limited to the user's preferred brands" in ho["searched"]["rule"]
        and "do not search again without them" in ho["searched"]["rule"] and "ask whether to search all hotels" in ho["searched"]["rule"], (q, ho.get("searched")))
hq = lambda: [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_hotels" and not x.get("property_token")]
n0 = len(hq())
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-westin", "call_id": "c-westin", "tool": "hotels_search",
                                                   "args": {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": OUT, "brands": ["Westin"], "currency": "SGD"}}).json()
ho = (r.get("result") or {}).get("content") or {}
qs = hq()[n0:]
K.check("a single brand: its Google id from the area's brand list (a first, unfiltered search), then the filtered one (Westin → brands=70)",
        len(qs) == 2 and "brands" not in qs[0] and qs[1].get("brands") == "70" and [h["name"] for h in ho.get("hotels", [])] == ["The Westin Tokyo"], (qs, ho.get("hotels")))
n0 = len(hq())
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-aman", "call_id": "c-aman", "tool": "hotels_search",
                                                   "args": {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": OUT, "brands": ["Aman"], "currency": "SGD"}}).json()
ho = (r.get("result") or {}).get("content") or {}
qs = hq()[n0:]
K.check("a brand Google lists no filter for here: searched by its name in the query instead, and the result says so",
        len(qs) == 2 and qs[1].get("q") == "Aman hotels near Ginza Tokyo" and "brands" not in qs[1]
        and "no brand filter for Aman" in str((ho.get("searched") or {}).get("brand_filter")), (qs, (ho.get("searched") or {}).get("brand_filter")))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-alli", "call_id": "c-alli", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "airlines": ["Star Alliance"], "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights" and not x.get("booking_token")][-1]
fo = ((r.get("result") or {}).get("content") or {})
K.check("an alliance narrows the Google search (include_airlines=STAR_ALLIANCE) and flags its members (SQ, ANA), not Cathay (oneworld)", q.get("include_airlines") == "STAR_ALLIANCE"
        and fo["offers"][0].get("preferred") is True and "preferred" not in fo["offers"][1] and fo["offers"][2].get("preferred") is True, (q.get("include_airlines"), [(o.get("airlines"), o.get("preferred")) for o in fo.get("offers", [])]))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-name", "call_id": "c-name", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "airlines": ["Singapore Airlines", "oneworld"], "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights" and not x.get("booking_token")][-1]
K.check("airline names narrow the Google search too, by code, beside an alliance (Singapore Airlines → SQ, oneworld → ONEWORLD)",
        q.get("include_airlines") == "SQ,ONEWORLD", q.get("include_airlines"))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-group", "call_id": "c-group", "tool": "hotels_search",
                                                   "args": {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": OUT, "guests": 2, "brands": ["IHG One Rewards"], "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_hotels" and not x.get("property_token")][-1]
K.check("a hotel group by its programme's name takes Google's group filter (IHG One Rewards → brands=17), not query words",
        q["q"] == "hotels near Ginza Tokyo" and q.get("brands") == "17", (q.get("q"), q.get("brands")))
# invalid numbers are replaced before any search runs: a price cap of 0 = no limit, 0 travellers / guests = 1
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-zero", "call_id": "c-zero-f", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "adults": 0, "max_price": 0, "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights" and not x.get("booking_token")][-1]
fo = ((r.get("result") or {}).get("content") or {})
K.check("Google Flights: adults 0 is searched as 1 and max_price 0 as no limit (every offer comes back)", q.get("adults") == "1" and "max_price" not in q
        and fo.get("count") == 3 and fo["searched"]["adults"] == 1 and fo["searched"]["max_price"] == "no limit (every price)", (q, {k: v for k, v in fo.get("searched", {}).items() if k != "rule"}))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-zero", "call_id": "c-zero-h", "tool": "hotels_search",
                                                   "args": {"city": "Tokyo", "check_in": DATE, "check_out": OUT, "guests": -2, "max_price_per_night": "0", "min_stars": 9, "currency": "SGD"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_hotels" and not x.get("property_token")][-1]
ho = ((r.get("result") or {}).get("content") or {})
K.check("Google Hotels: guests -2 is searched as 1, a nightly cap of \"0\" as no limit, 9 stars dropped", q.get("adults") == "1" and "max_price" not in q and "hotel_class" not in q
        and ho.get("searched", {}).get("guests") == 1 and ho["searched"]["max_price_per_night"] == "no limit (every price)", (q, ho.get("searched")))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-zero", "call_id": "c-zero-p", "tool": "places_search",
                                                   "args": {"query": "ramen", "city": "Tokyo", "min_rating": 0, "max_price_level": 0}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_maps"][-1]
K.check("Google Maps (restaurants): a minimum rating of 0 and a price level of 0 are dropped, the search still runs", r.get("status") == "ok"
        and "min_rating" not in q and "max_price" not in q, q)
# Kiwi.com up again: flights come from Kiwi even with a key, and no SerpApi search is spent; the fallback says why when it is used
c.post(FAKE + "/kiwi-mcp/_down", json={"on": False})
n_serp = len([x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights"])
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-kiwi", "call_id": "c-kiwi", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "currency": "SGD"}}).json()
src = (r.get("result") or {}).get("source", "")
K.check("with a key and Kiwi.com up, flights come from Kiwi.com and no Google Flights search is made", "Kiwi.com" in src
        and len([x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights"]) == n_serp, (src, n_serp))
c.post(FAKE + "/kiwi-mcp/_down", json={"on": True})
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-fb", "call_id": "c-fb", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "currency": "SGD"}}).json()
res = (r.get("result") or {}).get("content") or {}
K.check("Kiwi.com down: Google Flights answers as the fallback and says why", (r.get("result") or {}).get("source") == "Google Flights (fallback)"
        and "Kiwi.com failed" in res.get("fallback", "") and res.get("count", 0) > 0, (r.get("result") or {}).get("source"))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-web", "call_id": "c-web", "tool": "flights_search",
                                                   "args": {"origin": "ZZZ", "destination": "NRT", "date": DATE, "currency": "SGD"}}).json()
w = r.get("result") or {}
K.check("Kiwi.com down and Google Flights with no flights: the web search chain answers last and says both failed", r.get("status") == "ok"
        and "web search" in str(w.get("source", "")) and "Kiwi.com failed" in w.get("flights_fallback", "") and "Google Flights found no flights" in w.get("flights_fallback", ""), str(r)[:400])
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-hweb", "call_id": "c-hweb", "tool": "hotels_search",
                                                   "args": {"city": "Atlantis", "check_in": DATE, "check_out": OUT}}).json()
w = r.get("result") or {}
K.check("with a key, Google Hotels finding nothing falls back to web search and says so", r.get("status") == "ok" and "web search" in str(w.get("source", ""))
        and "Google Hotels found no hotels" in w.get("hotels_fallback", ""), str(r)[:300])
c.post(FAKE + "/kiwi-mcp/_down", json={"on": False})
r = c.delete(B + "/sentinel/api/connections/travel/credential", headers=H)
K.check("disconnect removes the key; the flight tools stay (Kiwi.com data), the hotel and place tools too", r.status_code == 200 and not conn()["has_credential"] and "flights_search" in cat_names(), r.text[:100])
a = c.post(B + "/internal/act", headers=RT, json={"task_id": "travel-t1", "call_id": "c1", "tool": "hotel_rooms", "args": {"hotel_id": "ht_x", "check_in": DATE, "check_out": OUT}}).json()
K.check("without the key the hotel rates tool is refused with the way to connect, flights still work (Kiwi.com data)",
        a.get("status") in ("denied", "error") and "SerpApi" in str(a) and "flights work without it" in str(a), str(a)[:300])
a = c.post(B + "/internal/act", headers=RT, json={"task_id": "travel-t1", "call_id": "c3", "tool": "place_details", "args": {"place_id": "pl_x"}}).json()
K.check("without the key the place details tool is refused the same way", a.get("status") in ("denied", "error") and "SerpApi" in str(a), str(a)[:300])
a = c.post(B + "/internal/act", headers=RT, json={"task_id": "travel-t1", "call_id": "c4", "tool": "hotels_search", "args": {"city": "Tokyo", "check_in": DATE, "check_out": OUT}}).json()
K.check("without the key hotels_search answers from web search", a.get("status") == "ok" and "web search" in str((a.get("result") or {}).get("source", "")), str(a)[:200])
a = c.post(B + "/internal/act", headers=RT, json={"task_id": "travel-t1", "call_id": "c2", "tool": "flights_search", "args": {"origin": "SIN", "destination": "NRT", "date": DATE}}).json()
K.check("flights_search without the key answers from Kiwi.com data", a.get("status") == "ok" and "Kiwi.com" in (a.get("result") or {}).get("source", ""), str(a)[:200])
K.finish()
