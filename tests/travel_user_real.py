"""Travel searches as the user does them, with a real model and the real services (OMUSE_E2E_REAL=1, the e2e env file
with OMUSE_E2E_SERPAPI_KEY): in the browser the user pastes a SerpApi key on Connections → Flights & hotels (the most
common setup), then asks Main's chat for a flight, a flight on any day of a range, a hotel and a rental car. Only what
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

        # 2. a flight on any day of a range (before the airline preference below, which the user's memory keeps)
        a = await u.ask(f"What's the cheapest flight from Singapore to Tokyo on any day between {day(D1)} and {day(D1 + dt.timedelta(days=4))}?", label="range")
        check("range: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        nothing = re.search(r"(no|couldn.t find|didn.t find)\b.*(flight|fare)", a.text, re.I) and re.search(r"all airlines|broader|other airlines|without", a.text, re.I)
        check("range: the answer names the cheapest day and its price — or says the search found nothing and offers a broader one",
              (a.has_prices() and any(mentions_day(a.text, D1 + dt.timedelta(days=i)) for i in range(5))) or bool(nothing), a.text[:400])
        if not nothing:
            bk = [x for x in a.links if "kiwi.com" in x or "google.com/travel" in x]
            check("range: the answer has a booking link, and it opens", bool(bk) and opens(bk[0])[0], a.links)

        # 3. a flight, with an airline preference
        a = await u.ask(f"Find me the cheapest flight from San Francisco to Chengdu on {day(D1)}. I prefer United.", label="flight")
        check("flight: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("flight: the answer gives flights (flight numbers, or the airline and departure times) with prices", a.has_prices()
              and (re.search(r"\b(UA|CA|MU|CZ|3U|HU|NH|JL|CX|BR|CI)\s?\d{2,4}\b", a.text)
                   or (re.search(r"United|Air China|China Eastern|Sichuan", a.text) and re.search(r"\b\d{1,2}:\d\d\b", a.text))), a.text[:600])
        check("flight: the answer covers the preferred airline (United)", "United" in a.text, a.text[:400])
        g = [x for x in a.links if "google.com/travel/flights" in x]
        k = [x for x in a.links if "kayak.com/flights" in x]
        check("flight: the answer links a flight's booking page and Kayak to compare", g and k, a.links)
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
        gh = [x for x in a.links if "google.com/travel/hotels" in x]
        kh = [x for x in a.links if "kayak.com/hotels" in x]
        check("hotel: the answer links a hotel's booking page and Kayak to compare", gh and kh, a.links)
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
