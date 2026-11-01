"""Flights & hotels with a SerpApi key (Google Flights / Google Hotels / Google Maps through tests/fake_apps.py's
stand-in): what a real run cannot show — the exact searches sent (one call per request, IATA codes from city names,
filters, brands, alliances, invalid numbers replaced), and forced failures (Google failing → Kiwi.com, both failing,
nothing found). Connecting the key as the user does it (a bad key refused, the plan and searches left) and the answers
the user sees are real-model suites: tests/travel_connect_real.py and tests/travel_user_real.py."""
import datetime as dt
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

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
# with the key a one-date flight search is Google Flights alone: Kiwi.com stays up and is never asked (checked below)
c.post(FAKE + "/kiwi-mcp/_down", json={"on": False})
kiwi_n0 = len(c.get(FAKE + "/kiwi-mcp/_log").json())
# the key is connected as setup (tests/travel_connect_real.py checks connecting it in the UI against the real SerpApi)
r = c.post(B + "/sentinel/api/connections/travel/credential", headers=H, json={"token": KEY})
assert r.status_code == 200, r.text[:300]
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
K.check("with the key the connection view says flights come from Google Flights with Kiwi.com as the fallback", conn().get("flights_source") == "google" and conn().get("flights_fallback") == "kiwi", conn())

# a flight and a hotel in one message: ONE travel search per request — the flight now, the hotel offered and searched
# when the user says yes in the next message
t = K.run(f"TRAVELSEARCH cheapest flight Singapore to Tokyo on {DATE} and a 4-star hotel near Ginza for 3 nights DATE={DATE}")
K.trace(t)
names = K.names(t)
res = K.result(t)
K.check("the first result has the flights, the pick's sellers and its Google Flights booking link, and lists the hotel as the next search",
        t["status"] == "COMPLETED" and "SQ 638" in json.dumps(res.get("options")) and any(x.get("book_with") == "Singapore Airlines" for x in res.get("sellers") or [])
        and "https://www.google.com/travel/flights/booking?" in json.dumps(res.get("options")) and res.get("next_items") == ["hotel"],
        (t["status"], t.get("error"), res))
K.check("the reply carries every must_say fact and offers the hotel search next", K.says_all(t) and "Shall I search the hotel now?" in (t.get("result") or ""),
        (t.get("result") or "")[:600])
fo = [x["args"] for x in K.calls(t) if x.get("name") == "flight_offer"]
fs = [x["args"] for x in K.calls(t) if x.get("name") == "flights_search"]
K.check("skill, the flight search, the choice card, the pick's booking options (its offer id, SGD) — one travel search, no browser",
        K.work(t) == ["flights_search", "present_choices", "flight_offer"] and fo and fo[0].get("offer_id", "").startswith("fl_")
        and fo[0].get("currency") == "SGD" and fs[0].get("sort") == "price" and fs[0].get("currency") == "SGD", (names, fs, fo))
th = K.run(f"TRAVELHOTEL yes, the hotel please DATE={DATE}", conv=t["_conv"])
K.trace(th)
K.check("the next message searches the hotel: hotels, the choice card, the pick's rooms — and the answer gives the room",
        th["status"] == "COMPLETED" and K.work(th) == ["hotels_search", "present_choices", "hotel_rooms"]
        and "Hotel Ginza Grand" in (th.get("result") or "") and (K.result(th).get("pick") or {}).get("room") and K.says_all(th), (K.names(th), (th.get("result") or "")[:400]))
log = c.get(FAKE + "/serpapi/_log").json()
fl = [q for q in log if q.get("engine") == "google_flights" and not q.get("booking_token")]
K.check("flights: one Google Flights search, city names resolved to IATA codes, one way, sorted by price, in SGD",
        len(fl) == 1 and fl[0]["departure_id"] == "SIN" and fl[0]["arrival_id"] == "HND,NRT" and fl[0]["outbound_date"] == DATE
        and fl[0]["type"] == "2" and fl[0]["sort_by"] == "2" and fl[0]["currency"] == "SGD" and fl[0]["adults"] == "1", fl)
bo = [q for q in log if q.get("booking_token")]
K.check("the booking options were fetched with the offer's token", len(bo) == 1 and bo[0]["booking_token"].startswith("WlRCdz"), bo)
ht = [q for q in log if q.get("engine") == "google_hotels" and not q.get("property_token")]
K.check("hotels: one Google Hotels search near Ginza, the dates (3 nights), 1 guest (none named), 4 and 5 stars",
        len(ht) == 1 and ht[0]["q"] == "hotels near Ginza Tokyo" and ht[0]["check_in_date"] == DATE and ht[0].get("check_out_date") == (dt.date.fromisoformat(DATE) + dt.timedelta(days=3)).isoformat()
        and ht[0]["adults"] == "1" and ht[0]["hotel_class"] == "4,5", ht)
