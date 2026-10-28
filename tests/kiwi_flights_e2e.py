"""No Flights & hotels key: flights_search still works, with Kiwi.com's public MCP (tests/fake_apps.py's stand-in) behind
it — the flight data only (airlines, flight numbers, local times, stops, price, baggage), no booking link anywhere — and the
flight-booking skill calls it together with web_search in ONE turn; the answer sends the user to the airline's site or an
agency, never to Kiwi. Without the key hotels and places come from web search and the detail tools say they need the key (travel_search_e2e has the keyed paths)."""
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
K.check("a date range is ONE flights_search: three days asked of Kiwi, the return moving with each day, one merged result",
        len(kl) == 3 and sorted(x["departureDate"] for x in kl) == [d1, (dt.date.fromisoformat(d1) + dt.timedelta(days=1)).isoformat(), d3]
        and all(x["returnDate"] == (dt.date.fromisoformat(x["departureDate"]) + dt.timedelta(days=5)).isoformat() for x in kl)
        and [b["date"] for b in rg.get("by_date", [])] == sorted(x["departureDate"] for x in kl) and all(o.get("date") for o in rg.get("offers", []))
        and rg["offers"][0]["price"] <= rg["offers"][-1]["price"] and rg["searched"]["dates"].endswith("(3 days)"), (kl, rg.get("by_date"), rg.get("searched")))
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
K.check("preferred airlines flag the matching offers and drop nothing: SQ preferred, the CX pair kept", res3.get("count") == 2 and res3["offers"][0].get("preferred") is True
        and "preferred" not in res3["offers"][1] and res3["searched"]["airlines"] == ["Singapore Airlines", "NH"] and res3["searched"]["preferred_found"] == 1
        and "preferred airlines" in res3["searched"]["rule"], (res3.get("searched"), [o.get("preferred") for o in res3.get("offers", [])]))
c.post(FAKE + "/kiwi-mcp/_down", json={"on": True})
rw = act("flights_search", {"origin": "SIN", "destination": "TYO", "date": DATE, "cabin": "business", "stops": "nonstop", "currency": "SGD"})
c.post(FAKE + "/kiwi-mcp/_down", json={"on": False})
w = (rw.get("result") or {})
K.check("no key and Kiwi.com down: the flight search falls back to the web search chain, with the request spelled out and a note that these are not flight data",
        rw.get("status") == "ok" and "web search" in str(w.get("source", "")) and "business class nonstop flights SIN to TYO" in str(w.get("source", ""))
        and w.get("results") and "Kiwi.com failed" in w.get("flights_fallback", "") and "not flights data" in w.get("flights_fallback", ""), str(rw)[:500])
oid = res["offers"][0]["offer_id"]
r = act("flight_offer", {"offer_id": oid})
d = (r.get("result") or {}).get("content") or {}
K.check("flight_offer brings the offer's details back without a link", r.get("status") == "ok" and d.get("offer_id") == oid and d["airlines"] == "Singapore Airlines"
        and d["booking_options"][0]["link"] == "" and "information only" in d["booking_options"][0]["note"], str(r)[:300])
out_d = (dt.date.fromisoformat(DATE) + dt.timedelta(days=3)).isoformat()
r = act("hotels_search", {"city": "Tokyo", "area": "Ginza", "check_in": DATE, "check_out": out_d, "guests": 2, "min_stars": 4, "brands": ["Hilton"]})
w = r.get("result") or {}
K.check("hotels_search without the key falls back to web search with the request spelled out (brand, stars, area, city, dates)", r.get("status") == "ok"
        and "web search" in str(w.get("source", "")) and f"Hilton 4 star hotels near Ginza Tokyo {DATE} to {out_d}" in str(w.get("source", ""))
        and w.get("results") and "no SerpApi key" in w.get("hotels_fallback", "") and "not hotels data" in w.get("hotels_fallback", ""), str(r)[:400])
r = act("places_search", {"query": "ramen", "area": "Shinjuku", "city": "Tokyo", "open_now": True})
w = r.get("result") or {}
K.check("places_search without the key falls back to web search too", r.get("status") == "ok" and "best ramen near Shinjuku Tokyo open now" in str(w.get("source", ""))
        and w.get("results") and "places_fallback" in w, str(r)[:300])
r = act("hotel_rooms", {"hotel_id": "ht_x", "check_in": DATE, "check_out": out_d})
K.check("hotel_rooms (Google ids) still needs the key, and the refusal says the searches come from web search without it", r.get("status") in ("denied", "error")
        and "SerpApi" in str(r) and "web search" in str(r), str(r)[:300])

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

# a business-class, direct request from a city with two airports: one search, the cabin and the stop limit as asked;
# the profile's preferred airline rides with the skill sheet and goes into the search
K.set_profile(preferred_airline="Star Alliance", **{"custom:Shoe size": "43", "custom:Star Alliance status": "Gold"})
t = K.run(f"BIZFLIGHTS cheapest business class direct flight from Tokyo to Singapore on {DATE} DATE={DATE}")
K.trace(t)
names = K.names(t)
fs = [x for x in K.calls(t) if x.get("name") == "flights_search"]
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("business class + direct + Tokyo: ONE flights_search (city code TYO, cabin business, stops nonstop) beside web_search, no plan",
        names == ["load_skill", "flights_search", "web_search"] and len(fs) == 1 and fs[0]["args"]["cabin"] == "business" and fs[0]["args"]["stops"] == "nonstop"
        and fs[0]["args"]["origin"] == "TYO" and kl[-1]["flyFrom"] == "Tokyo" and kl[-1]["cabinClass"] == "BUSINESS" and not any(e["type"] == "plan" for e in t["events"]),
        (names, [f["args"] for f in fs], kl[-1:]))
