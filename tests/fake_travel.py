"""Fake travel data provider + booking site for the travel e2e suites (port 8097).

* Streamable-HTTP MCP server at /mcp (requires `Authorization: Bearer test-travel-token`; same wire subset as
  tests/fake_mcp.py: initialize / tools/list / tools/call answered as SSE, Mcp-Session-Id issued). Six read-only tools:
  flights_search, flight_offer, hotels_search, hotel_rooms, michelin_search, restaurant_availability. All data is
  generated deterministically from the request (route, dates, ids), so the same question always gets the same answer.
* Booking pages (what the agent's browser opens, as http://shop.test:8097/book/...):
  GET  /book/flight/<offer_id>            traveller + passport + card form, "Pay now"
  GET  /book/hotel/<room_offer_id|hotel_id> guest form (+ card when prepaid), "Confirm booking"
  GET  /book/restaurant/<slot_id|restaurant_id> guest form (+ card for a deposit), "Confirm booking"
  POST the same URL -> confirmation page with a booking reference (or the form again with what is missing).
* /_log: tool calls, page views and bookings. POST /_reset clears them.

Only FastAPI (no python-multipart): form bodies are parsed with urllib.
"""
from __future__ import annotations

import asyncio
import datetime as dt
import hashlib
import html
import json
import os
import re
import time
import uuid
from urllib.parse import parse_qs

from fastapi import FastAPI, Request
from fastapi.responses import HTMLResponse, JSONResponse, Response, StreamingResponse

app = FastAPI()
TOKEN = "Bearer test-travel-token"
BASE = os.environ.get("TRAVEL_PUBLIC_BASE", "http://shop.test:8097").rstrip("/")
LOG: dict[str, list] = {"calls": [], "views": [], "bookings": [], "rejected": []}

# ------------------------------------------------------------------ reference data
CITIES = {
    "TYO": {"name": "Tokyo", "tz": 9, "airports": ["HND", "NRT"], "cur": "JPY", "sym": "¥", "rate": 112.0,
            "alias": ["tokyo", "东京", "東京", "tyo", "hnd", "nrt", "haneda", "narita"]},
    "OSA": {"name": "Osaka", "tz": 9, "airports": ["KIX", "ITM"], "cur": "JPY", "sym": "¥", "rate": 112.0,
            "alias": ["osaka", "大阪", "osa", "kix", "itm", "kansai"]},
    "SEL": {"name": "Seoul", "tz": 9, "airports": ["ICN", "GMP"], "cur": "KRW", "sym": "₩", "rate": 1030.0,
            "alias": ["seoul", "首尔", "首爾", "서울", "sel", "icn", "gmp", "incheon", "gimpo"]},
    "SIN": {"name": "Singapore", "tz": 8, "airports": ["SIN"], "cur": "SGD", "sym": "S$", "rate": 1.0,
            "alias": ["singapore", "新加坡", "sin", "changi"]},
    "HKG": {"name": "Hong Kong", "tz": 8, "airports": ["HKG"], "cur": "HKD", "sym": "HK$", "rate": 5.9,
            "alias": ["hong kong", "hongkong", "香港", "hkg", "hk"]},
    "BKK": {"name": "Bangkok", "tz": 7, "airports": ["BKK", "DMK"], "cur": "THB", "sym": "฿", "rate": 25.5,
            "alias": ["bangkok", "曼谷", "bkk", "dmk", "suvarnabhumi", "don mueang"]},
    "TPE": {"name": "Taipei", "tz": 8, "airports": ["TPE", "TSA"], "cur": "TWD", "sym": "NT$", "rate": 24.5,
            "alias": ["taipei", "台北", "臺北", "tpe", "tsa", "taoyuan", "songshan"]},
    "SHA": {"name": "Shanghai", "tz": 8, "airports": ["PVG", "SHA"], "cur": "CNY", "sym": "¥", "rate": 5.5,
            "alias": ["shanghai", "上海", "pvg", "sha", "pudong", "hongqiao"]},
}
AIRPORT_CITY = {a: c for c, v in CITIES.items() for a in v["airports"]}
HOURS = {("SIN", "TYO"): 7.0, ("SIN", "OSA"): 6.4, ("SIN", "SEL"): 6.4, ("SIN", "HKG"): 3.9, ("SIN", "BKK"): 2.4,
         ("SIN", "TPE"): 4.6, ("SIN", "SHA"): 5.6, ("TYO", "OSA"): 1.3, ("TYO", "SEL"): 2.6, ("TYO", "HKG"): 4.9,
         ("TYO", "BKK"): 6.6, ("TYO", "TPE"): 3.9, ("TYO", "SHA"): 3.1, ("OSA", "SEL"): 1.9, ("OSA", "HKG"): 4.3,
         ("OSA", "BKK"): 6.1, ("OSA", "TPE"): 3.1, ("OSA", "SHA"): 2.4, ("SEL", "HKG"): 3.8, ("SEL", "BKK"): 5.8,
         ("SEL", "TPE"): 2.6, ("SEL", "SHA"): 2.1, ("HKG", "BKK"): 2.9, ("HKG", "TPE"): 1.8, ("HKG", "SHA"): 2.6,
         ("BKK", "TPE"): 3.9, ("BKK", "SHA"): 4.4, ("TPE", "SHA"): 1.9}
# (code, airline, full service?, preferred airport index at its home city)
CARRIERS = {
    "SIN": [("SQ", "Singapore Airlines", True), ("TR", "Scoot", False)],
    "TYO": [("JL", "Japan Airlines", True), ("NH", "ANA", True), ("ZG", "ZIPAIR", False)],
    "OSA": [("MM", "Peach", False)],
    "SEL": [("KE", "Korean Air", True), ("OZ", "Asiana Airlines", True), ("7C", "Jeju Air", False)],
    "HKG": [("CX", "Cathay Pacific", True), ("UO", "HK Express", False)],
    "BKK": [("TG", "Thai Airways", True), ("FD", "Thai AirAsia", False)],
    "TPE": [("BR", "EVA Air", True), ("CI", "China Airlines", True), ("JX", "Starlux", True)],
    "SHA": [("MU", "China Eastern", True), ("9C", "Spring Airlines", False)],
}
HUBS = [("HKG", "CX", "Cathay Pacific"), ("TPE", "BR", "EVA Air"), ("BKK", "TG", "Thai Airways"), ("SEL", "KE", "Korean Air")]

