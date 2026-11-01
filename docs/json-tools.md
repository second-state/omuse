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
