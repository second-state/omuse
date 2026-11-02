# Code tools on JSON (0.6.0 redesign)

The rule (CLAUDE.md, "Natural language is the model's; code tools work only on JSON"): natural language is processed
only by the model; code tools take JSON in and give JSON out and never call the model; a known website and its HTML is
processed by code. This page is how the agent is built around that rule.

## The shape of every workflow

```
user's words ──model──▶ tool call with JSON arguments ──code──▶ validated arguments
                                                              │
                         every step in code (APIs, form functions with fixed ids, known pages' fixed parts,
                         choice cards, the hand-over, the resume after a takeover) — no model call
                                                              │
reply the user reads ◀──model── JSON result ◀─────────────────┘
```

- **Intent and parsing are the model's tool call.** Which workflow runs is the tool the model calls; the request's
  details are that call's arguments (a JSON schema in the catalog). There is no intent classifier and no extraction
  call inside code, and no regex or keyword list over the user's wording anywhere.
- **Validation is code, on the JSON.** A date parses (relative dates are the model's to resolve — the live state gives
  today), a city is in the table, an address is well-formed, a party size is 1–9, an address the results are mailed to
  appears in the user's words (an exact substring check, not parsing). A bad field is dropped or fixed; when a needed
  field is missing the tool returns `needs_input` at once, before any page opens.
- **Every step is code.** The machine's next step is a pure function of (validated arguments, the transcript after the
  tool call). It never calls the model.
- **The result is JSON.** The model writes the reply from it. The facts the user must read are in the result as
  structured items (`show`: a reference, an amount, a deadline, a link, a file, the days, the line to call) and code
  appends them to the answer after the reply, always (see "What the answer shows"). `hints` are optional wording help
  for the model, never enforced. Code never scans the reply's prose and the model never has to copy a fact.

## Checking the model's JSON (one layer, for every tool call and every JSON the model returns)

The model's JSON is checked at the boundary where language becomes data, in one place (`agent/json_in.rs`), never in
each machine:

1. **Repair the syntax.** Arguments that do not parse are repaired in code first: a missing closing `"`, `}` or `]`,
   a trailing comma, single quotes, an unquoted key, a code fence or prose around the object, a number with absurd
   precision (`0.0360000…` cut to its value). Today this is `llm::extract_json` and the `_unparsed` /
   `[bad_arguments]` path in `llm.rs` / `agent/exec.rs`; they move behind this layer.
