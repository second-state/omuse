# OMuse knowledge base

How OMuse works inside, and the knowledge files it ships. Building, running and the travel use cases are in
[README.md](README.md); repository rules, the test commands and the release steps are in [AGENTS.md](AGENTS.md);
`docs/rust-port.md` records how the earlier Python version (v0.3.6, commit 562a36f) maps to the Rust crates.

## Architecture

```
 browser / Telegram ──► Sentinel :8080  (UI, login gate, policy, vault, approvals, audit, connectors)
                            │
                            ├──► Runtime :8081  (planner / executor, sub-agents, scheduler, goals, memory, skills)
                            │        └──► LLM (OpenAI-compatible; default on Olares: the Olares router)
                            └──► Browser broker :8082  (Chromium over CDP, Xvfb or headless)
```

One binary (`omuse`, Axum + Tokio) runs the three services in one process; `omuse --role
sentinel|runtime|browser|all` runs one of them (the Olares chart runs three containers). They talk over loopback HTTP
with shared tokens (`X-Persona-Runtime` runtime ⇄ Sentinel, `X-Browser-Token` Sentinel → broker, `X-Persona-UI: 1`
from the web UI); `[internal]` tokens are generated per start when blank. Sentinel also serves the optional voice app
on 8083.

### Crates (`crates/`)

| Crate | Role |
|---|---|
| `omuse` | the binary: config, roles, process lifecycle |
| `omuse-common` | config (`config.toml` → `Config`), SQLite helper, URL / injection guard, Python-compatible JSON, utilities |
| `omuse-docs` | Markdown → HTML, xlsx / docx writers, data queries, charts |
| `omuse-web` | the embedded web UI (`web/`) and the login gate |
| `omuse-sentinel-core` | store, vault, password, catalog, **policy**, ledger (no HTTP) |
| `omuse-mail`, `omuse-apps`, `omuse-mcp`, `omuse-phone` | connectors: mailboxes; Notion / Slack / calendars / market data / billing; MCP client and OAuth; phone |
| `omuse-sentinel` | Sentinel routes, actions, Telegram bot, watchers |
| `omuse-runtime` | agent loop, prompts, LLM client, scheduler, goals, memory, library, skills, image and video generation, attachments |
| `omuse-browser` | the browser broker and its page scripts (`js/`) |

The web UI, the skills (`skills/`) and the broker scripts are compiled into the binary (`include_dir`); built-in skills
are extracted to `<data>/runtime/skills-builtin/` at start unless `[skills] dir` points elsewhere.

### Configuration

Every setting comes from one `config.toml` (`--config`, or `OMUSE_CONFIG`); `config.example.toml` documents each key
and the environment variable that sets it in a container without a config file. The binary itself never reads
settings from the environment: the container entrypoint writes a config file from `OMUSE_*` variables when none is
mounted, and a mounted file always wins. `OMUSE_VERSION` is a build-time value (the release tag).

### Data layout (`[data] dir`, default `/omuse`)

```
<dir>/sentinel/        sentinel.db  audit.jsonl  vault.key  auth.json  session.key  shots/
<dir>/runtime/         runtime.db  skills-builtin/
<dir>/workspace/       the agent's files (uploads/, reports/, charts/, media/, library/, …)
<dir>/browser-profile/ Chromium user data
```

## How a request runs

1. **Plan**: the planner model turns the request into steps (`update_plan`); steps that don't depend on each other
   carry the same `"parallel": "p1"` group.
2. **Execute**: the executor model calls tools step by step. Read-only tools in one turn run concurrently; a parallel
   group runs as sub-agents (`delegate`, up to 8), each with its own browser tab ("lane", broker page
   `<task>~sub<n>`), and the executor continues when all have reported. `llm_concurrency` (Settings, default 4) caps
   simultaneous model calls.
   Every chat request is OpenAI `/chat/completions`. `service_tier: "fast"` (fast mode) and `reasoning_effort` are
   added from the settings in force: `[model] fast` / `[model] reasoning_effort` for the system's models, the user's
   own choice (Settings → Models) only while "use my own model" is on — the API ignores changes otherwise
   (`store::MODEL_OPTIONS`). An endpoint that rejects either field by name gets the request again without it;
   `reasoning_effort = "none"` also sends `chat_template_kwargs: {enable_thinking: false}` for chat-template models.
   Both are chat options only: image generation, edits and variations (`imagegen.rs`, the Images API), video
   generation (`videogen.rs`) and speech to text build their own requests and never carry them (`imagegen_e2e.py`,
   `videogen_e2e.py` and the unit tests check it).