HOTELS = {
    "TYO": [("Ginza Kaede Hotel", "Ginza", 4, 8.9, "Ginza-itchome Station, 3 min walk"),
            ("Hotel Sakura Shinjuku", "Shinjuku", 3, 8.4, "Shinjuku-sanchome Station, 4 min walk"),
            ("The Marunouchi Grand", "Marunouchi / Tokyo Station", 5, 9.3, "Tokyo Station, 2 min walk"),
            ("Asakusa River Inn", "Asakusa", 3, 8.6, "Asakusa Station, 5 min walk"),
            ("Shibuya Stream Hotel Annex", "Shibuya", 4, 8.8, "Shibuya Station, 6 min walk"),
            ("Toranomon Sky Residence", "Toranomon", 5, 9.1, "Toranomon Hills Station, 1 min walk")],
    "OSA": [("Namba Lantern Hotel", "Namba", 3, 8.5, "Namba Station, 4 min walk"),
            ("Umeda Garden Tower Hotel", "Umeda", 4, 8.9, "Osaka Station, 5 min walk"),
            ("Nakanoshima Riverside Hotel", "Kitahama", 5, 9.2, "Kitahama Station, 3 min walk"),
            ("Shinsaibashi Arcade Inn", "Shinsaibashi", 3, 8.2, "Shinsaibashi Station, 2 min walk"),
            ("Tennoji Park Hotel", "Tennoji", 4, 8.6, "Tennoji Station, 3 min walk")],
    "SEL": [("Myeongdong Hanok Stay", "Myeongdong", 3, 8.7, "Myeongdong Station, 3 min walk"),
            ("Gangnam Lumiere Hotel", "Gangnam", 4, 8.8, "Gangnam Station, 5 min walk"),
            ("Han River Grand Seoul", "Yeouido", 5, 9.2, "Yeouinaru Station, 4 min walk"),
            ("Insadong Courtyard Hotel", "Insadong", 4, 8.9, "Anguk Station, 3 min walk"),
            ("Hongdae Indie Hotel", "Hongdae", 3, 8.3, "Hongik University Station, 2 min walk")],
    "SIN": [("Marina Bayfront Hotel", "Marina Bay", 5, 9.1, "Bayfront MRT, 3 min walk"),
            ("Tiong Bahru Heritage Hotel", "Tiong Bahru", 4, 8.8, "Tiong Bahru MRT, 6 min walk"),
            ("Bugis Junction Inn", "Bugis", 3, 8.1, "Bugis MRT, 2 min walk"),
            ("Orchard Gardens Hotel", "Orchard", 4, 8.7, "Orchard MRT, 5 min walk"),
            ("Sentosa Cove Resort", "Sentosa", 5, 9.0, "Sentosa Express Beach Station, 8 min walk")],
    "HKG": [("Victoria Harbour Hotel", "Tsim Sha Tsui", 5, 9.2, "East Tsim Sha Tsui Station, 3 min walk"),
            ("Central Peak Hotel", "Central", 4, 8.8, "Central Station, 4 min walk"),
            ("Causeway Bay Lantern Hotel", "Causeway Bay", 3, 8.3, "Causeway Bay Station, 2 min walk"),
            ("Sheung Wan Tea House Hotel", "Sheung Wan", 4, 8.9, "Sheung Wan Station, 3 min walk"),
            ("Mong Kok Night Market Inn", "Mong Kok", 3, 8.0, "Mong Kok Station, 2 min walk")],
    "BKK": [("Chao Phraya Riverside Hotel", "Riverside / Charoen Krung", 5, 9.3, "Saphan Taksin BTS, 6 min walk"),
            ("Sukhumvit Orchid Hotel", "Sukhumvit Soi 11", 4, 8.7, "Nana BTS, 5 min walk"),
            ("Silom Garden Hotel", "Silom", 4, 8.6, "Sala Daeng BTS, 4 min walk"),
            ("Ari Courtyard Boutique", "Ari", 3, 8.9, "Ari BTS, 3 min walk"),
            ("Siam Square Loft", "Siam", 3, 8.4, "Siam BTS, 2 min walk")],
    "TPE": [("Xinyi Skyline Hotel", "Xinyi", 5, 9.1, "Taipei 101 Station, 4 min walk"),
            ("Daan Park Hotel", "Daan", 4, 8.8, "Daan Park Station, 3 min walk"),
            ("Ximending Suites", "Ximending", 3, 8.2, "Ximen Station, 2 min walk"),
            ("Zhongshan Heritage Hotel", "Zhongshan", 4, 8.9, "Zhongshan Station, 4 min walk"),
            ("Beitou Hot Spring Resort", "Beitou", 4, 9.0, "Xinbeitou Station, 6 min walk")],
    "SHA": [("The Bund Riverview Hotel", "The Bund", 5, 9.2, "East Nanjing Road Station, 7 min walk"),
            ("Jingan Garden Hotel", "Jingan", 4, 8.8, "Jingan Temple Station, 3 min walk"),
            ("Xintiandi Lane House Hotel", "Xintiandi", 4, 9.0, "Xintiandi Station, 4 min walk"),
            ("Lujiazui Tower Hotel", "Pudong / Lujiazui", 5, 9.1, "Lujiazui Station, 5 min walk"),
            ("Yuyuan Old Town Inn", "Huangpu / Yu Garden", 3, 8.3, "Yuyuan Garden Station, 3 min walk")],
}
CITY_PRICE = {"TYO": 1.15, "OSA": 0.95, "SEL": 0.9, "SIN": 1.25, "HKG": 1.2, "BKK": 0.6, "TPE": 0.8, "SHA": 0.85}
STAR_PRICE = {3: 135, 4: 235, 5: 470}

# name, distinction, cuisine, area, price per person (local currency, low-high), closed days (0=Mon), seatings
RESTAURANTS = {
    "TYO": [("Sushi Hoshizora", "3 Stars", "Sushi", "Ginza", (45000, 60000), [6], ["18:00", "20:30"]),
            ("Kappo Yamabuki", "2 Stars", "Kaiseki", "Akasaka", (35000, 45000), [], None),
            ("Tempura Ishigaki", "1 Star", "Tempura", "Nihonbashi", (18000, 26000), [2], None),
            ("L'Atelier Minato", "1 Star", "French", "Roppongi", (22000, 32000), [0], None),
            ("Ramen Tsukimi", "Bib Gourmand", "Ramen", "Shibuya", (1200, 1800), [], None),
            ("Soba Kotobuki", "Bib Gourmand", "Soba", "Kanda", (1500, 3000), [6], None)],
    "OSA": [("Kitashinchi Hana", "2 Stars", "Kaiseki", "Kitashinchi", (30000, 40000), [6], None),
            ("Sushi Naniwa", "1 Star", "Sushi", "Fukushima", (20000, 28000), [2], ["17:30", "20:00"]),
            ("Kani Tsuru", "1 Star", "Crab kaiseki", "Dotonbori", (15000, 22000), [], None),
            ("Okonomiyaki Hanabi", "Bib Gourmand", "Okonomiyaki", "Namba", (1500, 2500), [1], None)],
    "SEL": [("Gaon Hall", "3 Stars", "Korean", "Gangnam", (330000, 450000), [6], None),
            ("Hansik Bom", "2 Stars", "Contemporary Korean", "Cheongdam", (220000, 280000), [0], None),
            ("Soban Seoul", "1 Star", "Korean", "Jongno", (120000, 160000), [], None),
            ("Myeongdong Kalguksu House", "Bib Gourmand", "Noodles", "Myeongdong", (11000, 15000), [], None)],
    "SIN": [("Maison Orchidee", "3 Stars", "French", "Tanglin", (450, 650), [6, 0], None),
            ("Jade Peony", "2 Stars", "Cantonese", "Marina Bay", (220, 380), [], None),
            ("Saison Marina", "1 Star", "Modern European", "Keppel Bay", (180, 280), [0], None),
            ("Tiong Bahru Lor Mee", "Bib Gourmand", "Hawker - lor mee", "Tiong Bahru", (5, 8), [3], None),
            ("Kampong Glam Nasi Padang", "Bib Gourmand", "Malay", "Kampong Glam", (10, 18), [], None)],
    "HKG": [("Jade Harbour", "3 Stars", "Cantonese", "Central", (1800, 2800), [], None),
            ("Le Comptoir Central", "2 Stars", "French", "Central", (1500, 2200), [6], None),
            ("Kowloon Kitchen Ming", "1 Star", "Cantonese", "Tsim Sha Tsui", (700, 1100), [], None),
            ("Dim Sum Hui", "Bib Gourmand", "Dim sum", "Sham Shui Po", (80, 160), [], None),
            ("Sheung Wan Roast Goose", "Bib Gourmand", "Cantonese roast meats", "Sheung Wan", (90, 180), [1], None)],
    "BKK": [("Baan Saeng", "2 Stars", "Southern Thai", "Sathorn", (4500, 6500), [0], None),
            ("Yaowarat Crab Omelette", "1 Star", "Thai street food", "Yaowarat", (1000, 2000), [1, 2], None),
            ("Riverside Prik Thai", "1 Star", "Thai", "Charoen Krung", (2800, 4200), [], None),
            ("Ari Boat Noodles", "Bib Gourmand", "Noodles", "Ari", (80, 200), [], None)],
    "TPE": [("Taipei Jade Pavilion", "3 Stars", "Cantonese", "Zhongshan", (6000, 9000), [], None),
            ("Fu Rong", "2 Stars", "Taiwanese", "Daan", (4800, 6800), [0], None),
            ("Kiln Taipei", "1 Star", "Contemporary", "Xinyi", (3800, 5200), [1], None),
            ("Dongmen Beef Noodle", "Bib Gourmand", "Beef noodles", "Dongmen", (200, 350), [], None),
            ("Ningxia Oyster Omelette", "Bib Gourmand", "Night market", "Datong", (80, 150), [], None)],
    "SHA": [("Bund Jade Garden", "3 Stars", "Cantonese", "The Bund", (1500, 2500), [], None),
            ("Lao Jing Shanghainese", "2 Stars", "Shanghainese", "Jingan", (900, 1400), [0], None),
            ("Shikumen Table", "1 Star", "Contemporary Chinese", "Xintiandi", (700, 1100), [], None),
            ("Xiaolongbao Fang", "Bib Gourmand", "Dumplings", "Huangpu", (60, 120), [], None),
            ("Huaihai Noodle Shop", "Bib Gourmand", "Noodles", "Xuhui", (40, 90), [], None)],
}
WEEKDAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]


