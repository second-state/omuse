---
name: travel-health-emergency
description: 旅途中的紧急情况：护照丢失/被盗（报警回执、使领馆、临时旅行证件）、生病受伤（能讲英文的医院、保险救援电话、药房和药品规定）、各国急救电话、台风地震预警、保险理赔要留的材料 Emergencies on the road — lost or stolen passport (police report, consulate, emergency travel document), illness or injury (English-speaking hospitals, the insurer's assistance line, pharmacies and medication rules), emergency numbers, typhoon and earthquake alerts, what the insurance claim will need.
---
# Health & emergencies while travelling / 旅途紧急情况

Calm, numbered, specific. Life-threatening → the local emergency number first (below), then everything else.
Verify any phone number or address you did not read on an official page (browser_read, date checked); never give a
number from a web search snippet as an emergency line.

## 1. Emergency numbers (checked Oct 2026 — still verify when it matters)
Japan 110 police / 119 fire & ambulance · Korea 112 / 119 · Mainland China 110 / 120 ambulance / 119 fire · Hong Kong
999 · Taiwan 110 / 119 · Singapore 999 / 995 · Malaysia 999 · Thailand 191 police / 1669 ambulance / 1155 tourist
police · Vietnam 113 / 115 · Indonesia 110 / 118–119 · Philippines 911. Most have English lines or interpreters; the
hotel front desk can call too.

## 2. Illness or injury
- Find the user's insurance first: gmail_search `insurance (policy OR certificate OR assistance OR travel)` — the
  **24-hour assistance line** on the policy arranges hospitals, direct billing and evacuation; call it before paying
  large bills (phone_call if configured, approved by the user). Credit-card travel cover: check the card's benefits mail.
- Where to go: the embassy's list of English-speaking hospitals / clinics (browser_read the embassy page for the
  city), international clinics in big cities, hotel doctors; pharmacies: Japan (drugstores, pharmacist for some
  medicines), Korea (약국), Thailand (pharmacies sell many medicines over the counter), Singapore (Guardian, Watsons,
  polyclinics). Say what you found with the date; don't rank hospitals by quality from memory.
- Medication: bring prescriptions in generic names; Japan's import rules (1-month supply, banned stimulants, Yakkan
  Shoumei for more) are in `entry-requirements`; some drugs legal at home are controlled in Japan, Korea, Singapore,
  Thailand, UAE transit.
- Documents for the claim: medical report with diagnosis, itemised invoices and receipts, prescriptions, the
  assistance case number, the police report if an accident; keep boarding passes. Draft the claim email afterwards.
- Food / water, heat, dengue (mosquito repellent in SE Asia), altitude, air quality (AQI apps) when relevant.

## 3. Lost or stolen passport
1. Police report at the nearest station (needed for the replacement and the insurance; in Japan at a kōban).
2. The user's embassy / consulate in that city: browser_read its page for emergency travel documents, hours and what
   to bring (photos, the police report, a copy of the old passport — memory_search / files_search "passport copy",
   proof of travel); many issue an emergency passport in a day or two; appointments online.
3. The host country's exit requirement: a replacement visa / exit permit may be needed before departure (Japan:
   re-entry not needed for tourists; China and Vietnam usually need an exit visa from the local immigration office).
4. Tell the airline / change flights if the dates slip (`flight-disruption`); insurance often covers the extra nights.
5. Afterwards: report the passport number as lost to the home authority; memory_remember the new document details
   only in the vault (the user adds it), never in memory or files.

## 4. Natural events and alerts
Typhoons (Japan / Taiwan / HK / Philippines, Jul–Oct), earthquakes (Japan, Taiwan — the Japan Meteorological Agency
and NHK World pages, the "Safety tips" app), floods (Thailand / Vietnam monsoon), volcanic alerts (Indonesia): check
the official agency page when the user asks or when a booked day is affected; airlines and rail post suspensions a
day ahead — watch_create on the operator page and notify_user. Embassies' travel advisories for the country.

## 5. Money, phone, theft
Lost cards → freeze in the app, issuer line from the card / its app; stolen phone → remote lock, carrier SIM block,
the police report for insurance; keep a second card and copies of documents separately (say it once, kindly).

## 6. Answer
Numbered steps for the situation, the verified numbers and addresses with dates, what the insurer needs, the drafts
and reminders you can set up, and one line on travel insurance if none was found (medical costs are the largest
travel-claim category; lost luggage and delays follow) — no selling.

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
