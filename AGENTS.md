# Notes for coding agents working on this repo

**Skills are code first.** A skill uses code tools for every step that can be computed; a decision the model must make is a JSON multiple choice; the model's prose is only the reply to the user. The full rule is in `CLAUDE.md` ("Skills: code decides").

## Releasing (tag `omuse-vX.Y.Z`)

The version shown in the app comes from the release tag: the release workflow
(`.github/workflows/release-image.yml`) passes the tag as the `OMUSE_VERSION`
build argument (compiled into the Rust binary) and `omuse-v0.4.0` is shown as `v0.4.0`. Source checkouts and
untagged builds show `SOURCE_VERSION` in `crates/omuse-common/src/util.rs` (keep `version` in the root `Cargo.toml`
equal to it).

When asked to release `omuse-vX.Y.Z`, do it in this order so that the source
literal, the tagged commit and the image all agree:

1. On `main`, set `SOURCE_VERSION` in `crates/omuse-common/src/util.rs` and `version` in `Cargo.toml`
   (`cargo check` updates `Cargo.lock`).
2. Commit that change (`Release X.Y.Z`) and push `main` of `second-state/omuse`.
3. Tag that commit `omuse-vX.Y.Z` and push the tag to `second-state/omuse`. The tag push builds, smoke
   tests (including a check that the app reports `X.Y.Z`) and publishes
   `ghcr.io/second-state/omuse:omuse-vX.Y.Z` (the workflow still accepts the older `locius-v*` tags).
4. Create the GitHub release on `second-state/omuse` (`gh release create omuse-vX.Y.Z --repo second-state/omuse
   --verify-tag`), with the Olares chart from `sh chart/build.sh omuse` attached.
5. Report the run result and the image digest from the `image-import.json`
   artifact.

Do not move or re-publish an existing release tag; use a new version instead.

## Web UI assets

The page shell depends on nothing but this server: no CDN `<link>` or `<script>`,
no web fonts; `marked` and `DOMPurify` are vendored in `web/`. The sentinel
serves `web/` from memory (`crates/omuse-web/src/assets.rs`): `index.html` stamps every
asset URL with `?v=<digest>`, a stamped URL is immutable for a year, anything else
is `no-cache` with an ETag, and text assets are gzip-compressed once at load. A
unit test fails on an off-site asset; `tests/assets_e2e.py` checks the headers.

The UI shows one language at a time: English by default, Simplified or Traditional Chinese only when Settings →
Interface says so. Source strings in `web/app.js` are Simplified Chinese (often with an English half for the bilingual
fallback) wrapped in `T()` / `Tf()`; `web/i18n.js` holds the English and `web/i18n_tw.js` the Traditional text. In a
Chinese UI every string, the server's bilingual texts included, goes through `biZh`, which drops the English half and
turns "Agent" into 智能体; product and protocol names (`ZH_TERMS`) and identifiers stay. `python3 tests/i18n_check.py`
fails on a missing translation, on Chinese in an English value, and on a key or Traditional value that is still mixed
after the reducer — then reword the key (or add a real term to `ZH_TERMS`), never let a second language through.

## Model settings

The system's endpoint, key and model names come from `[model]` in `config.toml`
(`url`, `api_key`, `name`, ...). A user can switch
`model_override` on in Settings → Models and use their own endpoint, key and
models (`crates/omuse-runtime/src/store.rs`, `effective_model`): the saved `model_*` values
and `model_api_key` count only while the toggle is on, and the endpoint, key and
LLM name are required then. With it off they are ignored, never mixed with the
system's. The API never returns the key, only `model_api_key_set`, and the audit
log gets `(set)`. A test that changes the model must send `model_override: true`
with those three fields and switch it off afterwards (`tests/models_e2e.py`,
`tests/models_ui.py`).

## Login gate (Docker / Fly)

`crates/omuse-web/src/gate.rs` wraps Sentinel for standalone installs (`[auth]`). Browsers sign in
on `/login` and get a signed cookie (`omuse_session`, 180 days, invalid after a
password change; the key is `$SENTINEL_DATA/session.key` on the volume, so
sessions survive restarts). Browser navigations without a session are
redirected to `/login`, browser fetches get `401 {"detail": "login required"}`
(the web app then goes to `/login`), and non-browser clients (no
`Sec-Fetch-Mode`) still get the HTTP Basic challenge. The local e2e stack runs
Sentinel without the gate (`[auth] gate = false`); the gate is covered by unit
tests and `tests/docker_image_smoke.sh`. On Olares the entrance does the login
(`[auth] enabled = false` in the chart).