pt = [q for q in log if q.get("property_token")]
K.check("the hotel's rates were fetched with its property token", len(pt) == 1 and pt[0]["property_token"].startswith("ChkI"), pt)
res = {r["name"]: r for r in K.results(t, "flights_search")}
prev = (K.results(t, "flights_search") or [{}])[0].get("preview", "")
K.check("the flights result is compact: airline, route with local times, price, stops, an offer id",
        "Singapore Airlines" in prev and "SQ 638" in prev and '"price": 620' in prev and "fl_" in prev, prev[:400])
K.check("each Google Flights offer links to its own booking page on Google Flights (booking_url, no extra search)",
        "google.com/travel/flights/booking?" in prev and "booking_url" in prev, prev[:600])
K.check("the one-date flight search never asked Kiwi.com", len(c.get(FAKE + "/kiwi-mcp/_log").json()) == kiwi_n0, kiwi_n0)
prev = (K.results(th, "hotels_search") or [{}])[0].get("preview", "")
K.check("the hotels result is compact: name, stars, rating, price per night, a hotel id",
        "Hotel Ginza Grand" in prev and '"stars": 4' in prev and '"price_per_night": 210' in prev and "ht_" in prev, prev[:400])
rh = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-links", "call_id": "c-links", "tool": "hotels_search",
                                                    "args": {"city": "Tokyo", "check_in": DATE, "check_out": OUT, "currency": "SGD"}}).json()
h0 = (((rh.get("result") or {}).get("content") or {}).get("hotels") or [{}])[0]
K.check("each hotel links Google's page for it and the dates (booking_url) and its own site (hotel_site)",
        h0.get("hotel_site") == "https://ginzagrand.example" and str(h0.get("booking_url", "")).startswith("https://www.google.com/travel/hotels/entity/")
        and "&ts=" in str(h0.get("booking_url")), h0)
K.check("no approval was needed (reads only)", not t["_approvals"] and not th["_approvals"], (t["_approvals"], th["_approvals"]))
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-biz", "call_id": "c-biz", "tool": "flights_search",
                                                   "args": {"origin": "TYO", "destination": "London", "date": DATE, "cabin": "business", "max_stops": 1, "adults": 2, "currency": "GBP"}}).json()
q = [x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights" and not x.get("booking_token")][-1]
K.check("city codes and names become every airport of the city in ONE Google search; business class, at most one stop and 2 adults go through as asked",
        q["departure_id"] == "HND,NRT" and q["arrival_id"] == "LHR,LGW,STN,LTN,LCY" and q["travel_class"] == "3" and q["stops"] == "2" and q["adults"] == "2" and q["currency"] == "GBP", q)
K.check("the Google result carries Kayak's page for the same search (compare_url): every airport of both cities, business, 2 adults",
        ((r.get("result") or {}).get("content") or {}).get("compare_url") == f"http://127.0.0.1:8094/kayak/flights/HND,NRT-LHR,LGW,STN,LTN,LCY/{DATE}/business/2adults?sort=price_a",
        ((r.get("result") or {}).get("content") or {}).get("compare_url"))
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
K.check("a brand with no known group id: ONE search, its name in the search words (no first lookup search), and the result says so",
        len(qs) == 1 and qs[0].get("q") == "Westin hotels near Ginza Tokyo" and "brands" not in qs[0]
        and "no brand filter for Westin" in str((ho.get("searched") or {}).get("brand_filter")), (qs, (ho.get("searched") or {}).get("brand_filter")))
n0 = len(hq())
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-aman", "call_id": "c-aman", "tool": "hotels_search",
                                                   "args": {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": OUT, "brands": ["Aman"], "currency": "SGD"}}).json()
ho = (r.get("result") or {}).get("content") or {}
qs = hq()[n0:]
K.check("a brand Google lists no filter for here: searched by its name in the query instead, and the result says so",
        len(qs) == 1 and qs[0].get("q") == "Aman hotels near Ginza Tokyo" and "brands" not in qs[0]
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
# with the key: a range of dates and self-transfer go to Kiwi.com (ONE search, no Google credit); Google failing outright
# falls back to Kiwi.com; Google finding nothing is the answer (nothing else is searched)
gfl = lambda: len([x for x in c.get(FAKE + "/serpapi/_log").json() if x.get("engine") == "google_flights"])
kn = lambda: len(c.get(FAKE + "/kiwi-mcp/_log").json())
g0, k0 = gfl(), kn()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-range", "call_id": "c-range", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "date_to": OUT, "currency": "SGD"}}).json()
res = (r.get("result") or {}).get("content") or {}
K.check("with a key, a range of dates is ONE Kiwi.com search and no Google Flights search", "Kiwi.com" in (r.get("result") or {}).get("source", "")
        and gfl() == g0 and kn() == k0 + 1 and len(res.get("by_date", [])) == 3 and res["offers"][0].get("booking_url", "").startswith("https://kiwi.com/u/"), ((r.get("result") or {}).get("source"), gfl() - g0, kn() - k0))
g0, k0 = gfl(), kn()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-self", "call_id": "c-self", "tool": "flights_search",
                                                   "args": {"origin": "SIN", "destination": "NRT", "date": DATE, "self_transfer": True, "currency": "SGD"}}).json()
K.check("with a key, self-transfer (asked for explicitly) is a Kiwi.com search with allow_self_transfer", "Kiwi.com" in (r.get("result") or {}).get("source", "")
        and gfl() == g0 and kn() == k0 + 1 and c.get(FAKE + "/kiwi-mcp/_log").json()[-1].get("allow_self_transfer") is True, (r.get("result") or {}).get("source"))
g0, k0 = gfl(), kn()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-fb", "call_id": "c-fb", "tool": "flights_search",
                                                   "args": {"origin": "ERR", "destination": "NRT", "date": DATE, "currency": "SGD"}}).json()