class ToolError(Exception):
    pass


def _rnd(*key) -> float:
    """Deterministic pseudo-random number in [0, 1) from the key."""
    h = hashlib.sha256("|".join(str(k) for k in key).encode()).digest()
    return int.from_bytes(h[:8], "big") / 2 ** 64


def _city(q) -> str:
    s = str(q or "").strip().lower()
    if not s:
        raise ToolError("city / airport is required")
    if s.upper() in CITIES:
        return s.upper()
    if s.upper() in AIRPORT_CITY:
        return AIRPORT_CITY[s.upper()]
    for code, c in CITIES.items():
        if any(a == s or (len(a) > 3 and a in s) for a in c["alias"]):
            return code
    raise ToolError(f"unknown city or airport '{q}'. Covered: " + ", ".join(f"{c['name']} ({'/'.join(c['airports'])})" for c in CITIES.values()))


def _date(s, what="date") -> dt.date:
    try:
        d = dt.date.fromisoformat(str(s or "").strip()[:10])
    except ValueError:
        raise ToolError(f"{what} must be YYYY-MM-DD (got '{s}')")
    if d < dt.date.today():
        raise ToolError(f"{what} {d} is in the past")
    if d > dt.date.today() + dt.timedelta(days=330):
        raise ToolError(f"{what} {d} is more than 330 days ahead; schedules are not loaded yet")
    return d


def _money(v: float, cur: str) -> str:
    return f"{cur} {v:,.0f}" if cur in ("JPY", "KRW", "TWD", "THB") else f"{cur} {v:,.2f}"


def _hm(minutes: int) -> str:
    return f"{minutes // 60}h {minutes % 60:02d}m"


# ------------------------------------------------------------------ flights
def _route_offers(o: str, d: str, day: dt.date) -> list[dict]:
    if o == d:
        raise ToolError("origin and destination are the same city")
    hours = HOURS.get((o, d)) or HOURS.get((d, o))
    out = []
    carriers = [(c, n, fs, o) for c, n, fs in CARRIERS[o]] + [(c, n, fs, d) for c, n, fs in CARRIERS[d]]
    for i, (code, name, full, home) in enumerate(carriers[:5]):
        dep_min = 6 * 60 + int(_rnd(o, d, code, "dep") * 17 * 12) * 5    # 06:00 .. 23:55
        out.append(_offer(o, d, day, len(out) + 1, code, name, full, hours, dep_min, None, home))
    for hub, code, name in HUBS:
        if hub in (o, d) or any(c[0] == code for c in carriers):
            continue
        leg1 = HOURS.get((o, hub)) or HOURS.get((hub, o))
        leg2 = HOURS.get((hub, d)) or HOURS.get((d, hub))
        dep_min = 7 * 60 + int(_rnd(o, d, hub, "dep") * 14 * 12) * 5
        out.append(_offer(o, d, day, len(out) + 1, code, name, True, leg1 + leg2 + 1.5 + _rnd(o, d, hub) * 2, dep_min, hub, hub))
        if len(out) >= 7:
            break
    return out


