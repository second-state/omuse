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
- **The result is JSON.** The model writes the reply from it. What the reply must contain is in the result (a field
  `must_say`: the facts the user must read — reference, amount, deadline, link, the days, the line to call); the final
  review (`Runtime::review_answer`, the model) gets the result and adds what the reply left out. Code never scans the
  reply's prose.

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
   The model is then called once to write the reply (the machine's own tools are not offered on that turn).
5. A known site with no form function: the machine opens the page and its result is
   `{"status": "browser_use", "url": ...}`; the model loads the `browser-use` skill and fills that one page (the one
   place the model uses the input tools; untested).

`Step` becomes `Calls(Vec<ToolCall>) | Done(Value)`. `Step::Final(prose)` and `Step::Extract` go away, and with them
every `EXTRACT_SYSTEM`, `extract_exchange` and code-written answer text.

## The workflow tools

| tool | machine | replaces |
|---|---|---|
| `travel_search` / `travel_book` (kind flight · hotel · michelin · restaurant) | `agent/booking.rs` | regex request parsing, `search_request`, `follow_up`, `mail_to`, `profile_with_words`, `preferred_airlines`, `HABIT`, `WORK` |
| `shop_order` (a known shop's link, the criteria, buy or compare) | `agent/booking.rs` (shop) | `shop_ranking` regexes, `known_shop_request` |
| `arrival_card_fill` (country, trip facts) | `agent/driver.rs` | `user_asks`, `dates_in` over the request, `trip_dates_from_request` |
| `claim_file` / `insurance_buy` | `agent/claims.rs` | the claims machine's request regexes |
| `apdt_refund` | `agent/apdt.rs` | its request regexes |
| `bag_trace` (file reference, family name) | `agent/bagtrace.rs` | the reference/name patterns over words and mails |
| `news_gather` (subject, window) → `news_publish` (the model's stories as JSON) | `agent/news.rs` | the subject regexes and the in-code writing call |
| `train_times` (line, date, after) | `agent/trains.rs` | the model's web searches for a timetable (JR East's Narita Express page read by code, weekday / holiday table from a fixed calendar) |

Mail text is natural language too: a reference or a date inside a mail is read by the model (it passes them as tool
arguments), not by code. Structured mail data (an API's fields) is code's.

## Reading pages

Order of preference for any data on a page (a reference, a price, a status, a timetable, a result row):

1. **Deterministic rules on HTML elements** — a selector, an element id, a table's cells, an attribute of a known
   page, read by the browser service (not by a regex over the page's text).
2. **The model extracts it into JSON** — when no element rule can find it (an unknown site, data only in running text):
   the page text goes to the model with a JSON schema, the answer goes through the JSON layer above.
3. **Regex** — only to check a value that is already structured (an ISO date, a reference's format), never to find
   data in a page's text.

## Facts the reply must carry

Every fact the reply must carry is data (`agent/facts.rs`, `run_facts`), from structured sources only, and the final
review (`Runtime::review_answer`, the model) gets the list as `must_say`; what the reply left out is added by the model
(`Runtime::reply_with_facts`, the facts' JSON as the source). Code never scans the reply's prose and never appends text
to it (the hard stops and secrets aside: `with_masked_cards`, the link repair). The review also gets the language setting
(`prompts::review_system(must_say, lang)`) and answers `language_ok`: a reply against the language rule (English unless
the user asked for text in another language) is written again by the model (`Runtime::reply_in_language`, event
`language_fixed`) — code never guesses a text's language. It also flags a reply that says the assistant did a step the
user did (Submit, Pay, Confirm).

- **The `must_say` of a trusted code tool's result**: a machine's `T_result`, and the small tools the model calls with
  JSON when the user's words ask for them — `emergency_contacts{country, city, emergency, nationality}` (the official
  medical line, or the consular line and the embassy, from the checked tables), `visa_rules{destinations,
  nationality}` (each destination's official page and the official table's visa-free days), `fx_convert{amount, from,
  to}` (the market rate, computed in code), `loan_payment{principal, annual_rate_pct, years}` — and a `phone_call` to an
  emergency number, refused in code before dialling (`facts::emergency_call_refusal`). A page's or a mail's JSON is
  never a source (`facts::FACT_TOOLS`).
- **The run's structured facts** (`guards::*_fact(s)`): the hand-back page's reference, the approval edits, a delayed
  flight's scheduled and estimated times (an airline status page's labels), ETIAS's status, the card catalog's date,
  unverified rows, foreign fees, uncatalogued Vault cards and the user's cards side by side, an approved purchase's
  amount, an injection notice, a scheduled run's notifications, and the options a model-made `present_choices` showed.
- **What the user asked for, as JSON fields** instead of words: the file formats to receive (`deliver_as` on
  `files_write` / `make_*`: the file is sent and the other formats made), an "until" condition (`until` on
  `schedule_create` / `trigger_create`: a goal), "remember this" (`user_asked` on `memory_remember`), a request that names
  a website (`user_named_site` on `load_skill`), ID documents in a form task (`id_documents` on `memory_search`).
- **Removed** (they read the reply or the request and cannot be data): the travel appenders over direct search results
  (`with_hotel_links`, `with_flight_links`, `with_leg_times`, `with_fallback_*`, `with_compare_links`,
  `with_flight_date`, `with_preferred_airline` — the Travel bot's flight and hotel searches are the booking machine's,
  `guards::MACHINE_ONLY`, and its result carries those facts), the grounding of figures, flight codes and phone numbers
  in the reply, the request regexes (`mail_before_travel_guard`, `search_order_guard`, `fix_relative_day`,
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

**4. The Done JSON.** `status` (shown · booked · in_cart · not_confirmed · handed_over · nothing_found · browser_use ·
error), the data the reply needs (options with their links, the pick and its rule, the booking with reference and
amount, links, `mailed`, `next_items`, `next_question`), and `must_say`: short strings with the facts the reply MUST
carry, copied from structured data (a reference, an amount with currency, a link, a deadline, a step for the user).
No answer text: the runtime appends the `T_result` exchange (`workflow::result_exchange`), withdraws the workflow tools
for the next turn (`workflow::reply_turn`), the model writes the reply, and `Runtime::review_answer` gets
`must_say` (prompt `prompts::review_system(true)`); the facts it finds missing are added by the model
(`Runtime::reply_with_facts`). Runtime steps that act on a reply (held picks, auto hand-over, hand-back snapshot) skip
the reply to a machine's result, except a `browser_use` one.

**5. Tests.** Unit tests per machine as in `agent/booking/tests.rs`: `check` on good, bad and missing fields; a `run`
helper that starts from the transcript the run loop leaves (user message, the tool call, `{"status": "running",
"args"}`), answers each code call by name and returns the Done JSON — assert the calls (fixed ids, one per turn, no
question after a page opened) and the JSON (status, data, `must_say`). Protocol tests with the scripted model
(`scripted_rt` in `agent/tests.rs`): a call that runs its machine and the reply turn without the workflow tools, the
review with `must_say`, invalid → planner regeneration → invalid → `needs_input`, repaired syntax. In the fake model
(`tests/fake_llm.py`) a marker becomes the workflow call with JSON arguments (`workflow_args`), and the reply to a
`*_result` is written from the JSON (`workflow_reply`); fake suites check the call (`skills_common.workflow`), the
machine's steps (`work`), the result (`result`) and that the reply carries every `must_say` fact (`says_all`).

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
(cards), `confirmed` / `not_eligible` / `stopped` (APDT), each with `must_say`.

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
- **Mail money lines** (`actions::money_lines`): lines with an amount kept from a mail body to keep 100 receipts in one
  result; the model reads them and decides, code totals nothing.
- **Watcher prices** (`watchers::price_hits`): a `price_below` watch runs unattended with no model; it compares the
  currency amounts after the user's keyword to the user's threshold and only notifies.
- **The Vault ID document by label** (`actions::fill_form`): when several ID documents are allowed on a known form's
  site, the one the user labelled "passport" is used — a choice among the user's own items, never what is allowed.
