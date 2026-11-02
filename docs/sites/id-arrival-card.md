# All Indonesia arrival card — the site map

The real site, https://allindonesia.imigrasi.go.id/, walked end to end on 2026-11-02 for `forms::id_arrival_card` (the
card's form function) and `driver::CARDS` (`id-arrival-card`). The walk drove the product's own browser service
(`fill_form` with the ops `forms.rs` builds and its named scripts `SEARCH_LIST`, `SCROLL_LIST`, `CHOICE_ROW`) with clearly
fake traveller data (JORDAN LU, an Australian; passport E1234567X, expiry 01/05/2031; GA 715 Sydney → Denpasar; Alila
Seminyak), then replayed the form function's whole plan against the live site: it reaches the Declaration page. Submit
was never pressed.

## Flow

| step | URL | what is on it | who |
|---|---|---|---|
| home | `/` | "Languages" menu (its scripts answer clicks after ~4 s), tiles | code: Languages → `p` "English"; the page re-renders in English |
| tile | `/` | `h1` "Foreign Visitor" | code clicks |
| 1 Personal Information | `/arrival-card-submission/personal-information` | the traveller, Next | code |
| 2 Travel Details | `/arrival-card-submission/travel-details/individual` | the dates, the visa question, Back / Next | code |
| 3 Mode of Transportation & Address | `/arrival-card-submission/mode-of-transport/foreigner` | transport, purpose, the stay, Back / Next | code |
| 4 Declaration | `/arrival-card-submission/declaration/individual` | health, quarantine and customs declarations, the certification, Back / **Submit** | code answers; **hand-over** (declared final page: `button` = "Submit") |

A React app. Ids carry a session suffix (`spi_full_name_1793607155511`): prefix selectors `[id^="…"]`. Each step's
first field is awaited after the last one's Next (2–3 s). Next with a field missing opens a modal "Incomplete Data!
Please review your form again." (OK) and marks the field ("Flight Name is required"); one whose value is still loading
gives "Invalid Input: Required field(s) cannot be empty."

Dropdowns: a readonly `<input>` with a placeholder value ("Select Port of Entry"); a click opens a panel beside it (in the
same `position: relative` container) with its own search box `input[placeholder="Search"]` (no id) and a virtual list
(`[data-testid=virtuoso-item-list]`, rows `[data-index]` holding a `<p>`). Several panels stay in the page once opened, so
the script `SEARCH_LIST` looks for the search box and the rows in this box's own container, waits for the list's rows
(lists that depend on an earlier field load late), types, waits for the row, and checks the box shows it.

## Step 1 — Personal Information

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| Passport / Country / Region | `[id^="spi_nationality_"]` | dropdown | "AUSTRALIA" | `SEARCH_LIST` the nationality, upper-cased |
| Full Name | `[id^="spi_full_name_"]` | text | | `text` |
| Date of Birth | `[id^="spi_dob_"]` | text DD/MM/YYYY (a date picker) | | `keys` |
| Country / Place of Birth | `[id^="spi_country_or_place_of_birth_"]` | dropdown | the same list; only from the profile's `birthplace` | `SEARCH_LIST` |
| Gender | `[id^="spi_gender_"]` | two rows, readonly inputs MALE / FEMALE | | `CHOICE_ROW` |
| Passport Number | `[id^="spi_passport_no_"]` | text | | Vault `number` (`keys`) |
| Date of Passport Expiry | `[id^="spi_date_of_passport_expiry_"]` | text DD/MM/YYYY | | Vault `expiry` (`keys`, `%d/%m/%Y`) |
| A different passport? | `[id^="spi_diff_passport_"]` | rows Yes / No | No | `CHOICE_ROW` |
| Mobile code | the readonly box in the "Mobile Number" section (no id) | dropdown, rows "+61 - AUSTRALIA" | from the profile's phone | `click` + `pick` "+61 -" |
| Mobile Number | `[id^="spi_mobile_no_"]` | text | digits | `text` |
| Email | `[id^="spi_email_"]` | email | | `text` |

Buttons: Scan MRZ, Add Traveller, Cancel, Next.

## Step 2 — Travel Details

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| Arrival Date to Indonesia | `[id^="std_arrival_date_foreigner_individual"]` | scrolling list of the allowed days, "04 NOVEMBER 2026" | | `SCROLL_LIST` |
| Departure Date from Indonesia | `[id^="std_departure_date_foreigner_individual"]` | text DD/MM/YYYY | the departure (default arrival + 7) | `keys` |
| Visa or Stay Permit? | `[id^="std_do_have_visa_kitas_kitap_foreigner_individual"]` | rows Yes / No | No ("You can apply for a Visa on Arrival after you have completed the Arrival Card.") | `click` the row's input "No" |

## Step 3 — Mode of Transportation & Address

