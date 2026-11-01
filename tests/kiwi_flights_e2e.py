"""No Flights & hotels key, kept as a scripted suite for what a real run cannot force or count: flights_search on Kiwi.com's
public MCP (tests/fake_apps.py's stand-in) asked exactly once per search (a range of dates too, no MCP handshake) with the
exact arguments (invalid numbers replaced, city codes, cabins, stops, alliances and airline codes, self-transfer only when
asked), Kiwi.com down (the web search answers for the same trip), the compact result fields (booking_url, book_at,
compare_url), the booking machine's search arguments and code answer (agent/booking.rs: each offer's booking link and
airline site, the cabin searched, the preferred airline, the next travel search offered), ONE travel search per turn and
per request when the model tries more, the executor's instructions and standing preferences, hotels and places falling
back to web search (the detail tools saying they need the key), and a booking through the travel agency's connector
filled by its form function from the profile and the vault. What the user sees without a key — real
flights with the airline, times, price and a Kiwi.com booking link that opens, one date and a range — is
tests/travel_kiwi_real.py (real model, real Kiwi.com)."""
import datetime as dt
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

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
# invalid numbers never reach Kiwi.com: a price cap of 0 means no limit, 0 travellers means 1
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "adults": 0, "max_price": 0, "currency": "SGD"})
kl0 = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
s0 = ((r.get("result") or {}).get("content") or {}).get("searched") or {}
cars0 = len(c.get(FAKE + "/cars/_log").json())
K.check("max_price 0 and adults 0: Kiwi.com is asked with no price limit and 1 adult, and the result says so", "price_to" not in kl0 and kl0.get("adults") == 1
        and s0.get("max_price") == "no limit (every price)" and s0.get("adults") == 1 and ((r.get("result") or {}).get("content") or {}).get("count") == 2, (kl0, s0))
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "adults": "-3", "max_price": -50, "currency": "SGD"})
kl0 = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
K.check("a negative price and a negative traveller count are fixed the same way", "price_to" not in kl0 and kl0.get("adults") == 1, kl0)
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "adults": 2, "max_price": 700, "currency": "SGD"})
kl0 = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
K.check("a real budget and a real party size go through unchanged", kl0.get("price_to") == 700 and kl0.get("adults") == 2, kl0)
# an alliance preference narrows the search itself: Kiwi.com takes IATA codes only, so Star Alliance goes in as its
# members' codes (SQ's direct flight stays, Cathay's one-stop goes); a name goes in as its code; an airline OMuse has no
# code for leaves every airline in and says so
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "airlines": ["Star Alliance"], "currency": "SGD"})
kl = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
res = (r.get("result") or {}).get("content") or {}
sel = str(kl.get("select_airlines") or "").split(",")
K.check("Star Alliance: Kiwi.com is asked for its 25 member airlines by code (SQ, NH, UA … not CX)",
        len(sel) == 25 and {"SQ", "NH", "UA", "LH"} <= set(sel) and "CX" not in sel, kl)
K.check("Star Alliance: only Star Alliance flights come back, marked preferred, and the result says what was searched",
        res.get("count") == 1 and all(o.get("preferred") for o in res.get("offers", [])) and "SQ" in json.dumps(res.get("offers"))
        and "only these airlines were searched" in str((res.get("searched") or {}).get("airline_filter")), (res.get("count"), res.get("searched")))
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "airlines": "Cathay Pacific", "currency": "SGD"})
kl = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
res = (r.get("result") or {}).get("content") or {}
K.check("an airline by name goes in as its code (Cathay Pacific → CX)", kl.get("select_airlines") == "CX" and res.get("count") == 1, (kl, res.get("count")))
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "airlines": ["Air Nowhere"], "currency": "SGD"})
kl = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
res = (r.get("result") or {}).get("content") or {}
K.check("an airline with no known code: all airlines searched, and the result says why", "select_airlines" not in kl and res.get("count") == 2
        and "all airlines were searched" in str((res.get("searched") or {}).get("airline_filter")), (kl, res.get("searched")))
