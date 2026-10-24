# OMuse Rust port: Python vs Rust report

Date: 2026-10-24. Python reference: commit 562a36f (v0.3.6). Rust: the Cargo workspace in `crates/` (one binary,
`omuse`, Axum + Tokio; settings from `config.toml`).

## Verdict

The Rust port passes every e2e suite the Python version passes, its UI renders identically, and it does the same
work with about 9 times less CPU and 6 times less memory. End-to-end task time improves 15–25% on browser tasks, where
most of the time is Chromium and the waits the app deliberately keeps, and up to 4.7 times on tasks without a browser.

## Correctness

| Check | Python | Rust |
|---|---|---|
| E2E suites (`tests/run_in_docker.sh`, unchanged tests, scripted model) | 54 / 54 pass | 54 / 54 pass |
| Travel suites with real models (planner xai/grok-4.7, executor openai/gpt-6-luna) | 3 / 3 pass | 3 / 3 pass |
| Image smoke test (config, login gate, Chromium, volume survives replacement) | pass | pass |
| UI screenshots, 18 views, scripted model | reference | max 0.12% pixels differ (task ids and clock times only) |
| Rust unit tests (`cargo test --workspace`) | — | 391 pass |
| Travel suites, Rust only, after the Python code was removed (planner openai-codex/gpt-6.1-sol, executor openai-codex/gpt-6-luna) | — | 3 / 3 pass, all three outcomes "verified" |
| Olares chart (`tests/olares_chart_smoke.sh`: Helm render, three role containers from the release image) | — | pass |

**Correction (cleanup pass).** In the first comparison `mail_generic_e2e.py` only exercised the Python mail client
in-process (`tests/run_in_docker.sh` never passed its `--api` flag), so the Rust column counted a suite that did not
reach Rust. The suite now connects the mailbox through Sentinel's API and runs every mail operation as the agent's tool
call; it passes 51 / 51 checks against Rust, Outlook (device-code sign-in, XOAUTH2 for IMAP and SMTP) included.

The 54 suites are the 51 original suites plus three new travel suites (flight, hotel, Michelin restaurant booking
in Asian cities) backed by three new skills that use a travel data API (an MCP connector) first and open the browser
only for the final booking page.

Screenshots: `ui-python/` vs `ui-rust/` (scripted model), `real-python/` vs `real-rust/` (real models), and
`real-rust-codex/` (Rust after the cleanup, openai-codex models). The UI shots of the cleaned-up build still differ from
`ui-python/` by at most 0.12% of pixels.

Final run after the Python code was removed (2026-10-24): 54 / 54 suites (904 checks), both images' smoke tests,
the Olares chart test and the three travel suites with openai-codex/gpt-6.1-sol (planner) and openai-codex/gpt-6-luna
(executor) all pass. Execution time without model calls in that run: flight 8.05 s, hotel 4.58 s, Michelin 7.84 s, in
line with the Rust numbers below.

## Performance

All runs used the same Docker image (`omuse-test`), the same machine, one run at a time. Model time is excluded:
the scripted model answers instantly, and real-model runs go through a timing proxy whose call intervals (and any
Retry-After waits) are subtracted from the wall time.

### Execution time excluding model calls

| Use case | Python | Rust | Rust faster by |
|---|---|---|---|
| Flight booking, real models | 11.04 s | 8.72 s | 21% |
| Hotel booking, real models | 6.80 s | 5.03 s | 26% |
| Michelin booking, real models | 10.23 s | 8.03 s | 22% |
| Flight booking, scripted model | 9.82 s | 7.86 s | 20% |
| Hotel booking, scripted model | 5.74 s | 4.45 s | 22% |
| Michelin booking, scripted model | 9.64 s | 7.80 s | 19% |
| 10 chat tasks, no browser | 2.24 s | 0.48 s | 4.7× |

### CPU used by the app itself (10 suites, median of 3 rounds, startup excluded)

| | Python | Rust |
|---|---|---|
| App processes (services + Playwright driver) | 27.8 s | 3.0 s |
| Chromium | 19.3 s | 25.2 s |

The app's own CPU drops about 9 times. Chromium uses about 30% more CPU under Rust. It runs headed on the
virtual display in both, with Playwright-equivalent flags.

### Memory, startup, HTTP API

| | Python | Rust |
|---|---|---|
| App memory, idle | 173 MiB | 27 MiB |
| App memory, average over all 54 suites | 172 MiB | 38 MiB |
| Startup until all three services answer | 1.71 s | 0.35 s |
| Production image | 3.93 GB | 1.64 GB |

HTTP throughput, 16 concurrent clients, 4 CPUs:

| Endpoint | Python req/s | Rust req/s |
|---|---|---|
| /api/tasks | 634 | 1257 |
| /api/settings | 596 | 1082 |
| /sentinel/api/approvals | 889 | 1219 |
| /sentinel/api/browser/state | 65 | 1240 |
| / (page shell) | 937 | 927 |
| /static/app.js (gzip) | 560 | 570 |

### Where the time goes

Most of a browser task's wall time is fixed by design and the same in both: Chromium page loads, the 1.3 s pause
between requests to one site, network-idle waits and approval round trips. The full suite run shows this: 804 s of
suite time on Python vs 757 s on Rust. The work the app does itself is what got cheaper.

## Differences found and how they were handled

- **Python bug not carried over.** When one model turn holds several actions that each need approval, Python drops
  the calls after the second approval (`app/runtime/agent.py`, resume path). The Rust port passes the remaining calls
  on. The new skills ask for one vault fill per turn, so both versions behave the same in the travel suites.
- **Cancel during a Retry-After wait** stops the task at once in Rust (Python kept sleeping until the wait ended).
  The retry notice, budgets and the 10-try / 300 s Retry-After rules are unchanged.
- **False "Not confirmed" on real bookings, fixed in Rust.** The outcome check looks for the booking reference next
  to its label. When a model bolds both (`**Booking reference:** **TRV-FBB71D4**`) Python's pattern finds nothing and
  appends "⚠️ Not confirmed" to a booking that went through; openai-codex/gpt-6-luna writes it that way. The Rust
  pattern accepts bold on either side (unit test `refs_with_markdown_bold_on_label_and_number`).
- **Role-split start.** `omuse --role sentinel` created the runtime's data directory too, which a pod container that
  mounts only its own volume cannot do; each role now creates only its own directories.
- **Config.** Every environment variable is now a `config.toml` key; `config.example.toml` lists each one with the
  variable it replaces. `[auth] gate = false` runs without the login page, as the Python test stack did.
