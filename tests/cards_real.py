"""Which credit card to use, US cards, with a real model and from the user's point of view (tests/user_common.py).

The user's Vault holds three US cards (issuing country USA): Chase Sapphire Reserve and Citi Double Cash saved by their
names only (number, expiry and CVC UNKNOWN) and an American Express Gold Card with its details. In Main's chat:
  1. dinner — which card for a US$300 dinner at a New York restaurant: the Credit cards bot answers (its "Continue in
     💳 Credit cards" link); the cards are the user's own, the name-only ones included; the pick is the Amex Gold
     (dining 4x at 1¢ = US$12 against US$9 and US$6) with the value per card; the catalog's date (August 2026);
     no card number.
  2. flight — a US$1,200 flight booked directly on united.com: the Sapphire Reserve (4x on direct airline bookings =
     US$48), with the date.
  3. missing — "I also have a Bread Cashback American Express": the answer says it is not in the card catalog and either
     gives what the issuer's page says (with the date it was read) or says it cannot be verified; the Memory page's
     Vault card then shows "Bread Cashback" by name, marked name-only, once.
  4. number — the user names a Citi Prestige and types its number in the chat: the answer tells them to add the
     number on the Vault card themselves (and does not repeat it); the Vault shows Citi Prestige by name only, and the
     number appears nowhere on the Memory page.
  5. again — a US$80 food-delivery order, naming the Citi Prestige and the Bread Cashback once more: the pick is made
     from all five cards, and the Vault still has each card once.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import cards_common as C  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

GOLD_NO = ("3782 822463 10005", "378282246310005")
TYPED_NO = ("5424 1800 0000 0004", "5424180000000004")


def no_number(text: str) -> bool:
    flat = text.replace(" ", "").replace("-", "")
    return not C.CARD_NO.search(text) and GOLD_NO[1] not in flat and TYPED_NO[1] not in flat


def by_cards(a: U.Answer, step: str) -> None:
    check(f"{step}: answered by the Credit cards bot (its link in the answer)", "Credit cards" in a.bot_link, a.bot_link)
    check(f"{step}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.seconds)
    check(f"{step}: the answer says the card terms are from the catalog as of August 2026", bool(C.CATALOG_DATE.search(a.text)), a.text[-900:])
    check(f"{step}: no card number in the answer", no_number(a.text), a.text[:300])


async def main():
    async with U.User() as u:
        # 1. a dinner in New York: the user's three cards, two of them by name only
        a = await u.ask("Which of my credit cards should I use for a US$300 dinner at a restaurant in New York tonight?", label="dinner")
        by_cards(a, "dinner")
        check("dinner: the user's own cards are compared, the name-only ones included",
              a.mentions("Sapphire Reserve") and a.mentions("Double Cash") and a.mentions("Gold"), a.text[:900])
        check("dinner: the pick is the Amex Gold (4x on dining)",
              bool(re.search(r"(use|pick|best|recommend|go with|winner)[^.\n]{0,80}Gold|Gold[^.\n]{0,80}(best|wins|top|highest|pick)", a.text, re.I)),
              a.text[:600])
        vals = [v for v in ("12", "9", "6") if re.search(rf"\$\s?{v}(\.00)?(?![\d,])|{v}\.00\b", a.text)]
        check("dinner: the value per card in dollars (at least two of US$12 / US$9 / US$6)", len(vals) >= 2, (vals, a.text[:900]))

        # 2. a flight booked directly with the airline
        a = await u.ask("Which of my cards should I use for a US$1,200 flight booked directly on united.com?", label="flight")
        by_cards(a, "flight")
        check("flight: the pick is the Sapphire Reserve (4x on flights booked directly)",
              bool(re.search(r"(use|pick|best|recommend|go with|winner)[^.\n]{0,80}Sapphire Reserve|Sapphire Reserve[^.\n]{0,80}(best|wins|top|highest|pick)",
                             a.text, re.I)), a.text[:600])
        check("flight: its value, US$48 (4,800 points)", bool(re.search(r"\$\s?48(\.00)?(?![\d,])|4,?800\b", a.text)), a.text[:900])

        # 3. a card that is not in the catalog
        a = await u.ask("I also have a Bread Cashback American Express card. Would it beat my other cards for a US$500 TV at Best Buy?",
                        label="missing", timeout=1200)
        check("missing: answered by the Credit cards bot", "Credit cards" in a.bot_link, a.bot_link)
        check("missing: the answer says the Bread Cashback is not in the card catalog",
              bool(re.search(r"(not|isn't|isn’t|no row|no entry|missing)[^.\n]{0,60}catalog|catalog[^.\n]{0,60}(doesn't|doesn’t|does not|no row|no entry|not (list|includ|cover))",
                             a.text, re.I)), a.text[:900])
        check("missing: it gives what the issuer's page says with a date, or says the terms cannot be verified",
              bool(re.search(r"(Oct(ober)?\.?|2026-10|10/\d{1,2}/2026)[^\n]{0,12}2026|2026-10-\d\d|could(n't| not) (be )?verif|unverified|not (been )?verified|can(not|'t) verify",
                             a.text, re.I)), a.text[-900:])
        check("missing: no card number in the answer", no_number(a.text), a.text[:300])
        cards = await C.vault_cards(u)
        bread = C.find(cards, "bread")
        check("missing: the Vault card now shows the Bread Cashback by name, once, marked name-only",
              len(bread) == 1 and bread[0]["name_only"], cards)

        # 4. the user types a card number into the chat
        a = await u.ask(f"Please also remember my Citi Prestige card, the number is {TYPED_NO[0]}, it expires 09/28.", label="number")
        check("number: the answer tells the user to add the number on the Vault card (Memory → Vault) themselves",
              bool(C.ADD_IN_VAULT.search(a.text)), a.text[:600])
        check("number: the answer does not repeat the card number", no_number(a.text), a.text[:300])
        cards = await C.vault_cards(u)
        prestige = C.find(cards, "prestige")
        check("number: the Vault card shows the Citi Prestige by name only (no number stored)",
              len(prestige) == 1 and prestige[0]["name_only"] and "0004" not in prestige[0]["line"], cards)
        page = await C.memory_page_text(u)
        check("number: the number appears nowhere on the Memory page", TYPED_NO[1] not in page.replace(" ", "").replace("-", "")
              and "5424" not in page, [ln for ln in page.splitlines() if "5424" in ln or "0004" in ln][:5])

        # 5. the user names the two cards again: the pick uses all five, the Vault keeps each once
        a = await u.ask("Which card for a US$80 Uber Eats order? Remember I have the Citi Prestige and the Bread Cashback too.", label="again")
        by_cards(a, "again")
        check("again: the pick is one of the user's five cards",
              a.mentions("Gold", "Sapphire Reserve", "Double Cash", "Prestige", "Bread"), a.text[:600])
        cards = await C.vault_cards(u)
        names = [C.norm(c["name"]) for c in cards]
        check("again: the Vault has each card once (the names said twice were not added again)",
              len(names) == len(set(names)) and len(C.find(cards, "prestige")) == 1 and len(C.find(cards, "bread")) == 1, names)
        check("again: the Vault shows the three cards the user started with", all(C.find(cards, w) for w in ("sapphire reserve", "double cash", "gold")), names)
        check("again: the full card shows only its last digits, the name-only ones no number",
              all(GOLD_NO[1] not in c["line"].replace(" ", "") for c in cards) and all(not C.CARD_NO.search(c["line"]) for c in cards), cards)
        check("the chat never showed a card number in an answer", no_number(" ".join(
            t for t in await u.page.locator(".msg.assistant .bubble").all_inner_texts())), "")


U.setup(vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100",
                              "address_home": "12 Mission St, San Francisco, CA 94105"})
for name in ("Chase Sapphire Reserve", "Citi Double Cash"):   # added on the Vault card by name and country
    K.vault_item(name, "card", {}, country="USA")
K.vault_item("American Express Gold Card", "card", {"number": GOLD_NO[0], "expiry": "11/29", "cvc": "1234", "holder": "LUCAS LU"}, country="USA")
U.run(main)