res = (r.get("result") or {}).get("content") or {}
K.check("Google Flights failing outright: Kiwi.com answers as the fallback and says why", (r.get("result") or {}).get("source") == "Kiwi.com flights (Google Flights failed)"
        and "Google Flights failed" in res.get("fallback", "") and res.get("count", 0) > 0 and gfl() == g0 + 1 and kn() == k0 + 1, str(r)[:400])
g0, k0 = gfl(), kn()
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-none", "call_id": "c-none", "tool": "flights_search",
                                                   "args": {"origin": "ZZZ", "destination": "NRT", "date": DATE, "currency": "SGD"}}).json()
res = (r.get("result") or {}).get("content") or {}
K.check("Google Flights finding nothing is the answer: no Kiwi.com, no web search, and a note to ask the user what to change",
        r.get("status") == "ok" and (r.get("result") or {}).get("source") == "Google Flights" and res.get("count") == 0 and "ask the user what to change" in res.get("note", "")
        and gfl() == g0 + 1 and kn() == k0, str(r)[:400])
c.post(FAKE + "/kiwi-mcp/_down", json={"on": True})
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-both", "call_id": "c-both", "tool": "flights_search",
                                                   "args": {"origin": "ERR", "destination": "NRT", "date": DATE, "currency": "SGD"}}).json()
c.post(FAKE + "/kiwi-mcp/_down", json={"on": False})
w = r.get("result") or {}
K.check("Google Flights and Kiwi.com both failing: the web search (its own chain) answers for the same trip and names both failures",
        r.get("status") == "ok" and "web search" in str(w.get("source", "")) and "Google Flights failed" in str(w.get("flights_fallback"))
        and "Kiwi.com failed" in str(w.get("flights_fallback")) and w.get("results"), str(r)[:400])
w0 = len(c.get(FAKE + "/serpapi/_log").json())
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-hnone", "call_id": "c-hnone", "tool": "hotels_search",
                                                   "args": {"city": "Atlantis", "check_in": DATE, "check_out": OUT}}).json()
ho = (r.get("result") or {}).get("content") or {}
K.check("with a key, Google Hotels finding nothing is the answer (ONE search, no web search) with a note to ask what to change",
        r.get("status") == "ok" and (r.get("result") or {}).get("source") == "Google Hotels" and ho.get("count") == 0 and "ask the user what to change" in ho.get("note", "")
        and len(c.get(FAKE + "/serpapi/_log").json()) == w0 + 1, str(r)[:300])
r = c.post(B + "/internal/act", headers=RT, json={"task_id": "ts-hweb", "call_id": "c-hweb", "tool": "hotels_search",
                                                   "args": {"city": "Errorville", "check_in": DATE, "check_out": OUT}}).json()
w = r.get("result") or {}
K.check("with a key, Google Hotels failing outright falls back to web search and says so", r.get("status") == "ok" and "web search" in str(w.get("source", ""))
        and "Google Hotels failed" in w.get("hotels_fallback", ""), str(r)[:300])
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
