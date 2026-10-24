# OMuse Rust port — architecture and porting conventions

This document is the contract for everyone (human or agent) porting a piece of the Python
application under `app/` to the Rust workspace under `crates/`. Read it fully before writing code.

## Status (2026-10-24)

The port is complete and the Python application has been removed from the tree. Every `app/...` path in this
document and in the crates' comments, like `deploy/docker/gate.py` and the Python unit tests `tests/test_units.py` /
`tests/test_027.py` that Rust tests cite, refers to the Python reference at v0.3.6 (commit 562a36f): `git show
562a36f:app/sentinel/main.py`. What the binary embeds moved: `app/web` → `web/`, `app/skills` → `skills/`,
`app/browser/*.js` → `crates/omuse-browser/js/`. The Olares chart runs the release image (`chart/build.sh`).

Four e2e suites imported Python modules to check results; they now check through the running binary or the files it
writes: `mail_generic_e2e.py` (the mailbox through the API and `/internal/act`, Outlook against the local Dovecot via
`[endpoints] outlook_imap` / `outlook_smtp`), `password_e2e.py` (the stored PBKDF2 hash), `e2e_0212.py` (the filled PDF
read with pypdf) and `e2e_029.py` (the click rule is a Rust unit test, `click_risk`). The Python unit tests
(`tests/test_units.py`, `tests/test_027.py`) tested the Python modules and were removed with them; the Rust crates carry
their own tests. Comparison numbers: `docs/rust-port-report/`.

## Goal

A faithful, 1:1 feature port of the Python services (FastAPI/uvicorn) to Rust (Axum/Tokio):
same HTTP routes, same JSON shapes, same status codes, same error bodies, same strings
(bilingual Chinese/English texts byte-identical), same files on disk (SQLite schema, audit.jsonl,
vault.key, auth.json, session.key, workspace layout). The existing end-to-end suites in
`tests/*.py` are **not** changed: they must pass against the Rust port as they pass against
the Python original. When the Python code is odd, port the oddity; do not "improve" behaviour.

Three logical services, exactly like the Python version, but one binary (`omuse`) and one
process by default:

| Service  | Python                 | Rust crate        | Listens                     |
|----------|------------------------|-------------------|-----------------------------|
| Sentinel | `app/sentinel/main.py` + `deploy/docker/gate.py` | `omuse-sentinel` | `0.0.0.0:8080` (UI, `/api/*` proxy to runtime, `/sentinel/api/*`, `/internal/*`) and the voice app on `0.0.0.0:8083` |
| Runtime  | `app/runtime/main.py`  | `omuse-runtime`   | `127.0.0.1:8081`            |
| Browser  | `app/browser/main.py`  | `omuse-browser`   | `127.0.0.1:8082`            |

They still talk to each other over loopback HTTP with the same headers
(`X-Persona-Runtime: <runtime_token>` runtime⇄sentinel, `X-Browser-Token: <browser_token>`
sentinel→browser, `X-Persona-UI: 1` for the web UI), so a role-split deployment (the Olares chart
runs three containers) keeps working: `omuse --role sentinel|runtime|browser|all`.

Shared code lives in `omuse-common` (`app/common/*` plus config, db, helpers).

## Configuration: `config.toml` instead of environment variables

All settings that the Python version read from the environment come from one TOML file
(`omuse --config path/config.toml`, default `./config.toml`; `OMUSE_CONFIG` may also name it).
The typed struct is `omuse_common::config::Config`; every field has the Python default.
**Never call `std::env::var` for a setting** — add a field to `Config` if one is missing
(keep the mapping table in `config.example.toml` up to date). The only environment variables
the binary reads: `OMUSE_CONFIG` (path), `DISPLAY` (X server, like the Python broker), `HOME`.
`OMUSE_VERSION` is a *build-time* value (`option_env!`), as the Dockerfile build arg.

Internal tokens (`[internal] runtime_token / browser_token`) are generated at start when blank
and shared in-process (`Config::internal_tokens()` fills them once, before the services start).