## Repository: second-state/omuse only

This repository is `second-state/omuse`, a fork of `Drlucaslu/locius`. Every
branch, commit, push, tag, issue and pull request goes to `second-state/omuse`;
nothing is ever sent upstream (no push, PR, issue or comment on
`Drlucaslu/locius`).

- Push only to `origin` (`https://github.com/second-state/omuse`). Do not add
  the upstream as a remote you push to.
- Open pull requests with an explicit target, because `gh` defaults to the
  parent repository for forks:
  `gh pr create --repo second-state/omuse --base main ...`
  (likewise `--repo second-state/omuse` for `gh pr view/merge`, `gh issue`,
  `gh release`).
- Before pushing or opening a PR, check the target (`git remote -v`,
  `gh pr view --repo second-state/omuse`).

## Pull requests

Open a PR against `second-state/omuse` and report it; merge only when the user
explicitly says to merge that PR. Never commit API keys, passwords or other credentials, and keep the
local e2e environment file outside the repository.

## Rust port

The app is one Rust binary (`crates/`, Cargo workspace), ported from the Python version (v0.3.6, commit 562a36f);
`docs/rust-port.md` has the conventions and the module map, and `docs/rust-port-report/` the comparison. Settings come
from `config.toml` (`config.example.toml`), never from environment variables; only the container entrypoint
(`deploy/docker/entrypoint.sh`) turns `OMUSE_*` variables into a config file when none is mounted. The web UI (`web/`),
the skills (`skills/`) and the browser scripts (`crates/omuse-browser/js/`) are compiled into the binary. The e2e suites
check behaviour, texts and JSON shapes, so keep them stable. The Olares chart (`chart/`, packaged by `chart/build.sh`)
runs the release image with one `--role` per container.

### Browser auto-approval

Settings → Agent → "auto-approve actions" (it covers every action that would ask since 2026-10-31 — mail, calendar,
Drive, calls and connectors as well as the browser; only the final submit / pay click is handed to the user) is the
browser connection's `config.auto_approve`; until the user
sets it, `[browser] auto_approve` in `config.toml` is the default (true; `tests/run_local.sh` sets false so the
suites keep testing the approval flow). In `routes::act::act`, an ASK that `Decision::browser_auto_ok` accepts (any
browser tool — vault fills and uploads included — and `purchase_confirm`, also after a suspected injection: with the
setting on nothing in the browser asks) is recorded as an approval resolved by `auto`, audited as approved, run at once, and returned with `auto_approved`; the runtime
adds an `approval_auto` event and the same chat card a manual approval gets. `tests/browser_auto_real.py`. The final
click that pays, confirms a purchase or booking, or changes an order is never the agent's, whatever the setting: the
policy refuses it with an instruction to hand the filled page over (`browser_request_takeover`), and the user presses it
in the live view (`policy::hand_over_click`; a checkbox or option, e.g. agreeing to a fee, is no such click; a page that
receives money — a refund method — takes a Vault card without a purchase, `guard::refund_page`).

### Parallel steps and sub-agents

The planner runs only when the executor asks for a plan (`plan_task`) or loads a skill whose front matter says
`plan: on` (`Runtime::plan_on_request`; ordinary skills are followed from their sheet), not before every turn; simple requests are answered by the executor directly (`tests/integration.py` checks that a plain
chat makes no plan). The planner (`prompts::PLANNER_SYSTEM`) gives steps that do not depend on each other the same `parallel` label; the
status note (`prompts::parallel_hint`) tells the executor to do the next such group in one turn. The executor loop
(`agent/runtime.rs`) runs a turn's calls that are all in `tools::PARALLEL_TOOLS` (sub-agents and reads that never touch
the task's browser page) at the same time, with `no_ask`, and appends the results in call order; everything else runs
one by one. `delegate` takes one sub-task or `tasks=[{role, task, step}]`; sub-agents of one call, or of one turn, run
side by side, each on its own browser page (`lane` in `/internal/act` → broker page `<task>~sub<n>`, see
`routes::act::page_id`), mark their plan step running / done, and are limited by `tools::SUBAGENT_MAX`. Their model
calls share `llm_concurrency` (Settings → LLM, 1..=8, applied without a restart). The scripted model has a PARALLEL
scenario (`tests/parallel_steps_e2e.py`).