def _offer(o, d, day, idx, code, airline, full, hours, dep_min, via, home) -> dict:
    co, cd = CITIES[o], CITIES[d]
    ao = co["airports"][int(_rnd(code, o, "ap") * len(co["airports"]))] if code not in ("SQ", "TR") or o != "SIN" else "SIN"
    ad = cd["airports"][int(_rnd(code, d, "ap") * len(cd["airports"]))]
    if code == "ZG":
        ao, ad = (ao if o != "TYO" else "NRT"), (ad if d != "TYO" else "NRT")
    if code == "MM":
        ao, ad = (ao if o != "OSA" else "KIX"), (ad if d != "OSA" else "KIX")
    dur = int(round((hours * 60 + (_rnd(o, d, code, "dur") - 0.3) * 40 + (10 if CITIES[d]["tz"] < CITIES[o]["tz"] else 0)) / 5.0)) * 5
    dep = dt.datetime.combine(day, dt.time(dep_min // 60, dep_min % 60))
    arr = dep + dt.timedelta(minutes=dur + (cd["tz"] - co["tz"]) * 60)
    plus = (arr.date() - day).days
    base = (110 + 72 * hours) * (1.0 + 0.35 * _rnd(o, d, code, "fare"))
    if not full:
        base *= 0.5 + 0.15 * _rnd(o, d, code, "lcc")
    if via:
        base *= 0.72 + 0.12 * _rnd(o, d, via, "via")
    fare = round(base)
    num = f"{code} {100 + int(_rnd(o, d, code, 'no') * 880)}"
    if full:
        bag = f"{25 if via else 30} kg checked + 7 kg cabin"
        refund = "Refundable with fee SGD 150; changes SGD 80 + fare difference" if _rnd(o, d, code, "rf") > 0.5 \
            else "Non-refundable (taxes refunded); changes SGD 120 + fare difference"
    else:
        bag = "Cabin bag only (7 kg); 20 kg checked bag SGD 48 each way"
        refund = "Non-refundable; changes SGD 60 + fare difference up to 4 h before departure"
    oid = f"F-{ao}-{ad}-{day:%Y%m%d}-{idx}"
    return {"offer_id": oid, "stops": 1 if via else 0, "fare_amount": fare,
            "title": f"{airline} {num}", "airline": airline, "flight_no": num if not via else f"{num} + {code} {300 + idx * 7}",
            "from": f"{co['name']} {ao}", "to": f"{cd['name']} {ad}",
            "schedule": f"{ao} {dep:%H:%M} - {ad} {arr:%H:%M}{f' (+{plus})' if plus else ''}",
            "depart_local": f"{dep:%Y-%m-%d %H:%M} (UTC+{co['tz']})", "arrive_local": f"{arr:%Y-%m-%d %H:%M} (UTC+{cd['tz']})",
            "duration": _hm(dur), "stops_text": "Direct" if not via else f"1 stop via {CITIES[via]['name']} ({CITIES[via]['airports'][0]})",
            "cabin": "Economy", "fare": f"SGD {fare:,.2f}", "fare_per": "per adult, taxes included",
            "baggage": bag, "refund_rules": refund, "currency": "SGD",
            "booking_url": f"{BASE}/book/flight/{oid}"}


def _parse_offer_id(oid: str) -> dict:
    m = re.fullmatch(r"F-([A-Z]{3})-([A-Z]{3})-(\d{8})-(\d+)", str(oid or "").strip())
    if not m:
        raise ToolError(f"unknown offer_id '{oid}' (use an offer_id from flights_search)")
    o, d = AIRPORT_CITY.get(m.group(1)), AIRPORT_CITY.get(m.group(2))
    day = dt.datetime.strptime(m.group(3), "%Y%m%d").date()
    for off in _route_offers(o, d, day) if o and d else []:
        if off["offer_id"] == oid:
            return off
    raise ToolError(f"offer '{oid}' is no longer available — search again")


def flights_search(a: dict) -> dict:
    o, d = _city(a.get("origin")), _city(a.get("destination"))
    day = _date(a.get("date"), "date")
    adults = max(1, min(9, int(a.get("adults") or 1)))
    cabin = str(a.get("cabin") or "economy").lower().replace(" ", "_")
    mult = {"economy": 1.0, "premium_economy": 1.7, "business": 3.4}.get(cabin, 1.0)
    offers = []
    for off in _route_offers(o, d, day):
        off = dict(off)
        if mult != 1.0:
            if "Cabin bag only" in off["baggage"]:
                continue                                         # low-cost carriers: no premium cabins here
            off["fare_amount"] = round(off["fare_amount"] * mult)
            off["fare"] = f"SGD {off['fare_amount']:,.2f}"
            off["cabin"] = cabin.replace("_", " ").title()
            off["offer_id"] = off["offer_id"] + {"premium_economy": "P", "business": "J"}[cabin]
            off["booking_url"] = f"{BASE}/book/flight/{off['offer_id']}"
        off["total_for_all_adults"] = f"SGD {off['fare_amount'] * adults:,.2f}"
        offers.append(off)
    want_o = str(a.get("origin") or "").strip().upper()
    want_d = str(a.get("destination") or "").strip().upper()
    if want_o in AIRPORT_CITY:
        offers = [x for x in offers if x["from"].endswith(want_o)] or offers
    if want_d in AIRPORT_CITY:
        offers = [x for x in offers if x["to"].endswith(want_d)] or offers
    if a.get("nonstop_only"):
        offers = [x for x in offers if x["stops"] == 0]
    if a.get("max_price"):
        offers = [x for x in offers if x["fare_amount"] <= float(a["max_price"])]
    key = str(a.get("sort") or "price")
    offers.sort(key=lambda x: (x["depart_local"] if key == "departure" else
                               int(x["duration"].split("h")[0]) * 60 + int(x["duration"].split()[1][:2]) if key == "duration"
                               else x["fare_amount"]))
    return {"route": f"{CITIES[o]['name']} -> {CITIES[d]['name']}", "date": str(day), "adults": adults,
            "times": "All times are local to each airport.", "prices": "SGD per adult incl. taxes, checked just now",
            "count": len(offers), "offers": offers}


def _with_cabin(oid: str) -> dict:
    cab = oid[-1] if oid and oid[-1] in "PJ" else ""
    off = dict(_parse_offer_id(oid[:-1] if cab else oid))
    if cab:
        mult = 1.7 if cab == "P" else 3.4
        off["fare_amount"] = round(off["fare_amount"] * mult)
        off["fare"] = f"SGD {off['fare_amount']:,.2f}"
        off["cabin"] = "Premium Economy" if cab == "P" else "Business"
        off["offer_id"] = oid
        off["booking_url"] = f"{BASE}/book/flight/{oid}"
    return off


def flight_offer(a: dict) -> dict:
    off = _with_cabin(str(a.get("offer_id") or "").strip())
    taxes = round(off["fare_amount"] * 0.18)
    return {**off, "fare_breakdown": {"base_fare": f"SGD {off['fare_amount'] - taxes:,.2f}",
                                      "taxes_and_fees": f"SGD {taxes:,.2f}", "total_per_adult": off["fare"]},
            "seat_selection": "Standard seats free at online check-in (48 h before departure); preferred seats from SGD 25",
            "passport_rule": "Passport valid for the whole stay is required at booking (name exactly as in passport)",
            "price_guarantee": "This fare is held for 30 minutes after this check",
            "booking_needs": ["traveller full name as in passport", "email", "phone", "passport number", "card payment"]}


# ------------------------------------------------------------------ hotels
def _nights(a) -> tuple[dt.date, dt.date, int]:
    ci, co = _date(a.get("check_in"), "check_in"), _date(a.get("check_out"), "check_out")
    n = (co - ci).days
    if n <= 0 or n > 30:
        raise ToolError("check_out must be 1-30 nights after check_in")
    return ci, co, n


def _hotel(code: str, idx: int) -> dict:
    name, area, stars, rating, station = HOTELS[code][idx - 1]
    nightly = round(STAR_PRICE[stars] * CITY_PRICE[code] * (0.85 + 0.35 * _rnd(code, idx, "p")))
    return {"hotel_id": f"H-{code}-{idx}", "name": name, "city": CITIES[code]["name"], "area": area, "stars": stars,
            "rating": f"{rating}/10", "reviews": 400 + int(_rnd(code, idx, "r") * 4200), "nearest_station": station,
            "_nightly": nightly}


def _rooms(h: dict, ci: dt.date, co: dt.date, guests: int) -> list[dict]:
    n = (co - ci).days
    base = h["_nightly"]
    out = []
    specs = [("Standard Double", "1 double bed, 18 m2", 0.9, False, "Room only", None),
             ("Superior Twin", "2 single beds, 24 m2", 1.0, True, "Room only", 3),
             ("Deluxe King with breakfast", "1 king bed, 32 m2", 1.35, True, "Breakfast for 2 included", 1)]
    for i, (rtype, bed, f, refundable, meal, days) in enumerate(specs, 1):
        nightly = round(base * f * (1 + 0.1 * max(0, guests - 2)))
        if refundable:
            until = ci - dt.timedelta(days=days)
            cancel = f"Free cancellation until {until} 23:59 hotel local time; after that the first night is charged"
            pay = "Pay at the hotel (no prepayment)"
        else:
            cancel = "Non-refundable: no changes or refund after booking"
            pay = "Prepay now by card"
        rid = f"{h['hotel_id']}-{ci:%Y%m%d}-{co:%Y%m%d}-{guests}-R{i}"
        out.append({"room_offer_id": rid, "room_type": rtype, "beds": bed, "meals": meal, "max_guests": 3,
                    "price_per_night": f"SGD {nightly:,.2f}", "nights": n, "total_amount": nightly * n,
                    "total": f"SGD {nightly * n:,.2f} for {n} nights, taxes included", "cancellation": cancel,
                    "payment": pay, "booking_url": f"{BASE}/book/hotel/{rid}"})
    return out


def _hotel_by_id(hid: str) -> dict:
    m = re.fullmatch(r"H-([A-Z]{3})-(\d+)", str(hid or "").strip())
    if not m or m.group(1) not in HOTELS or not 1 <= int(m.group(2)) <= len(HOTELS[m.group(1)]):
        raise ToolError(f"unknown hotel_id '{hid}' (use a hotel_id from hotels_search)")
    return _hotel(m.group(1), int(m.group(2)))


def hotels_search(a: dict) -> dict:
    code = _city(a.get("city"))
    ci, co, n = _nights(a)
    guests = max(1, min(6, int(a.get("guests") or 2)))
    out = []
    for i in range(1, len(HOTELS[code]) + 1):
        h = _hotel(code, i)
        rooms = _rooms(h, ci, co, guests)
        cheapest = min(rooms, key=lambda r: r["total_amount"])
        free = next(r for r in rooms if r["cancellation"].startswith("Free"))
        row = {k: v for k, v in h.items() if not k.startswith("_")}
        row.update({"from_price_per_night": cheapest["price_per_night"], "from_total": cheapest["total"],
                    "free_cancellation_from": free["price_per_night"] + " per night",
                    "booking_url": f"{BASE}/book/hotel/{h['hotel_id']}"})
        row["_n"] = int(cheapest["price_per_night"].split()[1].replace(",", "").split(".")[0])
        out.append(row)
    if a.get("min_stars"):
        out = [x for x in out if x["stars"] >= int(a["min_stars"])]
    if a.get("max_price_per_night"):
        out = [x for x in out if x["_n"] <= float(a["max_price_per_night"])]
    if a.get("area"):
        q = str(a["area"]).lower()
        out = [x for x in out if q in x["area"].lower()] or out
    out.sort(key=lambda x: x["_n"])
    for x in out:
        x.pop("_n", None)
    return {"city": CITIES[code]["name"], "check_in": str(ci), "check_out": str(co), "nights": n, "guests": guests,
            "prices": "SGD, taxes included; use hotel_rooms for room types, cancellation and payment terms",
            "count": len(out), "hotels": out}


def hotel_rooms(a: dict) -> dict:
    h = _hotel_by_id(a.get("hotel_id"))
    ci, co, n = _nights(a)
    guests = max(1, min(6, int(a.get("guests") or 2)))
    return {"hotel": {k: v for k, v in h.items() if not k.startswith("_")}, "check_in": str(ci), "check_out": str(co),
            "nights": n, "guests": guests, "check_in_time": "from 15:00", "check_out_time": "until 11:00",
            "rooms": _rooms(h, ci, co, guests)}


def _room_by_id(rid: str) -> tuple[dict, dict]:
    m = re.fullmatch(r"(H-[A-Z]{3}-\d+)-(\d{8})-(\d{8})-(\d)-R(\d)", str(rid or "").strip())
    if not m:
        raise ToolError(f"unknown room_offer_id '{rid}'")
    h = _hotel_by_id(m.group(1))
    ci = dt.datetime.strptime(m.group(2), "%Y%m%d").date()
    co = dt.datetime.strptime(m.group(3), "%Y%m%d").date()
    rooms = _rooms(h, ci, co, int(m.group(4)))
    return h, rooms[int(m.group(5)) - 1]


# ------------------------------------------------------------------ Michelin restaurants
def _rest(code: str, idx: int) -> dict:
    name, dist, cuisine, area, (lo, hi), closed, seatings = RESTAURANTS[code][idx - 1]
    c = CITIES[code]
    if dist in ("3 Stars", "2 Stars"):
        dep = round(lo * 0.4, -3 if lo >= 10000 else (-1 if lo >= 100 else 0))
        deposit = f"{_money(dep, c['cur'])} per person charged by card at booking; refunded if cancelled 72 h before"
        dep_pp = dep
    elif dist == "1 Star":
        deposit = f"Card guarantee only; no-show or cancellation within 24 h: {_money(round(lo * 0.5), c['cur'])} per person"
        dep_pp = 0
    else:
        deposit = "No deposit"
        dep_pp = 0
    return {"restaurant_id": f"R-{code}-{idx}", "name": name, "city": c["name"], "area": area,
            "distinction": dist if dist != "Bib Gourmand" else "Bib Gourmand (good value, not a star)",
            "cuisine": cuisine, "price_range": f"{c['sym']}{lo:,} - {c['sym']}{hi:,} per person ({c['cur']})",
            "closed": ", ".join(WEEKDAYS[d] for d in closed) or "open daily",
            "deposit_policy": deposit, "dress_code": "Smart casual" if dist != "Bib Gourmand" else "Casual",
            "booking_url": f"{BASE}/book/restaurant/R-{code}-{idx}",
            "_dep": dep_pp, "_closed": closed, "_seatings": seatings, "_cur": c["cur"], "_sym": c["sym"]}


def _rest_by_id(rid: str) -> dict:
    m = re.fullmatch(r"R-([A-Z]{3})-(\d+)", str(rid or "").strip())
    if not m or m.group(1) not in RESTAURANTS or not 1 <= int(m.group(2)) <= len(RESTAURANTS[m.group(1)]):
        raise ToolError(f"unknown restaurant_id '{rid}' (use a restaurant_id from michelin_search)")
    return _rest(m.group(1), int(m.group(2)))


def _public(r: dict) -> dict:
    return {k: v for k, v in r.items() if not k.startswith("_")}


def michelin_search(a: dict) -> dict:
    code = _city(a.get("city"))
    want = str(a.get("distinction") or "any").lower()
    out = []
    for i in range(1, len(RESTAURANTS[code]) + 1):
        r = _rest(code, i)
        d = r["distinction"].lower()
        if want not in ("", "any", "all"):
            if "bib" in want and "bib" not in d:
                continue
            if "star" in want and "bib" in d:
                continue
            m = re.match(r"(\d)", want)
            if m and not d.startswith(m.group(1)):
                continue
        q = str(a.get("cuisine") or "").strip().lower()
        if q and q not in ("any", "all", "*") and q not in (r["cuisine"] + " " + _family(r["cuisine"], code)).lower():
            continue
        out.append(_public(r))
    res = {"city": CITIES[code]["name"], "guide": "MICHELIN Guide 2026 selection (stars and Bib Gourmand)",
           "count": len(out), "restaurants": out,
           "next": "use restaurant_availability(restaurant_id, date, party_size) for bookable times"}
    if not out:
        res["note"] = ("no match for these filters; in " + CITIES[code]["name"] + " the guide lists: " +
                       "; ".join(f"{x[0]} ({x[1]}, {x[2]})" for x in RESTAURANTS[code]))
    return res


def _family(cuisine: str, code: str) -> str:
    """Broad cuisine family, so 'Japanese' finds kaiseki and 'Chinese' finds Cantonese."""
    c = cuisine.lower()
    for fam, words in (("Japanese", ("sushi", "kaiseki", "tempura", "ramen", "soba", "okonomiyaki")),
                       ("Chinese", ("cantonese", "shanghainese", "dim sum", "dumpling", "chinese")),
                       ("Korean", ("korean",)), ("Thai", ("thai",)), ("French European", ("french", "european"))):
        if any(w in c for w in words):
            return fam
    return {"TYO": "Japanese", "OSA": "Japanese", "SEL": "Korean", "BKK": "Thai", "TPE": "Taiwanese",
            "SHA": "Chinese", "HKG": "Chinese", "SIN": "Singaporean"}.get(code, "") if c in ("noodles", "contemporary", "night market") else ""


def _slots(r: dict, day: dt.date, party: int) -> list[str]:
    if day.weekday() in r["_closed"]:
        return []
    times = r["_seatings"] or ["17:30", "18:00", "18:30", "19:00", "19:30", "20:00", "20:30", "21:00"]
    return [t for t in times if _rnd(r["restaurant_id"], day, t, party) > (0.25 if party <= 2 else 0.45)]


def restaurant_availability(a: dict) -> dict:
    r = _rest_by_id(a.get("restaurant_id"))
    day = _date(a.get("date"), "date")
    party = max(1, int(a.get("party_size") or 2))
    if party > 8:
        raise ToolError("parties above 8 must contact the restaurant directly")
    out = {"restaurant": _public(r), "date": str(day), "weekday": WEEKDAYS[day.weekday()], "party_size": party,
           "times": "local time at the restaurant"}
    slots = _slots(r, day, party)
    if not slots:
        nxt = []
        d2 = day
        while len(nxt) < 3:
            d2 += dt.timedelta(days=1)
            if _slots(r, d2, party):
                nxt.append(f"{d2} ({WEEKDAYS[d2.weekday()]}): {', '.join(_slots(r, d2, party))}")
        out.update({"available": False, "reason": "closed that day" if day.weekday() in r["_closed"] else "fully booked",
                    "next_available": nxt})
        return out
    dep_total = r["_dep"] * party
    out["available"] = True
    out["slots"] = [{"time": t, "slot_id": f"{r['restaurant_id']}-{day:%Y%m%d}-{t.replace(':', '')}-{party}",
                     "booking_url": f"{BASE}/book/restaurant/{r['restaurant_id']}-{day:%Y%m%d}-{t.replace(':', '')}-{party}"}
                    for t in slots]
    out["deposit_due_now"] = _money(dep_total, r["_cur"]) if dep_total else "none"
    out["seating_limit"] = "2 hours" if "Bib" not in r["distinction"] else "90 minutes"
    return out


def _slot_by_id(sid: str) -> tuple[dict, dt.date, str, int]:
    m = re.fullmatch(r"(R-[A-Z]{3}-\d+)-(\d{8})-(\d{4})-(\d+)", str(sid or "").strip())
    if not m:
        raise ToolError(f"unknown slot '{sid}'")
    r = _rest_by_id(m.group(1))
    day = dt.datetime.strptime(m.group(2), "%Y%m%d").date()
    t = m.group(3)[:2] + ":" + m.group(3)[2:]
    return r, day, t, int(m.group(4))


# ------------------------------------------------------------------ MCP
OBJ = "object"
TOOLS = [
    {"name": "flights_search",
     "description": "Search flights between Asian cities (Tokyo, Osaka, Seoul, Singapore, Hong Kong, Bangkok, Taipei, Shanghai). "
                    "Returns offers with airline, flight number, local departure/arrival times, duration, stops, fare in SGD "
                    "incl. taxes, baggage, change/refund rules and a booking_url.",
     "inputSchema": {"type": OBJ, "properties": {
         "origin": {"type": "string", "description": "city name or IATA code, e.g. Singapore / SIN"},
         "destination": {"type": "string", "description": "city name or IATA code, e.g. Tokyo / HND / TYO"},
         "date": {"type": "string", "description": "departure date YYYY-MM-DD (local date at the origin)"},
         "adults": {"type": "integer", "minimum": 1, "maximum": 9},
         "cabin": {"type": "string", "enum": ["economy", "premium_economy", "business"]},
         "nonstop_only": {"type": "boolean"},
         "max_price": {"type": "number", "description": "max fare per adult in SGD"},
         "sort": {"type": "string", "enum": ["price", "departure", "duration"]}},
         "required": ["origin", "destination", "date"]},
     "annotations": {"readOnlyHint": True, "title": "Search flights"}},
    {"name": "flight_offer",
     "description": "Full details of one flight offer: fare breakdown, baggage, change/refund rules, seat selection, what the "
                    "booking needs, and its booking_url.",
     "inputSchema": {"type": OBJ, "properties": {"offer_id": {"type": "string"}}, "required": ["offer_id"]},
     "annotations": {"readOnlyHint": True, "title": "Flight offer details"}},
    {"name": "hotels_search",
     "description": "Search hotels in an Asian city for given dates: name, area, stars, guest rating, nearest station, "
                    "price per night and total in SGD, and a booking_url.",
     "inputSchema": {"type": OBJ, "properties": {
         "city": {"type": "string"}, "check_in": {"type": "string", "description": "YYYY-MM-DD"},
         "check_out": {"type": "string", "description": "YYYY-MM-DD"}, "guests": {"type": "integer", "minimum": 1},
         "min_stars": {"type": "integer", "minimum": 1, "maximum": 5},
         "max_price_per_night": {"type": "number", "description": "SGD"}, "area": {"type": "string"}},
         "required": ["city", "check_in", "check_out"]},
     "annotations": {"readOnlyHint": True, "title": "Search hotels"}},
    {"name": "hotel_rooms",
     "description": "Room types of one hotel for given dates: beds, meals, price per night and total, cancellation policy, "
                    "payment terms (pay at hotel / prepay) and a booking_url per room.",
     "inputSchema": {"type": OBJ, "properties": {
         "hotel_id": {"type": "string"}, "check_in": {"type": "string"}, "check_out": {"type": "string"},
         "guests": {"type": "integer", "minimum": 1}}, "required": ["hotel_id", "check_in", "check_out"]},
     "annotations": {"readOnlyHint": True, "title": "Hotel rooms and rates"}},
    {"name": "michelin_search",
     "description": "MICHELIN Guide restaurants in an Asian city: distinction (3/2/1 Stars or Bib Gourmand), cuisine, area, "
                    "price range per person in local currency, closing days, deposit policy and booking_url.",
     "inputSchema": {"type": OBJ, "properties": {
         "city": {"type": "string"},
         "distinction": {"type": "string", "description": "any | stars | 3 stars | 2 stars | 1 star | bib gourmand"},
         "cuisine": {"type": "string"}}, "required": ["city"]},
     "annotations": {"readOnlyHint": True, "title": "Search MICHELIN restaurants"}},
    {"name": "restaurant_availability",
     "description": "Bookable times at one restaurant for a date and party size, with a booking_url per time slot, the "
                    "deposit due now and the seating limit. When nothing is free it lists the next available dates.",
     "inputSchema": {"type": OBJ, "properties": {
         "restaurant_id": {"type": "string"}, "date": {"type": "string", "description": "YYYY-MM-DD"},
         "party_size": {"type": "integer", "minimum": 1, "maximum": 8}}, "required": ["restaurant_id", "date", "party_size"]},
     "annotations": {"readOnlyHint": True, "title": "Restaurant availability"}},
]
IMPL = {"flights_search": flights_search, "flight_offer": flight_offer, "hotels_search": hotels_search,
        "hotel_rooms": hotel_rooms, "michelin_search": michelin_search, "restaurant_availability": restaurant_availability}


def _call(name: str, args: dict) -> dict:
    LOG["calls"].append({"name": name, "args": args, "t": time.time()})
    fn = IMPL.get(name)
    if not fn:
        return {"isError": True, "content": [{"type": "text", "text": f"unknown tool {name}"}]}
    try:
        res = fn(args or {})
    except (ToolError, ValueError, TypeError) as e:
        return {"isError": True, "content": [{"type": "text", "text": f"error: {e}"}]}
    return {"content": [{"type": "text", "text": json.dumps(res, ensure_ascii=False)}]}


def _handle(msg: dict) -> dict | None:
    m, mid = msg.get("method"), msg.get("id")
    if mid is None:
        return None
    if m == "initialize":
        res = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {"listChanged": False}},
               "serverInfo": {"name": "fake-travel", "version": "1.0"},
               "instructions": "Travel data for Asian cities: flights, hotels and MICHELIN restaurants, with booking links."}
    elif m == "ping":
        res = {}
    elif m == "tools/list":
        res = {"tools": TOOLS}
    elif m == "tools/call":
        p = msg.get("params") or {}
        res = _call(p.get("name", ""), p.get("arguments") or {})
    else:
        return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found"}}
    return {"jsonrpc": "2.0", "id": mid, "result": res}


