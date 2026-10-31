---
name: cn-arrival-card
description: 代填中国在线入境卡（国家移民管理局 s.nia.gov.cn / NIA 12367 App，入境前或口岸填写，免费，含免签旅客）Fill in China's online arrival card for foreigners (NIA, free, before arrival or at the port, visa-free travellers included).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# China online arrival card / 中国在线入境卡

Since 20 November 2025 foreigners entering mainland China file the arrival card online, free, at https://s.nia.gov.cn/ArrivalCardFillingPC/ (or the NIA 12367 app and the WeChat / Alipay mini programs); it can also be done on the devices at the port, and paper cards still exist. It applies with or without a visa (visa-free entries included) and to air, land and sea arrivals. Exempt: holders of the Foreign Permanent Resident ID card, Mainland Travel Permit holders (HK / Macau residents), group-visa travellers, direct transit under 24 h inside the restricted area, cruise passengers in and out on the same ship, crew. The NIA warns about fake paid sites — only the addresses above.
This is the traveller's own legal declaration: you fill it, the user reviews it and presses Submit.

**How the user wants this done — applies to every step below.** Fill the whole form in one go and end with ONE
browser_request_takeover; never end the task with a question in the chat about a value that has a default. Answer the
defaults yourself and say what you assumed in the takeover reason, so the user corrects it on the page if needed:
every health and declaration question **No** (symptoms, travel history, other names / passports, refused entry,
criminal record, security, goods / cash to declare); **departure date = arrival + 7 days** and **next city = the city
the flight came from** when no return / onward flight or check-out date is known (a one-way ticket and a hotel booking
without a check-out are exactly that case — not a reason to ask); sex from the profile. These are draft answers the
user reviews before pressing Submit, like every value you took from the mail. The only questions you may ask in the
chat, once and before filling: the flight, the stay and the purpose of the visit, when neither the request nor the
mail / calendar / memory has them. Anything else you lack stays empty and is named in the takeover reason. Example
reason: "Filled from your e-ticket, hotel booking, profile and Vault. I answered every health and declaration
question No, set departure to 08/11/2026 (arrival + 7 days) and next city Sydney — change any that are not right,
type the CAPTCHA and press Submit; hand the browser back when the confirmation shows."

## 1. Gather the data (no browser yet, one turn)
- Flight and date from mail: gmail_search `(China OR Shanghai OR PVG OR SHA OR Beijing OR PEK OR PKX OR Shenzhen OR Guangzhou OR CAN OR Chengdu OR Hangzhou) (flight OR e-ticket OR itinerary) newer_than:60d`; the onward /
  return flight if the form asks for the next destination or length of stay.
- Accommodation: gmail_search `(hotel OR booking OR reservation) (China OR Shanghai OR PVG OR SHA OR Beijing OR PEK OR PKX OR Shenzhen OR Guangzhou OR CAN OR Chengdu OR Hangzhou) newer_than:60d` → name and address; or the
  host's address from the request.
- **The user's own address in China** (a resident going home, an owner's flat, family): profile_get the address fields
  — `address_home`, `address_work` and any number of `address:<label>` entries. Exactly one of them is in China → use it
  as the accommodation / address of stay without asking, and say so in the answer (a hotel booking for these dates in
  mail wins over it). Several are in China → **ask in the chat which one to use**, listing them by label, before
  opening the browser. None → the mail / calendar / memory sources above, then the one question. An address the
  user gives in the chat that the profile lacks → profile_suggest `address:<label>` so it is there next time.
- Identity: profile_get → name as in the passport, sex, date of birth, nationality, email, phone. Passport number and
  expiry are in the vault (vault_list, kind id_document) — never asked for in chat, never in the answer. Missing
  profile fields: ask the user to upload the passport data page once, file_look it for name / sex / birth date /
  nationality / expiry, propose those with profile_suggest, and ask them to save the number in the Vault.
- **No mail, or no booking in it** (the gmail tools are missing, or the searches find no flight / stay for this
  trip): in the same turn look at calendar_list_events around the travel dates (flight and hotel entries),
  memory_search "trip", "flight", "hotel" with the country name, and the travel connector's tracked flights if it is
  connected. Still missing → **ask once**, naming exactly what you need: "Which flight do you arrive on (number and
  date)? Where are you staying (hotel name or address)? Or upload the e-ticket / booking confirmation." Fill
  everything else from the profile and the vault meanwhile — never hand the user a blank form because a booking is
  missing. An uploaded PDF or screenshot: file_look it for flight number, date, origin, onward flight, hotel name and
  address. With no mail connected, add one line that connecting a mailbox (Settings → Connections) lets you find
  bookings yourself next time. Keep what the user tells you: memory_remember "Trip <country> <dates>: <flight>,
  staying at <hotel>" (domain=place), so the other travel skills don't ask again.