## Tests

- Rust: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
- Full local suite: `tests/run_in_docker.sh` (fresh stack per suite). After every suite `tests/args_guard.py` checks
  that no search ran with a price cap ≤ 0 or fewer than 1 traveller (the recorded tool calls and the requests the fake
  Kiwi.com, SerpApi and travel connector received); a violation fails the suite.
- Real-model runs: the travel suites with `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env` (`OMUSE_E2E_MODELS=alt` for
  the second model pair); `tests/travel_ask_real.py` asks two research-only travel questions and checks that no
  browser, approval or takeover was involved, with a TIMING line each; `tests/browser_auto_real.py` reads a hotel booking from mail and
  finds the hotel with SerpApi (`places_search`) and opens its Google Maps page, the result's `map_url` (a data-egress
  check, so Sentinel always asks) with "auto-approve browser
  actions" on (no dialog, recorded as approved by auto) and off (the same request stops at that step).
- Market and README images (`deploy/market/assets`): `OMUSE_E2E_OUT=<dir> bash tests/run_in_docker.sh marketing_shots`
  photographs the current UI (showcase scenarios `SHOW_*` in `tests/fake_llm.py`) and frames it; copy
  `<dir>/market/*.webp` and `overview.png` into `deploy/market/assets`. Rebuild the test image first.
- Image smoke test: `tests/docker_image_smoke.sh <image>`; set `OMUSE_VERSION`
  to the tag to also check the reported version.
- Real-model suites (`run_in_docker.sh real`) act as the user, no exceptions: `tests/user_common.py` drives the web UI
  (types into the chat, answers approval dialogs, takes over and hands back the browser) and the checks use only what the
  user sees — the answer and its links, file cards and downloads, dialogs, the browser view, and the user's own mailbox,
  Drive and sites. No tool names, arguments, events or logs in a real-model check; when one fails, fix the product.
- Use cases are tested by real-model suites; scripted (fake-model) suites only for what a real run cannot do. A fake
  suite stays only when the scenario cannot be produced or checked end to end — an injected outage, timeout, retry or
  rate limit, an exact internal guarantee (one search per turn, call counts), a forced security refusal — and its
  docstring says why. Any other fake suite is replaced by a real one from the user's side, then deleted. Official sites
  are stand-ins at their real addresses (`tests/https_standins.py` ROUTES, `STANDIN_HOSTS` in `run_in_docker.sh`).
- Flights, hotels & cars as the user: `tests/travel_user_real.py` (in `run_in_docker.sh real`, billed: a few SerpApi
  searches with `OMUSE_E2E_SERPAPI_KEY` from the e2e env file). In the browser the user pastes the key on Connections and
  asks Main's chat for a flight, a date range, a hotel and a rental car; only the answers on the page and their links are
  checked (prices, a booking link that opens the booking page, Kayak to compare), against the real SerpApi, Kiwi.com,
  Kayak and Rentalcars.com (`REAL_DATA_SUITES`). E2e checks should judge what the user sees, not the system's insides.
- Flights, hotels & cars in other setups, as the user (real model, `REAL_DATA_SUITES`): `tests/travel_kiwi_real.py` (no
  travel key: flights from the real Kiwi.com for a date and a range, with the airline, times, price and a Kiwi.com link
  that opens), `tests/travel_brave_real.py` (the user pastes `OMUSE_E2E_BRAVE_KEY` on Connections → Web search, no
  SerpApi key; a flight, a hotel and a car in one message, then "yes" to the hotel and the car: prices and booking links
  that open), `tests/travel_connect_real.py` (Connections → Flights & hotels against the real SerpApi: a non-key and an
  unknown key refused on the page, `OMUSE_E2E_SERPAPI_KEY` accepted with the plan and searches left, Disconnect; no
  model request). Answers never carry an undecoded `link.omuse.local` stand-in (`Answer.broken_links`).