2. **Validate against the tool's schema.** Required fields present, types right, enums in their list, then the tool's
   own field checks (dates parse, a city is in the table, an address is well-formed, a mailed-to address appears in the
   user's words). A field that can be fixed is fixed (`"2"` → `2`, a date in another format → ISO), one that cannot is
   dropped and reported.
3. **Regenerate with the planner model when it is truly wrong.** Still unparseable, or a required field missing or
   invalid after the repair: code asks the **planner model** once for the JSON again — the user's message (and the
   conversation's last answer), the tool's schema, the rejected JSON and the problems found go in; JSON comes out and
   goes through steps 1–2 again. Still wrong: the tool result is `{"status": "invalid", "problems": [...]}` and the
   executor model gets one more turn to call again; after that the workflow stops with a reply that says what is
   missing (`needs_input`).

Regenerating is part of turning language into JSON (the model's side of the line), so the runtime may call the planner
here; a code tool still never calls a model. Every repair and every regeneration is an event (`json_repaired`,
`json_regenerated`) so the timeline shows it, and each has a unit test with the broken JSON that caused it.

The same layer checks every other JSON the model returns: `decide()` choices, the news stories for `news_publish`, the
final review's verdict.

## The protocol in the run loop

1. The model calls a workflow tool `T(args)`.
2. Code checks `args` through the JSON layer above (repair, schema, planner regeneration). Problems that the model can fix (a field in the wrong form) → the tool result is
   `{"status": "invalid", "problems": [...]}` and the model calls again. A field only the user can give →
   `{"status": "needs_input", "missing": [...]}` and the model asks the user; the user's answer comes back as a new
   message and the model calls `T` again with every field.
3. Valid: the tool result of `T` is `{"status": "running"}` (so the message order stays valid for the provider), and
   the machine runs: its steps are code calls (assistant + tool messages, id prefix per machine: `bkg_`, `drv_`,
   `clm_`, `apd_`, `nws_`, `bag_`), exactly as the machines do today. A takeover suspends the run; the resume carries on
   from the transcript.
4. Done: code appends one more code exchange, `T_result` (id `res_<n>`), whose content is the machine's JSON result.
   The model is then called once to write the reply (the machine's own tools are not offered on that turn); after the
   review, code appends the result's `show` items to the reply.
5. A known site with no form function: the machine opens the page and its result is
   `{"status": "browser_use", "url": ...}`; the model loads the `browser-use` skill and fills that one page (the one
   place the model uses the input tools; untested).

### The compulsory workflow call (`agent/required.rs`)

A skill that asks through a workflow tool needs that tool's call before a final answer: the booking skills
(`travel_search` / `travel_book`, `shop_order`), the arrival cards (`arrival_card_fill`), the claims, the insurance and
the bag trace (`claim_file`, `insurance_buy`, `bag_trace`), the departure-tax refund (`apdt_refund`), the news
(`news_gather`) and `entry-requirements` / `arrival-cards` (`entry_requirements`). The map is read from the machines'
own tables (`required::tools_of`). The trigger is structured: the skills this request loaded (a `load_skill` whose
result is the sheet), never the request's or the reply's words. It is met by any answered call of one of the loaded
skills' tools (not an error, not `invalid`; a `needs_input` is the tool asking the user).

When the model answers without the call, the run loop does not take the answer: it sends the same request again with
only those tools and the provider's forced `tool_choice` (the tool by name; `required` when several skills' tools are
owed, so the model picks one of the listed tools), event `workflow_required`. The call then runs like any workflow
call, through the JSON layer (repair → schema → planner regeneration → `invalid` and one more forced try →
`needs_input`). No call even when forced (or the tool not offered, or the budget spent): the answer is kept and code
appends the fixed line "Not checked: OMuse could not look up … in code for this answer" (`required::unmet_item`,
event `workflow_required_unmet`). `ChatOpts::tool_choice` carries the choice; an endpoint that rejects it gets `auto`.

`Step` becomes `Calls(Vec<ToolCall>) | Done(Value)`. `Step::Final(prose)` and `Step::Extract` go away, and with them
every `EXTRACT_SYSTEM`, `extract_exchange` and code-written answer text.

## The workflow tools

| tool | machine | replaces |
|---|---|---|
| `travel_search` / `travel_book` (kind flight · hotel · michelin · restaurant) | `agent/booking.rs` | regex request parsing, `search_request`, `follow_up`, `mail_to`, `profile_with_words`, `preferred_airlines`, `HABIT`, `WORK` |
| `shop_order` (a shop's link and the ranking criteria; the page itself goes to `browser-use`) | `agent/booking.rs` (shop) | `shop_ranking` regexes, `known_shop_request` |
| `arrival_card_fill` (country, trip facts) | `agent/driver.rs` | `user_asks`, `dates_in` over the request, `trip_dates_from_request` |
| `claim_file` / `insurance_buy` | `agent/claims.rs` | the claims machine's request regexes |
| `apdt_refund` | `agent/apdt.rs` | its request regexes |
| `bag_trace` (file reference, family name) | `agent/bagtrace.rs` | the reference/name patterns over words and mails |
| `news_gather` (subject, window) → `news_publish` (the model's stories as JSON) | `agent/news.rs` | the subject regexes and the in-code writing call |
| `flight_status` (flight, date) | `agent/flightstatus.rs` | the prose regex for "Scheduled … Estimated" over any page (`flight_times_fact`): the airline's status page opened by code, its table's rows read by element (`browser_read_elements`, cells) |
| `entry_requirements` (destinations with arrival dates, nationality = the profile's) | `agent/entry.rs` | the model's own `visa_rules` call and the arrival-card windows in the skill text: each destination's visa-free days from the official table and its page (`facts::visa_rules`), its arrival card's official site and the day the window opens (`driver::CARDS`), as `show`, and `reminders` |
| `train_times` (line, date, after) | `agent/trains.rs` | the model's web searches for a timetable (JR East's Narita Express page read by code, weekday / holiday table from a fixed calendar) |

Mail text is natural language too: a reference or a date inside a mail is read by the model (it passes them as tool
arguments), not by code. Structured mail data (an API's fields) is code's. `gmail_find_receipts` and
`gmail_read_amounts` hand the model each mail's text cut plainly to its share of one size budget
(`actions::mail_text`, `receipt_text_share`; `truncated` when more was not shown) — code no longer picks the lines
with amounts; the model reads the amounts and passes them as JSON (`calculate`, a report's rows).

The reply language follows Settings → Language; with Reply language = "match" the model reads the request's
language as a JSON choice (`Runtime::request_lang`: a `decide` over `en | zh | other`, "other" leaves the setting), and
the chat title call answers `{"title", "lang"}` (`prompts::parse_title`, `lang` checked against `en | zh`). Code counts
no characters to decide a language; the character classes left in code only strip code's own bilingual strings
(`prompts::en_only`, `drop_cjk`, `system_text_en`) or report a written file's length (`files_write`).

## Email verification codes (one reusable step)

A site that sends a verification code by email ("enter your email → send code → type the code") is handled by one
reusable function, `email_code`, used by every machine with such a step (Taiwan and Korea arrival cards, the UK ETA,
any sign-up later):

1. **A mailbox is connected** (Gmail or IMAP): code fills the user's email and presses Send, then waits for the mail —
   a structured mailbox query (recent, from the site's sender or domain, after the press), with a timeout.
2. The mail's body is natural language: the **model extracts `{"code": "…"}`** through the JSON layer (repair, schema,
   one planner regeneration). Code checks the value's shape, types it and goes on.
3. **No mailbox, no mail before the timeout, or no valid code**: code fills the email, does not press Send, and hands
   over at that step.

Built as `agent/email_code.rs`: a machine whose card has an `email_code` spec (`driver::CardSpec`: the email box, the
Send button's text, the code box, the verify button, the sender's domain — Taiwan `immigration.gov.tw`, the UK ETA
`gov.uk`) emits the code call `email_code` after the fill that stopped at the email step, once per run; the runtime runs
it (`Runtime::email_code_step`): the fills go to Sentinel as `browser_fill_form` (the code as a secret), the mail comes
from Sentinel's `/internal/mail_code` (the fixed query `from:<domain> newer_than:1d`, dated after the press less a
minute, polled every 10 s for 150 s; its body unredacted only on this internal route — the agent's own mail reads stay
redacted), the model reads `{"code"}` (`read_code`: the default model, then the planner once), code checks 4–10 letters
or digits. The JSON result (`typed` · `no_mailbox` · `timeout` · `no_code` · `no_send_button` · …) never carries the
code; `typed` → the fill goes on, anything else → the hand-over at the step (the reason says whether the email is in and
the code was requested). With no mailbox the Send button is not pressed. A run that ends at the email step because
the code could not be read ends with the status `email_code_not_read` and its reason (`no_mailbox`, `timeout`,
`no_code`, …; the `email_code` event records it too), shown as the line "Email code: I could not read the verification
code (…) — enter the code from the email on the page" — the user's own step, and a successful hand-over.

## Reading pages

Order of preference for any data on a page (a reference, a price, a status, a timetable, a result row):

1. **Deterministic rules on HTML elements** — a selector, an element id, a table's cells, an attribute of a known
   page, read by the browser service (not by a regex over the page's text).
2. **The model extracts it into JSON** — when no element rule can find it (an unknown site, data only in running text):
   the page text goes to the model with a JSON schema, the answer goes through the JSON layer above.
   A confirmation reference on an unknown site (the `browser-use` path) is the model's `confirm_reference{reference,
   label}` (`agent/confirm.rs`): checked to be on a page read after the hand-back, recorded as `model_read`.

The element rules go through `browser_read_elements{read: [{name, sel, attr?, cells?}]}` (the browser service's
`/agent/read_elements`; withdrawn from the model like `browser_fill_form`): each selector's visible matches as text, an
attribute, or with `cells` each row's cells and links, wrapped as untrusted page content. Every known site's confirmation
reference is read this way — `ref_sel` (the element) and `ref_re` (the format, checked over the whole value) in
`driver::CARDS` (the claims' real confirmation pages are not known: `claims::SPECS` has none, and their results stay
`handed_over`) — and a page without it is `not_confirmed`, never a generic fallback. FWD Singapore's plans are its
quote summary's elements (`#plan-table`: the plan buttons' ids, the prices, the benefit lines' fields). The
outcome check and the ledger take the reference from structured JSON only (`confirm::confirmed`: a machine's result →
`known_page`, `confirm_reference` → `model_read`; the source is kept in the evidence).

Each read is `{name, sel}` plus one of: `attr` (an attribute; `href` absolute), `cells` (a row's cells and links),
`fields` (an object per match: `{field: "sub-selector" | {sel?, attr?, closest?}}` — `closest` goes up to an ancestor
first), and `hidden` (hidden matches too), `max` (40, at most 200). An input's ref for `browser_upload` /
`browser_fill_secret` is its `data-persona-ref` attribute, read by the input's fixed id after a snapshot gave the page's
elements their refs (`confirm::refs_args`) — never a label searched in the snapshot's text. The known pages read this
way:

- JR East's station timetable (`table.result_03 tr[id^='time_'] div.timetable_time`: `.train`, `.minute`,
  `data-misc` ◆, the row's hour cell) for `train_times`.
- WorldTracer's answer on mybag.aero after LOG IN (`#error-body-head` / `#error-body-text`, or the sign-in's
  `#mngRptRefNoLTxt` gone: the report opened).
- Card Claim Center's sign-in still there after the hand-back (`input[type=password]`).
- (APDT reads no element: its form function stops at the travel document's upload, `require` on "Click here to upload".)
- News: the engines' rows carry `date` (DuckDuckGo's timestamp) and `source` (Bing News' author); `browser_read`'s page
  has `lead` (the first paragraph under `<article>` / main), `site_name` (`og:site_name`) and `published`
  (`article:published_time` / `<time datetime>`).

A price watch (`watch_create` / `web.page` with `mode=price_below`; `watch_price.rs`, `watchers::evaluate_price`)
decides when it is made how its price is read, and stores it in its parameters (`scheduler::event_spec` →
`watch_price::decide_read`): `read=element` with `price_sel` (and `price_attr`) — a known site's rule
(`watch_price::KNOWN`: Amazon's buy box) or the selector
the model gave — read by Sentinel through `read_elements`, the value checked to be one price
(`watchers::parse_amount`); else `read=model`: Sentinel answers `{"extract": {text, title, url}}`, the scheduler has
the model extract `{price, currency, item}` (`watch_price::extract_price`, through `json_in`; `null` = no price for the
item, an error) and hands it back as `observed`, and Sentinel compares it with the threshold and the last alert
(structured events `{price, currency, item, seen}`). `watch_create` never calls the model: on a model-read page the
price the model read (`current_price`, required there) is the baseline; an element that shows no price switches the
watch to `read=model` with a note. No price is looked for in a page's text by a pattern.

On a shop the listings are the model's JSON: `present_choices` options with `price`, `price_text`, `stars`
and `ratings` (up to 40 with `ranking`); code ranks them (`listings::Listing::from_option`) and verifies their texts
against the pages read. Which button adds a product to the cart is a `decide` over the snapshot's
buttons by ref (`judgements::snapshot_buttons`). Providers' terms are their flags: `free_cancellation`, `prepaid`, a
Michelin restaurant's `stars` / `bib_gourmand` / `closed_days` (the texts are shown, not searched).
3. **Regex** — only to check a value that is already structured (an ISO date, a reference's format), never to find
   data in a page's text.

**Where a card is handed over (declared, checked by code).** Every arrival card declares in `driver::CARDS` its final
page's Submit (`final_page`: a fixed selector and text taken from the real page — the user's click, never code's) and
the markers of the user's own steps that come before the form (`user_step_at`: a Cloudflare check's response field,
the sign-in's password box, the passport upload, the email / code boxes). Before the hand-over the machine reads them
(`browser_read_elements`, `page_check_args`); a filled form whose final Submit is not on the page stopped early — the
hand-over says so and the result is `stopped_early` with the page it stopped on (nothing submitted), never "ready to
submit". The e2e suites read the same table (`tests/arrival_common.py` `declared`).

**Form functions' page scripts (code-only).** A control a framework builds (an Angular Material `mat-select`, a
custom dropdown, a date picker) is set by a named script instead of a chain of simulated clicks: the plan op
`{"op": "js", "script", "src", "sel", "nth", "value"}` (`forms::js` / `forms::js_vault`; the value may be a Vault
value Sentinel puts in) runs `src` — an `async (sel, nth, args) => {ok, value, why}` function from the fixed table
of constants in `forms.rs` (`forms::Script`, e.g. `MAT_SELECT`: open the select, click the option, check what it shows)
— on the page through the browser service's CDP evaluate, and the fill report carries the value the control now shows
(`shown`, never for a secret). The script text never comes from a tool's arguments: only form functions build such
ops, and the runtime refuses a `js` op in any `browser_fill_form` call the model wrote (`guards::model_script_guard`).
Used for Cambodia's date, gender and other `mat-select`s.

## Pictures, places and cards as data

- **Vision decisions are choices.** `browser_locate` lists the choices in code (`agent/locate.rs`: the coarse grid's
  cell labels + `NONE`, the zoomed grid's numbers + `NONE`) and the vision model picks one as JSON
  (`Runtime::decide_on_picture`, then `decide_flag_on_picture` for `{"on_item": bool}` on the red marker); code computes
  the point and returns JSON (`status` found · unsure · not_found, `x`, `y`, the element under the point).
- **The profile's places are codes.** `home_city` (a city code of `places::CITIES`), `home_country` and
  `address_country:<label>` (ISO 3166-1 alpha-2), validated by `places::clean_value` wherever a profile field is
  written (UI, `profile_suggest`, memory extraction). Code reads only these (`booking::home_currency`,
  `driver::own_addresses`, `claims::market_from_profile`), never an address's words. Addresses saved without them are
  read once by the model into JSON before a workflow's arguments are checked (`places_fill.rs`, event
  `profile_places`); the Memory page edits them as dropdowns.
- **Cards by id.** Each catalog row has an `id` (`build_cards.py`, the tables' ID column); the Vault keeps it with the
  card (`vault_add_card{name, country, card_id}`, checked against the catalog; `vault_list` shows `card_id=`). The card
  facts look a card up by that id only (`guards::catalog_card`); a card without one is "not in the card catalog".
  Tools take a card by its Vault id, its card_id or its whole label (`claims::pick_card`), a plan or an offer by its
  listed name or id (`pick_plan`, `booking::by_label`, `mentioned`) — never a word of a name.

## What the answer shows (`show`: required, appended by code; `hints`: optional)

Every fact the user must read is structured data in a trusted code tool's JSON result, a list `show`, and code appends
it to the answer after the model's reply (and after the review) — deterministically, every time (`agent/show.rs`,
`facts::show_items`). The model's prose is never read and never rewritten; the model never has to copy a reference, a
link or an amount for it to reach the user (gate 40 news_report_real: the editions' links left out of the reply and the
review did not add them; arrival_real: the TDAC reference missing after the hand-back).

```text
show: [{kind: "link"|"file"|"reference"|"amount"|"fact"|"block", label, label_zh?, url? | path? | value?, currency?, text?}]

reply …

---

- Thailand Digital Arrival Card confirmation number: **TH7D3K9Q2**
- HTML edition: [The Tidal Ledger](/artifacts/news/…html)
- PDF edition: [The Tidal Ledger](/artifacts/news/…pdf)
- Total: SGD 454.00
- New edition: every Monday at 08:00 — next: 2026-11-02 08:00
```

- **Rendering** (`show::render`): one Markdown line per item — a link `[text](url)` (an artifact's `/artifacts/…` as it
  is), a file `[name](/api/files/raw?path=…)`, a reference in bold, an amount with its currency and two decimals, a
  fact as `- Label: value`; a `block` (a table code rendered from data) as it is. The block opens with a rule (`---`,
  `show::MARK`): everything after the answer's last rule is the appended block, so a reader (the e2e helpers,
  `tests/show_common.py`, `Answer.block`) finds it without reading the reply. A line already in that block is not added
  twice (string equality of code's own lines).
- **Labels** are code's: `label` in English, `label_zh` when the answer is in Chinese (the language setting); a label
  taken from a page (a `confirm_reference` label) is shown as it is.
- **Sources** (`facts::trusted_results`, this request only — after the user's latest message): a machine's `T_result`;
  the small tools the model calls with JSON when the user's words ask for them — `emergency_contacts{country, city,
  emergency, nationality}` (the official medical line, or the consular line and the embassy, from the checked tables),
  `visa_rules{destinations, nationality}` (each destination's visa-free days from the official table and its official
  page; ETIAS's status), `fx_convert{amount, from, to}` (the market rate, computed in code),
  `loan_payment{principal, annual_rate_pct, years}` — a `phone_call` to an emergency number refused in code before
  dialling (`facts::emergency_call_refusal`), and a recorded `confirm_reference` (an unknown site's reference the model
  read after the hand-back, checked against the page). A page's or a mail's JSON is never a source.
- **The run's structured facts** (`guards::*_fact(s)`, `facts::run_facts`) are appended the same way, as unlabelled
  lines (a table as a block): the approval edits, ETIAS's status, the card catalog's date, unverified rows, foreign fees,
  uncatalogued Vault cards and the user's cards side by side, an approved purchase's amount, an injection notice, a
  scheduled run's notifications, and the options a model-made `present_choices` showed.
- **`hints`** (the former `must_say`) are optional: wording help for the model ("nothing has been submitted", "you
  pressed Submit yourself"). Nothing enforces them — the final review (`Runtime::review_answer`) checks only that the
  reply holds together (no contradiction, no claim that the assistant did the user's Submit / Pay) and the language rule
  (`prompts::review_system(lang)`, `language_ok`; a reply against it is written again by the model,
  `Runtime::reply_in_language`, event `language_fixed`). A reply that does not hold together is written again from the
  result (`Runtime::reply_fixed`) or the run (`rescue_final`). No test checks a hint.
- **Telegram** gets the same answer text: the rule becomes a divider, a link into the app is shown as its text and path
  (`telegram_bot::md_to_tg`).
- **What the user asked for, as JSON fields** instead of words: the file formats to receive (`deliver_as` on
  `files_write` / `make_*`: the file is sent and the other formats made), an "until" condition (`until` on
  `schedule_create` / `trigger_create`: a goal), "remember this" (`user_asked` on `memory_remember`), a request that names
  a website (`user_named_site` on `load_skill`), ID documents in a form task (`id_documents` on `memory_search`).
- **Removed** (they read the reply or the request and cannot be data): the review's `missing` check and
  `reply_with_facts` (the facts are appended by code now), the travel appenders over direct search results
  (`with_hotel_links`, `with_flight_links`, `with_leg_times`, `with_fallback_*`, `with_compare_links`,
  `with_flight_date`, `with_preferred_airline` — the Travel bot's flight and hotel searches are the booking machine's,
  `guards::MACHINE_ONLY`, and its result's `show` carries those facts), the grounding of figures, flight codes and phone
  numbers in the reply, the request regexes (`mail_before_travel_guard`, `search_order_guard`, `fix_relative_day`,
  `trip_dates_from_request`, `wants_file`, `names_website`, `until_in_request`, `REMEMBER_ASK`, `outcome::asks_send`),
  and the decisions over the reply or the request (`held_pick_call`, `held_offer_call`: `travel_book` takes the pick;
  the emergency, visa and format decisions).

## API for the next machines (phase 1: the run loop and the booking machine)

The protocol and the checking layer are in place; `agent/booking.rs` is the reference conversion. To convert a
machine (driver/apdt, claims/bagtrace, news, …):

**1. Declare the workflow tool.** A function returning the catalog entries, built with `tools::fn_(name, description,
properties, required)` (see `booking::tool_schemas`): JSON-schema properties with types, enums and descriptions that say
what the field holds (`"YYYY-MM-DD (resolve relative dates from today in the live state)"`), and a description that
says what the tool does and that the reply is written from the JSON result. Add the entries to `tools::local_tools()`
(they become local tools, so `is_local` and the catalog see them), the tool names to a `TOOLS` const of the machine,
and — when only one bot may use it — the names to that bot file's `tools:` line (`bots/travel.md`). In
`agent/exec.rs` the branch for `booking::TOOLS` sends the call to `Runtime::checked_workflow_args`; add the machine's
`TOOLS` there and to the machine selection in `agent/runtime.rs` (`workflow::active(&transcript, &TOOLS)` →
`Machine::Workflow { tool, args, from }`; give `Machine::Workflow` the machine's `next` by tool name).

**2. Validate.** The JSON layer (`agent/json_in.rs`) runs first and needs nothing from the machine: the syntax was
repaired where the call was read (`llm::parse_args` → `json_in::parse_args`, event `json_repaired`), then
`json_in::conform(schema.parameters, &mut args)` fixes types, enums and dates against the declared schema. The machine
supplies its field checks as a pure function `check(tool, &args, &Ctx) -> workflow::Checked` (see `booking::check`):
`Valid(normalised args)` (codes, defaults from the profile, ISO dates, bad optional fields dropped),
`Invalid(problems)` for what the model can fix (a past date, a city not in the table, an address the user never wrote),
`NeedsInput(missing)` for what only the user can give — before any page opens. `checked_workflow_args` then asks the
planner model once to regenerate an invalid call (`json_regenerated`), gives the executor one more try with
`{"status": "invalid", …}`, and turns a second failure into `needs_input`. Regex on structured values (an ISO date, an
email's shape) is fine; nothing reads the user's wording — the only use of the user's messages allowed is an exact
substring check (`mail_results_to`).

**3. The machine.** `next(tool, &validated_args, &transcript[from..], &Machine{…}) -> driver::Step`:
`Step::Calls(vec![code call])` with ids `<prefix><n>` (`workflow::code_call`; the `first` field of the context carries
the count of the machine's earlier calls in the transcript so the ids go on), or `Step::Done(json)`. It reads only its
validated arguments (`Req::from_args`), the profile (with `profile["mailbox"]`, the connected account, set by the
runtime), structured tool results and known pages' fixed parts — the transcript after its own call, so a run suspended
for a takeover resumes from the transcript. A page with no form function: `Done({"status": "browser_use", "url"})`;
the runtime then loads the `browser-use` skill in code and the model fills that page.

**4. The Done JSON.** `status` (shown · not_confirmed · handed_over · nothing_found · browser_use ·
error), the data the reply needs (options with their links, the pick and its rule, the booking with reference and
amount, links, `mailed`, `next_items`, `next_question`), `show`: the facts the user must read as structured items
(`show::reference`, `show::link`, `show::file`, `show::amount`, `show::fact` — copied from structured data: a reference,
an amount with currency, a link, a deadline, a step for the user), and `hints`: optional wording help. No answer text:
the runtime appends the `T_result` exchange (`workflow::result_exchange`), withdraws the workflow tools for the next
turn (`workflow::reply_turn`), the model writes the reply, `Runtime::review_answer` checks that it holds together, and
code appends the `show` items after it (`show::append`). Runtime steps that act on a reply (held picks, auto hand-over, hand-back snapshot) skip
the reply to a machine's result, except a `browser_use` one.

**5. Tests.** Unit tests per machine as in `agent/booking/tests.rs`: `check` on good, bad and missing fields; a `run`
helper that starts from the transcript the run loop leaves (user message, the tool call, `{"status": "running",
"args"}`), answers each code call by name and returns the Done JSON — assert the calls (fixed ids, one per turn, no
question after a page opened) and the JSON (status, data, and the lines its `show` renders to: `show::lines`).
Protocol tests with the scripted model (`scripted_rt` in `agent/tests.rs`): a call that runs its machine and the reply
turn without the workflow tools, the `show` block appended after the reply, invalid → planner regeneration → invalid →
`needs_input`, repaired syntax. In the fake model (`tests/fake_llm.py`) the reply to a `*_result` is written from the
JSON (`workflow_reply`); fake suites (app-only: no fake services or pages) can check the call (`skills_common.workflow`),
the machine's steps (`work`), the result (`result`) and that the block code appended carries every `show` item
(`says_all`, `tests/show_common.py`); real suites check the block as the user sees it on real sites (`Answer.block`,
`Answer.shows`). No test checks the model's wording or a hint.

Two runtime rules come with every machine: a machine's own `present_choices` (a `bkg_`-style code call) carries `_code`
(set by the run loop, never taken from the model) and is not checked against pages read — its cards come from the
structured result; and `ask_user` in a skill's run before its workflow tool was called is refused as data
(`booking::ask_before_tool`: call the tool, its `needs_input` decides what to ask; defaults are code's). Extend both to
the new prefixes / skills.

`Step::Final` and `Step::Extract`, `EXTRACT_SYSTEM` / `extract_exchange` and the `World` struct are removed: every
machine ends with `Step::Done`, and `Runtime::drive` runs workflow machines only. The news machine
(`agent/news.rs`) is converted: two tools in a row (`news_gather` → `news_publish`) — on the reply turn after a
`gathered` result only `news_publish` stays offered (`news::offered`), and its `check` reads the gathered articles from
the transcript (`news::gathered`).
The claims machine (`agent/claims.rs`: `claim_file`, `insurance_buy`, `bag_trace`, prefix `clm_`) is converted: its
`check` takes the transcript too (`claims::Ctx`). When facts are missing and the mailbox is connected but not read for
that tool yet, the call is valid with `_read_mail`: the machine runs the skill's fixed mail searches, reads the hits and
returns `{"status": "mail_read", "emails": [...], "facts_needed": [...]}` — the model reads those emails (natural
language) and calls the tool again with the facts; `workflow::reply_turn` keeps the workflow tools offered after a
`mail_read` result. Facts still missing after the mail was read are `needs_input`, before any page opens.
The arrival-card machine (`agent/driver.rs`: `arrival_card_fill`, prefix `drv_`) and the Hong Kong departure tax
refund (`agent/apdt.rs`: `apdt_refund`, prefix `apd_`) are converted the same way: `check` takes the transcript
(`driver::Ctx` / `apdt::Ctx`), the arguments of the chat's earlier calls carry over, a call with the trip (or the
documents) unknown and a mailbox connected is valid with `read_mail` — the machine runs the fixed searches (APDT also
saves the attachments and reads the PDFs) and returns `mail_read` — and what is still missing after that is
`needs_input`. Several of the user's own addresses in the country are a choice card (`needs_choice`, the pick comes
back as `own_address`). Results: `submitted` / `scheduled` / `not_confirmed` / `waiting_on_user` / `handed_over`
(cards), `confirmed` / `not_eligible` / `stopped` (APDT), each with `show` (and optional `hints`).

## Safety exceptions

Sentinel and the apps crates read no natural language to decide a step: a flight's cabin, stops and self-transfer are
required enums on `flights_search` (the request's words are not read); a click or an upload the user asked for is the
model's `user_asked` on `browser_click` / `browser_upload`, honoured only while the task has read nothing flagged as an
injection (task ctx), only for a click whose label is not about money, sending, booking or terms
(`guard::click_skip_allowed`), only for an attached file (`uploads/`) on the site the message names; Vault values go into
a page only through a form function's fixed ids (`browser_fill_form`) or the model's `browser_fill_secret` on a ref
(no field guessed from labels); `purchase_confirm`'s total is the model's (or a machine's, from structured data) and
a card fill before a confirmed purchase is refused, never confirmed in code from the page. What is kept, and why:

- **Injection scan** (`omuse_common::guard::scan_injection`, the `injection_warning` flag): regexes over untrusted
  prose (mails, pages, MCP results). A model classifier reading the same text could itself be injected; the flag only
  tightens (more approvals, no `user_asked` skip, no egress trust).
- **Risky-element heuristics** over a page element's own accessible name, as the browser reports it (not prose, not
  the user's words): `click_is_risky`, `money_click`, `order_change_click`, `checkout_step_click`,
  `user_submit_click`, `cart_edit_click`, `filter_submit`, `card_box_mismatch`, `click_skip_allowed`, and policy's
  `passport_field` (an ID or card number box: typing is refused, only `browser_fill_secret`). They raise an approval,
  hand a click to the user or refuse typing a secret; where they lower a risk it is bounded (add-to-cart, search,
  cookie decline, a cart line's Remove, a filter box).
- **The site the user named** (`guard::named_in_request`): an exact substring check of a page's domain (more than 4
  characters) in the user's message — the one use of the user's words allowed (as `mail_results_to`), never after an
  injection. It trusts the egress check for that site and bounds the `user_asked` upload.
- **Receiving money** (`guard::refund_page`): a table of known pages (host + path), no longer a page's address or
  title; there a card is filled to be credited without a purchase confirmation (each fill still approved).
- **The checkout page's total next to the model's** (`routes::act::purchase_site` with `guard::page_total`): display
  only — the approval card shows both, and a purchase whose total differs from the page's is never auto-approved.
  Known shops' total selectors would replace it; there is no such table yet (it needs a broker call that reads one
  selector).
- **The ledger's return vs. cancel** (`routes::act::RETURN_RX`) over the label of the order-change button the user
  approved: a status name in the ledger, no decision.
- **Allowed text handling** (kept as decided 2026-11-02; none of it picks a step from what the user or a page says):
  - the injection scan and the risky-element checks above (`scan_injection`; the click checks on an element's own
    label);
  - memory's secrets backstop (`store::looks_sensitive`, used by `memory_tidy`): card / ID-shaped numbers and
    passwords are kept out of memory (they belong in the Vault) even when the model files them as facts;
  - the exact domain-named-in-request check for uploads and egress (`guard::named_in_request`, above);
  - full-text search over the words the model itself passes to `memory_search` (`store::fts_query`, an FTS5 trigram
    index): retrieval over the model's own query terms, not a reading of the user's words;
  - URL and id repair over the model's text (`links.rs`, `ids.rs`): a link or an id the model copied wrong is put
    back to the one it was shown;
  - provider error strings in `llm.rs` (context too long, unknown model, streaming not supported …): a provider's
    machine messages, read to choose a retry or a fallback.
- **The Vault ID document by label** (`actions::fill_form`): when several ID documents are allowed on a known form's
  site, the one the user labelled "passport" is used — a choice among the user's own items, never what is allowed.
