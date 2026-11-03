# Task tools: the conversation, the input JSON, the output JSON

Every task follows the rule in CLAUDE.md ("How every task works"): the model converses and fills ONE input JSON, one
code tool does the whole site process with it, and returns ONE output JSON; the model words the reply and code
appends the facts (`show`). This page is the contract of each tool: what the model looks up, what it asks (once, in
one message), each field, and each result. The code is the authority (the `check` of each machine validates and
normalises the input; `tests` pin the output); this page must agree with it.

Conventions for every tool
- **Lookup before asking**: the profile (name, email, mobile, home country), the Vault (ID numbers — never in the
  JSON, code fills them), the connected mailbox (code runs a fixed search when the facts are missing: result
  `mail_read`, then the model calls again with what the mails say), and the files of the chat (`uploads/…`,
  `attachments/…`).
- **One question**: `needs_input` lists everything missing at once — required fields and optional documents. The model
  asks for all of it in one message. After that question the tool is always called with whatever the user gave (a reply
  to the tool's question owes the call — `agent/required.rs`); a required field still missing is asked again.
- **Invalid**: `{"status": "invalid", "problems": [...]}` — the model fixes its JSON and calls again (no question).
- **Output**: always `status`, `show` (facts code appends: references, links, amounts, deadlines, what is still the
  user's) and `hints` (for the model's wording). A session link or reference is in `show`, never only in prose.

---------------------------------------------------------------------------------------------------------------------

## `apdt_refund` — Hong Kong air passenger departure tax refund

Site: the Airport Authority's Refund Application Platform (`https://app.apdtrefund.hk/hkTaxMobile/?lang=en`), walked
2026-11-02/03 with the user's own documents. Machine: `agent/apdt.rs`; form function: `forms_claims.rs::apdt_start`.

### Conversation
Look up: the booking / boarding-pass / receipt mails (fixed search), the files of the chat, the profile. Ask once for
what is missing: how the user entered Hong Kong; by air the arrival flight and its date; the departure flight and its
date; the refund method; the documents (optional).

### Input JSON
| field | type | required | from |
|---|---|---|---|
| `entry_mode` | `air` \| `land_sea` \| `skypier` | yes | the conversation ("flew in" / "came from Shenzhen" / "SkyPier ferry") |
| `out_flight`, `out_date` | "CX 500", YYYY-MM-DD | yes | the words, the boarding pass, the receipt |
| `in_flight`, `in_date` | "SQ 890", YYYY-MM-DD | by air (asked); land / sea: the arrival date if given | the words, the arrival boarding pass |
| `tax_hkd` | 120 \| 200 \| 0 | no | the receipt ("Hong Kong Air Passenger Departure Tax") |
| `refund_method` | card \| cash \| alipay \| wechat \| alipayHK \| wechatHK | yes | the conversation |
| `wallet_account` | string | for an e-wallet | the conversation |
| `passport_file` | path | no (asked; it is what saves the application) | the chat's files |
| `out_pass_file`, `receipt_file` | path | no (asked) | the chat's files, the mail's attachments |
| `in_pass_file` | path | by air only (asked) | the chat's files |

### What the tool does
The rule (left by air the same or next day, within 28 days, after departing, tax paid) → the platform: notices, the
two statements, Start, New Application, the mode's tile and its Confirm → each document into its slot (the Sample
dialog's Upload opens the file chooser) → the recognised travel document saves the application → reads its reference
(`.bottom span.card-number.code`). SkyPier: the platform's face check comes before any saved application, so nothing is
uploaded; the user's steps are returned. Nothing is submitted: the refund method, the face check and Submit are the
user's, on any device with the reference.

### Output JSON
| `status` | when | fields |
|---|---|---|
| `mail_read` | facts missing, documents fetched | `files` (the model reads them and calls again) |
| `needs_input` / `invalid` | — | `missing` / `problems` |
| `not_eligible` | the rule fails | `reason`, `rule` |
| `saved` | the application is saved | `reference`, `home`, `to_upload` (documents still the user's), `apply_by`, `trip`, `refund` |
| `ready` | no travel document given | `home`, `apply_by`, `trip` (the user starts on the platform) |
| `skypier` | entry by SkyPier | `home`, the user's steps |
| `stopped` | a page did not open | `why`, `home` |

`show`: eligible / entered by, tax to be refunded, apply by, **Application reference**, **Still to upload (yours)**,
**Next steps (yours)** (View Application / Continue Application with the travel document number and the reference),
the platform's link.

---------------------------------------------------------------------------------------------------------------------

## `insurance_buy` — FWD travel insurance

Machine: `agent/claims.rs::next_buy`; form function: `forms_claims.rs::fwd_sg_quote`. Singapore walked 2026-11-02;
Hong Kong: `forms_claims.rs::fwd_hk_mytravel` (walked 2026-11-03). Thailand: walked, the tool comes in 0.6.6 (its
session lives only in cookies).

### Conversation
Look up: the trip in the booking mails (fixed search), the profile (home country → market; name, email, mobile). Ask
once: the destination, the dates, the travellers, the user's rule (a cover and its minimum, or a plan), compare or buy.

### Input JSON
| field | type | required | from |
|---|---|---|---|
| `market` | sg \| hk | yes (else the profile's home country) | the profile, the words |
| `action` | buy \| compare | default buy | the words |
| `destination` | country | yes | the words, the booking |
| `depart`, `return` | YYYY-MM-DD (not in the past) | yes | the words, the booking |
| `travellers` | 1–9 | default 1 | the words |
| `cover` + `cover_at_least` | cancellation \| medical \| delay \| baggage, amount | no | the user's rule |
| `plan` | the plan's name | no | the user's pick on the comparison card |

### What the tool does (Singapore)
FWD's quote (plan type, who, the region from the destination, the dates) → reads the plans (premium, covers) → picks
one by the rule (the plan chosen, else the cheapest with the cover asked for, else the cheapest) → the comparison card →
buy: saves the quote with FWD (name, email, mobile from the profile, NRIC/FIN from the Vault; the user agreed to FWD
contacting them) → FWD's saved-quote link. The user opens it on any device with their mobile number: FWD's eligibility
statements, the plan, the details and Pay are theirs.

### What the tool does (Hong Kong — MyTravel, underwritten by bolttech; walked 2026-11-03)
`https://www.fwd.com.hk/online-insurance/mytravel-insurance/en/`: the region (Asia — Mainland China (excl. Inner
Mongolia), Macau, Taiwan, Japan, South Korea, Brunei, Cambodia, Indonesia, Laos, Malaysia, Myanmar, the Philippines,
Singapore, Thailand, Vietnam — else Worldwide), the dates on its flatpickr calendars (each day by `aria-label`), one
traveller → the plans read from `[data-testid=PlanOption]` (`[data-part=name]`, `[data-part=premium]@data-value`, the
first `[data-part=currency]@data-value` = medical) → the pick by the rule, the comparison card → buy: the plan's card
(FWD creates `…/application/?leadId=<id>`), the insured's details (the name split by the model, title, date of birth,
the HKID from the Vault, its date of issue — a new Smart HKID —, the mobile, the email) and Next (FWD keeps them) → the
application's link, which opens prefilled on any device; add-ons, declarations and Pay are the user's. Extra input for
Hong Kong: `title` (Mr. / Miss / Mrs.), `hkid_issue_date` (YYYY-MM-DD), `family_name` + `given_names` (the model splits
the HKID name: Hong Kong names come family name first). Output `saved` with `quote_link` = the application's link;
`details_needed` when FWD refused a field (the link still opens the application at its first step).

### Thailand (FWD × Dhipaya "TIP Insure") — 0.6.6; walked 2026-11-03 with fake data up to step 4
`https://affinity.tipinsure.com/product/affinity/TIPINSUREFWD/ta_inter` → `/Tourist/ta_step_1/inter`: countries
(`#select-country`, up to 3, jQuery), single / annual (`#singletrip` / `#annualtrip`), `#startDate` / `#endDate`
(dd/mm/yyyy, jQuery datepicker), who (`#individual` / `#multipax` / `#family`), "ต่อไป" → `/ta_step_2/inter`: a tutorial
dialog (`#modal-tutorial`, button "ตกลง"), the packages (Backpack 1 … Backpack Go 3, THB premiums) and "เลือกแผนนี้" →
`/ta_step_3/inter`: `#tip-login-tablet` + `#confirm_email`, `#customer_phone`, `#id` (Thai ID, `#id-field`) or
`#passport` (`#passport-field`, `#customer_passport_country` — option values are ids, e.g. "1" = THAILAND),
`#customer_title_name` (MR. / MISS / MRS. / MASTER), `#customer_firstname` / `#customer_lastname` (English),
`#customer_birth_date` / `_month` / `_year` ("01", "01", "1990"), a Thai address (`#customer_address_house_no`, `_moo`,
`_village`, `_soi`, `_road`, `_province` → `_amphur` → `_tumbon` loaded in turn, `_post_code` filled by the site),
beneficiary `#legal` / `#other`, purpose `#sightseeing` …, delivery `#delivery-same-address`, policy by `#email` /
`#postal`, consents `#accept-condition-0` / `-1` → `/ta_step_4/inter`: the summary (insured, trip, plan, total premium),
payment `#creditcard` / `#netbanking` (counter) / `#promtpay` / `#2c2p_airpay` (ShopeePay), `#acceptance` (the purchase
conditions) and Next to the payment gateway. The session lives only in cookies: no step up to 4 gives a link or a
reference that opens on another device.

### Output JSON
| `status` | when | fields |
|---|---|---|
| `shown` | compare | `plans`, `picked`, `premium`, `rule` |
| `saved` | buy | + `quote_link` (Singapore: the saved quote, `quotation_no`; Hong Kong: the application) |
| `details_needed` | buy (Hong Kong): FWD refused a detail | `quote_link` (the application at its first step) |
| `handed_over` | no Singapore mobile / the save failed | the page is handed over (fallback) |
| `nothing_found` | no plans listed | `quote_page` |

`show`: plans quoted, chosen (premium), why, **Your saved FWD quote** (link), **FWD quotation**, **Next steps (yours)**.

---------------------------------------------------------------------------------------------------------------------

## `claim_file` (FWD) — a travel delay claim

Machine: `agent/claims.rs::next_sg_claim`; form function: `forms_claims.rs::fwd_sg_claim`; walked 2026-11-03
(`docs/sites/fwd-claim.md`).

### Conversation
Look up: the policy email (policy number, plan, the delay rule, the time limit, the emergency line), the airline's
delay email (times, reason, its PDF), the files of the chat. Ask once: the policy number (required), the delayed
flight, its scheduled and actual departure, the trip's dates and country, the itinerary and the boarding pass (FWD
requires both), the airline's letter (optional).

### Input JSON (Singapore)
| field | type | required | from |
|---|---|---|---|
| `insurer` = fwd, `market` = sg, `benefit` = flight_delay | — | yes | the words, the policy email's sender |
| `policy_no` | `PNTR20NN-NNNNNNNN[-NN]` | yes | the policy email, the profile, else asked |
| `incident_date`, `flight_no` | YYYY-MM-DD (past), "SQ 636" | yes | the words, the airline's letter |
| `scheduled_departure`, `actual_departure` | HH:MM | yes | the airline's letter |
| `leave_sg`, `return_sg` | YYYY-MM-DD | yes (the trip must have ended) | the words, the booking |
| `destination` | country | yes | the words |
| `itinerary_file`, `boarding_pass_file` | path | yes (FWD's form requires them) | the chat, the mail |
| `airline_letter_file`, `other_file` | path | no | the chat; the airline's letter from the mail is saved by code |
| `benefit_per_block`, `block_hours`, `benefit_cap`, `claim_within_days` | numbers | no (the market's rule otherwise) | the policy email |

### What the tool does
Counts the delay and the benefit by the policy's rule and the deadline → (trip not ended: a reminder for the day after
the return, nothing filled) → FWD's online claim: the claimant (NRIC/FIN from the Vault; the policy number without
FWD's fixed `PNTR20` prefix), Travel Disruption → Flight Delay, the trip's dates and country, the delayed flight, the
documents, PayNow by NRIC/FIN → the declaration → the user's approval in the app (`submit_confirm`: the facts and FWD's
declaration; never auto-approved) → agree and submit → the page FWD lands on → a weekly mail check for FWD's answer.

### Output JSON
| `status` | when | fields |
|---|---|---|
| `submitted` | approved and submitted | `claim_link` (FWD's page when it carries `?hash=`), `landed`, `documents` |
| `not_submitted` | the user declined | the form closed unsubmitted |
| `after_return` | the trip has not ended | `return` |
| `no_benefit` | the delay is under the first block | — |
| `stopped` | a step did not open | `why`, `portal` |

`show`: the delay, the claim amount, the rule (policy), tell FWD by, **Submitted** / **Not submitted**, **Your claim on
FWD** or "FWD emails the claim link", reminders.