@app.post("/mcp")
async def mcp(req: Request):
    if req.headers.get("authorization") != TOKEN:
        return JSONResponse({"error": "unauthorized"}, status_code=401)
    msg = await req.json()
    sid = req.headers.get("mcp-session-id") or ""
    if msg.get("method") != "initialize" and not sid:
        return JSONResponse({"error": "missing session"}, status_code=400)
    out = _handle(msg)
    if out is None:
        return Response(status_code=202)
    headers = {"Mcp-Session-Id": sid or uuid.uuid4().hex}
    if msg.get("method") == "tools/call":
        async def gen():
            yield 'event: message\ndata: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}\n\n'
            await asyncio.sleep(0.02)
            yield f"event: message\ndata: {json.dumps(out)}\n\n"
        return StreamingResponse(gen(), media_type="text/event-stream", headers=headers)
    return JSONResponse(out, headers=headers)


@app.get("/mcp")
async def mcp_get():
    return Response(status_code=405)


@app.delete("/mcp")
async def mcp_end():
    return Response(status_code=204)


@app.get("/_log")
async def log():
    return LOG


@app.post("/_reset")
async def reset():
    for v in LOG.values():
        v.clear()
    return {"ok": True}


@app.get("/")
async def root():
    return HTMLResponse("<!doctype html><title>TravelTest</title><h1>TravelTest bookings</h1>")


