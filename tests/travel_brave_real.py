"""A trip with a Brave Search key and no travel key, as the user does it, with a real model and the real web
(OMUSE_E2E_REAL=1, the e2e env file with OMUSE_E2E_BRAVE_KEY): on Connections → Web search the user pastes their own
Brave key (no SerpApi key on Flights & hotels), then asks Main's chat for a flight, a hotel and a rental car in one
message. OMuse searches one thing per message and offers the next, so the user says yes to the hotel and then asks for
the car. Only what the user sees is checked — each answer and its links: real options with prices and booking links
that open (Kiwi.com for the flight; a hotel's or a booking site's page and Kayak for the hotel; Kayak's or
Rentalcars.com's page for the car, for the airport and dates asked). One TIMING line per request.

(tests/brave_travel_e2e.py keeps what a real run cannot show: the browser's engines blocked so Brave answers, and
exactly one travel search per message.)

run_in_docker.sh runs it against the real Brave, Kiwi.com, Kayak, Rentalcars.com and the web (REAL_DATA_SUITES):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_brave_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check, opens  # noqa: E402

BRAVE = os.environ.get("OMUSE_E2E_BRAVE_KEY", "")
if not BRAVE:
    print("travel_brave_real needs a Brave Search key (OMUSE_E2E_BRAVE_KEY in the e2e env file)")
    raise SystemExit(0)
TODAY = dt.date.today()
D1 = TODAY + dt.timedelta(days=30)
D3 = D1 + dt.timedelta(days=3)
AIRLINES = r"Singapore Airlines|Scoot|ANA|All Nippon|Japan Airlines|JAL|ZIPAIR|Zipair|Cathay|Peach|Jetstar|AirAsia|Air Asia|Vietjet|VietJet|" \
           r"Thai|China Airlines|EVA|Korean|Asiana|China Eastern|China Southern|Air China|Philippine|Cebu|Malaysia|Delta|United|Hong Kong Airlines"
FLIGHT_NO = r"\b(SQ|TR|NH|JL|ZG|CX|MM|GK|3K|AK|D7|VJ|TG|CI|BR|KE|OZ|MU|CZ|CA|PR|5J|MH|DL|UA|HX)\s?\d{2,4}\b"
CAR_COS = r"Hertz|Avis|AVIS|Budget|Europcar|Sixt|SIXT|Enterprise|National|Alamo|Toyota Rent|Nippon Rent|Times Car|ORIX|Orix|Nissan Rent|Relax Rent|Ota Rent|" \
          r"OTS|Easy Car|Asuka|GALILEO|Galileo|Lotte|Fox|Payless|Ace|Economy Rent|NU Car|Yesaway|Movly|Priceless|U-Save|Green Motion|Goldcar|Rent[ -]a[ -]Car"


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def first_opening(links: list[str]) -> tuple[str, str]:
    """The first of `links` that opens for the user, with what it showed ("" when none does)."""
    why = ""
    for x in links[:6]:
        ok, why = opens(x)
        if ok:
            return x, why
    return "", why


async def main():
    async with U.User() as u:
        pg = u.page
        # 1. the user pastes their own Brave key on Connections → Web search
        await pg.goto(U.APP + "#connections")
        await pg.wait_for_selector("#websearch", timeout=20000)
        row = pg.locator("#websearch .provider[data-provider=brave]")
        await row.locator("input[type=password]").fill(BRAVE)
        n = await pg.locator(".toast").count()
        await row.locator("button", has_text="Save & test").click()
        await pg.wait_for_function("n => document.querySelectorAll('.toast').length > n", arg=n, timeout=60000)
        toast = await pg.locator(".toast").last.inner_text()
        check("the Brave key is accepted: the message says web search is connected with Brave", "Brave" in toast and "connected" in toast.lower()
              and "err" not in (await pg.locator(".toast").last.get_attribute("class") or ""), toast)
        await pg.wait_for_function("() => { const r = document.querySelector('#websearch .provider[data-provider=brave] .chip'); return r && r.textContent.trim() === 'your key'; }",
                                   timeout=20000)
        check("the Brave row now says 'your key'", True)
        txt = await pg.locator(".card", has_text="Flights & hotels").first.inner_text()
        check("no SerpApi key on Flights & hotels", "searches left" not in txt, " ".join(txt.split())[:200])

        # 2. a flight, a hotel and a car in one message: the flight first, the hotel offered next
        a = await u.ask(f"I'm flying from Singapore to Tokyo on {day(D1)} for 3 nights. Find me the cheapest flight, a 4-star hotel near Ginza "
                        f"and a compact rental car at Narita airport from {day(D1)} to {day(D3)}.", label="trip")
        check("trip: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("trip: the first answer gives real flights — the airline, the times and the price",
              a.has_prices() and bool(re.search(AIRLINES, a.text) or re.search(FLIGHT_NO, a.text)) and len(re.findall(r"\b\d{1,2}:\d\d\b", a.text)) >= 2, a.text[:600])
        bk = [x for x in a.block_links if "kiwi.com" in x]
        check("trip: the block code appends after the answer has the flight's Kiwi.com booking link, and it opens", bool(bk) and opens(bk[0])[0], a.block_links)

        # 3. the hotel
        a = await u.ask("Yes, please search the hotel now.", label="hotel", new_chat=False)
        check("hotel: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("hotel: the answer names hotels near Ginza with prices", a.has_prices() and bool(re.search(r"[Hh]otel|Inn|Ginza", a.text)), a.text[:600])
        hotel_links = [x for x in a.block_links if "kayak.com" not in x]
        kh = [x for x in a.block_links if "kayak.com/hotels" in x]
        ok_link, why = first_opening(hotel_links)
        check("hotel: the block code appends after the answer links a hotel's or a booking site's page, and it opens", bool(ok_link), (a.block_links, why))
        check("hotel: the block code appends after the answer links Kayak's hotels for Ginza and the dates to compare", bool(kh) and D1.isoformat() in kh[0], a.block_links)
        if kh:
            ok, why = opens(kh[0])
            check("hotel: the Kayak link opens", ok, why)

        # 4. the car
        a = await u.ask("And now the rental car, please.", label="car", new_chat=False)
        check("car: every link in the answer leads somewhere (no undecoded stand-in)", not a.broken_links(), a.broken_links())
        check("car: the answer names rental companies at the airport", bool(re.search(CAR_COS, a.text)), a.text[:600])
        car = [x for x in a.links if "kayak.com/cars" in x or "rentalcars.com" in x]
        check("car: the answer links the live offers for this rental (Kayak, else Rentalcars.com)", bool(car), a.links)
        if car:
            check("car: the link is for Narita and the dates asked", "NRT" in car[0].upper()
                  and (D1.isoformat() in car[0] or f"puDay={D1.day}" in car[0]), car[0])
            ok, why = opens(car[0])
            check("car: the rental link opens", ok, why)


U.setup(vault=False)
U.run(main)