- **"I'll fill it in myself"** (or the user won't give the trip details): open the official form, fill what you
  already have (profile, vault), then browser_request_takeover with the reason "Fill in the flight and accommodation,
  review the form and press Submit yourself; hand the browser back when the confirmation shows".
- **Defaults — fill these without asking** (only what the user said in this conversation overrides them):
  - every health question → **No**: symptoms (fever, cough, shortness of breath, rash, diarrhoea, any illness) and
    travel history (Africa / South America / yellow-fever countries, farms, contact with sick people or animals);
  - every declaration question → **No**: other names or other passports used, refused entry / deported / overstayed,
    criminal record or convictions, goods, cash or items to declare;
  - sex / gender → the profile's `gender` (as on the passport); missing → ask once and profile_suggest it;
  - next city / next port after this country → the city the arrival flight comes from (the last city before this
    country), unless an onward flight in mail or the request names another;
  - departure date from this country → arrival date + 7 days (count the days on the calendar, e.g. 01/11 → 08/11, 28/11 → 05/12; the calculate tool does no date arithmetic), unless a return / onward flight or the length
    of stay is known; length of stay → the days in between.
- Purpose of visit (when the request does not say it): **ask once**, in the same message as a missing flight or stay.
  Ask in the chat only for the flight, the stay and the purpose — nothing else stops the fill: any other value you do
  not have (occupation, a visa number, …) stays empty on the form and is named in the takeover reason for the user to
  type; the defaults below are never asked.
- The card asks: passport data, nationality, sex, date of birth, visa number or the visa-free policy used, flight and arrival date, purpose, intended address in China (hotel) and length of stay. Collect the visa / policy from the request or the visa email; with no visa in the mail, the default is the visa-free entry for the user's nationality (unilateral visa-free for a holiday or business stay of up to 30 days, e.g. Australian, most EU and Japanese passports) — choose it on the form and say so in the takeover reason, never ask it in the chat.
## 2. Timing
It can be filed in advance at any time once the flight and hotel are known (there is no fixed window); if the trip is more than two weeks away, offer a reminder instead.

## 3. Fill the form (browser)
1. browser_navigate https://s.nia.gov.cn/ArrivalCardFillingPC/ → "Entry Declaration" → I AGREE.
2. Step 1 asks for a photo of the passport's data page (the site reads it): that upload is the user's — hand the page
   over (browser_request_takeover, the reason "upload a photo of your passport's data page, then hand the browser
   back"). If the user attached the photo in this chat, upload it with browser_upload instead.
3. After the upload: **form_fill** with the trip facts from step 1 — flight_no, arrival_airport (from the e-ticket),
   arrival_date, stay_name / stay_address / stay_kind, purpose. Code fills Basic, Personal and Travel Information (by
   their labels) and stops before the companion question; it says what it filled and what it left.
4. Hand the page over again — browser_request_takeover, the reason naming the companion question, Submit and anything
   form_fill left. After the hand-back: browser_snapshot and quote the reference if it shows.
5. Only when form_fill says no form function knows the page (the site changed), fill it field by field instead:

1. browser_navigate the official page above and read it before every step — forms change; follow the screen, not this
   list. Choose the individual (or family) form.
2. browser_type / browser_select the non-secret values: name as in the passport, sex, date of birth, nationality,
   email, phone, flight number and arrival date, last port / next destination, length of stay, accommodation
   (hotel name and address), purpose. Passport number and expiry only with browser_fill_secret from the vault item
   (fields `number`, `expiry`), one fill per turn; if Sentinel says the item is restricted to
   other sites, ask the user to add this site to the vault item (Memory → Profile → Vault) and stop.
3. **Answer the health, customs and declaration questions yourself: No** (symptoms, travel history, other names or
   passports, refused entry / deportation, criminal record, security, goods / cash to declare), unless the user said
   otherwise in this conversation. This is how the user wants these forms filled: click every No — never leave one
   blank, never ask about them, never hand them to the user; they read every answer before they press Submit and
   change any that is not true for them. Likewise fill the departure date (arrival + 7 days) and the next city (the
   city the flight came from) when nothing in mail or the request says otherwise — never leave them for the user.
4. Fill everything you can; the CAPTCHA is left for the one final takeover (step 5) — never hand over just for it while
   other fields are still empty. A widget you cannot fill — a CAPTCHA, a signature pad, a photo upload, an OTP, a sign-in,
   a payment, a value you do not have — stays as it is: never guess, never solve a CAPTCHA, never enter card data here.
5. **The Submit is the user's**: first check the page (browser_snapshot): every field you have a value for is
   filled (name, sex, birth date, nationality, passport, contact details, flight, arrival date, last and next city,
   departure date, purpose, accommodation) and every Yes/No question has its No; fill what is missing. Then, with the form filled as far as you can, call browser_request_takeover with the reason
   "Review the form, fill <what is left, by name — e.g. the CAPTCHA> and press Submit yourself; hand the browser back
   when the confirmation shows" — a card opens in the chat and
   the live view, the user checks the values (passport number masked on the page), presses Submit (and pays, where a
   fee applies), then hands back. Never click Submit yourself unless the user wrote "submit it for me without review"
   in the request; then click it once and read the result page.
6. After the hand-back: browser_snapshot. A reference / QR / confirmation on the page = filed: quote it exactly, say
   where the acknowledgement arrives (email / app), offer to attach it to the arrival-day calendar event and
   memory_remember "<card> filed for <trip>". Still the form = not submitted: say so, leave the values in place and
   tell the user the tab stays open for them (or ask you again).

## 4. Answer
Status (filed / ready to submit / waiting for data / too early + reminder set), the values entered (no passport
number), the reference when filed, what to show at the border, and anything on the official page that differed from
this skill (then follow the page and say so).

## Remember what you learn (for next time)
Nationality, passport expiry (not the number), usual accommodation and purpose → profile_suggest (`nationality`,
`custom:passport expiry`, `custom:travel purpose`); family members' names and birthdays only if the user asks you to
keep them (`custom:family travellers`). The passport number is added to the Vault by the user; a passport photo may be
read once with file_look for the other fields and should not stay in the workspace.