# ------------------------------------------------------------------ booking pages
STYLE = ("body{font-family:system-ui,sans-serif;max-width:720px;margin:24px auto;padding:0 16px;color:#222}"
         "label{display:block;margin:10px 0 2px;font-weight:600}input,select,textarea{width:100%;padding:8px;font-size:15px;"
         "box-sizing:border-box}fieldset{margin:16px 0;border:1px solid #ccc;border-radius:8px}"
         "button{margin-top:18px;padding:12px 22px;font-size:16px;background:#0a7;color:#fff;border:0;border-radius:6px}"
         ".sum{background:#f4f7fa;padding:12px 16px;border-radius:8px}.err{color:#b00;font-weight:600}")


def _page(title: str, body: str) -> HTMLResponse:
    return HTMLResponse(f"<!doctype html><html><head><meta charset='utf-8'><title>{html.escape(title)} - TravelTest</title>"
                        f"<style>{STYLE}</style></head><body><p><b>TravelTest</b> secure booking</p>{body}</body></html>")


def _field(name: str, label: str, typ: str = "text", value: str = "", required: bool = True, ac: str = "", ph: str = "") -> str:
    return (f"<label for='{name}'>{html.escape(label)}{'' if required else ' (optional)'}</label>"
            f"<input id='{name}' name='{name}' type='{typ}' value='{html.escape(value, quote=True)}'"
            f"{' autocomplete=' + repr(ac) if ac else ''}{' placeholder=' + repr(ph) if ph else ''}>")


