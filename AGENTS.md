# Notes for coding agents working on this repo

## Releasing (tag `locius-vX.Y.Z`)

The version shown in the app comes from the release tag: the release workflow
(`.github/workflows/release-image.yml`) passes the tag as the `OMUSE_VERSION`
build argument (compiled into the Rust binary) and `locius-v0.3.1` is shown as `v0.3.1`. Source checkouts and
untagged builds show `SOURCE_VERSION` in `crates/omuse-common/src/util.rs` (keep `version` in the root `Cargo.toml`
equal to it).

When asked to release `locius-vX.Y.Z`, do it in this order so that the source
literal, the tagged commit and the image all agree:

1. On `main`, set `SOURCE_VERSION` in `crates/omuse-common/src/util.rs` and `version` in `Cargo.toml`
   (`cargo check` updates `Cargo.lock`).
2. Commit that change (`Release X.Y.Z`) and push `main`.
3. Tag that commit `locius-vX.Y.Z` and push the tag. The tag push builds, smoke
   tests (including a check that the app reports `X.Y.Z`) and publishes
   `ghcr.io/drlucaslu/locius:locius-vX.Y.Z`.
4. Report the run result and the image digest from the `image-import.json`
   artifact.

Do not move or re-publish an existing release tag; use a new version instead.

## Web UI assets

The page shell depends on nothing but this server: no CDN `<link>` or `<script>`,
no web fonts; `marked` and `DOMPurify` are vendored in `web/`. The sentinel
serves `web/` from memory (`crates/omuse-web/src/assets.rs`): `index.html` stamps every
asset URL with `?v=<digest>`, a stamped URL is immutable for a year, anything else
is `no-cache` with an ETag, and text assets are gzip-compressed once at load. A
unit test fails on an off-site asset; `tests/assets_e2e.py` checks the headers.

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

## Pull requests

Open a PR and report it; merge only when the user explicitly says to merge
that PR. Never commit API keys, passwords or other credentials, and keep the
local e2e environment file outside the repository.

## Rust port

The app is one Rust binary (`crates/`, Cargo workspace), ported from the Python version (v0.3.6, commit 562a36f);
`docs/rust-port.md` has the conventions and the module map, and `docs/rust-port-report/` the comparison. Settings come
from `config.toml` (`config.example.toml`), never from environment variables; only the container entrypoint
(`deploy/docker/entrypoint.sh`) turns `OMUSE_*` variables into a config file when none is mounted. The web UI (`web/`),
the skills (`skills/`) and the browser scripts (`crates/omuse-browser/js/`) are compiled into the binary. The e2e suites
check behaviour, texts and JSON shapes, so keep them stable. The Olares chart (`chart/`, packaged by `chart/build.sh`)
runs the release image with one `--role` per container.

## Tests

- Rust: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
- Full local suite: `tests/run_in_docker.sh` (fresh stack per suite).
- Image smoke test: `tests/docker_image_smoke.sh <image>`; set `OMUSE_VERSION`
  to the tag to also check the reported version.
- `python3 tests/i18n_check.py` must report no missing or stale translations
  after any UI string change.
