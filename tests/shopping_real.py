"""Shopping on a long product page with a real model, from the user's point of view (tests/user_common.py,
tests/shop_common.py). The shop is Kaya Mobile (tests/pages/kaya_*.html at http://shop.test:8099): a search-results page
for "iPhone 17 Pro Max case" whose header carries a mega-menu of 300 department links, then 28 products — sponsored
ones first, look-alikes for the wrong phone, accessories that are not cases, and the cases the user wants far down the
page; a product page per item (colour, Add to Cart, Buy Now), the cart, the checkout and the order confirmation.

1. choices — "show me the three best-rated cases under S$25 with at least 200 ratings as options I can choose from".
   Checked in the chat: choice cards with the "Verified" badge (their names and details were quoted from pages the
   agent read); exactly the Aurora Glitter (4.7), Slim Leather (4.6) and Carbon Fibre (4.5) cases, each with the price
   the page shows — not the sponsored, wrong-model, accessory or over-budget ones.
2. pick — the user presses Choose on the Slim Leather card. Checked: the pick is sent as the user's message, the cards
   are then closed (their buttons disabled), and the answer says the Slim Leather case (S$24.90) is in the cart. Adding
   to the cart is not a purchase: nothing is handed over and nothing is ordered, and even with "Auto-approve browser
   actions" off (Settings) no approval dialog asks about Add to Cart.
3. buy — "buy it, home delivery, my Visa from the Vault; I'll press Place order myself". Sentinel never lets the agent
   place an order: the agent fills the checkout and hands it over. Checked: a takeover on Kaya Mobile's checkout; the
   user's press of "Place your order" reaches the confirmation with exactly the Slim Leather case, S$24.90, the delivery
   address and the Vault card filled in; the answer gives the order number KM-50731, the item and the amount; Status →
   Ledger lists the order; the card number never appears in the chat.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh shopping_real"""
import os
import re
import sys
from urllib.parse import parse_qs, urlsplit

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import shop_common as S  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SEARCH = f"{S.SHOP}/kaya_search.html"
CARD_NO = "4111111111111111"
WANTED = {"Aurora Glitter": "21.90", "Slim Leather": "24.90", "Carbon Fibre": "18.90"}
WRONG = ("Lumi Crystal", "Titan Pro", "Kickstand", "Lens Protector", "iPhone 16", "Privacy Screen", "MagSafe Silicone", "Crossbody")


async def choice_cards(u: U.User) -> list[str]:
    return [" ".join(t.split()) for t in await u.page.locator(".bubble.choices .choice").all_inner_texts()]


