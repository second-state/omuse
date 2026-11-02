"""Travel searches as the user does them, with a real model and the real services (OMUSE_E2E_REAL=1, the e2e env file
with OMUSE_E2E_SERPAPI_KEY): in the browser the user pastes a SerpApi key on Connections → Flights & hotels (the most
common setup), then asks Main's chat for tomorrow's last Hong Kong → Singapore flight that lands before midnight (that one flight, the
Singapore Airlines evening flight Google Flights lists), a flight, a flight on any day of a range, a hotel and a rental car. Only what
the user sees is checked — the answer on the page and its links: real options with prices, a booking link that opens
the booking page (Google Flights' "Book with …", the hotel's or Kayak's page) and the Kayak link to compare. Nothing
inside the system (tools, arguments, logs) is looked at. One TIMING line per request: from Enter to the answer.

run_in_docker.sh runs it against the real SerpApi, Kiwi.com, Kayak and Rentalcars.com (REAL_DATA_SUITES):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_user_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check, opens  # noqa: E402

KEY = os.environ.get("OMUSE_E2E_SERPAPI_KEY", "")
if not KEY:
    print("travel_user_real needs a SerpApi key (OMUSE_E2E_SERPAPI_KEY in the e2e env file)")
    raise SystemExit(0)
TODAY = dt.date.today()
D1 = TODAY + dt.timedelta(days=30)
D3 = D1 + dt.timedelta(days=3)


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def mentions_day(text: str, d: dt.date) -> bool:
    return bool(re.search(rf"\b({d:%b}|{d:%B})\.? {d.day}\b|\b{d.day} ({d:%b}|{d:%B})\b|{d.isoformat()}", text))


TIMES = re.compile(r"(\d{4}-\d\d-\d\d) (\d\d:\d\d) → \S+ (\d{4}-\d\d-\d\d) (\d\d:\d\d)")


async def last_flight(u: U.User, a: U.Answer) -> None:
    """The answer to "the last flight that lands before midnight" tomorrow, as the user reads it: the block code appends
    names ONE flight with its times, it lands tomorrow before midnight, and it is the last such flight of every flight
    the search found for the day (the search's own structured times, from the run's record) — no other flight is
    suggested beside it, no card of alternatives. Google Flights has Singapore Airlines' evening flight that lands
    before midnight (SQ 895, 18:50 → 22:50; Kiwi.com does not sell it): the user gets that one."""
    tomorrow = (TODAY + dt.timedelta(days=1)).isoformat()
    shown = TIMES.findall(a.block)
    print(f"INFO flight: the block's times {shown}")
    check("flight: the block names exactly one flight (no other suggestion)", len(re.findall(r"(?m)^\W*Flight( \d+)?:", a.block)) == 1, a.block[:400])
    check("flight: the block gives the flight's departure and arrival times", len(shown) == 1, a.block[:400])
    if len(shown) != 1:
        return
    dep_d, dep_t, arr_d, arr_t = shown[0]
    check(f"flight: it leaves tomorrow ({tomorrow})", dep_d == tomorrow, shown[0])
    check("flight: it is Singapore Airlines' flight (SQ), which Google Flights lists landing before midnight",
          re.search(r"(?m)^\W*Flight( \d+)?:.*(Singapore Airlines|\bSQ ?\d)", a.block) is not None, a.block[:400])
    check("flight: it lands before midnight on the day it leaves", arr_d == dep_d, shown[0])
    # every flight the search found (the run's record: the search's result as JSON, its times from the API's fields)
    listed = []
    for t in await u.tasks():
        for e in t.get("events", []):
            r = (e.get("data") or {}).get("result") or {}
            if e.get("type") == "workflow_result" and r.get("status") == "pick_needed":
                listed = r.get("flights") or []
    same_day = [f for f in listed if (f.get("departure") or {}).get("time", "").startswith(tomorrow)
                and (f.get("arrival") or {}).get("time", "").startswith(tomorrow)]
    print(f"INFO flight: {len(listed)} flights found, {len(same_day)} land the same day")
    check("flight: the search listed the day's flights with their times", bool(same_day), len(listed))
    if not same_day:
        return
    last_dep = max(f["departure"]["time"] for f in same_day)
    check(f"flight: it is the last flight landing before midnight (leaves {last_dep[11:]})",
          f"{dep_d} {dep_t}" == last_dep, (shown[0], last_dep))
    cards = U.conv_choices(await u.conv_id())
    check("flight: no choice card of alternatives for the one flight asked for",
          not any(len(c.get("options") or []) > 1 and "Flights" in str(c.get("question") or "") for c in cards),
          [c.get("question") for c in cards])


async def main():
    async with U.User() as u:
        pg = u.page
        # 1. the user connects their SerpApi key on Connections → Flights & hotels
        await pg.goto(U.APP + "#connections")
        card = pg.locator(".card", has_text="Flights & hotels").first
        await card.wait_for(timeout=20000)
        await card.locator("input[type=password]").fill(KEY)
        await card.locator("button", has_text="Connect & test").click()
        await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && /searches left/.test(c.textContent))",
                                   timeout=60000)
        check("the user's SerpApi key connects: the card shows the plan and the searches left", "searches left" in await card.inner_text())

        # 2. tomorrow's last flight that lands before midnight: THE one flight, not the cheapest or suggestions
        # (the user's request of 2026-11-02, word for word; also the Travel bot's example in the UI — bots/travel.md,
        # checked by bots.rs tests)
        a = await u.ask("I need to go from HK to singapore tomorrow. Find me the last flight that lands before midnight", label="last")
        k = [x for x in a.block_links if "kayak.com/flights" in x]
        check("last: the block code appends after the answer links a booking page and Kayak to compare",
              any("kayak" not in x for x in a.block_links) and bool(k), a.block_links)
        await last_flight(u, a)

        # 2b. a flight on any day of a range (before the airline preference below, which the user's memory keeps)
        a = await u.ask(f"What's the cheapest flight from Singapore to Tokyo on any day between {day(D1)} and {day(D1 + dt.timedelta(days=4))}?", label="range")
        check("range: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        nothing = re.search(r"(no|couldn.t find|didn.t find)\b.*(flight|fare)", a.text, re.I) and re.search(r"all airlines|broader|other airlines|without", a.text, re.I)
        check("range: the answer names the cheapest day and its price — or says the search found nothing and offers a broader one",
              (a.has_prices() and any(mentions_day(a.text, D1 + dt.timedelta(days=i)) for i in range(5))) or bool(nothing), a.text[:400])
        if not nothing:
            bk = [x for x in a.block_links if "kiwi.com" in x or "google.com/travel" in x]
            check("range: the block code appends after the answer has a booking link, and it opens", bool(bk) and opens(bk[0])[0], a.block_links)

        # 3. a flight, with an airline preference
        a = await u.ask(f"Find me the cheapest flight from San Francisco to Chengdu on {day(D1)}. I prefer United.", label="flight")
        check("flight: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("flight: the answer gives flights (flight numbers, or the airline and departure times) with prices", a.has_prices()
              and (re.search(r"\b(UA|CA|MU|CZ|3U|HU|NH|JL|CX|BR|CI)\s?\d{2,4}\b", a.text)
                   or (re.search(r"United|Air China|China Eastern|Sichuan", a.text) and re.search(r"\b\d{1,2}:\d\d\b", a.text))), a.text[:600])
        check("flight: the answer covers the preferred airline (United)", "United" in a.text, a.text[:400])
        g = [x for x in a.block_links if "google.com/travel/flights" in x]
        k = [x for x in a.block_links if "kayak.com/flights" in x]
        check("flight: the block code appends after the answer links a flight's booking page and Kayak to compare", g and k, a.block_links)
        if g:
            page = await u.ctx.new_page()
            await page.goto(g[0], wait_until="domcontentloaded", timeout=60000)
            await page.wait_for_timeout(6000)
            body = await page.inner_text("body")
            check("flight: the booking link opens the flight's booking options (\"Book with …\")", "Book with" in body or "Booking options" in body,
                  " ".join(body.split())[:300])
            await page.close()
        if k:
            ok, why = opens(k[0])
            check("flight: the Kayak link opens Kayak's page for the same search", ok and "SFO" in why.upper(), why)

        # 4. a hotel
        a = await u.ask(f"Find a 4-star hotel near Ginza, Tokyo from {day(D1)} to {day(D3)} for 2 people.", label="hotel")
        check("hotel: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("hotel: the answer lists hotels with prices", a.has_prices(2) and re.search(r"[Hh]otel|Inn|Ginza", a.text), a.text[:400])
        gh = [x for x in a.block_links if "google.com/travel/hotels" in x]
        kh = [x for x in a.block_links if "kayak.com/hotels" in x]
        check("hotel: the block code appends after the answer links a hotel's booking page and Kayak to compare", gh and kh, a.block_links)
        if gh:
            page = await u.ctx.new_page()
            await page.goto(gh[0], wait_until="domcontentloaded", timeout=60000)
            await page.wait_for_timeout(6000)
            dates = await page.evaluate("() => [...document.querySelectorAll('input')].map(i => (i.getAttribute('aria-label') || '') + '=' + i.value).filter(s => /check/i.test(s))")
            body = " ".join((await page.inner_text("body")).split())
            check("hotel: the booking link opens the hotel's page for the dates asked, with sellers and prices",
                  f"Check-in={D1:%a}, {D1:%b} {D1.day}" in dates and f"Check-out={D3:%a}, {D3:%b} {D3.day}" in dates and len(U.PRICE.findall(body)) >= 2,
                  (dates, body[:200]))
            await page.close()
        if kh:
            ok, why = opens(kh[0])
            check("hotel: the Kayak link opens", ok, why)

        # 5. a rental car
        a = await u.ask(f"I need an SUV at LAX from {day(D1)} 10am to {day(D3)} 10am.", label="car")
        check("car: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("car: the answer names rental companies", re.search(r"Hertz|Avis|Enterprise|Budget|National|Alamo|Sixt|Dollar|Thrifty|Fox|Payless|Economy Rent|Europcar|NU Car|Yesaway|Ace|Movly|NextCar|Priceless|Green Motion|GREEN MOTION|Drive Rental", a.text), a.text[:400])
        car = [x for x in a.links if "kayak.com/cars" in x or "rentalcars.com" in x]
        check("car: the answer links the live offers for this rental (Kayak, else Rentalcars.com)", bool(car), a.links)
        if car:
            ok, why = opens(car[0])
            check("car: the rental link opens the offers for LAX", ok and ("LAX" in why.upper() or "rentalcars" in car[0]), why)
            check("car: the link is for the dates asked", D1.isoformat() in car[0] or f"puDay={D1.day}" in car[0], car[0])
            if "kayak.com/cars" in car[0]:
                check("car: Kayak's page is for the pickup and return times and the SUVs asked",
                      f"{D1.isoformat()}-10h" in car[0] and f"{D3.isoformat()}-10h" in car[0] and "carclass=suv" in car[0].lower(), car[0])


U.setup(vault=False)
U.run(main)
