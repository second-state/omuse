# Taiwan Arrival Card (TWAC) — the site map

The real site, https://twac.immigration.gov.tw/, mapped 2026-11-02 for `forms::tw_arrival_card` (the card's form
function) and `driver::CARDS` (`tw-arrival-card`). Sources: the product's own runs on the live site (the page handed
over — its inputs and buttons read by element, and its picture; `tests/arrival_common.py` `inputs_of_page`), and the
site's JS bundle (`/static/js/main.<hash>.js`: the field list, the required rules and the option lists). Clearly fake
traveller data only; the final SUBMIT is never pressed.

## Flow

| step | URL | what is on it | who |
|---|---|---|---|
| landing | `/` | "Submit Arrival Card" | code clicks |
| notice | `/notice` | START | code clicks |
| 1 Email Verification | `/submit` | `input[name=email]`, SEND VERIFICATION CODE (a Cloudflare Turnstile token is fetched in the background: `/acard-api-proxy/turnstile/*`), the code box, VERIFY | `email_code` (agent/email_code.rs): with a mailbox code presses Send, the model reads `{"code"}` from the mail, code types it and presses VERIFY; else the hand-over here |
| 2 Traveler Information | `/submit` (same URL) | the traveller's fields below, NEXT | code |
| 3 Itinerary (Trip Information) | `/submit` | the trip's fields below, PREVIOUS, NEXT | code |
| 4 Review | `/submit` | the whole card read back, the declaration, SUBMIT | **hand-over** (declared final page: `.steps li.current .stepTitle` = "Review") |
| 5 Confirmation | `/submit` | after the user's SUBMIT | the user's |

The steps are one React page; the stepper is `.steps li` (`current` on the shown step). NEXT validates the step and,
when a required field is empty, opens a dialog "Please check all fields for "Lead Traveler" and try again." naming the
fields (CONFIRMATION closes it) — the step does not change.

The mail: from `niasys@immigration.gov.tw`, subject "TAIWAN ARRIVAL CARD", text/plain, "Verification Code: <6
letters, mixed case>", valid 10 minutes. `email_code`'s shape check (4–10 ASCII letters or digits, either case)
accepts it.

## Step 2 — Traveler Information (every field required unless noted)

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| Date of Entry | `#traveler1_expectEntryDate` | text, DD/MM/YYYY (react-datepicker) | the arrival day | `text` |
| Expected Departure Date | `#traveler1_intendedDepartureDate` | text, DD/MM/YYYY | ≥ entry date; the departure day (default arrival + 7) | `text` |
| English Name | `#traveler1_englishName` | text | given names then surname, upper-cased by the site | `text` |
| Chinese Name | (no id) | text | optional | — |
| Passport Number | `#traveler1_passportNo` | text | letters and digits | Vault `number` |
| Passport Expiry | `#traveler1_passportExpiryDate` | text, DD/MM/YYYY | not expired, 6 months' validity checked | Vault `expiry` |
| Gender | `label.btnStyle:has(input[type=radio][value=Female|Male])` | radio | | `click` |
| Date of Birth | `#traveler1_birthDate` | text, DD/MM/YYYY | | `text` |
| Nationality | `#traveler1_nation` | react-select (search) | "AUS,AUSTRALIA" (ISO3,NAME) | `pick` |
| Country / Place of Birth | `#traveler1_birthPlace1` | react-select | same list; from the profile's `birthplace` only | `pick` |
| City / State of birth | (no id) | text | optional | — |
| Place of Residence | `#traveler1_livingCity` | react-select | same list; the profile's `home_country` | `pick` |
| Visa type | `.radioGrp:has(#traveler1_visaTypeRadio)` | radio | `VisaExempt` for the visa-exempt nationalities (`TW_VISA_EXEMPT`); a permit number is then not required | `click` |
| Country / Region Code | `#traveler1_phoneCountryCode` | react-select (`/web-ed-card/queryPhoneCountry`) | "61 AUS" | `pick` |
| Mobile Number | `#traveler1_phoneNumber` | text | digits only (`/^[0-9]+$/`) | `text` |
| Occupation | `#traveler1_occupation` (`name=occupation`) | native select, fixed list | **"10" = 商/執行長/總裁/BUSINESS/DIRECTOR/CEO** — the list's general business person (user's decision 2026-11-02); "24" shows a job-title box | `select` |
| Email Address | `#traveler1_email` | email | | `text` |

NEXT (`button` "NEXT") → step 3 when every required field holds a value.

The occupation list (value → label): 29 ACCOUNTANT/CPA, 12 ACTOR/ENTERTAINER, 26 ARMY/NAVY, 1 ARTIST, 30 BANKER,
**10 BUSINESS/DIRECTOR/CEO**, 40 CARETAKER, 2 CLERK/EMPLOYEE/STAFF, 27 CONSULTANT, 3 crew … (the bundle's fixed
`<option>`s; the first option "" is "Search").

## Step 3 — Itinerary

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| Arrival mode | `.radioGrp:has(#traveler1_arrivalTravelModeFlag) label.btnStyle:has(input[value=AIR])` | radio AIR / SEA | AIR for a flight | `click` |
| Entry flight code | `#traveler1_arrivalFlightCode` | react-select (`/web-ed-card/queryFlight`: "CI : CHINA AIRLINES", … "NONE OF ABOVE") | required with AIR | `pick` typed "CI : CHINA AIRLINES", option starting "CI :" |
| Entry flight number | `#traveler1_arrivalFlightNumber` | text, max 10 | digits (letters too for some codes) | `text` "52" |
| Ship number | `#traveler1_shipNo` | text | only with SEA | — |
| **Departure mode** | `.radioGrp:has(#traveler1_departureTravelModeFlag) label.btnStyle:has(input[value=AIR])` | radio AIR / SEA | **required** ("Expected Mode of Departure: Please choose an option.") | `click` |
| **Exit flight code** | `#traveler1_departureFlightCode` | react-select (same list) | **required with AIR** | `pick` |
| **Exit flight number** | `#traveler1_departureFlightNumber` | text, max 10 | **required with AIR** | `text` |
| Purpose of Visit | `#traveler1_reason` (`name=reason`) | native select (`/web-ed-card/queryReason`) | 16 Business, 213 Study, **1 Sightseeing / Travel / Leisure**, 60 Exhibition, 3 Visit Relative (then the relative's name and phone are required), … 33 Others (then a text is required) | `select` |
| Accommodation | `.radioGrp:has(#traveler1_hotelFlag) label.btnStyle:has(input[value="Hotel Name"])` | radio "Residential Address" / "Hotel Name" / "Transfer" | | `click` |
| Address / hotel name | `#traveler1_address` | text: letters, digits, spaces, `. , -` | required unless Transfer; a Residential Address is checked by the site (Google + TGOS address lookup, then a declaration to tick when not found); a Hotel Name is not | `text` (the hotel's name, `tw_text`) |

NEXT → step 4 Review when the step's required fields hold values. **The departure by air needs the flight that
leaves Taiwan** — it is never invented: the tool takes `departure_flight_no`, and a Taiwan card without it is
`needs_input` before the form opens (`CardSpec::needs_departure_flight`). This field was missing from the first map
(it did not show until the itinerary step was reached on the live site: the 2026-11-02 runs stopped at step 2's
occupation, then at step 3's departure mode).

## Step 4 — Review (the hand-over)

The stepper's current step is "Review" (`.steps li.current .stepTitle`). The user checks the card, ticks the
declaration and presses SUBMIT; code never does.