r = act("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": DATE, "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"})
env = r.get("result") or {}
res = env.get("content") or {}
txt = str(r)
K.check("flights_search answers from Kiwi.com data: compact offers with airlines, flight numbers, times, stops, price, baggage", r.get("status") == "ok"
        and "Kiwi.com" in env.get("source", "") and res.get("count") == 2 and res["offers"][0]["airlines"] == "Singapore Airlines" and res["offers"][0]["legs"][0]["flight_number"] == "SQ638"
        and res["offers"][0]["price"] == 620 and res["offers"][0]["stops"] == 0 and res["offers"][1]["layovers"] == ["HKG"] and res["offers"][0]["baggage"]["checkedBag"] == 1, txt[:500])
K.check("each offer has its Kiwi.com booking link (booking_url) and the airline's own site (book_at); the guidance names both",
        res["offers"][0].get("booking_url") == "https://kiwi.com/u/abc123" and res["offers"][1].get("booking_url") == "https://kiwi.com/u/def456"
        and res["offers"][0].get("book_at") == ["singaporeair.com"] and "booking_url" in res["book"] and "airline's own site" in res["book"], txt[:500])
K.check("connections on one booking only: Kiwi.com was asked without self-transfer, and each offer says so",
        c.get(FAKE + "/kiwi-mcp/_log").json()[-1].get("allow_self_transfer") is False and "no self-transfer" in res["offers"][0].get("ticket", ""), res["offers"][0].get("ticket"))
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("ONE request to Kiwi.com per search (tools/call, no MCP handshake)", c.get(FAKE + "/kiwi-mcp/_requests").json().get("initialize", 0) == 0, c.get(FAKE + "/kiwi-mcp/_requests").json())
K.check("Kiwi was asked with the route, date, cabin and currency", kl and kl[-1]["flyFrom"] == "Singapore" and kl[-1]["flyTo"] == "Tokyo" and kl[-1]["departureDate"] == DATE
        and kl[-1]["cabinClass"] == "ECONOMY" and kl[-1]["currency"] == "SGD" and kl[-1]["sort"] == "price", kl[-1:])
K.check("the result repeats what was searched: economy, any stops, the date, one way", res.get("searched", {}).get("cabin") == "economy"
        and res["searched"]["stops"] == "any number of stops" and res["searched"]["date"] == DATE and res["searched"]["trip"] == "one way", res.get("searched"))
r2 = act("flights_search", {"origin": "TYO", "destination": "SIN", "date": DATE, "cabin": "business", "stops": "nonstop", "currency": "SGD"})
res2 = ((r2.get("result") or {}).get("content") or {})
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("a city code is one search over the whole city: TYO goes to Kiwi as Tokyo; business class and nonstop reach it as asked",
        kl[-1]["flyFrom"] == "Tokyo" and kl[-1]["flyTo"] == "Singapore" and kl[-1]["cabinClass"] == "BUSINESS" and kl[-1]["max_sector_stopovers"] == 0, kl[-1:])
r2b = act("flights_search", {"origin": "SIN", "destination": "NRT", "date": DATE, "stops": "any", "max_stops": 0, "nonstop_only": False, "max_price": 0, "currency": "SGD"})
klb = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
K.check("a request with every field filled by default (stops any, max_stops 0, max_price 0) asks for no stop limit and no price cap",
        "max_sector_stopovers" not in klb and "price_to" not in klb and ((r2b.get("result") or {}).get("content") or {}).get("searched", {}).get("stops") == "any number of stops", klb)
n0 = len(c.get(FAKE + "/kiwi-mcp/_log").json())
d1 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=1)).isoformat(); d3 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=3)).isoformat()
rr = act("flights_search", {"origin": "SIN", "destination": "NRT", "date": d1, "date_to": d3, "return_date": (dt.date.fromisoformat(d1) + dt.timedelta(days=5)).isoformat(), "currency": "SGD"})
rg = ((rr.get("result") or {}).get("content") or {})
kl = c.get(FAKE + "/kiwi-mcp/_log").json()[n0:]
days = [d1, (dt.date.fromisoformat(d1) + dt.timedelta(days=1)).isoformat(), d3]
K.check("a date range is ONE Kiwi.com search (departureDate … departureDateTo, a 5-night stay), split per day in one result",
        len(kl) == 1 and kl[0]["departureDate"] == d1 and kl[0].get("departureDateTo") == d3 and kl[0].get("nights_in_dst_from") == 5 and kl[0].get("nights_in_dst_to") == 5
        and "returnDate" not in kl[0] and [b["date"] for b in rg.get("by_date", [])] == days and all(o.get("date") for o in rg.get("offers", []))
        and rg["offers"][0]["price"] <= rg["offers"][-1]["price"] and rg["searched"]["dates"].endswith("(3 days)"), (kl, rg.get("by_date"), rg.get("searched")))