K.check("the answer is in the cabin searched", t["status"] == "COMPLETED" and "BUSINESS CLASS" in (t.get("result") or "") and "searched as business" in (t.get("result") or ""), (t.get("result") or "")[:300])
K.check("the preferred alliance was passed to the search; SQ (a Star Alliance member) came back flagged and the answer says preferred", fs[0]["args"].get("airlines") == ["Star Alliance"]
        and "your preferred airline" in (t.get("result") or ""), (fs[0]["args"], (t.get("result") or "")[-200:]))
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "BIZFLIGHTS"}).json()]
K.check("the executor's instructions carry the standing preferences (the alliance, the loyalty custom field) and nothing else from the profile",
        sysp and "Standing preferences" in sysp[0] and "- Preferred airline: Star Alliance" in sysp[0] and "- Star Alliance status: Gold" in sysp[0]
        and "Shoe size" not in sysp[0] and "nothing is dropped" in sysp[0], (sysp[0][sysp[0].find("Standing"):][:300] if sysp else None))

# a range of dates from the chat: one flights_search with date + date_to, no plan
f1 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=7)).isoformat(); f2 = (dt.date.fromisoformat(DATE) + dt.timedelta(days=11)).isoformat()
tf = K.run(f"FLEXFLIGHTS cheapest Singapore to Tokyo any day FROM={f1} TO={f2}")
nf = K.names(tf)
fsa = [x["args"] for x in K.calls(tf) if x.get("name") == "flights_search"]
K.check("a date range from the chat is ONE flights_search with date and date_to, no plan, and the answer covers all five days",
        nf == ["load_skill", "flights_search"] and len(fsa) == 1 and fsa[0].get("date") == f1 and fsa[0].get("date_to") == f2
        and not any(e["type"] == "plan" for e in tf["events"]) and "5 days searched in one call" in (tf.get("result") or ""), (nf, fsa, (tf.get("result") or "")[:200]))

# "book it" in the same conversation: no plan, no second search — the airline's booking page with the traveller, passport and card
# filled from the vault, then the page is handed over for the user to check and pay (tests/fake_travel.py's booking page stands in)
from tests import fake_travel  # noqa: E402
from tests import travel_common as T  # noqa: E402
T.seed_vault()
off = fake_travel.flights_search({"origin": "SIN", "destination": "TYO", "date": DATE})["offers"][0]
t2 = K.run(f"BOOKIT book the SQ638 please URL={off['booking_url']} AMOUNT={off['fare_amount']}", conv=t["_conv"])
K.trace(t2)
n2 = K.names(t2)
K.check("the follow-up stops at the hand-over with the page filled (WAITING_EXTERNAL)", t2["status"] == "WAITING_EXTERNAL", t2["status"])
K.check("the follow-up made no plan", not any(e["type"] == "plan" for e in t2["events"]), [e["type"] for e in t2["events"] if e["type"] in ("plan", "status")][:10])
K.check("the follow-up searched nothing again and showed no choices again", not any(x in n2 for x in ("plan_task", "flights_search", "web_search", "present_choices")),
        [x for x in n2 if x in ("plan_task", "flights_search", "web_search", "present_choices")])
navs = [x["args"].get("url") for x in K.calls(t2) if x.get("name") == "browser_navigate"]
K.check("it went straight to the booking page after the vault", n2[:2] == ["vault_list", "browser_navigate"] and navs == [off["booking_url"]], (n2[:3], navs))
fill_res = [r for r in K.results(t2, "browser_fill_secret")]
K.check("the purchase was confirmed (approved) before the card, and every vault fill succeeded", n2.index("purchase_confirm") < max(i for i, x in enumerate(n2) if x == "browser_fill_secret")
        and any(a["tool"] == "purchase_confirm" for a in t2["_approvals"]) and fill_res and all("DENIED" not in str(r.get("preview") or r.get("error")) for r in fill_res),
        ([str(r.get("preview") or r.get("error"))[:80] for r in fill_res], [a["tool"] for a in t2["_approvals"]]))
typed = " ".join(str(x.get("args", {}).get("text")) for x in K.calls(t2) if x.get("name") == "browser_type")
fills = [x["args"] for x in K.calls(t2) if x.get("name") == "browser_fill_secret"]
K.check("the traveller was typed and the passport and card filled from the vault (approved), never typed", "Lucas Lu" in typed and "lucas@example.com" in typed
        and {f.get("field") for f in fills} >= {"number", "expiry", "cvc"} and any(a["tool"] == "browser_fill_secret" for a in t2["_approvals"]) and "4111" not in typed, (typed, fills, t2["_approvals"]))
K.hand_back()
t2 = K.wait_end(t2["id"])
K.check("after the hand-back the task completes and the answer says the page is ready to pay", t2["status"] == "COMPLETED" and "Pay" in (t2.get("result") or ""), (t2["status"], (t2.get("result") or "")[:200]))
K.finish()