- Flights, hotels & cars, scripted (what a real run cannot force or count, each docstring says what):
  `tests/travel_search_e2e.py` (the SerpApi key: the exact Google searches, Kiwi.com for a range and self-transfer, Google
  failing → Kiwi.com, one search per request), `tests/kiwi_flights_e2e.py` (no key: the exact Kiwi.com arguments,
  Kiwi.com down, one travel search per turn), `tests/cars_search_e2e.py` (Kayak's page failing → Rentalcars.com, neither
  answering, cars_search alone), `tests/travel_card_ui.py` (the card).
- Web search: `tests/web_search_e2e.py` (web_search through Brave / Tavily / Google (SerpApi), the browser's engines
  without a key or when switched off), `tests/search_card_ui.py` (the card), `tests/brave_travel_e2e.py` (a Brave key, no
  travel key and the browser's engines blocked: exactly one travel search per message, Brave's queries, no browser page;
  the user's side is `travel_brave_real`).
  `tests/bench_search_timing.py` (not in the default list, real network) times each web search source — Brave, Tavily, the
  free Exa MCP and the browser's own engines — with `OMUSE_E2E_SEARCH_ENGINES=` (empty: the real DuckDuckGo / Bing
  instead of the stand-in) and `SEARCH_TIMING_BURST=n` for a burst of browser searches. `tests/web_search_real.py` (not in the default list, billed against the free tiers) connects the real Brave and Tavily
  APIs with the keys `OMUSE_E2E_BRAVE_KEY` / `OMUSE_E2E_TAVILY_KEY` from the e2e env file — the endpoints come from
  `OMUSE_E2E_BRAVE_EP` / `OMUSE_E2E_TAVILY_EP` on the command line, see its docstring.