K.check("a range's Kayak link is its cheapest day, the return keeping the 5-night stay",
        rg.get("compare_url") == f"{FAKE}/kayak/flights/SIN-NRT/{d1}/{(dt.date.fromisoformat(d1) + dt.timedelta(days=5)).isoformat()}?sort=price_a", rg.get("compare_url"))
rs = act("flights_search", {"origin": "SIN", "destination": "NRT", "date": DATE, "self_transfer": "yes", "currency": "SGD"})
ks = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
K.check("self_transfer asked for explicitly goes to Kiwi.com as allow_self_transfer, and the offers say they may be separate tickets",
        ks.get("allow_self_transfer") is True and "self-transfer" in str((((rs.get("result") or {}).get("content") or {}).get("offers") or [{}])[0].get("ticket")), (ks, str(rs)[:300]))
r4 = act("flights_search", {"origin": "Osaka", "destination": "SIN", "date": DATE, "cabin": "business", "currency": "SGD"})
o4 = ((r4.get("result") or {}).get("content") or {}).get("offers") or []
mixed = [o for o in o4 if o.get("cabin_mix")]
K.check("a business search keeps a business itinerary with an economy feeder, labelled cabin_mix with the economy leg named",
        len(mixed) == 1 and mixed[0]["cabin"] == "business" and mixed[0]["cabin_mix"] == "business + economy" and mixed[0]["economy_legs"] and "NH12" in mixed[0]["economy_legs"][0].replace(" ", ""),
        [(o.get("airlines"), o.get("cabin"), o.get("cabin_mix"), o.get("economy_legs")) for o in o4])
K.check("…and the result says so: business, nonstop only, with the rule not to present another cabin", res2.get("searched", {}).get("cabin") == "business"
        and res2["searched"]["stops"] == "nonstop only" and res2["searched"]["origin"] == "Tokyo" and "business fare with nonstop only" in res2["searched"]["rule"], res2.get("searched"))
r3 = act("flights_search", {"origin": "SIN", "destination": "NRT", "date": DATE, "cabin": "economy", "airlines": ["Singapore Airlines", "NH"], "currency": "SGD"})
res3 = ((r3.get("result") or {}).get("content") or {})
kl3 = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
K.check("preferred airlines narrow the search itself (select_airlines SQ,NH): the SQ flight, marked preferred; the CX pair not asked for",
        kl3.get("select_airlines") == "SQ,NH" and res3.get("count") == 1 and res3["offers"][0].get("preferred") is True
        and res3["searched"]["airlines"] == ["Singapore Airlines", "NH"] and res3["searched"]["preferred_found"] == 1
        and "preferred airlines" in res3["searched"]["rule"], (kl3.get("select_airlines"), res3.get("searched"), [o.get("preferred") for o in res3.get("offers", [])]))
K.check("the result has Kayak's page for the same search (compare_url), the preferred airlines as its filter — built, not fetched",
        res3.get("compare_url") == f"{FAKE}/kayak/flights/SIN-NRT/{DATE}?sort=price_a&fs=airlines=SQ,NH", res3.get("compare_url"))
K.check("the result's rule makes the preferred search the only one and says to ask before searching all airlines",
        "do not search again without them" in res3["searched"]["rule"] and "ask whether to search all airlines" in res3["searched"]["rule"], res3["searched"]["rule"])
c.post(FAKE + "/kiwi-mcp/_down", json={"on": True})
rw = act("flights_search", {"origin": "SIN", "destination": "TYO", "date": DATE, "cabin": "business", "stops": "nonstop", "currency": "SGD"})
c.post(FAKE + "/kiwi-mcp/_down", json={"on": False})
w = (rw.get("result") or {})
K.check("no key and Kiwi.com down: the flight search falls back to the web search (its own chain) for the same trip and says why",
        rw.get("status") == "ok" and "web search" in str(w.get("source", "")) and "flights SIN to TYO" in str(w.get("source", ""))
        and "Kiwi.com failed" in str(w.get("flights_fallback")) and "no SerpApi key" in str(w.get("flights_fallback")) and w.get("results"), str(rw)[:500])