## Crate graph and module map (Python → Rust)

```
omuse-common        app/common/util.py → util.rs (VERSION, new_id, iso, now_ts, truncate, take_chars, token_ok, …)
                    app/common/util.py:DB → db.rs (Db: rusqlite, WAL, JSON rows, params![] macro)
                    app/common/stallwatch.py → stallwatch.rs      app/common/calc.py → calc.rs
                    app/sentinel/guard.py → guard.rs (shared with the browser broker)
                    (new) config.rs (config.toml), pyjson.rs (Python-style json.dumps), fernet.rs, api.rs (ApiError), http.rs
omuse-docs          app/common/{mdhtml,xlsx,docx_writer,dataq,charts}.py → same names
omuse-web           app/sentinel/assets.py → assets.rs; deploy/docker/gate.py → gate.rs (+ templates/login.html);
                    app/web/index.html → templates/index.html (Tera, asset URLs carry ?v={{ v }}); app/web/* embedded (include_dir)
omuse-sentinel-core app/sentinel/{store,vault,passwd,catalog,policy,ledger,suggest}.py → same names (no HTTP)
omuse-mail          app/sentinel/{gmail,mailboxes,mailproviders}.py
omuse-apps          app/sentinel/{notion,slack,gcal,ical,market,billing}.py
omuse-mcp           app/sentinel/{mcp_client,mcp_oauth,mcp_hub}.py
omuse-phone         app/sentinel/{phone,voice_app,dialmcp}.py (depends on omuse-mcp)
omuse-sentinel      app/sentinel/{actions,telegram_bot,watchers}.py → same names; app/sentinel/main.py → routes.rs (+ state.rs)
                    (`ActionError` lives in omuse-sentinel-core::errors so every connector crate can raise it)
omuse-runtime       app/runtime/{store,llm,prompts,context,agent,scheduler,goals,memory_tidy,library,skills,imagegen,
                    attachments,pdfforms,golden,health,metrics,outcome}.py → same names; app/runtime/main.py → routes.rs (+ state.rs)
                    app/skills/* embedded (include_dir), extracted to <runtime data>/skills-builtin at start unless [skills].dir is set
omuse-browser       app/browser/main.py → broker.rs (+ more files as needed) and routes.rs; app/browser/*.js via include_str!
omuse               the binary: config, roles, process lifecycle
```

Dependency direction: `omuse-common` ← `omuse-docs`, `omuse-web`, `omuse-sentinel-core` ← `omuse-sentinel`;
`omuse-common`, `omuse-docs` ← `omuse-runtime`; `omuse-common` ← `omuse-browser`; everything ← `omuse`.

## Shared contracts

* `omuse_sentinel_core::store::Store` mirrors the Python `Store` method for method (same names, `serde_json::Value`
  in and out where Python used dicts): `connection(name) -> Value`, `save_connection(name, config, permissions, enabled) -> Value`,
  `put_secret/get_secret/has_secret/delete_secret`, `kv_get(k) -> Option<Value>`, `kv_set`, `task_ctx`, `update_task_ctx`,
  `add_grant/active_grants/revoke_grant`, `create_approval/approval/approvals/resolve_approval`, `audit(actor, action, Audit{..}) -> Value`
  (`Audit` is a `Default` struct with `task_id, resource, risk, decision, result: String` and `detail: Option<Value>`),
  `audit_list`, `audit_verify`, purchases, user requests, `count_done`. Store is `Send + Sync` and used behind `Arc`.
* `omuse_common::guard`: `check_url(url) -> (bool, String)`, `host_is_private(host) -> bool`, `domain_of(url) -> String`,
  `redact_secrets(text) -> (String, usize)`, `scan_injection(text) -> Vec<String>` (or `bool`-like: empty = clean), `click_is_risky(role, name, kind) -> bool`, `max_class(a, b)`.
* `omuse_web::gate::Gate` takes `verify: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>` and
  `version: Arc<dyn Fn() -> i64 + Send + Sync>` (so it does not depend on the password module) plus the user name,
  the session key path and the session days; it is an axum middleware layer plus `/login` + `/logout` handlers.