CARD = (("card_number", "Card number", "cc-number", "1234 5678 9012 3456"), ("card_expiry", "Expiry (MM/YY)", "cc-exp", "MM/YY"),
        ("card_cvc", "CVC", "cc-csc", ""), ("card_name", "Name on card", "cc-name", ""))


def _card_fields(required: bool, vals: dict) -> str:
    head = "Payment card" if required else "Card to guarantee the booking"
    return (f"<fieldset><legend>{head}{'' if required else ' (optional)'}</legend>"
            + "".join(_field(n, lbl, "text", vals.get(n, ""), required, ac, ph) for n, lbl, ac, ph in CARD) + "</fieldset>")


def _resolve(kind: str, bid: str) -> dict:
    """Everything a booking page needs: a summary, the amount due now, which fields are required."""
    if kind == "flight":
        off = _with_cabin(bid)
        return {"title": f"{off['title']} {off['from']} to {off['to']}",
                "lines": [f"Flight: {off['flight_no']} ({off['airline']}), {off['cabin']}",
                          f"Route: {off['from']} to {off['to']}, {off['stops_text']}",
                          f"Departs: {off['depart_local']}", f"Arrives: {off['arrive_local']}",
                          f"Duration: {off['duration']}", f"Baggage: {off['baggage']}", f"Fare rules: {off['refund_rules']}"],
                "amount": float(off["fare_amount"]), "cur": "SGD", "due_label": "Total (1 adult, taxes included)",
                "passport": True, "card": True, "button": "Pay now", "extra": []}
    if kind == "hotel":
        if re.fullmatch(r"H-[A-Z]{3}-\d+", bid):
            h = _hotel_by_id(bid)
            return {"title": h["name"], "hotel_only": h, "lines": [], "amount": 0, "cur": "SGD", "button": "Show rooms"}
        h, room = _room_by_id(bid)
        prepay = room["payment"].startswith("Prepay")
        return {"title": f"{h['name']} - {room['room_type']}",
                "lines": [f"Hotel: {h['name']} ({h['stars']}-star, {h['area']}, {h['city']})",
                          f"Room: {room['room_type']}, {room['beds']}, {room['meals']}",
                          f"Stay: {bid.split('-')[3][:4]}-{bid.split('-')[3][4:6]}-{bid.split('-')[3][6:]} to "
                          f"{bid.split('-')[4][:4]}-{bid.split('-')[4][4:6]}-{bid.split('-')[4][6:]} ({room['nights']} nights), "
                          f"{bid.split('-')[5]} guests",
                          f"Rate: {room['price_per_night']} per night", f"Cancellation: {room['cancellation']}",
                          f"Payment: {room['payment']}"],
                "amount": float(room["total_amount"]), "cur": "SGD",
                "due_label": "Total for the stay, taxes included" + (" (charged now)" if prepay else " (paid at the hotel)"),
                "passport": False, "card": prepay, "card_optional": not prepay, "button": "Confirm booking",
                "extra": [("arrival_time", "Estimated arrival time"), ("requests", "Special requests")]}
    if kind == "restaurant":
        if re.fullmatch(r"R-[A-Z]{3}-\d+", bid):
            r = _rest_by_id(bid)
            return {"title": r["name"], "rest_only": r, "lines": [], "amount": 0, "cur": r["_cur"], "button": "Find a table"}
        r, day, t, party = _slot_by_id(bid)
        if t not in _slots(r, day, party):
            raise ToolError(f"{t} on {day} for {party} is no longer available at {r['name']}")
        dep = r["_dep"] * party
        return {"title": f"{r['name']} {day} {t}",
                "lines": [f"Restaurant: {r['name']} ({r['distinction']}, {r['cuisine']}, {r['area']}, {r['city']})",
                          f"Date: {day} ({WEEKDAYS[day.weekday()]})", f"Time: {t}", f"Party size: {party}",
                          f"Price range: {r['price_range']}", f"Deposit policy: {r['deposit_policy']}"],
                "amount": float(dep), "cur": r["_cur"], "sym": r["_sym"], "due_label": "Total due now (deposit)",
                "passport": False, "card": dep > 0 or r["distinction"] == "1 Star", "button": "Confirm booking",
                "extra": [("requests", "Dietary needs / occasion")]}
    raise ToolError("unknown booking kind")


