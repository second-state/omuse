# Cambodia e-Arrival Card — the site map

The real site, https://arrival.gov.kh/, walked end to end on 2026-11-02 for `forms::kh_arrival_card` (the card's form
function) and `driver::CARDS` (`kh-arrival-card`). The walk drove the product's own browser service (`fill_form` with
the ops `forms.rs` builds, its named script `MAT_SELECT` included) with clearly fake traveller data (JORDAN LU, an
Australian; passport E1234567X, expiry 01/05/2031; hotel phone +855 23 000 000), then replayed the form function's whole
plan against the live site: it reaches step 5. The final Submit was never pressed; each walk leaves an unsubmitted draft
on the site.

## Flow

| step | URL | what is on it | who |
|---|---|---|---|
| home | `/` | "Submit e-Arrival" | code clicks |
| sign-in dialog | `/` (dialog) | "Sign in as Guest", "Sign In With Gmail", a QR code | code clicks "Sign In as Guest" (a browser that already has a guest session goes straight to the form: the click is skipped) |
| 1 Profile | `/e-arrival/foreigner/individual-form/draft` (renders in 6–8 s) | passport, names, gender, birth, contact, Next | code |
| check dialog | (overlay) | "Is the information correct?" — the passport number and country read back; Incorrect / Correct | code clicks Correct |
| 2 Travel Information | `/e-arrival/foreigner/individual-form/<draft id>?step=2` (a loading skeleton for ~6 s) | the contact block again (empty), the trip, the stay, Previous / Next | code |
| 3 Health Information | `…?step=3` | three Yes / No questions | code (No) |
| 4 Customs Declaration | `…?step=4` | two Yes / No checkbox pairs, the declaration box | code (No, box ticked) |
| 5 Confirm Declaration | `…?step=5` | the declaration text, the box "I have read and agreed to the declaration." (`mat-checkbox[formcontrolname=confirm]`), Submit | **hand-over** (declared final page: `button.swot-primary` = "Submit"; on steps 1–4 the same button reads "Next") |

The stepper is Angular Material (`mat-step-header`, `aria-selected=true` on the shown step: "1 Profile" …
"5 Confirm Declaration"; a done step reads "Editable create Profile"). Every step's controls stay in the document
(`cdk-step-content-<n>-<i>`, `visibility: hidden` when not shown), so `mat-select`s are counted in document order: the
date triples are expiry (0), birth (1), departure (2). No CAPTCHA was shown on any step.

Pickers: the visible box after a control (`input[formcontrolname=X] + input`, readonly) opens a dialog with a search box
`input[formcontrolname=searchControl]` ("Search...") and rows `a.avatar-vt-item` (a virtual list: ~20 rows rendered;
typing filters it). The control itself holds an object (`[object Object]`), the visible box the label.

## Step 1 — Profile (every field required unless noted)

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| Passport Number | `input[formcontrolname=passportNumber]` | text | | Vault `number` (`vault_as`) |
| Passport Issued Country | `input[formcontrolname=nationality] + input` | picker | rows "Australian . AUS" (demonym . ISO3); typed the ISO3 | `dropdown`: type "AUS", row ". AUS" |
| Date Of Passport Expiry | `mat-select[formcontrolname=day|month|year]` nth 0 | mat-select ×3 | Day 01–31, Month 01–12, Year 2026…2045 (20) | `js_vault` `MAT_SELECT` (`%d`, `%m`, `%Y`) |
| Surname | `input[formcontrolname=surname]` | text | optional | `text` |
| Given Name | `input[formcontrolname=givenName]` | text | | `text` |
| Gender | `mat-select[formcontrolname=gender]` | mat-select | Female / Male / Other | `js` `MAT_SELECT` |
| Date Of Birth | `mat-select[formcontrolname=day|month|year]` nth 1 | mat-select ×3 | Year 2026 back 120 years | `js` `MAT_SELECT` |
| Country of birth | `input[formcontrolname=birthCountry] + input` | picker | rows are country names ("Australia"); only from the profile's `birthplace` | `dropdown` |
| Occupation | `input[formcontrolname=occupationOption] + input` | picker | fixed list of 17 (below) | `dropdown` "Company employee" |
| Email | `input[formcontrolname=email]` | email | optional | `text` |
| Country/Region Code | `input[formcontrolname=phoneCountryCode] + input` | picker | rows "Australia (+ 61)"; default +855; typed the country (a code search is ambiguous: "61" also lists Christmas Island, Cocos) | `dropdown` by `dial_country` |
| Phone Number | `input[formcontrolname=phoneNumber]` | tel | digits | `text` |

