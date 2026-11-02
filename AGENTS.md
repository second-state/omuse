# Notes for coding agents working on this repo

**Natural language is the model's; code tools work only on JSON (hard rule).** The model turns the user's words (and any natural language: mail text, a page's prose) into a code tool's JSON arguments; the code tool uses only that JSON, runs every step in code and returns JSON; it never calls the model; the model words the result for the user. Known websites and their HTML are code's (form functions with fixed ids, the fixed parts of known pages). No regex or keyword list over natural language, anywhere. Read `docs/json-tools.md` (the design: workflow tools, the run-loop protocol, `show` — the facts code appends to the answer — and the optional `hints`, the API for adding a machine) before changing a skill, a machine or the run loop. The rule is in `CLAUDE.md` ("Natural language is the model's; code tools work only on JSON").

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
adds an `approval_auto` event and the same chat card a manual approval gets. The final
click that pays, confirms a purchase or booking, or changes an order is never the agent's, whatever the setting: the
policy refuses it with an instruction to hand the filled page over (`browser_request_takeover`), and the user presses it
in the live view (`policy::hand_over_click`; a checkbox or option, e.g. agreeing to a fee, is no such click; a page that
receives money — a refund method — takes a Vault card without a purchase, `guard::refund_page`).

### Parallel steps and sub-agents

The planner runs only when the executor asks for a plan (`plan_task`) or loads a skill whose front matter says
`plan: on` (`Runtime::plan_on_request`; ordinary skills are followed from their sheet), not before every turn; simple requests are answered by the executor directly (`tests/integration.py` checks that a plain
chat makes no plan). The planner (`prompts::PLANNER_SYSTEM`) gives steps that do not depend on each other the same `parallel` label; the
live state (`prompts::live_state`, JSON at the end of each request) carries the next such group as data
(`plan.next_parallel`). The executor loop
(`agent/runtime.rs`) runs a turn's calls that are all in `tools::PARALLEL_TOOLS` (sub-agents and reads that never touch
the task's browser page) at the same time, with `no_ask`, and appends the results in call order; everything else runs
one by one. `delegate` takes one sub-task or `tasks=[{role, task, step}]`; sub-agents of one call, or of one turn, run
side by side, each on its own browser page (`lane` in `/internal/act` → broker page `<task>~sub<n>`, see
`routes::act::page_id`), mark their plan step running / done, and are limited by `tools::SUBAGENT_MAX`. Their model
calls share `llm_concurrency` (Settings → LLM, 1..=8, applied without a restart).

## Tests

**Walk the whole real process before coding a form function** (CLAUDE.md, hard rules): every screen mapped (URL, input ids/names, required, options, validation, what enables Next) up to the user's step, with fake data, before any code; the map is kept beside the form function (`docs/sites/<site>.md`).

**Only real-URL e2e tests have meaning.** An e2e test runs the real model against the real site or service (real URLs, real APIs, real official pages) and stops at the first step the user owns (sign-in, emailed code, CAPTCHA, Submit, Pay), with clearly fake test data and nothing paid, submitted or sent. The e2e suites (the real-model ones, `*_real.py`) use no fake pages, fake sites, fixture data or fake workflows at all. The fake tests (the scripted fake-model suites) stay because they are fast, but they are never counted as e2e.

**When a test fails, first decide whether the test is needed at all.** Keep it only if it checks something real from the user's point of view: real pages and real data, or a code path whose input and output are structured. Remove it (and say so) when it checks the model's optional wording, depends on pages or ids we invented (a stand-in's confirmation page, a made-up shop's selectors), or duplicates another check. A test on our own fake pages is a fake test, never an e2e test, and is listed as such. Only a test worth keeping gets the product fixed for it.


- Rust: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
- **No fake services, no stand-in pages, no fixture data** (2026-11-02). The fake servers (Notion/Slack/Google/Drive,
  travel connector, MCP, Microsoft, Telegram, telephony, voice), the HTTPS stand-ins of official sites, `tests/pages/`
  and the test IMAP/SMTP server are gone. The only fake is the scripted model `tests/fake_llm.py`. Do not add a fake
  server, a stand-in page or recorded responses back; a use case is tested on the real site.
- The test image (`Dockerfile.test`) always compiles the current sources: its cargo target cache is shared by every
  build (any branch or worktree) and cargo rebuilds by file mtimes, so an older checkout's sources once shipped a
  newer build's stale binary. The builder touches every source file before `cargo build`, compiles in a build stamp
  (`OMUSE_BUILD_STAMP`, a hash of `Cargo.*`, `crates/`, `web/`, `skills/`, `bots/`), copies the binary out of the
  cache mount in the same `RUN` and fails unless `omuse --build-stamp` prints that stamp; the final stage checks it
  again against `/opt/omuse/build-stamp` and prints it. To check an image: `docker run --rm --entrypoint sh <image> -c
  'omuse_stamp=$(/opt/omuse/bin/omuse --build-stamp); cat /opt/omuse/build-stamp; echo $omuse_stamp'`. Build from a
  worktree with the context on stdin, e.g. `tar --exclude=./target --exclude=./.git -cf - . | docker build -f
  Dockerfile.test -t omuse-test -`.
- Scripted (fake-model) suites — fast, never counted as e2e: `bash tests/run_in_docker.sh` (every `*_e2e.py`, `*_ui.py`,
  `e2e_0*.py`, `integration`, `takeover_burst`, `i18n_check`; fresh stack per suite, `-j 4` to spread them). The stack
  (`tests/run_local.sh`) is the app and the scripted model only; web search, travel data and market data point at a
  port nothing listens on, so these suites stay off the internet (browser pages that need a site use `example.com`).
  After every suite `tests/args_guard.py` checks the recorded tool calls: no search with a price cap ≤ 0 or fewer than
  1 traveller, no skill refused as another bot's. They cover what needs nothing but the app: the run loop (`retry_e2e`,
  `loop_e2e`, `stuck_e2e`, `stop_ui` Stop, `batch1_e2e` forced outages, `e2e_027` step budget, `stream_e2e`, `title_e2e`), approvals
  and security (`integration`, `memory_vault_e2e`, `sendfile_e2e`, `attach_media_e2e`, `pdf_e2e`), takeovers
  (`takeover_burst`, `takeover_tabs_ui`), memory (`batch2_e2e`), bots (`bots_e2e`, `bots_ui`), skills (`skills_e2e`,
  `city_guides_e2e`), media (`imagegen_e2e`, `videogen_e2e`), PDF forms (`e2e_0212`, its form in `tests/fixtures/`),
  models and settings (`models_e2e`, `models_ui`, `settings_media_ui`, `e2e_028`, `e2e_029`), and the web UI
  (`*_ui.py`: navigation, cards before a key, profile, chat, resize, idle, live updates, brand, run-pending).
  `python3 tests/i18n_check.py` must report no missing or stale translations after any UI string change.