- Profile fields: `tests/profile_fields_ui.py` (custom fields and addresses without limit, add and remove).
- Bots: `tests/bots_e2e.py` (each bot's skills and tools, the planner's routing and its checks, the hand-over from Main and its answer with the link, a follow-up to
  the same bot conversation, an approval and a takeover inside a bot shown in Main, cancel), `tests/bots_ui.py` (the
  menu, the bot views, the links between Main and a bot's conversation). `tests/bots_real.py` (not in the default list,
  billed) sends travel, city, card and general requests to Main with a real model and checks where each one went:
  `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh bots_real` (`OMUSE_E2E_PLANNER` /
  `OMUSE_E2E_EXECUTOR` on the command line override the env file's models). `tests/run_in_docker.sh real` runs every
  real-model suite (REAL_SUITES in the script; tests/real_common.py drives the new ones) with the env file's models —
  planner xai/grok-4.7, executor openai/gpt-6-luna (openai-codex/gpt-6-luna times out too often through the gateway for now). With `OMUSE_E2E_OUT`, each real
  suite also saves its model calls (`models-<suite>.json`: model, purpose, timing).
- Streamed answers: `tests/stream_e2e.py` (answer_delta events, reasoning held back, tool-call turns flagged, the
  setting off), `tests/stream_real.py` (not in the default
  list, billed) does one request against the gateway in the e2e env file and, when that provider streams as it
  writes (`OMUSE_E2E_MODELS=alt` picks one that does), checks the deltas arrive well before the answer.
- `python3 tests/i18n_check.py` must report no missing or stale translations
  after any UI string change.
- Use cases as the user, real model (all in `run_in_docker.sh real`, billed; browser auto-approve on as on a fresh
  install, the final pay / submit / order-change click always the user's in a takeover):
  - Travel: `travel_flight_real` / `travel_hotel_real` / `travel_michelin_real` (`travel_book_user.py`: book through the
    agency's connector, the page handed over, "My trips"), `travel_user_real`, `travel_kiwi_real`, `travel_brave_real`, `travel_connect_real`, `travel_ask_real`, `travel_nomail_real`,
    `trip_hotel_real` (trip timeline from mail and calendar; early check-in request drafted), `travel_info_real` (entry
    rules from the official stand-ins with reminders; Narita Express from JR East; payments with sourced rates),
    `travel_receipts_real` (menu photo and allergy card; expense report Excel/PDF), `city_guides_real` +
    `city_guides2_real` (every guide), `apdt_real` (Hong Kong departure tax refund: every field and document, the Vault
    card for the refund, the reference number and the link for the user's own face check).
  - Arrival cards: `arrival_real`, `arrival_cards_real` … `arrival_cards4_real` and `arrival_address_real` + `arrival_address2_real` (the user's own address from the profile; two addresses → asked once) (every country's card on a realistic stand-in at its official
    address; health / declaration answers No, gender from the profile, next city = where the flight came from,
    departure = arrival + 7 days, passport and expiry from the Vault, the CAPTCHA left, the user submits in a takeover).
  - Insurance, claims, refunds: `insurance_real`, `insurance_hk_real`, `claims_real`, `emergency_real`, `refunds_real`
    (helpers `claims_common.py`; `fake_apps.official_hits` returns the official stand-ins to searches).
  - Credit cards: `cards_real`, `cards_markets_real`, `cards_pay_real`, `cards_claim_real`, `cards_interruption_real`
    (cards in the Vault by name; helpers `cards_common.py`).
  - Everyday: `shopping_real`, `watch_real`, `support_chat_real`, `pdf_forms_real`, `brief_real`, `gcal_real`,
    `assistant_real` (`shop_common.py`); Drive and media: `drive_storybook_real`, `drive_video_real`,
    `drive_cleanup_real`, `attach_media_real`, `chart_real`, `library_real` (about 22 min), `market_real`
    (`drive_common.py`); connectors and memory: `apps_real`, `automations_real`, `telegram_real`, `mcp_real`,
    `phone_real`, `memory_real`, `chat_real` (`conn_common.py`); `bots_real`, `stream_real`, `media_real`, `mail_real`,
    `calendar_real`, `docs_real`, `photos_real`, `research_real`, `browser_auto_real`, `web_search_real`.
- Scripted suites kept for what a real run cannot do (each docstring says what and why): forced failures and retries
  (`retry_e2e`, `loop_e2e`, `stuck_e2e`, `batch1_e2e`, `batchR_e2e`, `market_e2e`), exact search guarantees
  (`kiwi_flights_e2e`, `travel_search_e2e`, `brave_travel_e2e`, `cars_search_e2e`, `web_search_e2e`, `webread_e2e`),
  security and protocol (`memory_vault_e2e`, `sendfile_e2e`, OAuth / MCP / telephony /
  Telegram protocol parts of `apps_e2e`, `mcp_e2e`, `dial_e2e`, `phone_e2e`, `telegram_e2e`, `calendar_connect_e2e`,
  `drive_connect_e2e`), the ledger internals (`batch3_e2e`), UI without a model (`*_ui.py`), and the trimmed
  `e2e_027`, `e2e_029`, `e2e_0212`, `integration`, `city_guides_e2e`, `attach_media_e2e` (the upload API's forced refusals only), `bots_e2e`, `stream_e2e`,
  `imagegen_e2e`, `videogen_e2e`, `title_e2e`, `parallel_steps_e2e`, `batch2_e2e`. The profile's first-login nudge and
  ID-photo OCR: `profile_onboarding_ui.py`. Knowledge files a skill ships (`load_skill(file=…)`) are described in
  `KNOWLEDGE.md`.
- Real web where it is safe: the read-only use cases run against the real web search and the real sites
  (`REAL_DATA_SUITES` in `run_in_docker.sh`: flights/hotels/cars, web search, research and the library, entry rules,
  trains and payments, emergencies abroad, Japan's tax refund, market data, card terms on the issuers' sites). For those
  suites the official sites' names resolve to the real sites (the stand-in host lines are taken out of the container's
  `/etc/hosts` and put back afterwards) and their checks judge live content (the test reads the same live page or
  figure itself, or checks a range and the source). Stand-ins stay only where a real site would get a real submission
  (arrival cards, insurance, the Hong Kong refund, bookings, checkouts, claim portals) or the test needs a page it
  controls (a flight in the user's own mail).
- Skill edits without a rebuild: `OMUSE_E2E_SKILLS_DIR=1 bash tests/run_in_docker.sh <suite>` loads `skills/` from the
  working tree (bots stay compiled in).