## Conventions

* **Edition 2021, stable toolchain, `cargo clippy -- -D warnings` clean, `cargo fmt`.**
* Errors: `anyhow::Result` inside; typed errors (`thiserror`) where Python had exception classes
  the callers branch on (`ActionError`, `GmailError`, `NotionError`, `SlackError`, `GCalError`, `PasswordError`, …).
* HTTP errors must look like FastAPI's: body `{"detail": "<message>"}` with the same status.
  Use `omuse_common::api::ApiError` (`ApiError::new(404, "no such task")`, `ApiError::bad_request(..)`),
  which implements `IntoResponse`. Request bodies that FastAPI validated (typed models) returned
  422 on bad input — port with `ApiError::new(422, ...)` where a test could observe it; otherwise 400 as the Python code did.
* JSON: use `serde_json::Value` freely to mirror Python dicts (the `preserve_order` feature is on, so key
  order is insertion order like Python). Use `omuse_common::pyjson::dumps(&v)` / `dumps_indent(&v, 2)` wherever the
  Python called `json.dumps` — it reproduces Python's separators (`", "` / `": "`), `ensure_ascii=False`
  and float formatting, because tests and the scripted LLM match on substrings like `"status": "ended"`.
  Axum's `Json(...)` responses may use compact serde output (the clients parse them).
* Strings: Python slices by code point. Use `util::take_chars(s, n)` / `util::truncate(s, n)` instead of byte slicing;
  never index a `&str` by byte offsets derived from character counts. `str::to_lowercase`/`trim` match Python closely enough.
* Regex: `regex` crate; `fancy_regex` only when the Python pattern needs look-around or back-references.
  Translate flags: `re.I`→`(?i)`, `re.M`→`(?m)`, `re.S`→`(?s)`. Keep patterns in `LazyLock<Regex>` statics.
* Time: `util::now_ts()` (f64 seconds), `util::iso(ts)` (Python `datetime.isoformat()` form), `chrono` + `chrono_tz`
  for zone math; cron via `croner` (5-field, like croniter).
* Shared state: `Arc<State>` in axum; interior mutability with `parking_lot::Mutex/RwLock` for short critical
  sections that never await, `tokio::sync::Mutex` when held across `.await`. SQLite is behind `Db` (one connection,
  `parking_lot::Mutex`), called directly from handlers like the Python did.
* Background work: `tokio::spawn`; never block the runtime for long (CPU-heavy work → `tokio::task::spawn_blocking`).
* Outbound HTTP: `reqwest` (rustls). Build clients once per service in the state. Same timeouts as Python.
* Logging: keep the Python `print(f"[name] ...", flush=True)` lines as `println!` (or `tracing::info!` with the same
  text) — operators grep for them.
* Keep every user-facing / model-facing string **byte-identical** to the Python source (Chinese + English).
* Doc-comment public items briefly; port the Python module docstring as the module doc.
* Tests: port the relevant assertions of `tests/test_units.py` / `tests/test_027.py` for your module as Rust
  `#[cfg(test)]` unit tests (they are the executable spec), at least for pure functions.
* Do not edit files outside your assignment. If you need something from another module that does not exist yet,
  write the smallest `pub fn` stub in **your** module and say so in your report, or ask.
* Cargo: all crates use `[workspace.dependencies]`; add a dependency at the workspace root only if truly needed,
  pure Rust only (no system libraries; `rustls`, `bundled` SQLite). External *programs* (chromium, Xvfb, ffmpeg,
  heif-convert) are allowed and must be listed in the Dockerfiles.

## Data layout (`[data] dir`, default `/omuse`)

```
<dir>/sentinel/        sentinel.db  audit.jsonl  vault.key  auth.json  session.key  shots/  loop_stalls.log
<dir>/runtime/         runtime.db  skills-builtin/  loop_stalls.log
<dir>/workspace/       the agent's files (uploads/, reports/, charts/, media/, library/, …)
<dir>/browser-profile/ Chromium user data dir
```
