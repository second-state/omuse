"""Booking a flight and a hotel on the REAL booking sites, as the user does it, with a real model (OMUSE_E2E_REAL=1, the
e2e env file with OMUSE_E2E_SERPAPI_KEY). Nothing is booked or paid: there is no card in the Vault, the profile holds
clearly fake traveller data, and the suite hands the browser back without pressing anything.

1. A fresh install (no travel key): the user asks Main's chat to book the cheapest Singapore → Bangkok flight about a
   month out. Code searches Kiwi.com's real data, the model picks the cheapest (travel_pick), asks to approve the fare
   (auto-approved, as a fresh install has it), opens the itinerary's REAL Kiwi.com booking page and hands it to the
   browser-use skill, which fills it with the profile's fake data and asks the user to take over at the first step
   that is theirs (sign-in, CAPTCHA, payment).
2. The user pastes their SerpApi key on Connections → Flights & hotels and asks to book a hotel in Bangkok: Google
   Hotels' real data, the room by rule, the seller's REAL booking page (the hotel's own site or an OTA), the same
   hand-over.

Checked, from what the user sees: the hotel's choice card (real options, one picked), the hand-over on the real booking site,
no card number or other secret put into the page, the block code appends (the booking page's link and the amount), and
nothing booked (no confirmation reference; the page handed over is not a confirmation). A site that blocks automation
is handed over at the block — the suite does not get around bot protection. One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh -j 1 travel_book_real"""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.handover_common import FAKE, Handover, check_handover  # noqa: E402
from tests.user_common import check  # noqa: E402

KEY = os.environ.get("OMUSE_E2E_SERPAPI_KEY", "")
TODAY = dt.date.today()
D1 = TODAY + dt.timedelta(days=30)
D3 = D1 + dt.timedelta(days=2)


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


async def choice_card(u: U.User, label: str) -> None:
    """The comparison card code showed before the booking: real options, the pick among them."""
    cards = [c for c in await u.choices() if c.get("type") == "choices" and c.get("kind") == "comparison"]
    opts = cards[-1].get("options") if cards else []
    print(f"INFO {label} choice card: {[(o.get('label'), o.get('note')) for o in opts or []][:5]}")
    check(f"{label}: the chat shows the choice card with the options compared and the one picked by rule",
          len(opts or []) >= 1 and any("picked" in str(o.get("note") or "") for o in opts or []), cards[-1:] or "no card")


async def main():
    async with U.User() as u:
        # 1. a flight, no travel key: Kiwi.com's real data and its real booking page
        seen = Handover()
        a = await u.ask(f"Book me the cheapest flight from Singapore to Bangkok on {day(D1)}, 1 adult, economy.",
                        label="flight", on_takeover=seen.look(u, "flight"))
        # a flight is the model's pick (travel_pick) shown alone — no comparison card (0.6.1)
        check_handover(a, seen, "flight", sites=("kiwi.com",))

        if not KEY:
            print("INFO no SerpApi key (OMUSE_E2E_SERPAPI_KEY): the hotel booking is skipped")
            return
        # 2. a hotel with the user's SerpApi key: Google Hotels' real data, the seller's real booking page
        pg = u.page
        await pg.goto(U.APP + "#connections")
        card = pg.locator(".card", has_text="Flights & hotels").first
        await card.wait_for(timeout=20000)
        await card.locator("input[type=password]").fill(KEY)
        await card.locator("button", has_text="Connect & test").click()
        await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && /searches left/.test(c.textContent))",
                                   timeout=60000)
        seen = Handover()
        a = await u.ask(f"Book the cheapest 4-star hotel in Bangkok near Sukhumvit from {day(D1)} to {day(D3)}, 1 guest.",
                        label="hotel", on_takeover=seen.look(u, "hotel"))
        await choice_card(u, "hotel")
        check_handover(a, seen, "hotel")


U.setup(vault=False, profile=FAKE)
U.run(main)
