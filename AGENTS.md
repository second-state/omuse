# Notes for coding agents working on this repo

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

Settings → Agent → "auto-approve browser actions" is the browser connection's `config.auto_approve`; until the user
sets it, `[browser] auto_approve` in `config.toml` is the default (true; `tests/run_local.sh` sets false so the
suites keep testing the approval flow). In `routes::act::act`, an ASK that `Decision::browser_auto_ok` accepts (a
browser tool, not `browser_fill_secret` or `browser_upload`, not per-use, not after a suspected injection) is recorded
as an approval resolved by `auto`, audited as approved, run at once, and returned with `auto_approved`; the runtime
adds an `approval_auto` event and the same chat card a manual approval gets. `tests/browser_auto_approve_e2e.py`.

### Parallel steps and sub-agents

The planner (`prompts::PLANNER_SYSTEM`) gives steps that do not depend on each other the same `parallel` label; the
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
- Full local suite: `tests/run_in_docker.sh` (fresh stack per suite).
- Real-model runs: the travel suites with `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env` (`OMUSE_E2E_MODELS=alt` for
  the second model pair); `tests/travel_ask_real.py` asks two research-only travel questions and checks that no
  browser, approval or takeover was involved, with a TIMING line each; `tests/browser_auto_real.py` reads a hotel booking from mail and
  opens its address on openstreetmap.org (a data-egress check, so Sentinel always asks) with "auto-approve browser
  actions" on (no dialog, recorded as approved by auto) and off (the same request stops at that step).
- Market and README images (`deploy/market/assets`): `OMUSE_E2E_OUT=<dir> bash tests/run_in_docker.sh marketing_shots`
  photographs the current UI (showcase scenarios `SHOW_*` in `tests/fake_llm.py`) and frames it; copy
  `<dir>/market/*.webp` and `overview.png` into `deploy/market/assets`. Rebuild the test image first.
- Image smoke test: `tests/docker_image_smoke.sh <image>`; set `OMUSE_VERSION`
  to the tag to also check the reported version.
- Google Drive: `tests/drive_connect_e2e.py` (one Google sign-in for Calendar + Drive), `drive_cleanup_e2e.py`,
  `drive_video_e2e.py` (ffmpeg in the test image), `drive_storybook_e2e.py`; the Drive fake is in `tests/fake_apps.py`.
- Image and video generation: `tests/imagegen_e2e.py` and `tests/videogen_e2e.py` (scripted, in the default list);
  `tests/media_real.py` makes one picture and one short clip with the gateway in the e2e env file
  (`xai/grok-imagine-image`, `xai/grok-imagine-video`, billed): `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env
  bash tests/run_in_docker.sh media_real`.
- `python3 tests/i18n_check.py` must report no missing or stale translations
  after any UI string change.
- Travel skills: one scripted suite per skill (`tests/trip_today_e2e.py`, `sgac_e2e.py`, `arrival_cards_e2e.py`, `arrival_nomail_e2e.py`,
  `entry_requirements_e2e.py`, `flight_disruption_e2e.py`, `baggage_e2e.py`, `travel_expense_e2e.py`,
  `payments_asia_e2e.py`, `getting_around_e2e.py`, `tax_refund_e2e.py`, `emergency_e2e.py`, `hotel_requests_e2e.py`,
  `dining_dietary_e2e.py`, `credit_card_picker_e2e.py`, `city_guides_e2e.py`; the profile's first-login nudge and
  ID-photo OCR: `profile_onboarding_ui.py`; the runner turns the first-login profile nudge off for every suite, that
  one turns it on) on `tests/skills_common.py` (mail in the local IMAP server,
  an iCal calendar, the travel connector, pages under `tests/pages`); the scenarios live in `tests/fake_llm.py`
  (`travel_skill`). Knowledge files a skill ships (`load_skill(file=…)`) are described in `KNOWLEDGE.md`.
