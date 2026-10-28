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
K.check("the result repeats what was searched: economy, any stops, the date, one way", res.get("searched", {}).get("cabin") == "economy"
        and res["searched"]["stops"] == "any number of stops" and res["searched"]["date"] == DATE and res["searched"]["trip"] == "one way", res.get("searched"))
r2 = act("flights_search", {"origin": "TYO", "destination": "SIN", "date": DATE, "cabin": "business", "max_stops": 0, "currency": "SGD"})
res2 = ((r2.get("result") or {}).get("content") or {})
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("a city code is one search over the whole city: TYO goes to Kiwi as Tokyo; business class and nonstop reach it as asked",
        kl[-1]["flyFrom"] == "Tokyo" and kl[-1]["flyTo"] == "Singapore" and kl[-1]["cabinClass"] == "BUSINESS" and kl[-1]["max_sector_stopovers"] == 0, kl[-1:])
K.check("…and the result says so: business, nonstop only, with the rule not to present another cabin", res2.get("searched", {}).get("cabin") == "business"
        and res2["searched"]["stops"] == "nonstop only" and res2["searched"]["origin"] == "Tokyo" and "business fare with nonstop only" in res2["searched"]["rule"], res2.get("searched"))
r3 = act("flights_search", {"origin": "SIN", "destination": "NRT", "date": DATE, "cabin": "economy", "airlines": ["Singapore Airlines", "NH"], "currency": "SGD"})
res3 = ((r3.get("result") or {}).get("content") or {})
K.check("preferred airlines flag the matching offers and drop nothing: SQ preferred, the CX pair kept", res3.get("count") == 2 and res3["offers"][0].get("preferred") is True
        and "preferred" not in res3["offers"][1] and res3["searched"]["airlines"] == ["Singapore Airlines", "NH"] and res3["searched"]["preferred_found"] == 1
        and "preferred airlines" in res3["searched"]["rule"], (res3.get("searched"), [o.get("preferred") for o in res3.get("offers", [])]))
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

# a business-class, direct request from a city with two airports: one search, the cabin and the stop limit as asked;
# the profile's preferred airline rides with the skill sheet and goes into the search
K.set_profile(preferred_airline="Star Alliance", **{"custom:Shoe size": "43", "custom:Star Alliance status": "Gold"})
t = K.run(f"BIZFLIGHTS cheapest business class direct flight from Tokyo to Singapore on {DATE} DATE={DATE}")
K.trace(t)
names = K.names(t)
fs = [x for x in K.calls(t) if x.get("name") == "flights_search"]
kl = c.get(FAKE + "/kiwi-mcp/_log").json()
K.check("business class + direct + Tokyo: ONE flights_search (city code TYO, cabin business, max_stops 0) beside web_search, no plan",
        names == ["load_skill", "flights_search", "web_search"] and len(fs) == 1 and fs[0]["args"]["cabin"] == "business" and fs[0]["args"]["max_stops"] == 0
        and fs[0]["args"]["origin"] == "TYO" and kl[-1]["flyFrom"] == "Tokyo" and kl[-1]["cabinClass"] == "BUSINESS" and not any(e["type"] == "plan" for e in t["events"]),
        (names, [f["args"] for f in fs], kl[-1:]))
K.check("the answer is in the cabin searched", t["status"] == "COMPLETED" and "BUSINESS CLASS" in (t.get("result") or "") and "searched as business" in (t.get("result") or ""), (t.get("result") or "")[:300])
K.check("the preferred alliance was passed to the search; SQ (a Star Alliance member) came back flagged and the answer says preferred", fs[0]["args"].get("airlines") == ["Star Alliance"]
        and "your preferred airline" in (t.get("result") or ""), (fs[0]["args"], (t.get("result") or "")[-200:]))
sysp = [x["system"] for x in c.get(LLM + "/calls_for", params={"marker": "BIZFLIGHTS"}).json()]
K.check("the executor's instructions carry the standing preferences (the alliance, the loyalty custom field) and nothing else from the profile",
        sysp and "Standing preferences" in sysp[0] and "- Preferred airline: Star Alliance" in sysp[0] and "- Star Alliance status: Gold" in sysp[0]
        and "Shoe size" not in sysp[0] and "nothing is dropped" in sysp[0], (sysp[0][sysp[0].find("Standing"):][:300] if sysp else None))

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
