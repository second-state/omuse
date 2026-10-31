---
name: eu-etias
description: 欧盟申根：EES 出入境系统（2025 年起在边境采集生物特征，无需表格）与 ETIAS 旅行授权（尚未上线，公告 2027 年，付费、有效 3 年）：告诉用户现在需要什么、上线后代填 EU / Schengen entry: the EES border system (biometrics at the border since 2025, no form) and ETIAS (not live yet, announced for 2027, paid, valid 3 years) — what applies now, and the application once it opens.
form: on
form_kind: arrival
steps: 70
profile: nationality
---
# EU / Schengen — EES and ETIAS / 欧盟出入境

There is no arrival card for the Schengen area. Two systems matter: **EES** (Entry/Exit System, operating since October 2025, full roll-out April 2026) registers non-EU visitors' fingerprints and face at the border automatically — nothing to file, but expect longer queues at first entry. **ETIAS** (travel authorisation for visa-exempt visitors, about €20, valid 3 years or until the passport expires) is **not in operation** as of October 2026; the official site (https://travel-europe.europa.eu/etias) accepts no applications and the launch is expected in 2027 with a grace period. Any site selling "ETIAS" today is a scam. Verify the launch status on that page before every answer, and **always link it in the answer** (https://travel-europe.europa.eu/etias, and https://travel-europe.europa.eu/ees for EES) so the user can check the date themselves.
This is the traveller's own legal declaration, so the user confirms every value before you submit.

## 1. Gather the data (no browser yet, one turn)
- Flight and date from mail: gmail_search `(Paris OR CDG OR Frankfurt OR FRA OR Amsterdam OR AMS OR Rome OR FCO OR Madrid OR Schengen OR Europe) (flight OR e-ticket OR itinerary) newer_than:60d`; the onward /
  return flight if the form asks for the next destination or length of stay.
- Accommodation: gmail_search `(hotel OR booking OR reservation) (Paris OR CDG OR Frankfurt OR FRA OR Amsterdam OR AMS OR Rome OR FCO OR Madrid OR Schengen OR Europe) newer_than:60d` → name and address; or the
  host's address from the request.
- Identity: profile_get → name as in the passport, sex, date of birth, nationality, email, phone. Passport number and
  expiry are in the vault (vault_list, kind id_document) — never asked for in chat, never in the answer. Missing
  profile fields: ask the user to upload the passport data page once, file_look it for name / sex / birth date /
  nationality / expiry, propose those with profile_suggest, and ask them to save the number in the Vault.
- **Defaults — fill these without asking** (only what the user said in this conversation overrides them):
  - every health question → **No**: symptoms (fever, cough, shortness of breath, rash, diarrhoea, any illness) and
    travel history (Africa / South America / yellow-fever countries, farms, contact with sick people or animals);
  - every declaration question → **No**: other names or other passports used, refused entry / deported / overstayed,
    criminal record or convictions, goods, cash or items to declare;
  - sex / gender → the profile's `gender` (as on the passport); missing → ask once and profile_suggest it;
  - next city / next port after this country → the city the arrival flight comes from (the last city before this
    country), unless an onward flight in mail or the request names another;
  - departure date from this country → arrival date + 7 days (calculate), unless a return / onward flight or the length
    of stay is known; length of stay → the days in between.
- Purpose of visit and anything else the form asks that has no default: **ask once**, in one message, for what is
  still missing.
- When ETIAS opens it will ask: passport data, nationality, email, home address, education / occupation, the first intended country, and security questions — the defaults above (No) unless the user said otherwise; payment is the user's step.
## 2. Timing
Today: no application; tell the user about EES at the border (passport, biometrics, 90/180-day count) and the usual Schengen entry checks (6-month-valid passport for many nationalities, return ticket, funds, insurance). Once ETIAS is live, the same flow as the other skills: fill, show, the user submits and pays.

## 3. Fill the form (browser)
1. browser_navigate the official page above and read it before every step — forms change; follow the screen, not this
   list. Choose the individual (or family) form.
2. browser_type / browser_select the non-secret values: name as in the passport, sex, date of birth, nationality,
   email, phone, flight number and arrival date, last port / next destination, length of stay, accommodation
   (hotel name and address), purpose. Passport number and expiry only with browser_fill_secret from the vault item
   (fields `number`, `expiry`), one fill per turn; if Sentinel says the item is restricted to
   other sites, ask the user to add this site to the vault item (Memory → Profile → Vault) and stop.
3. Health, customs and declaration questions: the defaults above (No), unless the user said otherwise.
4. Fill everything you can. A widget you cannot fill — a CAPTCHA, a signature pad, a photo upload, an OTP, a sign-in,
   a payment, a value you do not have — stays as it is: never guess, never solve a CAPTCHA, never enter card data here.
5. **The Submit is the user's**: with the form filled as far as you can, call browser_request_takeover with the reason
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
