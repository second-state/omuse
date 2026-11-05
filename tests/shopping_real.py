"""The Shopping bot on the real Shopify Global Catalog, as the user does it, with a real model (OMUSE_E2E_REAL=1, the e2e
env file with OMUSE_E2E_SHOPIFY_CLIENT_ID / _SECRET). Read-only: nothing is bought, no page is opened by the agent.

A Singapore user (the profile's home city and country) asks the Shopping bot for running shoes under S$150. Code
searches the Catalog for what ships to Singapore, prices every offer in SGD (the merchant's own SGD price, or converted at
the market rate), ranks them; the model picks a few by their offer ids; code shows the choice card and appends each
offer's link. Checked, from what the user sees: a choice card of a few offers that ship to SG, each priced in SGD within
the budget with its merchant; the block's links are the cards' merchants' pages and some open; and Main hands the same
request to the Shopping bot.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh -j 1 shopping_real"""
import os
import sys
from urllib.parse import urlparse

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

REQUEST = "Find me running shoes under S$150."
# a brand named: Google Shopping too (SerpApi, the Flights & hotels key) — also the Shopping bot's example in the UI
BRAND_REQUEST = "Find me Nike running shoes under S$250."
KEY = os.environ.get("OMUSE_E2E_SERPAPI_KEY", "")
PROFILE = {"name_en": "OMUSE TESTER", "home_city": "SIN", "home_country": "SG",
           "email_personal": "omuse.e2e.tester@example.com"}


def host(u: str) -> str:
    return (urlparse(u or "").hostname or "").lower().removeprefix("www.")


async def main():
    async with U.User() as u:
        await u.open_bot("Shopping")
        a = await u.ask(REQUEST, label="find", new_chat=False)
        cards = [c for c in await u.choices() if c.get("kind") == "comparison"]
        opts = (cards[-1].get("options") if cards else None) or []
        print(f"INFO card: {cards[-1].get('question') if cards else None} "
              f"{[(o.get('label'), o.get('price'), o.get('currency'), o.get('details')) for o in opts]}")
        check("the chat shows a choice card with a few offers", 2 <= len(opts) <= 6, opts)
        check("the card is for what ships to Singapore", bool(cards) and "ship to SG" in (cards[-1].get("question") or ""),
              cards[-1].get("question") if cards else None)
        check("every offer is priced in SGD within the budget (its price in its own field)",
              opts and all(o.get("currency") == "SGD" and isinstance(o.get("price"), (int, float)) and 0 < o["price"] <= 150
                           for o in opts), [(o.get("price"), o.get("currency")) for o in opts])
        check("every offer names its merchant", opts and all(any("sold by" in d for d in o.get("details") or []) for o in opts),
              [o.get("details") for o in opts])
        card_hosts = {host(o.get("source_url")) for o in opts}
        print(f"INFO block: {a.block_links}")
        check("the block code appended links each offer's page on the card", a.block_links
              and {host(x) for x in a.block_links} <= card_hosts and len(a.block_links) >= len(opts), (a.block_links, card_hosts))
        opened = [(x, U.opens(x)) for x in a.block_links]
        print(f"INFO links: {[(x[:80], r) for x, r in opened]}")
        # a 429 is the shop's own server answering (the link exists; it throttles automated checks from this address —
        # every Shopify store did from the test container on 2026-11-05): reachable, but never a 404 or a bot page
        check("the offers' links open for the user (at least one: a shop may block or throttle automated requests)",
              any(ok or r.startswith("429") for _, (ok, r) in opened), opened)
        check("no link leads nowhere", not a.broken_links(), a.broken_links())

        # a brand named: Google Shopping (SerpApi) joins, from the user's city; a pick links the seller's own page
        if KEY:
            pg = u.page
            await pg.goto(U.APP + "#connections")
            card = pg.locator(".card", has_text="Flights & hotels").first
            await card.wait_for(timeout=20000)
            await card.locator("input[type=password]").fill(KEY)
            await card.locator("button", has_text="Connect & test").click()
            await pg.wait_for_function("() => [...document.querySelectorAll('.card')].some(c => c.textContent.includes('Flights & hotels') && /searches left/.test(c.textContent))",
                                       timeout=60000)
            await u.open_bot("Shopping")
            c = await u.ask(BRAND_REQUEST, label="brand", new_chat=True)
            cards = [x for x in await u.choices() if x.get("kind") == "comparison"]
            copts = (cards[-1].get("options") if cards else None) or []
            print(f"INFO brand card: {[(o.get('label'), o.get('price'), o.get('details')) for o in copts]}")
            check("brand: the card shows Nike offers in SGD", bool(copts) and all(o.get("currency") == "SGD" for o in copts)
                  and any("nike" in str(o.get("label")).lower() for o in copts), copts)
            if copts:
                pick = copts[0].get("label")
                d = await u.ask(f"I'll take the {pick}.", label="pick", new_chat=False)
                buy = [x for x in d.block_links if host(x) and not host(x).endswith("google.com")]
                print(f"INFO pick block: {d.block!r} links {d.block_links}")
                check("pick: the block links a store's own page for it (not Google's)", bool(buy), d.block_links)
                check("pick: the block says where it is delivered and that the store's checkout confirms the address",
                      d.shows("the store's checkout confirms your address") or d.shows("Buy it on the merchant's page"), d.block)
                check("pick: nothing was bought or submitted", not d.approvals and not d.takeovers, (d.approvals, d.takeovers))
        else:
            print("SKIP the brand search: no OMUSE_E2E_SERPAPI_KEY")

        # the same request to Main goes to the Shopping bot
        await u.open_bot("Main")
        b = await u.ask(REQUEST, label="main", new_chat=False)
        print(f"INFO main: {b.bot_link!r}")
        check("Main hands the request to the Shopping bot", "Shopping" in b.bot_link, b.bot_link)


U.setup(vault=False, profile=PROFILE)
U.run(main)