- Stop: the chat's task card has a quiet Stop button in its opened Activity panel (`POST /api/tasks/{id}/cancel`, like
  the Tasks page's Cancel). `Runtime::cancel` sets the flag that every model call (`Llm::cancelled`), tool call
  (`exec_call`, which also drops a call still waiting), code-machine step and sub-agent turn checks; the status stays
  CANCELLED (`set_status` never leaves it) and the bot task it handed over to is cancelled too. Unit tests `stop_*` in
  `agent/tests.rs`; `tests/stop_ui.py` (LOOPFOREVER in `tests/fake_llm.py`) stops a never-ending task from the chat.
- E2e suites (`*_real.py`, real model; billed): `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh
  -j 3 real` (`realdata` for `REAL_DATA_SUITES` only). They act as the user, no exceptions: `tests/user_common.py`
  drives the web UI (types into the chat, answers approval dialogs, takes over and hands back the browser) and the checks
  use only what the user sees — the answer and its links, file cards and downloads, dialogs, the browser view. No tool
  names, arguments, events or logs in a real-model check; when one fails, first decide whether the check is needed,
  then fix the product. They run against real sites and APIs only, stop at the first step the user owns, never pay,
  submit, book or send, and check only real external data (the test reads the same live page or figure itself, or
  checks a range and the source), fixture-independent outcomes (handed over, approval shown, file produced, nothing
  sent), the block code appends, and anti-hallucination. Models: planner xai/grok-4.7, executor openai/gpt-6-luna; with
  `OMUSE_E2E_OUT` each suite saves its model calls (`models-<suite>.json`).
  - `REAL_DATA_SUITES` (real web and APIs): `travel_user_real` (SerpApi key pasted on Connections: flight, date range,
    hotel, car; prices and booking links that open), `travel_kiwi_real` (no key: Kiwi.com), `travel_brave_real` (a Brave
    key, no SerpApi), `travel_connect_real` (the Flights & hotels card with a real key), `web_search_real` (real Brave and
    Tavily keys; the browser's engines switched off for it), `travel_info_real` (official entry links, JR East fares,
    reminders), `emergency_real`, `tax_refund_real`, `market_real`, `research_real`, `library_real` (about 22 min),
    `cards_real`, `cards_markets_real` (issuers' sites), `news_report_real` (a real subject on the real web),
    `travel_ask_real`, `stream_real`, `bots_real` (routing), `shopping_real` (the Shopping bot on Shopify's real
    Global Catalog with `OMUSE_E2E_SHOPIFY_CLIENT_ID` / `_SECRET` from the env file — written into `[shopping]` by
    `tests/run_local.sh` in real mode: a Singapore profile's request gets a choice card of offers that ship to SG,
    priced in SGD; the map of the APIs is `docs/sites/shopify.md`).
  - `REAL_APP_SUITES` (the app with the real model and media provider, nothing outside faked): `assistant_real`,
    `automations_real`, `memory_real`, `chat_real`, `docs_real`, `chart_real`, `media_real`, `attach_media_real`,
    `travel_receipts_real`.
  - `REAL_SITES` — bookings and orders on the real booking sites and shops, run one at a time:
    `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh realsites` (forces `-j 1`; not in
    `real`). Code searches the real data, picks by rule, opens the REAL booking page (the offer's booking link, the
    hotel seller's page, the shop's product page) and hands it to browser-use, which fills it with the profile's clearly
    fake data (`tests/handover_common.py` `FAKE`) and hands over at the first step that is the user's (sign-in, CAPTCHA,
    Pay / Place order); the suite hands back without pressing anything. No card in the Vault (`U.setup(vault=False)`):
    not even a test card goes into a real payment form. Checked: the choice card, the hand-over on the real site, no
    card number or secret in the page, the appended block (booking link, amount), nothing booked. A site that blocks
    automation is handed over at the block — never get around bot protection. `travel_book_real` (a Kiwi.com flight
    without a key, then a Google Hotels hotel with the SerpApi key). (Shopping has no browser path: the Shopping bot
    searches by API only, `shopping_real`.)
    The arrival cards are in `REAL_SITES` too (`arrival_real`, `arrival_cards_real` … `arrival_cards4_real`,
    `arrival_address_real`, `arrival_address2_real`): each opens the official government site, the card's form function
    (`forms.rs`, mapped from the live sites 2026-11-02) fills what it can reach and the hand-over is at the first user
    step — TDAC: the Cloudflare check on its home page; Visit Japan Web and eTravel: the sign-in to the user's account
    (the email put in); China: the passport photo of step 1; Taiwan and the UK ETA: the emailed code (`email_code`,
    agent/email_code.rs — with the test inbox connected from `OMUSE_E2E_IMAP_*` code sends the code, the model reads it
    from the mail and code types it; without one a SKIP line and the hand-over at the email step; Taiwan's traveller form
    then needs the occupation from its fixed list — code selects the list's general business person, a fixed option
    value per site, the user's decision 2026-11-02); SG, MDAC, Korea: the filled form for the user's review and Submit;
    Indonesia and Cambodia (walked end to end 2026-11-02, `docs/sites/id-arrival-card.md`, `kh-arrival-card.md`): the
    Declaration page and step 5 Confirm Declaration, each by its Submit — Indonesia's hotel must be on the site's own
    list (the request names Alila Seminyak), Cambodia's travel step needs the hotel's phone (`stay_phone`, in the
    request; asked before the form opens otherwise). Fake traveller data (`tests/arrival_common.py`
    `PROFILE`, a fake passport in the Vault; the email is the test inbox). Success (user's decision 2026-11-02, the same
    for any multi-page form suite): the run reached the form's final page — the one with the user's Submit, found by its
    element — and handed it over, nothing submitted; or, where the site stops earlier at a user step, the hand-over at
    that step. No per-field value checks on the handed-over page.
    Insurance, claims, baggage and the departure tax are in `REAL_SITES` too, each on the real site (`forms_claims.rs`,
    mapped 2026-11-02) and stopped at the first user step: `insurance_real` (FWD Singapore: the quote → the plan's
    button → handed over at FWD's eligibility statements; the claim's step 1 filled by input names → handed over (no per-field value checks), since
    "next" checks the policy on FWD's server), `insurance_hk_real` (bolttech's fillable PDF opened directly and filled),
    `cards_claim_real` / `cards_interruption_real` (eClaimsLine redirects to Card Claim Center: the hand-over at the
    sign-in is checked — the claim lies behind the account), `claims_real` (Singapore Airlines' status page;
    WorldTracer's mybag.aero for an airline on it, ANA: a fake reference gets WorldTracer's own "No record" answer) and
    `apdt_real` (the APDT platform up to its travel-document upload; never a made-up document, never handed over).
- Market and README images (`deploy/market/assets`): `OMUSE_E2E_OUT=<dir> bash tests/run_in_docker.sh marketing_shots`
  photographs the current UI (showcase scenarios `SHOW_*` in `tests/fake_llm.py`) and frames it; copy
  `<dir>/market/*.webp` and `overview.png` into `deploy/market/assets`. Rebuild the test image first.
- Image smoke test: `tests/docker_image_smoke.sh <image>`; set `OMUSE_VERSION` to the tag to also check the reported
  version.
- Skill edits without a rebuild: `OMUSE_E2E_SKILLS_DIR=1 bash tests/run_in_docker.sh <suite>` loads `skills/` from the
  working tree (bots stay compiled in).
- Knowledge files a skill ships (`load_skill(file=…)`) are described in `KNOWLEDGE.md`.
