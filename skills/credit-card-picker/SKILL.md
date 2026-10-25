---
name: credit-card-picker
description: 这笔消费该刷哪张卡：根据用户档案里登记的信用卡和内置的信用卡权益数据库（美国、香港、新加坡；按类别的积分/里程/返现、上限、年费、外币手续费），算出每张卡能拿到多少，推荐最优的一张，并指出没持有但更划算的卡 Which card to use for a purchase: the cards the user registered in their profile against a built-in card-terms catalog (US, HK, SG — earn rates by category, caps, fees, FX fees), the value each card returns, the best pick, and a better card they don't hold.
---
# Which credit card should I use? / 该刷哪张卡

Two inputs: the user's own cards (profile) and the card catalog shipped with this skill (`cards/<market>.md`, built
from `cards/catalog.json`; how to update it is in `KNOWLEDGE.md` at the repository root). No browser unless a card is
missing from the catalog. Never ask for or store card numbers — names only; numbers live in the Vault if at all.

## 1. The user's cards (profile first)
- profile_get fields `custom:credit cards` (and memory_search "credit card"): a list like
  `DBS Altitude Visa Signature; HSBC Revolution; UOB One` — issuer + product, optionally a nickname or last-4.
- None registered → ask the user to list their cards once (issuer and product name as printed on the card; which
  market / country issued them; nothing else), then profile_suggest(field="custom:credit cards", value=<the list>)
  so they are never asked again (they confirm it on the Memory page). Also useful and worth proposing when told:
  `custom:home currency`, `custom:card market` (US / HK / SG), `custom:card preferences` ("miles over cashback").

## 2. The catalog
- load_skill(name="credit-card-picker", file="cards/<market>.md") for the user's market (`us`, `hk`, `sg`; load two
  when the cards span markets). Each row: issuer, card and aliases, network, unit and point value, annual fee, FX fee,
  earn rates per canonical category, caps, merchant rules with end dates, verification date; the Notes section holds
  the conditions that change a rate (direct bookings only, monthly caps, minimum spend, promo periods).
- Match the user's cards to rows by issuer + product (aliases help; tiers differ: "Platinum" ≠ "Signature"). A card
  with no row: say so, browser_read the issuer's product page for its earn rates (2–3 pages at most), use what you read
  with the date, and tell the user it can be added to the catalog (KNOWLEDGE.md).
- Rows marked **UNVERIFIED (seed)** are starting points from general knowledge: before recommending on them, say they
  are unverified or verify on the issuer page (browser_read) and give the date.

## 3. Work it out (calculate, never in your head)
- Classify the purchase into one category (dining, travel — direct airline / hotel vs portal vs OTA matters —,
  transit, groceries, gas, online_retail, streaming, entertainment, drugstore, utilities, insurance, foreign, other),
  the amount, the currency and the country of the merchant (foreign spend → FX fee applies; online shops abroad too).
- Per card: rate for that category (merchant rules first, then caps — a cap already used this month means the base
  rate; ask if unknown), value = amount × rate × point value (cashback: amount × %), minus the FX fee on foreign
  spend; note minimum-spend tiers (UOB One-style cards return nothing below the tier) and annual-fee waivers.
- Rank; show the top two with the maths; add one line for a catalog card the user doesn't hold that beats theirs by a
  clear margin (same market), without pushing it.
- Big tickets (a trip, a hotel block): also mention travel insurance included with the card when the catalog notes
  it, and foreign-transaction fees on the whole amount.

## 4. Answer
A short table (card / rate / value in home currency / caveat), the pick in one sentence, the caveats (caps, minimum
spend, verification date, unverified rows), and "the numbers come from the catalog as of <date>; terms change".
Offer to remember a preference ("you prefer miles") with memory_remember.

## Remember what you learn (for next time)
The card list, market and preferences → profile_suggest (`custom:credit cards`, `custom:card market`,
`custom:card preferences`); "this cap is already used this month" is temporary — don't store it. Card numbers,
expiry dates and CVCs: never, anywhere; if the user types one, tell them to put it in the Vault instead.