oid = res["offers"][0]["offer_id"]
r = act("flight_offer", {"offer_id": oid})
d = (r.get("result") or {}).get("content") or {}
K.check("flight_offer brings the offer's details back: Kiwi.com with its booking link first, then the airline's own site", r.get("status") == "ok" and d.get("offer_id") == oid
        and d["airlines"] == "Singapore Airlines" and d["booking_options"][0]["book_with"] == "Kiwi.com" and d["booking_options"][0]["link"] == "https://kiwi.com/u/abc123"
        and d["booking_options"][1]["book_with"] == "singaporeair.com", str(r)[:300])
out_d = (dt.date.fromisoformat(DATE) + dt.timedelta(days=3)).isoformat()
r = act("hotels_search", {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": out_d, "guests": 2, "min_stars": 4, "brands": ["Hilton"]})
w = r.get("result") or {}
K.check("…and still carries Kayak's page for the same stay", w.get("compare_url") == f"{FAKE}/kayak/hotels/Ginza%2CTokyo/{DATE}/{out_d}/2adults?sort=price_a", w.get("compare_url"))
K.check("hotels_search without the key falls back to web search with the request spelled out (brand, stars, area, city, dates)", r.get("status") == "ok"
        and "web search" in str(w.get("source", "")) and f"Hilton 4 star hotels near Ginza Tokyo {DATE} to {out_d}" in str(w.get("source", ""))
        and w.get("results") and "no SerpApi key" in w.get("hotels_fallback", "") and "not hotels data" in w.get("hotels_fallback", ""), str(r)[:400])
K.check("the compare links cost no call: Kayak was never asked during the flight and hotel searches", len(c.get(FAKE + "/cars/_log").json()) == cars0,
        c.get(FAKE + "/cars/_log").json()[cars0:])
r = act("places_search", {"query": "ramen", "area": "Shinjuku", "city": "Tokyo", "open_now": True})
w = r.get("result") or {}
cont = w.get("content") or w   # the browser step's result is wrapped as untrusted content
fb = w.get("places_fallback", "") or cont.get("places_fallback", "")
maps = cont.get("source") == "Google Maps in the browser"
K.check("places_search without the key: the chain in code — no SerpApi key, then the Google Maps page in the browser, else a web search",
        r.get("status") == "ok" and "no SerpApi key" in fb
        and ((maps and "google.com/maps" in cont.get("map_url", "") and len(cont.get("text", "")) >= 200)
             or ("best ramen near Shinjuku, Tokyo open now" in str(w.get("source", "")) and w.get("results")
                 and "Google Maps in the browser" in fb and fb.index("no SerpApi key") < fb.index("Google Maps in the browser"))),
        str(r)[:400])
K.check("…and nothing is left for the model to fetch: the Maps page's text, or the web search's top pages read in code",
        maps or ("pages" in w and "most relevant" not in fb), str(w)[:300])
r = act("hotel_rooms", {"hotel_id": "ht_x", "check_in": DATE, "check_out": out_d})
K.check("hotel_rooms (Google ids) still needs the key, and the refusal says the searches come from web search without it", r.get("status") in ("denied", "error")
        and "SerpApi" in str(r) and "web search" in str(r), str(r)[:300])

t = K.run(f"KIWIFLIGHTS cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}")
K.trace(t)
names = K.names(t)
K.check("the task completed and gives each offer's Kiwi.com booking link and the airline's own site (book_at), the pick by rule and any price",
        t["status"] == "COMPLETED" and "Book it at https://kiwi.com/u/abc123 (Kiwi.com) or on singaporeair.com" in (t.get("result") or "")
        and "Book it at https://kiwi.com/u/def456 (Kiwi.com) or on cathaypacific.com" in (t.get("result") or "")
        and "Picked: **Cathay Pacific" in (t.get("result") or "") and "any price" in (t.get("result") or ""), (t["status"], (t.get("result") or "")[:600]))
fs = [x["args"] for x in K.calls(t) if x.get("name") == "flights_search"]
K.check("skill, then ONE flights_search sorted by price in the home currency (SGD), the choice card — no web_search beside it, no browser",
        [x for x in names if x != "load_skill"] == ["flights_search", "present_choices"] and fs[0].get("sort") == "price" and fs[0].get("currency") == "SGD"
        and fs[0]["origin"] == "SIN" and fs[0]["destination"] == "TYO" and fs[0]["date"] == DATE, (names, fs))
K.check("Kayak's page for the same search is in the answer", f"{FAKE}/kayak/flights/" in (t.get("result") or ""), (t.get("result") or "")[-300:])
K.check("an ordinary skill is followed from its sheet: no plan, no planner call, never PLANNING", not any(e["type"] == "plan" for e in t["events"])
        and not (t.get("plan") or {}).get("steps") and "PLANNING" not in t["_seen"], ([e["type"] for e in t["events"]], t["_seen"]))
# the flight search is code's (no model call); the instructions a model-path travel request gets, from a question
K.run("KIWIPROMPT which sources do you use for flights?")
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "KIWIPROMPT"}).json() if not x["system"].startswith("You name conversations")]
K.check("the agent's instructions carry the rules: Kiwi.com data with each offer's booking_url, ONE flights_search per turn and request, no web_search beside it",
        sysp and "Kiwi.com data" in sysp[0] and "booking_url" in sysp[0] and "ONE travel search per turn and per request" in sysp[0]
        and "the only call a flight question needs" in sysp[0] and "Never add a web_search call beside them" in sysp[0], sysp[0][:200] if sysp else None)

