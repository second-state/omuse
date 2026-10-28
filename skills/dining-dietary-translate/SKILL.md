---
name: dining-dietary-translate
description: 吃饭与语言：过敏/清真/素食/低钠等饮食要求写成当地语言的卡片，拍菜单或路牌照片翻译（视觉模型），点餐与餐桌礼仪、小费习惯；订餐厅交给 restaurant-booking / michelin-dining Eating and language help — allergy, halal, vegetarian and other dietary cards in the local language, menu and sign photos translated with the vision model, ordering and table customs; table bookings go to the restaurant-booking / michelin-dining skills.
---
# Dining, dietary needs & translation / 吃饭与语言

No browsing for this: your own languages, the vision model for photos (file_look), memory for the user's needs
(memory_search "allergy", "diet", "halal", "vegetarian", "spice"). Ask once if the dietary need is not known.

## 1. Dietary cards (text the user shows on the phone, or prints)
For each destination language the user names (Japanese, Korean, Mandarin / Cantonese in traditional or simplified
script, Thai, Vietnamese, Bahasa Indonesia / Malay, Tagalog), write a short card:
- the restriction stated clearly ("I have a severe peanut allergy. Even a trace can make me very ill. Please check
  the sauces and cooking oil."), the common hidden sources in that cuisine (dashi / fish sauce / shrimp paste /
  oyster sauce / lard / bone broth / peanut oil / MSG, as relevant), what the person **can** eat, and a thank-you;
- halal: no pork, no alcohol in cooking, halal-certified meat only; in Japan and Korea mention "pork-based broth" and
  mirin / sake; vegetarian vs vegan vs "no beef" spelled out — "vegetarian" is understood differently in each country;
- the local script first, then romanisation, then English; keep it to 3–5 lines per card. make_pdf one page per
  language if the user wants a printable, else answer in chat.
Say honestly that a card helps but does not guarantee a kitchen's practice; for severe allergies suggest choosing
restaurants that state allergen policies and carrying medication.

## 2. Menus, signs, labels — photos
file_look on each uploaded photo with the question "transcribe the text and translate it to <language>; for a menu list
each dish with price and likely main ingredients; flag items containing <allergens>". Unreadable parts → say so, ask
for a closer photo. For packaged food, read the ingredient list for the allergens and the common hidden names (e.g.
Japanese 乳 / 卵 / 小麦 / えび / かに / そば / 落花生 labelling, Korean 땅콩 / 우유 / 밀, Chinese 花生 / 牛奶 / 麸质).

## 3. Ordering and customs (short, per country, only what the user asks)
Japan: say "sumimasen" to call staff, no tipping, cash at small places, oshibori, no eating while walking in some
areas, slurping noodles fine; Korea: side dishes are free and refilled, pour drinks for others, no tipping;
China / Taiwan / HK: shared dishes, tea pouring tap thanks, no tipping, "dǎbāo" to take away; Thailand: fork and
spoon, mild spice = "mai phet", small tips; Singapore / Malaysia hawker centres: "chope" a table with a tissue pack,
self-return trays, halal stalls marked; Vietnam: street food is normal, agree prices where unlisted; Indonesia / Bali:
most restaurants add service and tax. Useful phrases in the local script + romanisation on request (water, bill,
no ice, less sugar, allergy sentence).

## 4. Finding and booking a place
- `places_search` only (Google Maps data: query such as "vegan ramen", city, area, min_rating, open_now; without the
  key it falls back to web search by itself) — no `web_search` beside it. Book on the restaurant's own site or the platform the user names.
The `restaurant-booking` skill (Google Maps / Chope / TableCheck / official sites) or `michelin-dining` (starred and
Bib Gourmand via the travel data connector) — filter by the dietary need and say which places state it on their page;
never promise a kitchen is allergen-free.

## 5. Answer
The cards (local script / romanisation / English), the translated menu with flags, and the one or two customs that
matter for the meal. No invented dish names or prices from a photo you could not read.

## Remember what you learn (for next time)
Whatever the user tells you here is worth keeping, so they are never asked twice — but each kind has its place:
- **Profile fields** (name as in the passport, birthday, nationality, phone, emails, addresses, company, loyalty
  programme names, dietary needs, home currency, employer's expense policy, insurer's name and assistance line):
  profile_suggest(field, value) — standard fields or `custom:<label>` (e.g. `custom:passport expiry`,
  `custom:allergies`, `custom:travel insurer`); the user confirms it on the Memory page.
- **Preferences and facts** ("prefers aisle seats", "always files SGAC for the family", "company reimburses per diem
  at …"): memory_remember with its domain.
- **Secrets** — passport, ID, membership and card numbers, and photos of them: never into memory, files or the
  answer. A passport photo the user uploads may be read once with file_look to fill the form or the profile fields
  (name, birthday, nationality, expiry); then ask the user to save the number in the Vault (Memory → Profile → Vault),
  which browser_fill_secret uses from then on, and say the photo itself should not stay in the workspace.
- Uploaded documents that will be needed again (insurance policy PDF, expense policy, company template): keep them
  where they were uploaded and memory_remember the path (domain=account / work).