| field | selector | control | value rule / options | plan op |
|---|---|---|---|---|
| Mode of Transport | `[id^="smta_mode_transport_foreigner"]` | dropdown | AIR (the air section then shows; its lists load a moment later — picked at once, the port's list showed the airlines' rows) | `SEARCH_LIST` "AIR", then await the port box and wait 2.5 s |
| Port of Entry | `[id^="smta_place_of_arrival_air_foreigner"]` | dropdown | "CGK - SOEKARNO-HATTA AIRPORT", "DPS - I GUSTI NGURAH RAI AIRPORT", "SUB - JUANDA AIRPORT", "KNO - KUALANAMU AIRPORT", "SOC - ADI SOEMARMO AIRPORT", … | `SEARCH_LIST` typed the IATA, row "DPS -" (the 0.6.0 failure: the script looked for the rows under the search box, but they sit beside it) |
| Type of Air Transport | `[id^="smta_air_transport_type_foreigner"]` | dropdown | COMMERCIAL FLIGHT, … | `SEARCH_LIST`, then wait 1.5 s (the airlines' list loads after it) |
| Flight Name | `[id^="smta_flight_name_foreigner"]` | dropdown | the site's airline names ("GARUDA INDONESIA", "SINGAPORE AIRLINES", "QANTAS AIRWAYS", "JETSTAR AIRWAYS", "VIRGIN AUSTRALIA", "INDONESIA AIRASIA", "MALAYSIA AIRLINES", "CATHAY PACIFIC AIRWAYS", "EMIRATES", "QATAR AIRWAYS", "KLM", "SCOOT", "LION AIR", "BATIK AIR", "THAI AIRWAYS", "KOREAN AIR", "JAPAN AIRLINES", "ALL NIPPON AIRWAYS", "AIR CHINA", "CHINA AIRLINES", "EVA AIR", "TURKISH AIRLINES", "PHILIPPINE AIRLINES", "VIETNAM AIRLINES", "CITILINK", "AIR NEW ZEALAND", …) | `SEARCH_LIST` by the flight's code (`id_airline`), else the trip's airline name |
| Flight code | `[id^="smta_flight_no_prefix_foreigner"]` | readonly | fills itself from the airline ("GA") | — |
| Flight Number | `[id^="smta_flight_no_foreigner"]` | text | the digits ("715") | `text` |
| Purpose of Travel | `[id^="smta_purpose_travel_foreigner"]` | dropdown | 1-DAY TRANSIT, BUSINESS/MEETING/CONFERENCE/CONVENTION/EXHIBITION, CREW, EDUCATION/TRAINING, EMPLOYMENT, HOLIDAY/SIGHTSEEING/LEISURE, MEDICAL CARE, OFFICIAL/GOVERNMENT VISIT, RELIGION, SPORT EVENT, VISITING FRIENDS/RELATIVES, OTHERS | `SEARCH_LIST` |
| Type of Accommodation | `[id^="smta_residence_type_foreigner"]` | dropdown | RESIDENTIAL, HOTEL, OTHERS | `SEARCH_LIST` |
| Name of Hotel (HOTEL) | `[id^="smta_hotel_name_foreigner"]` | dropdown **searched on the server** ("Try Searching Data..." until typed); rows "ALILA SEMINYAK, KAB. BADUNG" | **a hotel on the site's own list** — typed the stay's name, the row starting with it; a hotel not on the list is left for the user (the step then stops: the hand-over says it stopped early) | `SEARCH_LIST`, then wait 3 s |
| Nearest Immigration Office | `[id^="smta_hotel_nearest_immigration_office_foreigner"]` | readonly | fills itself from the hotel ("KANIM KELAS I TPI DENPASAR"); Next before it is in: "Invalid Input" | — |
| Residential address (RESIDENTIAL) | `[id^="smta_residential_address_foreigner"]` | text | the stay's address | `text` (from the 0.6.0 map; the RESIDENTIAL branch was not walked on 2026-11-02) |

## Step 4 — Declaration (the hand-over)

| field | selector | control | value rule | plan op |
|---|---|---|---|---|
| Symptoms now? | 1st `input[readonly][value="No"]` | rows Yes / No (no ids) | No | `click` nth 0 |
| Countries visited in the 21 days before (several) | `input[readonly][value="Select"]` | dropdown, multi-select ("Selected countries:" chips) | the flight's origin country | `SEARCH_LIST` |
| Seat Number | `[id^="fd_seat_number_"]` | text | unknown — the user's | — |
| Animals, fish, plants? | 2nd `input[readonly][value="No"]` | rows | No | `click` nth 1 |
| Number of Baggage | `#asd_total_baggage_individual` | number (Pkg(s)) | unknown — the user's | — |
| Goods to declare to Customs? | 3rd `input[readonly][value="No"]` | rows | No | `click` nth 2 |
| Phones / tablets bought abroad (IMEI)? | 4th `input[readonly][value="No"]` | rows | No ("not required for foreign visitors staying under 90 days") | `click` nth 3 |
| "I, the Applicant hereby certify …" | checkbox | | **the user's** | — |
| Submit | `button` "Submit" | | **the user's** (the declared final page) | — |

Back / Submit. The confirmation's reference (`ref_sel`/`ref_re` in `driver::CARDS`) is not mapped: no card was submitted.
