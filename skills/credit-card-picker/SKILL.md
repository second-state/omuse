---
name: credit-card-picker
description: 这笔消费该刷哪张卡：根据用户保险箱里的信用卡（卡名）和内置的信用卡权益数据库（美国、香港、新加坡；按类别的积分/里程/返现、上限、年费、外币手续费），算出每张卡能拿到多少，推荐最优的一张，并指出没持有但更划算的卡 Which card to use for a purchase: the user's cards in the Vault (by name) against a built-in card-terms catalog (US, HK, SG — earn rates by category, caps, fees, FX fees), the value each card returns, the best pick, and a better card they don't hold.
---
# Which credit card should I use? / 该刷哪张卡

Two inputs: the user's own cards (the Vault, by name) and the card catalog shipped with this skill (`cards/<market>.md`, built
from `cards/catalog.json`; how to update it is in `KNOWLEDGE.md` at the repository root). No browser unless a card is
missing from the catalog. Never ask for or store card numbers — names only; numbers live in the Vault if at all.

## 1. The user's cards (the Vault)
- vault_list: the user's cards are the Vault's `card` items — the name (issuer + product, e.g. `Citi Prestige`) and,
  when known, the country that issued it as a field of its own (`Citi Prestige · issued in USA`). The country gives the
  market (`us`, `hk`, `sg`); a card without one: tell from the name if it is clear (a DBS card is Singaporean), else
  ask the user once and add it with vault_add_card(name, country, card_id). A card with its number, expiry and CVC `UNKNOWN` was saved by its name only: it counts as a card the
  user holds for picking; it just cannot be filled or paid with until the user adds those in Memory → Vault.
- A card the user names that is not there ("I also have a UOB One from Singapore") → vault_add_card(name="UOB One",
  country="Singapore", card_id="sg-uob-one-card"; the country only when known, card_id = the ID column of the catalog
  row that is this card, empty when the catalog has no row for it), one call per card, before you answer — never a
  number. A Vault card listed without a card_id that has a catalog row: vault_add_card again with its name and the
  card_id (it is kept with the card). Name it
  as issuer + product the way the user wrote it, without "card" / "credit card" tacked on ("UOB One", "HSBC Red"), and
  never add a card the Vault already holds under a slightly different name ("UOB One Card" is "UOB One").
- None in the Vault → ask the user once which cards they hold (issuer and product as printed on the card, and the
  country that issued each; nothing else), then vault_add_card(name, country, card_id) for each, so they are never asked again.
- Never read cards from the profile or write them there: cards live in the Vault only.

## 2. The catalog
- load_skill(name="credit-card-picker", file="cards/<market>.md") for the user's market (`us`, `hk`, `sg`; load two
  when the cards span markets). Each row: issuer, card and aliases, network, unit and point value, annual fee, FX fee,
  earn rates per canonical category, caps, merchant rules with end dates, verification date; the Notes section holds
  the conditions that change a rate (direct bookings only, monthly caps, minimum spend, promo periods).
- Match the user's cards to rows by issuer + product (aliases help; tiers differ: "Platinum" ≠ "Signature"). A card
  with no row: browser_read (or web_search) the issuer's product page for its earn rates (2–3 pages at most). The
  answer must then say, in so many words, "<card> is not in the card catalog", and either "per <issuer>'s page, read
  <today's date as YYYY-MM-DD>: <the rates>" or, when no issuer page confirmed them, "I could not verify its terms" —
  never a silent assumption ("assuming its standard 2%"). Add that it can be added to the catalog (KNOWLEDGE.md).
- Rows marked **UNVERIFIED (seed)** are starting points from general knowledge: before recommending on them, say they
  are unverified or verify on the issuer page (browser_read) and give the date.

## 3. Work it out (calculate, never in your head)
- Classify the purchase into one category (dining, travel — direct airline / hotel vs portal vs OTA matters —,
  transit, groceries, gas, online_retail, streaming, entertainment, drugstore, utilities, insurance, foreign, other),
  the amount, the currency and the country of the merchant (foreign spend → FX fee applies; online shops abroad too).
- Per card: rate for that category (merchant rules first, then caps — a cap already used this month means the base
  rate; ask if unknown), value = amount × rate × point value (cashback: amount × %), minus the FX fee on foreign
  spend; note minimum-spend tiers (UOB One-style cards return nothing below the tier) and annual-fee waivers.
- Rank; show the top two with the maths (the table lists every card the user holds, at least two rows, even when one
  card clearly wins); add one line for a catalog card the user doesn't hold that beats theirs by a
  clear margin (same market), without pushing it.
- Big tickets (a trip, a hotel block): also mention travel insurance included with the card when the catalog notes
  it, and foreign-transaction fees on the whole amount.

## 4. Answer
A short table (card / rate / value in home currency / caveat), the pick in one sentence, the caveats (caps, minimum
spend, verification date, unverified rows), and "the numbers come from the catalog as of <date>; terms change".
Each of the user's cards with no catalog row gets its own row or line, worded exactly like this: "<card>: not in the
card catalog — <rates> per <issuer>'s page, read <YYYY-MM-DD>" or "<card>: not in the card catalog — I could not
verify its terms". Its value is never computed from an assumed rate.
Offer to remember a preference ("you prefer miles") with memory_remember.

## Remember what you learn (for next time)
Each new card → vault_add_card(name="<issuer product>", country="<issuing country, if known>", card_id="<the catalog
row's ID, if it has one>"), one call per card. A lasting preference ("miles
over cashback") → memory_remember. "This cap is already used this month" is temporary — don't store it. Card numbers,
expiry dates and CVCs: never in the chat, a memory or a tool call; if the user types one, tell them to add it on the
Vault card (Memory → Vault) themselves.
