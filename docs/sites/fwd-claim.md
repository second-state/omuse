# FWD Singapore online travel claim: the walked map

Walked on the live site on 2026-11-03 with clearly fake data (claimant "Test Walker", NRIC S1234567D, mobile 91234567,
walker@example.com, policy `PNTR2012-34567890`, specimen PDFs), up to the final button, which was **not** pressed.
No request reaches FWD's servers before that button: every step below is checked in the page.

Start: `https://www.fwd.com.sg/claim-online/travel.html` → `/claim-online/travel-insurance/quickQuestions`.

## Step 1 — details (`quickQuestions`)
| field | selector | notes |
|---|---|---|
| Name of claimant (as in NRIC/FIN) | `input[name='claimant name']` | required |
| Preferred name | `input[name='preferred name']` | optional |
| NRIC/FIN | `input[name='NRIC/FIN']` | the Vault's ID number |
| Mobile number | `input[name='mobile']` | 8 digits (Singapore) |
| Email | `input[name='email']` | |
| Policy number | `input[name='policy number']` | the box has a fixed prefix `PNTR20`: type only the rest. Validator `polic_number`: `^\d{2}-\d{8}$` or `^\d{2}-\d{8}-\d{2}$` (so a full number reads `PNTR2012-34567890`). Typing the whole number fails the check silently. |

"next": `#TI_GetQuotepage_Next_1` → `typeClaim` (no server check of the policy).

## Step 2a — claim type (`typeClaim`)
Radios (`div[role=radio]`, by their text): Medical Benefits, **Travel Disruption**, Cancel for any reason, Theft of or
Damage to Baggage…, Other Benefits, Total & Permanent Disability, Death. Travel Disruption opens: **Flight Delay**,
Transportation Delay (non-flight), Trip Cancellation and Loss of Deposit, Trip Postponement, Trip Cut Short, Trip
Disruption (during the trip), Insolvency Protection, Missed Connections Or Trip Diversion, Baggage Delay.

Flight Delay, first section ("confirm details of your travel insurance cover"):
| field | selector | notes |
|---|---|---|
| scheduled to leave Singapore | `input[name=dateOfLeaveSG]` | read-only, Vuetify date picker (`.menuable__content__active .picker--date…`), shown `dd-mm-yyyy` |
| scheduled to return | `input[name=dateOfBackSG]` | same picker |
| country travelled to | `[data-vv-name=placeOfIncident]` | Vuetify select; type the name to filter, then `.menuable__content__active a.list__tile` |

"next" (`button` "next") opens the second section on the same page ("now we'll ask you about the delayed flight"):
| field | selector | notes |
|---|---|---|
| original date of the delayed flight | `input[name=dateOfOriginalFlight]` | same picker |
| flight number | `input[name=originalFlightNumber]` | e.g. `SQ636` |
| claiming for someone else | name / NRIC inputs, "add another" | optional |

"next": `#TI_GetQuotepage_Next_1` → `uploadDocument`.

## Step 2b — documents (`uploadDocument`, "snap & upload", jpg or pdf)
| document | input |
|---|---|
| Itinerary of every claimant | `input#itinerary[type=file]` |
| Re-scheduled boarding pass of every claimant | `input#newItinerary` |
| Memo from the airline stating the hours of delay and the reason | `input#operatorLetter` |
| Other supporting documents | `input#supporting` |

Plain file inputs: setting their files works (each shows "<name> (0MB) remove"). "next": `#TI_Application_Next`
(it goes on without files too) → `paymentMethod`.

## Step 2c — payment (`paymentMethod`)
- **Deposit via PayNow** (radio): ID type NRIC/FIN (the only one), `input[name=payNowId]` and
  `input[name=confirmPayNowId]` (the NRIC twice, password inputs), the confirm box (`[role=checkbox]`: "the payment
  details above are accurate and belong to policyholder or rightful claimant").
- Deposit to bank (radio, default): bank name (select), `accountNumber`, `payeeName`, `accountNRIC`, a bank statement
  (`input#bankStmt`), the same confirm box.

"next": `#TI_GetQuotepage_Next_1` → the declaration dialog.

## Step 3 — submit (declaration dialog)
"By submitting this eClaim form, you: confirm that all the information … is true, complete and accurate; … notified
the insured person … ; will produce all original bills …; have not claimed … from any other insurer …; agree to FWD's
Privacy Policy …; agree to indemnify FWD …". Buttons: cancel `#TI_Optfordiscount_Discount_Skip`, **agree and submit**
`#TI_BuyEnhacements_Continue_Yes` (files the claim — not pressed in the walk).

After a submission FWD's app routes to `thankYou`; its claim links carry `?hash=<sub-claim number>` (the
`additionalDocument` page adds documents to a submitted claim, after the mobile number). Not yet seen live: the walk
stopped before the submission.