async def main():
    async with U.User() as u:
        # ---- 1. options to choose from
        a = await S.ask(u, f"I need a case for my iPhone 17 Pro Max. Here are Kaya Mobile's search results: {SEARCH} — show me the three "
                           "best-rated cases under S$25 that have at least 200 ratings, as options I can choose from. I'll pick one for my cart.",
                        label="choices")
        cards = await choice_cards(u)
        print(f"INFO choice cards: {cards}")
        badge = await u.page.locator(".bubble.choices", has_text="Verified").count()
        check("choices: the answer came (no time-out)", bool(a.text.strip() or cards) and not a.timed_out, a.text[:300])
        check("choices: the options are shown as choice cards in the chat", len(cards) >= 2, (cards, a.text[:600]))
        check("choices: the cards carry the Verified badge (quoted from the pages the agent read)", badge > 0, cards)
        named = {k for k in WANTED if any(k.lower() in x.lower() for x in cards)}
        check("choices: exactly the three best-rated cases under S$25 with 200+ ratings (Aurora Glitter, Slim Leather, Carbon Fibre)",
              named == set(WANTED) and len(cards) == 3, cards)
        check("choices: no sponsored, wrong-model, accessory or over-budget item is offered",
              not [w for w in WRONG if any(w.lower() in x.lower() for x in cards)], cards)
        check("choices: each card shows the price the shop's page shows",
              all(any(k.lower() in x.lower() and v in x for x in cards) for k, v in WANTED.items() if k in named) and bool(named), cards)

        # ---- 2. the user picks one
        pick = u.page.locator(".bubble.choices .choice", has_text="Slim Leather").locator("button")
        if await pick.count():
            await S.set_auto(u, False)   # the user wants to see what needs approval: adding to the cart must not
            a = await S.ask(u, "", label="pick", new_chat=False, send=lambda: pick.first.click())
            asked = [" ".join(t.split()) for t in await u.page.locator(".msg.user .bubble").all_inner_texts()]
            check("pick: the user's pick is sent as their message", any("Slim Leather" in q for q in asked[-2:]), asked[-2:])
            check("pick: the cards are closed after the pick (their buttons disabled)",
                  await u.page.locator(".bubble.choices .choice button[disabled]").count() >= 3, await choice_cards(u))
            if not re.search(r"cart", a.text, re.I) and "?" in a.text:
                a = await S.ask(u, "Any colour is fine — please add it to my cart.", label="pick follow-up", new_chat=False)
            check("pick: the answer says the Slim Leather case (S$24.90) is in the cart",
                  re.search(r"Slim Leather", a.text, re.I) and re.search(r"cart", a.text, re.I) and "24.90" in a.text, a.text[:700])
            check("pick: adding to the cart needed no takeover and placed no order", "KM-50731" not in a.text, a.text[:400])
            check("pick: with auto-approve off, Add to Cart still asked for no approval (it is not a purchase)",
                  not [x for x in a.approvals if re.search(r"add to cart", x, re.I)], a.approvals)
            await S.set_auto(u, True)
        else:
            check("pick: a Slim Leather card to press", False, await choice_cards(u))

        # ---- 3. buy it: the agent fills the checkout, the user presses Place your order
        seen = {"url": "", "after": ""}

        async def place(pg, request: str) -> None:
            seen["url"] = await u.view_url()
            print(f"INFO takeover: {request!r} url {seen['url']!r}")
            if "kaya_checkout" in seen["url"]:
                seen["after"] = await S.press(u, f"{S.SHOP}/kaya_checkout.html", "#place", "kaya_done", "checkout")
            print(f"INFO after Place your order: {seen['after']!r}")

        b = await u.ask("Please buy it now — home delivery, and pay with my Visa from the Vault. I'll press Place order myself.",
                        label="buy", new_chat=False, on_takeover=place, timeout=1200)
        answers = [b]
        if not b.takeovers and not b.timed_out:
            b = await u.ask("Yes, go ahead — my name, phone and address are in my profile and the card is in the Vault.",
                            label="buy follow-up", new_chat=False, on_takeover=place, timeout=1200)
            answers.append(b)
        text = "\n".join(x.text for x in answers)
        takeovers = [t for x in answers for t in x.takeovers]
        check("buy: the answer came (no time-out)", bool(b.text.strip()) and not b.timed_out, b.text[:300])
        check("buy: the user was asked to take over to place the order", any(re.search(r"place|order|pay", t, re.I) for t in takeovers), takeovers)
        check("buy: the page handed over is Kaya Mobile's checkout (the agent did not place the order)", "kaya_checkout" in seen["url"], seen["url"])
        q = {k: v[0] for k, v in parse_qs(urlsplit(seen["after"]).query).items()}
        print(f"INFO the order the user placed: { {k: v for k, v in q.items() if k not in ('card', 'cvc')} }")
        check("buy: the user's press reaches the confirmation with exactly the Slim Leather case at S$24.90",
              "kaya_done" in seen["after"] and "Slim Leather" in q.get("items", "") and q.get("items", "").count("for iPhone") == 1
              and q.get("total") == "24.90", q.get("items"))
        check("buy: … with the delivery address and the Vault card filled in", bool(q.get("address")) and C.flat(q.get("card", "")) == CARD_NO,
              {k: q.get(k) for k in ("name", "address")})
        check("buy: the answer gives the order number, the item and the amount (KM-50731, Slim Leather, S$24.90)",
              "KM-50731" in text and re.search(r"Slim Leather", text, re.I) and re.search(r"(S\$|SGD)\s?24\.90", text), text[:900])
        rows = await C.ledger_rows(u)
        check("buy: Status → Ledger lists the order (KM-50731, 24.90)", any("KM-50731" in r and "24.90" in r for r in rows), rows)
        check("buy: the card number never appears in the chat", CARD_NO not in (await u.thread_text()).replace(" ", ""), "")


U.setup(mail=[], vault=False, profile={"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567",
                              "address_home": "10 Anson Road, #12-01, Singapore 079903"})
K.vault_item("Visa Wei Ming", "card", {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123", "holder": "TAN WEI MING"},
             domains="shop.test")
U.run(main)
