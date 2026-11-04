"""Which credit card to use, Singapore and Hong Kong cards, with a real model and from the user's point of view
(tests/user_common.py). The user lives in Singapore and starts with no cards in the Vault. In Main's chat:
  1. ask — which card for a S$180 dinner in Singapore: the Money bot (its link) asks which cards the user holds
     (one question, no pick yet, no card number asked for).
  2. cards — the user answers in the same chat (a DBS Altitude and a UOB One from Singapore, an HSBC Red from Hong
     Kong): the block code appends gives the catalog's date and the note that the Singapore rows are unverified; the
     question is not asked again. The Memory page's Vault card shows the three
     cards by name and country, marked name-only.
  3. The user adds a Hang Seng Travel+ Visa Signature on the Vault card by its name, with no country.
  4. hk — which card for a HK$1,500 dinner in Hong Kong: the block gives the foreign fee on the Singapore cards and the
     catalog's date.
  5. named — the user names a Citi PremierMiles from Singapore (and the UOB One again) for a S$1,500 flight on
     singaporeair.com: the Vault shows it by name with Singapore, and the UOB One still once.
No card number in any answer. (Which card the model picks and the values it quotes are its wording, not checked.) One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_markets_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import cards_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SG_C = re.compile(r"singapore|\bSG\b", re.I)
HK_C = re.compile(r"hong kong|\bHK\b", re.I)


async def main():
    async with U.User() as u:
        # 1. no cards yet: the bot asks once
        a = await u.ask("Which of my credit cards should I use for a S$180 dinner at a restaurant in Singapore?", label="ask")
        check("ask: answered by the Money bot (its link in the answer)", "Money" in a.bot_link, a.bot_link)
        # with no cards on file the bot asks (a choice card, or an answer that comes with no takeover, no approval and
        # no pick made from the Vault): state, never the words of the question
        asked0 = await u.choices()
        check("ask: with no cards on file the first turn asks (a choice card, or an answer with no takeover and no approval)",
              bool(a.text.strip() or asked0) and not a.takeovers and not a.approvals and not a.timed_out, (a.takeovers, a.approvals, a.text[:300]))
        check("ask: no card number in the question", not C.CARD_NO.search(a.text + " " + U.choice_text(asked0)), a.text[:300])

        # 2. the user answers in the same chat
        a = await u.ask("I have a DBS Altitude Visa Signature and a UOB One card, both issued in Singapore, and an HSBC Red Credit Card "
                        "from Hong Kong.", label="cards", new_chat=False)
        check("cards: answered by the Money bot", "Money" in a.bot_link, a.bot_link)
        check("cards: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.seconds)
        check("cards: the block code appends after the answer gives the catalog's date (August 2026)",
              "Card terms from the catalog as of" in a.block and bool(C.CATALOG_DATE.search(a.block)), a.block)
        check("cards: the block code appends after the answer says the Singapore rows are unverified seeds",
              "are unverified seeds" in a.block, a.block)
        check("cards: the question is not asked again (no second choice card, no takeover, no approval)",
              len(await u.choices()) == len(asked0) and not a.takeovers and not a.approvals, (len(asked0), a.takeovers, a.approvals))
        check("cards: no card number in the answer", not C.CARD_NO.search(a.text), a.text[:300])
        # cards named in the chat are saved when the request ends (memory extraction, a few seconds after the answer):
        # the user looks at the Memory page, and again a little later
        for _ in range(8):
            cards = await C.vault_cards(u)
            alt, uob, red = C.find(cards, "altitude"), C.find(cards, "uob", "one"), C.find(cards, "red")
            if alt and uob and red:
                break
            await u.page.wait_for_timeout(5000)
        check("cards: the Vault card shows the DBS Altitude and the UOB One with Singapore, the HSBC Red with Hong Kong",
              len(alt) == 1 and len(uob) == 1 and len(red) == 1 and SG_C.search(alt[0]["country"]) and SG_C.search(uob[0]["country"])
              and HK_C.search(red[0]["country"]), cards)
        check("cards: all three marked name-only (no number stored)", all(c["name_only"] for c in alt + uob + red), cards)

        # 3. the user adds a card on the Vault card by its name, without a country
        await C.add_card_on_vault(u, "Hang Seng Travel+ Visa Signature")

        # 4. a dinner in Hong Kong: Hong Kong's table for the Hong Kong cards
        a = await u.ask("I'm in Hong Kong next week: which of my cards should I use for a HK$1,500 dinner at a restaurant there?", label="hk")
        check("hk: answered by the Money bot", "Money" in a.bot_link, a.bot_link)
        check("hk: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.seconds)
        check("hk: the block code appends after the answer gives the foreign transaction fee of the Singapore cards (catalog)",
              "foreign transaction fee" in a.block, a.block)
        check("hk: the block code appends after the answer gives the catalog's date (August 2026)",
              "Card terms from the catalog as of" in a.block and bool(C.CATALOG_DATE.search(a.block)), a.block)
        check("hk: no card number in the answer", not C.CARD_NO.search(a.text), a.text[:300])

        # 5. a card named in the chat (and one named again)
        a = await u.ask("I forgot: I also have a Citi PremierMiles card from Singapore, besides my UOB One. Which card is best for a "
                        "S$1,500 Singapore Airlines flight booked on singaporeair.com?", label="named")
        check("named: answered by the Money bot", "Money" in a.bot_link, a.bot_link)
        check("named: the block code appends after the answer gives the catalog's date (August 2026)",
              "Card terms from the catalog as of" in a.block and bool(C.CATALOG_DATE.search(a.block)), a.block)
        check("named: no card number in the answer", not C.CARD_NO.search(a.text), a.text[:300])
        cards = await C.vault_cards(u)
        pm = C.find(cards, "premiermiles") or C.find(cards, "premier", "miles")
        check("named: the Vault card shows the Citi PremierMiles by name with Singapore, marked name-only",
              len(pm) == 1 and pm[0]["name_only"] and SG_C.search(pm[0]["country"]), cards)
        names = [C.norm(c["name"]) for c in cards]
        check("named: each card once in the Vault (the UOB One named again was not added twice)",
              len(C.find(cards, "uob", "one")) == 1 and len(names) == len(set(names)) and len(cards) == 5, names)


U.setup(vault=False, profile={"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567",
                              "address_home": "10 Anson Road, Singapore 079903"})
U.run(main)