Next → the "Is the information correct?" dialog (`.cdk-overlay-container button`: "Incorrect", "Correct") → Correct →
the draft is saved and step 2 opens. **The contact block (occupation, email, code, phone) then shows on step 2, empty
again**: it is filled on both steps.

The occupation list (all of it — no business owner, manager or director): Student, Teacher, Farmer, Journalist,
Government official, **Company employee**, Military personnel, Retired, Religious personnel, Member of parliament, Crew
member, NGO staff, Artist, Self-employed, Worker, Police officer, Other. Code picks "Company employee" — the list's
general business person (the user's rule, 2026-11-02; `KH_OCCUPATION_BUSINESS`). The list has no "Business person"
option; "Self-employed" fits only an owner, and the rule asks for the general one, so "Company employee" stays (rechecked
against this map when the card moved to 0.6.2).

## Step 2 — Travel Information

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| (contact block) | as on step 1 | | filled again | as on step 1 |
| Date of Arrival in Cambodia | `mat-radio-group[formcontrolname=dateOfArrival]` → `mat-radio-button` | radio per day | the 7 days from today, labels "November 04, 2026" | `click` the button with that text |
| Date of Departure from Cambodia | `mat-select[formcontrolname=day|month|year]` nth 2 | mat-select ×3 | the departure (default arrival + 7) | `js` `MAT_SELECT` |
| Port of Entry | `mat-select[formcontrolname=portOfEntry]` | mat-select | Siem Reap Angkor International Airport (SAI), Techo International Airport (KTI), Sihanoukville International Airport (KOS) | `kh_port`: PNH/KTI → Techo (Phnom Penh's flights land there since 09-2025), REP/SAI, KOS |
| Travel purpose | `input[formcontrolname=travelPurposeOption] + input` | picker | Holiday/Pleasure/Vocation, Transit, Business/Meeting/Conference/ Convention/Exhibition, Education/Training, Employment, Medical Care, Official/Government visit, Religion, Sports event, To take up residence, Visiting Friends/Relatives, Others | `dropdown` |
| Type of Air Transport | `mat-select[formcontrolname=typeOfAirTransport]` | mat-select | Commercial (default) / Private | `js` "Commercial" |
| Flight Number | `input[formcontrolname=flightNumber]` | text | the site writes it "SQ 166" | `text` (the validated "SQ 166") |
| Accommodation Type | `mat-select[formcontrolname=accommodationType]` | mat-select | Hotel (default), Private Residential, Day Trip, Transit, Other | `js` |
| Hotel name (Hotel) | `input[formcontrolname=hotelName]` | text | | `text` |
| Hotel contact number (Hotel) / Private residential contact number | `input[formcontrolname=hotelNumber]` | text | **required** — never invented: `stay_phone`, asked before the form opens (`CardSpec::needs_stay_phone`); the user's own home takes their own phone | `text` |
| Province (Private Residential) | `input[formcontrolname=province] + input` | picker | rows not mapped | left for the user |
| Last country/Port of Embarkation | `input[formcontrolname=lastCity] + input` | picker | country names | `dropdown` the flight's origin country |
| Next country/Port of Disembarkation | `input[formcontrolname=nextCity] + input` | picker | country names; the box "Same as last…" (`mat-checkbox[formcontrolname=isSamelastCity]`) copies only a value already committed (ticked right after the last country's pick it left the field empty) | `dropdown` the same country (the user's default) |

Next with a field missing shows "This field is required." under it and stays on step 2.

## Step 3 — Health Information

Three `mat-radio-group[formcontrolname=answer]`, each Yes then No (`mat-radio-button:last-of-type` = No): vaccinated
against yellow fever; fever, cough … or rash now; visited the Middle East, Africa or Latin America in the past 14 days.
Code answers No to each (by order; the user's default — they review the card). Next → step 4.

## Step 4 — Customs Declaration

Two questions as checkbox pairs `.check-box-group mat-checkbox` (Yes, No; no control names): foreign currency over
US$10,000; goods to declare. Code ticks No (by order). **Next needs the box "I hereby confirm that all the statements in
this declaration are true and correct." (`mat-checkbox[formcontrolname=confirmDeclaration]`)**: code ticks it so the card
reaches its last step, where the user reviews everything; the final declaration and Submit stay the user's.

## Step 5 — Confirm Declaration (the hand-over)

The declaration text, `mat-checkbox[formcontrolname=confirm]` ("I have read and agreed to the declaration.") and the
action button `button.swot-primary` reading "Submit". The user ticks the box and presses Submit; code never does. The
confirmation's reference (`ref_sel`/`ref_re` in `driver::CARDS`) is not mapped: no card was submitted.