# a business-class, direct request from a city with two airports: one search, the cabin and the stop limit as asked;
# the profile's preferred airline rides with the skill sheet and goes into the search
K.set_profile(preferred_airline="Star Alliance", **{"custom:Shoe size": "43", "custom:Star Alliance status": "Gold"})
t = K.run(f"BIZFLIGHTS cheapest business class direct flight from Tokyo to Singapore on {DATE} DATE={DATE}")
K.trace(t)
names = K.names(t)
fs = [x for x in K.calls(t) if x.get("name") == "flights_search"]
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("business class + direct + Tokyo: ONE flights_search (city code TYO, cabin business, stops nonstop), the choice card and nothing else, no plan",
        [x for x in names if x != "load_skill"] == ["flights_search", "present_choices"] and len(fs) == 1 and fs[0]["args"]["cabin"] == "business" and fs[0]["args"]["stops"] == "nonstop"
        and fs[0]["args"]["origin"] == "TYO" and kl[-1]["flyFrom"] == "Tokyo" and kl[-1]["cabinClass"] == "BUSINESS" and not any(e["type"] == "plan" for e in t["events"]),
        (names, [f["args"] for f in fs], kl[-1:]))
K.check("the answer is in the cabin searched", t["status"] == "COMPLETED" and "BUSINESS CLASS" in (t.get("result") or "") and "searched as business" in (t.get("result") or ""), (t.get("result") or "")[:300])
K.check("the preferred alliance was passed to the search; SQ (a Star Alliance member) came back flagged and the answer says preferred", fs[0]["args"].get("airlines") == ["Star Alliance"]
        and "your preferred airline" in (t.get("result") or ""), (fs[0]["args"], (t.get("result") or "")[-200:]))
K.run("BIZPROMPT which airline do I prefer?")
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "BIZPROMPT"}).json() if not x["system"].startswith("You name conversations")]
K.check("the executor's instructions carry the standing preferences (the alliance, the loyalty custom field) and nothing else from the profile",
        sysp and "Standing preferences" in sysp[0] and "- Preferred airline: Star Alliance" in sysp[0] and "- Star Alliance status: Gold" in sysp[0]
        and "Shoe size" not in sysp[0], (sysp[0][sysp[0].find("Standing"):][:300] if sysp else None))