3. **Skills**: the executor loads a skill (`load_skill(name)`) when the request matches its description; a skill can
   ship reference files read with `load_skill(name, file=…)` (relative path, 400 KB cap).
4. **Act**: every action goes through Sentinel (`/internal/act`), which decides ALLOW / DENY / ASK, runs it, audits
   it and returns a redacted result. The task waits in `WAITING_APPROVAL` for an approval card, or in
   `WAITING_EXTERNAL` for a takeover (the user drives the live browser, then hands it back).
5. **Answer**: an outcome check makes sure the answer doesn't claim actions that didn't happen; memory extraction
   proposes facts and profile fields for the user to confirm.

## Google Drive

One Google sign-in covers Calendar and Drive. The Drive card (Settings → Connections) starts the same OAuth flow as
the calendar (`connections/drive/google/start` with the managed client and the relay page, or `connections/drive/start`
with the user's own client and this box's callback) with `services: ["calendar", "drive"]`; `gcal::scopes_for` builds
the consent's scope string and the pending flow (`calendar_oauth` kv) records the services. The shared callback
(`/sentinel/api/connections/calendar/callback`, so no new redirect URI has to be registered) narrows the services to
the scopes Google actually granted (`gcal::granted_services`; the user can untick one), verifies each (a calendar
listing, a Drive `about`), stores the same refresh token as `cred_calendar_1` and/or `cred_drive_1`, and enables the
connections (a Drive row made before this version gets `read` / `upload` / `write` seeded once). Disconnecting one
leaves the other. The Drive scope (`auth/drive`) is restricted at Google: the user's own OAuth client in Testing
status may use it; the managed client would need Google's verification first.

`omuse-apps/src/gdrive.rs` is the Drive v3 client (`about`, `list` with the Drive query language, `get`, `download`
with Google Docs / Sheets / Slides exported as PDF / XLSX / PDF, multipart `upload`, `create_folder`, `update` =
move / rename, `trash` — never a permanent delete). It borrows `GCal`'s token refresh (`access_token()`); access
tokens are cached in `cred_drive_access`. Sentinel's `actions::drive_call` runs the tools: `drive_search` (`drive_query`
turns kind / folder / dates / `unused_days` into `q`; `min_size_mb` is filtered after fetching pages ordered by
`quotaBytesUsed`, since the query language has no size term; files never opened count as unused), `drive_get`,
`drive_download` (≤ 200 MB, bytes handed to the runtime, which writes `workspace/drive/<name>` like a mail
attachment), `drive_upload` (the runtime serves the workspace file; approval), `drive_create_folder`, `drive_move`
(approval), `drive_trash` (≤ 50 ids per call; approval card lists the names; policy reason says the bin keeps them
30 days), `drive_storage`. Reads taint the task like mail; an upload from a tainted task is a possible egress (high
risk). File names and metadata are untrusted (`untrusted()` scans them).

`edit_video` (`agent/videos.rs`) is local ffmpeg: `info`, `concat` (every clip fitted to the first's frame; audio only
when every clip has it), `trim`, `resize` (presets 720p / 1080p / square / vertical), `speed` (`atempo` chained),
`mute`, `add_audio` (looped and cut to the video), `caption` (drawtext with the Noto CJK font, text via a temp file),
`gif`. Outputs go to `videos/`, a `video` event is emitted and the file is sent to the chat unless `send=false`.

Skills: `drive-cleanup` (quota, big and unused files in one turn, a numbered list the user confirms, one `drive_trash`
batch, the 30-day note), `drive-video-edit` (search → download → `edit_video` chain with `send=false` on intermediate
steps → upload back), `drive-photo-storybook` (photos → `file_look` → optional edits → text → one `make_pdf` with
`![…](drive/…)` pages → upload). All three are `browser: off`.

