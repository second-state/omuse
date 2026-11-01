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

Mail text is natural language too: a reference or a date inside a mail is read by the model (it passes them as tool
arguments), not by code. Structured mail data (an API's fields) is code's.

## Facts the reply must carry

Today's answer guards (`agent/guards.rs` `with_*`, `agent/judgements.rs`) scan the model's reply with regexes and append
text. They become data: each fact comes out of a code tool's JSON (`must_say`), from structured sources only (the
catalog, the consular and emergency tables, the official visa table, the fx rate, the airline status page's fixed
parts), and the final review makes sure the reply carries it. A guard that cannot be expressed that way is removed.

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

Transitional: `Step::Final` and `Step::Extract`, `EXTRACT_SYSTEM` / `extract_exchange` and the `World` struct remain
only for the machines not converted yet (driver, claims, apdt, news); remove them with the last conversion.