K.check("the standing preferences say: ONE search with the preference (flights, hotels, car rental), none without it, all only when asked or after asking",
        sysp and "never a second search without it to compare" in sysp[0] and "ONE flights_search with the airline in airlines=[…]" in sysp[0]
        and "ONE hotels_search with the brand" in sysp[0] and "ONE cars_search with the rental company in companies=[…]" in sysp[0]
        and "When the user asks for all airlines, hotels or rental companies" in sysp[0] and "ask whether to search all airlines / hotels / companies" in sysp[0], (sysp[0][sysp[0].find("Standing"):][:700] if sysp else None))

# no price limit and no party size in the request: the machine's call has no cap and 1 adult, and the answer says any price
tz = K.run(f"ZEROCAP cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}")
fz = [x["args"] for x in K.calls(tz) if x.get("name") == "flights_search"]
kz = c.get(FAKE + "/kiwi-mcp/_log").json()[-1]
K.check("through the agent: the recorded flights_search has no max_price and adults 1; Kiwi.com got no price_to; offers came back",
        tz["status"] == "COMPLETED" and len(fz) == 1 and "max_price" not in fz[0] and fz[0].get("adults") == 1 and "price_to" not in kz and kz.get("adults") == 1
        and "any price" in (tz.get("result") or ""), (fz, kz, (tz.get("result") or "")[:200]))

# the preferred airline has nothing on the route: still ONE search (with the preference), and the answer asks before broadening
K.set_profile(preferred_airline="Emirates")
tp = K.run(f"PREFNONE cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}")
np_ = K.names(tp)
fsp = [x["args"] for x in K.calls(tp) if x.get("name") == "flights_search"]
K.check("a preferred airline with no offer: ONE flights_search carrying the preference, no second search without it, no choice card",
        [x for x in np_ if x != "load_skill"] == ["flights_search"] and len(fsp) == 1 and fsp[0].get("airlines") == ["Emirates"], (np_, fsp))
rp = [e["data"] for e in tp.get("events", []) if e["type"] == "tool_result" and e["data"].get("name") == "flights_search"]
K.check("Emirates flies nothing here: the narrowed search's empty result is the answer (Kiwi.com, preferred_found 0), not a web search",
        rp and "Kiwi.com flights" in str(rp[0].get("preview")) and "web search" not in str(rp[0].get("preview")).lower() and "web_search" not in np_,
        [str(x.get("preview"))[:300] for x in rp])
K.check("the search found no preferred offer and the answer says so and asks whether to search all airlines",
        tp["status"] == "COMPLETED" and "no Emirates flight" in (tp.get("result") or "") and "Shall I search all airlines?" in (tp.get("result") or ""),
        (tp["status"], (tp.get("result") or "")[:300]))

# a range of dates from the chat: one flights_search with date + date_to, no plan (no standing airline preference now)
K.set_profile(preferred_airline="")
f1 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=7)).isoformat(); f2 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=11)).isoformat()
tf = K.run(f"FLEXFLIGHTS cheapest flight Singapore to Tokyo any day between {f1} and {f2}")
nf = K.names(tf)
fsa = [x["args"] for x in K.calls(tf) if x.get("name") == "flights_search"]
K.check("a date range from the chat is ONE flights_search with date and date_to, no plan, and the answer covers all five days",
        [x for x in nf if x != "load_skill"] == ["flights_search", "present_choices"] and len(fsa) == 1 and fsa[0].get("date") == f1 and fsa[0].get("date_to") == f2
        and not any(e["type"] == "plan" for e in tf["events"]) and "5 days searched in one call" in (tf.get("result") or ""), (nf, fsa, (tf.get("result") or "")[:200]))

# a model that searches six times in one turn and three more next turn: ONE travel search per request — the first runs, the
# rest of the turn is skipped, the next turn's are refused, and nothing more reaches Kiwi.com
n0 = len(c.get(FAKE + "/kiwi-mcp/_log").json())
tw = K.run(f"TWOSEARCH flights and a hotel in Tokyo DATE={DATE}")
kw = c.get(FAKE + "/kiwi-mcp/_log").json()[n0:]
fr = [e["data"] for e in tw.get("events", []) if e["type"] == "tool_result" and e["data"].get("name") in ("flights_search", "hotels_search", "cars_search")]
ran = [(x["name"], x.get("ok")) for x in fr]
txt = " ".join(str(x.get("preview") or x.get("error") or "") for x in fr)
K.check("two flight, two hotel and two car searches in one turn, and one of each next turn: ONE travel search ran (the first flight), Kiwi.com asked once",
        tw["status"] == "COMPLETED" and len(kw) == 1 and kw[0]["flyTo"] == "NRT" and ran.count(("flights_search", True)) == 1
        and ("hotels_search", True) not in ran and ("cars_search", True) not in ran, (ran, kw))