Tests: `tests/fake_apps.py` serves a Drive fake (`/drive`, `/drive-upload`: seeded folders, two real MP4 clips made with
ffmpeg at start, three JPEG photos, big and old files with metadata only, a shared file, a trashed one; `/drive/_state`
and `/drive/_reset` for the suites) behind the same fake Google OAuth as the calendar; `tests/drive_connect_e2e.py`
(one sign-in connects both, a sign-in where Drive was unticked, catalog tools and permissions, disconnect one),
`drive_cleanup_e2e.py`, `drive_video_e2e.py`, `drive_storybook_e2e.py` (scripted flows `DRIVECLEAN`, `DRIVEVIDEO`,
`DRIVEBOOK` in `tests/fake_llm.py`).

## Image and video generation

`make_image` (`imagegen.rs`, the OpenAI Images API: `POST {base}/images/generations`, `/images/edits`) and
`make_video` (`videogen.rs`, the OpenAI Videos API: `POST {base}/videos` starts a job, `GET {base}/videos/{id}` polls
it, `GET {base}/videos/{id}/content` downloads the clip; a server that answers the POST with `data[0].b64_json` /
`url` is taken as it is) share one pattern:

- **Endpoint and key are optional.** Each kind resolves `<kind>_base_url` ("" = the model endpoint) and
  `<kind>_model` ("" = auto-pick from the endpoint's `/models` by mode or name) from the settings in force, and its
  bearer key with `imagegen::media_auth_headers`: `<kind>_api_key` when set, else the model endpoint's key. One
  provider for everything needs nothing but the model fields; a chat model at one provider and pictures or clips at
  another needs the extra URL (and key, if different).
- **Never a mix of the system's and the user's.** With the system's models the values are `[model] image_url` /
  `image` / `image_api_key` and `video_url` / `video` / `video_api_key` from `config.toml` (`store::system_model`,
  `effective_model`). With "use my own model" (Settings → Models) they are the user's own, optional, values; the
  system's media endpoints and keys are never used with the user's set, and the user's never with the system's.
  The defaults of `image_base_url` / `video_base_url` in the settings table are therefore "" — the system's URLs are
  overlaid by `system_model()` only while the system's models are in force.
- **Keys never leave the runtime.** `store::SECRET_KEYS` (`model_api_key`, `image_api_key`, `video_api_key`) are
  stripped by `redacted()` and reported as `<key>_set`; the audit log carries `(set)` / `(cleared)`. `PUT
  /api/settings` keeps a saved key on "" and clears it on `null`.
- **Vendor differences are absorbed, not configured.** Optional request fields (`quality`, `background`,
  `response_format`, … for images; `seconds`, `size`, `aspect_ratio` for video) are dropped one by one when a server
  rejects them by name; sizes walk a fallback list, and after the first rejected size of a non-square shape the shape
  is sent once as `aspect_ratio` without a size (xAI's grok-imagine models take ratios and only 1k/2k squares as
  sizes; OpenAI answers "unknown parameter" and the next size is tried). A video length the server does not offer is
  snapped to OpenAI's 4 / 8 / 12 once, then left to the server. Moderation refusals are relayed as such, without a
  retry.
- **Cost guards.** `[model] image_budget` (6) pictures and `[model] video_budget` (2) clips per task; the tools tell
  the model when the budget is spent, and the prompt says to generate once and offer changes.
- **Tests.** Unit tests with a fake transport in both modules; `tests/imagegen_e2e.py` and `tests/videogen_e2e.py`
  against the fake server (`/v1` and a second provider at `/vid/v1`, which records the bearer of every call);
  `tests/media_real.py` makes one picture and one clip with a real gateway (`xai/grok-imagine-image`,
  `xai/grok-imagine-video`) and is not in the default list.

## Profile

The profile (Memory → Profile) holds the fixed details forms and emails need: the named fields in
`store::PROFILE_FIELDS` (name as in the passport, phone, emails, home and work address, company, birthday,
nationality…), any number of further addresses as `address:<label>` ("Singapore flat", "Parents in Sydney"), and free
fields as `custom:<label>`. The agent reads it with `profile_get` (only the fields a form needs; `address:*` selects
every further address) and proposes changes with `profile_suggest`, which the user confirms on the Memory page.
Identity, membership and card numbers never go into the profile: they live in the Sentinel vault.

**First login.** While the profile lacks the name as in the passport, the birthday or the nationality
(`GET /api/profile/status`), the first chat view of a browser session goes straight to Memory → Profile and the chat
shows a banner until the profile is complete or the user chooses "Later" (`profile_reminder` setting). The profile
card takes a photo of the passport data page or an ID card: `PUT /api/profile/ocr` sends it to the vision model
(`You read an identity document…`, JSON answer) and returns the name, birthday and nationality to prefill the form
and the document number as a proposed vault item, which the user saves with one click. The picture is read in memory
and never written to the workspace; the number is not logged and not stored in the runtime.

The arrival-card skills use the addresses like this: a hotel booking for the dates in mail is the accommodation;
otherwise the one profile address in the destination country is used without asking (a resident going home, an
owner's flat); several addresses in that country → the agent asks in the chat which one, before opening the browser;
none → calendar and memory, then one question (or an uploaded booking). `tests/sgac_e2e.py` covers the one-address and
the two-address cases.

## Sentinel policy

- **Data classes and taint**: reading mail, the vault or private connectors taints the task; a tainted task that
  sends data somewhere new (a URL with parameters on a site it hasn't used, a message to a new recipient) is asked
  for (the data-egress check). A domain written in the user's own request is trusted.
- **Browser auto-approval** (`[browser] auto_approve`, default on; Settings → Agent): page loads, clicks, typing and
  submits that would have asked run at once, with an approval record resolved by "auto", audited, and shown in chat
  and the timeline like a manual approval. Never auto-approved: vault fills (`browser_fill_secret`), uploads, clicks
  that pay or change an order, anything after a suspected prompt injection; logins and CAPTCHAs are takeovers.
- **Vault**: secrets are encrypted with `vault.key`; the agent refers to an item by id and Sentinel fills the value
  into the page, restricted to the item's domains, so the value never reaches the model, the chat or the trace.
- **Prompt injection**: mail, pages and tool output are untrusted; suspicious content escalates writes to approval.
- **Audit**: `audit.jsonl` is append-only and hash-chained (`/sentinel/api/audit/verify`).

## Web UI language

The UI shows one language at a time: English by default, Chinese only when the language setting says so. Strings go
through `T()` / `Tf()` with the tables in `web/i18n.js` and `web/i18n_tw.js`; `python3 tests/i18n_check.py` fails on
missing or stale translations and on mixed-language strings.

## Tests

- Rust unit tests live next to the code (`cargo test --workspace`).
- The end-to-end suites (`tests/*_e2e.py`, `*_ui.py`, `e2e_0*.py`) run inside the `Dockerfile.test` image: each gets a
  fresh stack started by `tests/run_local.sh` (the binary, the scripted model `tests/fake_llm.py`, fake third-party
  services in `tests/fake_apps.py`, an IMAP server, test pages under `tests/pages` served as `http://shop.test:8099`).
  The scripted model plays the agent: it matches a marker in the request (`SGAC`, `ARRCARD`, `ARRNOMAIL`, …) and
  returns the tool calls a correct agent would make, so a suite checks the tools, approvals, takeovers and the
  rules a skill must keep.
- The travel-skill suites share `tests/skills_common.py` (mail seeded by IMAP APPEND, calendar, travel connector,
  profile, vault, uploads, approvals and takeover hand-backs).
- Real-model suites (`*_real.py`, and the travel suites with `OMUSE_E2E_ENV_FILE`) go through `tests/model_proxy.py`,
  which times model calls so the `TIMING` line reports the agent's own time.

## Publishing images for Products Provisioner

`.github/workflows/release-image.yml` builds `Dockerfile.fly` for linux/amd64, tests the image
(`tests/docker_image_smoke.sh`) and pushes it to `ghcr.io/<repository-owner>/<repo>` on an `omuse-v*` tag (older
`locius-v*` tags are still accepted). It uses the workflow's `GITHUB_TOKEN` (`packages: write`); make the GHCR package
public after the first push. No password or key is baked in: each instance gets them at run time.

In the provisioner's `product.toml`, use a registry image source and **Sync image** with the new tag:

```toml
[image_source]
type = "registry"
repository = "ghcr.io/second-state/omuse"
```

Use a new version for changed source; never move a release tag (the provisioner does not let a version change its
image). For an optional callback import (`import_release` on a manual run, callback image sources only), set the
Actions variables `RELEASE_PRODUCT_KEY` and `RELEASE_IMPORT_URL` (HTTPS origin only) and the secret
`RELEASE_IMPORT_TOKEN` (that product's `callback_token`, not an admin key). The import is `POST /api/images` with
`product_key`, `version`, `image` (digest) and `parent_version: null`; the `image-manifest-<version>` artifact keeps the
JSON. Provisioner settings: port **8080**, persistent storage at **`/omuse`**, health check **`/sentinel/api/health`**,
a generated `OMUSE_PASSWORD`; keep the app always-on for schedules, triggers and Telegram.

Check an image locally without credentials:

```bash
docker build --platform linux/amd64 --build-arg OMUSE_VERSION=omuse-v0.4.1 -f Dockerfile.fly -t omuse:local .
OMUSE_VERSION=omuse-v0.4.1 bash tests/docker_image_smoke.sh omuse:local
```

The smoke test checks the three services, login, unprivileged start with a root-owned volume, Chromium PDF rendering
and persistence across container replacement, and shows the version from the tag (`omuse-v0.4.1` → `v0.4.1`).

# Knowledge files shipped with OMuse

Some skills carry data beside their `SKILL.md`; the agent reads it with `load_skill(name, file=…)`. This file says
where each knowledge base lives, what shape it has and how to update it. Rules shared by all of them:

- The source of truth is the file named below; generated files are rebuilt by the skill's script and never edited by
  hand (a header says so).
- Every fact carries a verification date and, where possible, a source URL; the agent is told to say the date and to
  treat rows marked unverified as leads to check on the official page, not as facts.
- Reference files are plain text or Markdown under the skill's folder (they are compiled into the binary with the
  other skills, extracted at start under `skills-builtin/`, and read with a 400 KB cap per file).

## Credit card catalog (`skills/credit-card-picker/`)

What the `credit-card-picker` skill reads to answer "which card should I use for this purchase".

| File | Role |
|---|---|
| `cards/catalog.json` | **source of truth**: one record per card |
| `cards/us.md`, `cards/hk.md`, `cards/sg.md` | generated per-market tables the agent loads (`load_skill(file="cards/sg.md")`) |
| `build_cards.py` | rebuilds the tables from the JSON, checks it, and can import the personal_finance catalog |

### Record shape

```json
{"issuer": "Chase", "product": "Sapphire Reserve", "aliases": ["CSR"], "region": "US", "currency": "USD",
 "network": "Visa", "unit": "points", "point_value_cents": 1.0, "annual_fee_cents": 79500, "fx_fee_pct": 0,
 "rules": {"dining": 3.0, "travel": 4.0, "transit": 5.0, "other": 1.0},
 "caps": {"travel": "direct airline / hotel bookings only"},
 "merchant_rules": [{"keywords": ["lyft"], "earn_rate": 10.0, "end_date": "2027-09-30", "notes": "…"}],
 "rule_notes": {"travel": "4x only on flights and hotels booked directly …"},
 "benefits": ["8x through Chase Travel …"], "verified_at": "2026-08-25", "verified": true,
 "source": "https://creditcards.chase.com/…"}
```

- `region` is the issuing market (`US`, `HK`, `SG`, …) and decides which table the card lands in; `currency` is the
  card's billing currency.
- `unit` is `points`, `miles` or `cashback`; `point_value_cents` is the value of one point / mile at a plain
  redemption (cash, statement credit, the programme's standard rate) — for `cashback` the rate itself is the value.
- `rules` maps the canonical categories (`dining, travel, transit, groceries, gas, online_retail, streaming,
  entertainment, drugstore, utilities, insurance, foreign, other`) to the multiplier (points per unit of currency) or
  the cashback percentage; `other` is the base rate and must be present.
- `caps` and `rule_notes` carry the conditions in words (monthly caps, direct bookings only, minimum spend); the agent
  quotes them. `merchant_rules` are merchant-limited multipliers with an end date.
- `verified_at` is the day someone checked the issuer's page; `verified: false` marks a seed from general knowledge
  (the Singapore rows shipped today), which the table shows as **UNVERIFIED (seed)**.

### Updating

1. **Change or add a card**: edit `cards/catalog.json` (a new record, or new numbers in an existing one), set
   `verified_at` to today and `verified` to true, and put the issuer's page in `source`. Remove a discontinued card
   or set its `rules` to what still applies and say so in `rule_notes.other`.
2. **Rebuild and check**: `python3 skills/credit-card-picker/build_cards.py` — rewrites the `.md` tables and prints
   `PROBLEM` lines for duplicates (issuer + product + region), missing fields, unknown categories and verification
   dates older than 400 days (those are warnings; the others fail the build).
3. **Import from personal_finance** (the same catalog, maintained there with a research agent that re-verifies
   entries): `python3 skills/credit-card-picker/build_cards.py --import ../personal_finance/crates/card_rewards/card_catalog.json`
   merges by issuer + product + region — imported records replace ours, our own additions (e.g. the SG seed) stay.
   Then review `git diff skills/credit-card-picker/cards/`.
4. **Verify a seed row**: open the issuer's page, fill `rules` / `caps` / `rule_notes` from it, set `source`,
   `verified_at` and `verified: true`, rebuild.
5. **Test**: `cargo test -p omuse-runtime load_skill` (the reference-file reader) and the Docker suite
   `tests/run_in_docker.sh credit_card_picker_e2e` (the skill reads the profile and the table, no browser).
6. The binary embeds `skills/`, so a catalog change ships with the next build; users who imported a copy of the skill
   from GitHub get it on re-import.

### What the agent does with it

It reads the user's cards from the profile (`custom:credit cards`, names only — never numbers), loads the market's
table, matches by issuer + product (aliases, tiers kept apart), computes the value per card with `calculate`, and
answers with the maths, the caveats and the verification date. A card missing from the catalog is looked up on the
issuer's page and reported as "can be added to the catalog".

## City guides (`skills/<city>/SKILL.md`)

Fifteen briefings for Westerners: `beijing`, `shanghai`, `shenzhen`, `guangzhou`, `chengdu`, `tokyo`, `osaka`, `seoul`,
`singapore`, `hong-kong`, `taipei`, `bangkok`, `kuala-lumpur`, `ho-chi-minh-city`, `hanoi`. Each guide is itself the
curated data: facts researched from official sources and reputable press, each volatile one dated ("checked October
2026"). The agent answers from the sheet and from data tools (`market_data` for exchange rates, the travel connector for
flights, hotels and prices, mail and calendar for the user's bookings), **never the browser**; for a rule that decides
whether the user can enter, it hands off to `entry-requirements`, and for tasks to the other travel skills.

The rule is enforced, not only written: the guides carry `browser: off` in their front matter. When `load_skill`
loads such a skill, the runtime records it for the task (`Inner::no_browser`): the browser tools and `delegate` are
no longer offered to the executor or its sub-agents, a call made anyway is refused and audited (`skill_policy`,
`browser_off`), and loading a skill without the flag (a booking skill, say) lifts it for the rest of the task. Any
skill can use the flag.

### Shape
Frontmatter (`name` = folder, a bilingual `description` ending "use for any question about visiting, doing business in
or moving to <City>"), then: How to use this skill (the **No browser** rule and the hand-off line naming the country's
arrival-card skill) · Orientation · Entering … and the airports · Money · Getting online and the apps · Getting around ·
Business scene and customs · Living here · Food · What to see · Timeline · Sources to verify against · Remember what you
learn. 14–20 KB each (CJK, Thai and Vietnamese text counts 2–3 bytes per character); `tests/city_guides_e2e.py` checks
the shape, the hand-offs, the absence of browser tools, the dates and the size, and that a scripted answer uses only
the guide and `market_data`.

### Refreshing a guide
1. Work through the facts most likely to change first: visa-free lists and durations, arrival cards and entry fees,
   airport rail fares, new metro lines and airports, payment rules for foreign cards, tax refund rules, rents and
   school fees, population and GDP (official statistics bulletins).
2. Check each against the official sources listed at the end of the guide; replace the fact and its date ("checked
   <month year>"). A fact that cannot be confirmed is removed or marked "(verify)" — never estimated.
3. Keep China-wide (or country-wide) facts identical across the guides of the same country (e.g. the 30-day visa-free
   list and the 240-hour transit in all five Chinese guides; the RTS Link date in `singapore` and `kuala-lumpur`).
4. Run `bash tests/run_in_docker.sh city_guides_e2e` (rebuild the test image first: skills are compiled in).

