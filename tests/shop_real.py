"""Shopping on a REAL shop, as the user does it, with a real model (OMUSE_E2E_REAL=1, the e2e env file). Nothing is
ordered or paid: there is no card in the Vault, the profile holds clearly fake data, and the suite hands the browser back
without pressing anything.

The shop is Allbirds' own store (a Shopify store with guest checkout: product pages, the cart and the checkout's contact
and delivery steps open without an account).

1. Compare: the user gives the men's shoes page and asks for the 3 cheapest. Code opens the real page and hands it to
   the browser-use skill, which lists the page's products as JSON options; code ranks them by price and shows the
   choice card. Checked: the card's products and prices are on the real page (the suite reads the same page itself),
   cheapest first.
2. Buy: the user gives a product page and asks to buy a size. Code opens it, browser-use picks the size, adds it to the
   cart, goes to the checkout and fills the contact and delivery fields with the profile's fake data, then asks the
   user to take over at the first step that is theirs (Pay / Place order, or a sign-in / CAPTCHA the shop shows).
   Checked: the hand-over on the real shop, no card number or other secret put into the page, the block code appends
   (the shop page's link; the amount when a purchase was approved) and nothing ordered. A shop that blocks automation
   is handed over at the block — the suite does not get around bot protection.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh -j 1 shop_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.handover_common import FAKE, Handover, check_handover, host  # noqa: E402
from tests.user_common import check  # noqa: E402

COLLECTION = "https://www.allbirds.com/collections/mens"
PRODUCT = "https://www.allbirds.com/products/mens-tree-runners"
AMOUNT = re.compile(r"\$\s?(\d[\d,]*(?:\.\d\d)?)")


async def page_text(u: U.User, url: str) -> str:
    """The shop's page as the user reads it in their own browser (the suite's own look at the real page)."""
    p = await u.ctx.new_page()
    try:
        await p.goto(url, wait_until="domcontentloaded", timeout=60000)
        await p.wait_for_timeout(6000)
        for _ in range(8):   # the whole collection: it loads as the page scrolls
            await p.mouse.wheel(0, 4000)
            await p.wait_for_timeout(700)
        return " ".join((await p.inner_text("body")).split())
    finally:
        await p.close()


def price_of(option: dict) -> float | None:
    m = AMOUNT.search(" ".join(str(d) for d in option.get("details") or []))
    return float(m.group(1).replace(",", "")) if m else None


async def main():
    async with U.User() as u:
        # 1. compare on the real collection page: code ranks the products the model listed
        a = await u.ask(f"Show me the 3 cheapest men's shoes on {COLLECTION}", label="compare")
        cards = [c for c in await u.choices() if c.get("type") == "choices" and c.get("kind") == "comparison"]
        opts = (cards[-1].get("options") if cards else None) or []
        print(f"INFO compare card: {[(o.get('label'), o.get('details')) for o in opts][:6]}")
        prices = [price_of(o) for o in opts]
        check("compare: the chat shows a choice card with at least 2 of the shop's products and their prices",
              len(opts) >= 2 and all(p is not None for p in prices), opts)
        check("compare: code ranked them cheapest first", all(x <= y for x, y in zip(prices, prices[1:])) if None not in prices else False, prices)
        real = await page_text(u, COLLECTION)
        missing = [o.get("label") for o in opts if str(o.get("label") or "").lower()[:18] not in real.lower()]
        check("compare: every product on the card is on the real page (read by the suite itself)", opts and not missing, missing)
        shown = [p for p in prices if p is not None and f"${p:,.0f}" in real or (p is not None and f"${p:,.2f}" in real)]
        check("compare: the card's prices are the real page's", opts and len(shown) == len(prices), (prices, real[:300]))
        check("compare: every link in the answer leads somewhere", not a.broken_links(), a.broken_links())

        # 2. buy on the real product page: the checkout is filled with fake data and handed over before Pay
        seen = Handover()
        a = await u.ask(f"Buy the Men's Tree Runners in size 10 from {PRODUCT} and have them delivered to my home.",
                        label="buy", on_takeover=seen.look(u, "buy"))
        # a purchase approved on the checkout (its total read off the page): the block then carries that amount
        approved = [c for c in await u.thread_cards() if re.search(r"confirm this purchase|purchase_confirm", c, re.I)]
        print(f"INFO buy approval cards: {approved[:3]}")
        check_handover(a, seen, "buy", sites=("allbirds.com", "shopify.com", "shop.app"), amount=bool(approved))
        check("buy: the shop's page link is in the block code appends", any(host(x).endswith("allbirds.com") for x in a.block_links), a.block_links)


U.setup(vault=False, profile=FAKE)
U.run(main)