K.check("the rest of the turn is skipped and the next turn's searches refused, naming the search that ran",
        txt.count("this turn already ran its one travel search") == 7 and txt.count("this request already ran its one travel search") == 1,
        (txt.count("this turn already ran its one travel search"), txt.count("this request already ran its one travel search")))

# booking through the user's travel agency (tests/fake_travel.py's connector and booking page): ONE search on the
# connector, the pick by rule, then the booking stage in code — the vault, the purchase approved, the booking page
# filled by its form function (fixed ids) with the traveller from the profile and the passport from the vault, the card
# by its fixed labels, and the hand-over for the user to check and pay (the Pay click is the user's)
from tests import fake_travel  # noqa: E402
from tests import travel_common as T  # noqa: E402
T.seed_vault()
K.connect_travel()
K.set_profile(name_en="Lucas Lu", email_personal="lucas@example.com", phone="+65 8000 0000")
off = sorted(fake_travel.flights_search({"origin": "SIN", "destination": "TYO", "date": DATE})["offers"], key=lambda o: o["fare_amount"])[0]
t2 = K.run(f"BOOKIT book the cheapest flight from Singapore to Tokyo on {DATE} through my travel agency")
K.trace(t2)
n2 = K.names(t2)
K.check("the booking stops at the hand-over with the page filled (WAITING_EXTERNAL)", t2["status"] == "WAITING_EXTERNAL", (t2["status"], n2))
K.check("the booking made no plan", not any(e["type"] == "plan" for e in t2["events"]), [e["type"] for e in t2["events"] if e["type"] in ("plan", "status")][:10])
K.check("ONE search, on the travel agency's connector, then the choice card — no Kiwi.com search, no web search",
        [x for x in n2 if x != "load_skill"][:2] == ["mcp_travel__flights_search", "present_choices"] and "flights_search" not in n2 and "web_search" not in n2, n2)
navs = [x["args"].get("url") for x in K.calls(t2) if x.get("name") == "browser_navigate"]
K.check("the cheapest offer's booking page opened after the vault and the approved purchase",
        [x for x in n2 if x != "load_skill"][2:5] == ["vault_list", "purchase_confirm", "browser_navigate"] and navs == [off["booking_url"]], (n2, navs, off["booking_url"]))
fill_res = K.results(t2, "browser_fill_secret")
K.check("the purchase was confirmed (approved) before the card, and every vault fill succeeded", "purchase_confirm" in n2 and "browser_fill_secret" in n2
        and n2.index("purchase_confirm") < n2.index("browser_fill_secret") and any(a["tool"] == "purchase_confirm" for a in t2["_approvals"])
        and fill_res and all("DENIED" not in str(r.get("preview") or r.get("error")) and r.get("ok") is not False for r in fill_res),
        ([str(r.get("preview") or r.get("error"))[:80] for r in fill_res], [a["tool"] for a in t2["_approvals"]]))
form = [x["args"] for x in K.calls(t2) if x.get("name") == "browser_fill_form"]
fills = [x["args"] for x in K.calls(t2) if x.get("name") == "browser_fill_secret"]
vals = json.dumps(form)
K.check("the page was filled by its form function (fixed ids): the traveller from the profile, the passport from the vault; the card by its labels, never typed",
        form and form[0].get("form") == "traveltest-flight" and "LUCAS LU" in vals and "lucas@example.com" in vals and '"vault"' in vals
        and {f.get("field") for f in fills} >= {"number", "expiry", "cvc"} and "4111" not in vals and "browser_type" not in n2, (form, fills))
K.hand_back()
t2 = K.wait_end(t2["id"])
K.check("after the hand-back without paying the task completes and the answer says it is not confirmed (nothing booked)",
        t2["status"] == "COMPLETED" and "not confirmed" in (t2.get("result") or ""), (t2["status"], (t2.get("result") or "")[:300]))
K.finish()