def _form(kind: str, bid: str, info: dict, vals: dict, missing: list[str]) -> str:
    cur = info["cur"]
    amount = _money(info["amount"], cur) if info["amount"] else "nothing (no deposit)"
    parts = [f"<h1>{html.escape(info['title'])}</h1>", "<div class='sum'>"]
    parts += [f"<p>{html.escape(x)}</p>" for x in info["lines"]]
    parts.append(f"<p><b>{html.escape(info['due_label'])}: {html.escape(amount)}</b></p>")
    if info["amount"]:
        parts.append(f"<p>Total: {html.escape(_money(info['amount'], cur))}</p>")
    parts.append("</div>")
    if missing:
        parts.append(f"<p class='err' role='alert'>Please complete: {html.escape(', '.join(missing))}</p>")
    parts.append(f"<form method='post' action='/book/{kind}/{bid}'><fieldset><legend>"
                 f"{'Traveller' if kind == 'flight' else 'Guest'} details</legend>")
    parts.append(_field("full_name", "Full name (as in passport)" if info["passport"] else "Full name", "text",
                        vals.get("full_name", ""), True, "name"))
    parts.append(_field("email", "Email", "email", vals.get("email", ""), True, "email"))
    parts.append(_field("phone", "Phone", "tel", vals.get("phone", ""), True, "tel"))
    if info["passport"]:
        parts.append(_field("passport", "Passport number", "text", vals.get("passport", ""), True))
        parts.append(_field("passport_expiry", "Passport expiry (YYYY-MM-DD)", "text", vals.get("passport_expiry", ""), False))
    for n, lbl in info.get("extra", []):
        parts.append(_field(n, lbl, "text", vals.get(n, ""), False))
    parts.append("</fieldset>")
    if info["card"] or info.get("card_optional"):
        parts.append(_card_fields(bool(info["card"]), vals))
    parts.append(f"<button type='submit'>{info['button']}</button></form>")
    return "".join(parts)


def _picker(kind: str, info: dict) -> str:
    """The hotel / restaurant landing page (search result booking_url): pick dates and a room / time by link."""
    if kind == "hotel":
        h = info["hotel_only"]
        d1 = dt.date.today() + dt.timedelta(days=21)
        rooms = _rooms(h, d1, d1 + dt.timedelta(days=2), 2)
        items = "".join(f"<li><a href='/book/hotel/{r['room_offer_id']}'>{html.escape(r['room_type'])} - "
                        f"{html.escape(r['price_per_night'])} per night - {html.escape(r['cancellation'])}</a></li>" for r in rooms)
        return (f"<h1>{html.escape(h['name'])}</h1><p>{h['stars']}-star, {html.escape(h['area'])}, rating {h['rating']}</p>"
                f"<p>Rooms for {d1} to {d1 + dt.timedelta(days=2)}, 2 guests (use the hotel_rooms data for other dates):</p><ul>{items}</ul>")
    r = info["rest_only"]
    d1 = dt.date.today() + dt.timedelta(days=21)
    while not _slots(r, d1, 2):
        d1 += dt.timedelta(days=1)
    items = "".join(f"<li><a href='/book/restaurant/{r['restaurant_id']}-{d1:%Y%m%d}-{t.replace(':', '')}-2'>{t}</a></li>"
                    for t in _slots(r, d1, 2))
    return (f"<h1>{html.escape(r['name'])}</h1><p>{html.escape(r['distinction'])} - {html.escape(r['cuisine'])}</p>"
            f"<p>Tables for 2 on {d1}:</p><ul>{items}</ul>")


@app.get("/book/{kind}/{bid}")
async def book_page(kind: str, bid: str):
    LOG["views"].append({"kind": kind, "id": bid, "t": time.time()})
    try:
        info = _resolve(kind, bid)
    except ToolError as e:
        return _page("Not available", f"<h1>Not available</h1><p>{html.escape(str(e))}</p>")
    if "hotel_only" in info or "rest_only" in info:
        return _page(info["title"], _picker(kind, info))
    return _page(info["title"], _form(kind, bid, info, {}, []))


@app.post("/book/{kind}/{bid}")
async def book_submit(kind: str, bid: str, req: Request):
    raw = (await req.body()).decode("utf-8", "replace")
    vals = {k: v[0].strip() for k, v in parse_qs(raw, keep_blank_values=True).items()}
    try:
        info = _resolve(kind, bid)
    except ToolError as e:
        return _page("Not available", f"<h1>Not available</h1><p>{html.escape(str(e))}</p>")
    need = [("full_name", "Full name"), ("email", "Email"), ("phone", "Phone")]
    if info.get("passport"):
        need.append(("passport", "Passport number"))
    if info.get("card"):
        need += [(n, lbl) for n, lbl, _, _ in CARD]
    missing = [lbl for n, lbl in need if not vals.get(n)]
    if vals.get("email") and not re.fullmatch(r"[^@\s]+@[^@\s]+\.[^@\s]+", vals["email"]):
        missing.append("a valid Email")
    digits = re.sub(r"\D", "", vals.get("card_number", ""))
    if vals.get("card_number") and not 12 <= len(digits) <= 19:
        missing.append("a valid Card number")
    if missing:
        LOG["rejected"].append({"kind": kind, "id": bid, "missing": missing, "t": time.time()})
        return _page(info["title"], _form(kind, bid, info, vals, missing))
    n = len(LOG["bookings"]) + 1
    ref = "TRV-" + kind[0].upper() + hashlib.sha1(f"{kind}{bid}{vals['full_name']}{n}".encode()).hexdigest()[:6].upper()
    rec = {"ref": ref, "kind": kind, "id": bid, "title": info["title"], "amount": info["amount"], "currency": info["cur"],
           "name": vals["full_name"], "email": vals["email"], "phone": vals["phone"],
           "passport_last4": vals.get("passport", "")[-4:], "card_last4": digits[-4:],
           "extra": {k: vals[k] for k in ("arrival_time", "requests", "passport_expiry") if vals.get(k)}, "t": time.time()}
    LOG["bookings"].append(rec)
    amount = _money(info["amount"], info["cur"]) if info["amount"] else "nothing charged"
    paid = "Total paid" if info.get("card") and info["amount"] and not info.get("card_optional") else "Total"
    body = (f"<h1>Booking confirmed</h1><div class='sum'><p><b>Booking reference: {ref}</b></p>"
            + "".join(f"<p>{html.escape(x)}</p>" for x in info["lines"])
            + f"<p>Booked for: {html.escape(vals['full_name'])}</p><p>{paid}: {html.escape(amount)}</p>"
            + (f"<p>Card: ending {digits[-4:]}</p>" if digits else "")
            + f"<p>A confirmation email is on its way to {html.escape(vals['email'])}.</p></div>")
    return _page("Booking confirmed " + ref, body)
