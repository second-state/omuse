#!/bin/bash
# Run the scripted suites (integration.py, *_e2e.py, *_ui.py, ...) and the e2e suites (*_real.py) inside a container of the test image
# (Dockerfile.test: Playwright's Python image + the `omuse` binary), so they need nothing on the host but Docker.
# Each suite gets a freshly started stack (tests/run_local.sh: the scripted model + the app; no fake services, no stand-in
# pages), because several suites expect a fresh install.
#   bash tests/run_in_docker.sh                 # every suite
#   bash tests/run_in_docker.sh e2e_027 stuck_e2e # only these
#   bash tests/run_in_docker.sh real            # the real-model suites (REAL_SUITES below; billed), with the env file
#                                                 OMUSE_E2E_ENV_FILE (default ~/.config/omuse/e2e.env)
#   bash tests/run_in_docker.sh realdata        # only the e2e suites that run on the real services now (REAL_DATA_SUITES)
#   bash tests/run_in_docker.sh realsites       # the bookings and orders on the real booking sites and shops (REAL_SITES),
#                                                 always one at a time (-j 1)
# OMUSE_IMAGE picks the image (default: omuse-test, built from Dockerfile.test when missing). Exit code 1 if a suite fails.
# docker_browser_e2e.py is not part of this: it needs a real model (see its docstring).
set -u
# the e2e suites (real model, real sites and services only — no fake servers, stand-in pages or fixture data); each needs
# OMUSE_E2E_REAL (OMUSE_E2E_ENV_FILE)
# REAL_DATA_SUITES: read-only use cases on the real web and the real APIs (search, flights/hotels/cars, market data, …)
REAL_DATA_SUITES=" travel_user_real travel_kiwi_real travel_brave_real travel_connect_real web_search_real travel_info_real \
emergency_real money_real market_real research_real library_real cards_real cards_markets_real news_report_real \
travel_ask_real stream_real bots_real btc_monitor_real shopping_real travel_events_real "
# the other e2e suites: the app with the real model and the real image / video provider, nothing outside faked
REAL_APP_SUITES="assistant_real automations_real memory_real chat_real docs_real chart_real media_real attach_media_real \
travel_receipts_real cards_claim_real"
# REAL_SITES: bookings and orders on the real booking sites and shops — code searches and picks, opens the real booking
# page and hands it to browser-use, which fills it with the profile's fake data and hands over at the first step that is
# the user's (sign-in, CAPTCHA, Pay); no card in the Vault, nothing paid. Run one at a time (`realsites` forces -j 1):
# a real site sees one session at a time, never a burst.
# The insurance, claims, baggage and departure-tax suites too: FWD Singapore's quote and claim, FWD Hong Kong and
# Thailand's quotes, bolttech's PDF, WorldTracer's mybag.aero and the APDT platform, each stopped at the first user step.
REAL_SITES="travel_book_real insurance_real insurance_hk_real insurance_th_real claims_real apdt_real"
# On demand only (the user, 2026-11-04): the arrival cards' walkthroughs on the official government sites — code fills
# each card's form and hands over at the first user step (Cloudflare check, sign-in, passport photo, emailed code,
# Submit); the Taiwan and UK ETA suites read the emailed code from the test inbox (OMUSE_E2E_IMAP_*). They are in no
# list, so no gate runs them; run one by name, e.g.
#   OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh arrival_cards2_real
# REAL_SITES="$REAL_SITES arrival_real arrival_cards_real arrival_cards2_real arrival_cards3_real arrival_cards4_real \
#   arrival_address_real arrival_address2_real"
REAL_SUITES="$REAL_DATA_SUITES $REAL_APP_SUITES"

# bash reads a script as it runs: a copy keeps a run going while the file is edited (another suite added to the lists)
if [ -z "${OMUSE_RUNNER_COPY:-}" ] && [ "${1:-}" != "--inside" ]; then
  cp "$0" "${TMPDIR:-/tmp}/omuse_run_in_docker.$$.sh"
  OMUSE_RUNNER_COPY="$(cd "$(dirname "$0")/.." && pwd)" exec bash "${TMPDIR:-/tmp}/omuse_run_in_docker.$$.sh" "$@"
fi
if [ "${1:-}" = "-j" ]; then
  # -j N: the suites spread over N containers at once (each has its own stack and browser), then one
  # summary; e.g. `bash tests/run_in_docker.sh -j 4 real`
  JOBS="$2"; shift 2
  cd "$OMUSE_RUNNER_COPY"
  if [ "$*" = real ] || [ "$*" = realdata ] || [ "$*" = realsites ]; then
    export OMUSE_E2E_ENV_FILE="${OMUSE_E2E_ENV_FILE:-$HOME/.config/omuse/e2e.env}"
    case "$*" in real) ALL="$REAL_SUITES" ;; realdata) ALL="$REAL_DATA_SUITES" ;; *) ALL="$REAL_SITES"; JOBS=1 ;; esac
  elif [ $# -gt 0 ]; then
    ALL="$*"
  else
    ALL="integration $(ls tests | grep -E '(_e2e|_ui)\.py$|^e2e_0|^takeover_burst|^i18n_check' | sed 's/\.py$//' | grep -v docker_browser_e2e)"
  fi
  # one build for all the groups
  if [ -z "${OMUSE_IMAGE:-}" ]; then docker build -q -f Dockerfile.test -t omuse-test . >/dev/null || exit 1; export OMUSE_IMAGE=omuse-test; fi
  TMPD=$(mktemp -d)
  k=0
  for f in $ALL; do echo "$f" >> "$TMPD/group$((k % JOBS))"; k=$((k + 1)); done
  for g in "$TMPD"/group*; do
    bash "$0" $(tr '\n' ' ' < "$g") > "$g.out" 2>&1 &
  done
  wait
  failed=""
  for g in "$TMPD"/group*.out; do
    # byte-safe (a suite's output may hold any bytes): macOS sed / grep stop at an illegal byte sequence otherwise
    LC_ALL=C grep -a -v -E '^(ALL SUITES PASS$|FAILED SUITES:)' "$g"
    failed="$failed $(LC_ALL=C sed -n 's/^FAILED SUITES://p' "$g")"
  done
  rm -rf "$TMPD"
  failed=$(echo $failed)
  [ -z "$failed" ] && echo "ALL SUITES PASS" || { echo "FAILED SUITES: $failed"; exit 1; }
  exit 0
fi
if [ "${1:-}" != "--inside" ]; then
  cd "$OMUSE_RUNNER_COPY"
  if { [ "${1:-}" = real ] || [ "${1:-}" = realdata ] || [ "${1:-}" = realsites ]; } && [ -z "${OMUSE_E2E_ENV_FILE:-}" ]; then
    OMUSE_E2E_ENV_FILE="$HOME/.config/omuse/e2e.env"
    [ -f "$OMUSE_E2E_ENV_FILE" ] || { echo "real: needs OMUSE_E2E_ENV_FILE (the model endpoint, key and names)"; exit 1; }
  fi
  IMAGE="${OMUSE_IMAGE:-omuse-test}"
  # built from the working tree on every run (cached: seconds when nothing changed); OMUSE_IMAGE names a ready image
  if [ -z "${OMUSE_IMAGE:-}" ]; then docker build -q -f Dockerfile.test -t "$IMAGE" . >/dev/null || exit 1; fi
  # OMUSE_E2E_ENV_FILE: real-model runs (endpoint, key and model names; the key stays in that file).
  # OMUSE_E2E_EXECUTOR / OMUSE_E2E_PLANNER set on the command line override that file's model names.
  # OMUSE_E2E_OUT: a writable host directory, mounted at /repo-out (suite logs, TIMING lines, screenshots_real.py pictures).
  EXTRA=()
  [ -n "${OMUSE_E2E_ENV_FILE:-}" ] && EXTRA+=(--env-file "$OMUSE_E2E_ENV_FILE" -e OMUSE_E2E_REAL=1 -e OMUSE_E2E_MODELS ${OMUSE_E2E_EXECUTOR:+-e OMUSE_E2E_EXECUTOR} ${OMUSE_E2E_PLANNER:+-e OMUSE_E2E_PLANNER} ${OMUSE_E2E_SEARCH_ENGINES+-e OMUSE_E2E_SEARCH_ENGINES} ${OMUSE_E2E_KEEP:+-e OMUSE_E2E_KEEP} ${OMUSE_E2E_APDT_DOCS:+-e OMUSE_E2E_APDT_DOCS} ${OMUSE_E2E_APDT_DOC_NO:+-e OMUSE_E2E_APDT_DOC_NO} ${SEARCH_TIMING_BURST:+-e SEARCH_TIMING_BURST})
  # OMUSE_E2E_SKILLS_DIR=1: the skills come from the working tree (mounted at /repo) instead of the binary, so a skill
  # edit is tested without rebuilding the image (bots stay compiled in)
  [ -n "${OMUSE_E2E_SKILLS_DIR:-}" ] && EXTRA+=(-e OMUSE_E2E_SKILLS_DIR=/repo/skills)
  [ -n "${OMUSE_E2E_OUT:-}" ] && { mkdir -p "$OMUSE_E2E_OUT"; EXTRA+=(-v "$(cd "$OMUSE_E2E_OUT" && pwd)":/repo-out); }
  # OMUSE_E2E_ONLY=kr: a suite that runs several cases runs only these (comma-separated keys), e.g. one arrival card
  [ -n "${OMUSE_E2E_ONLY:-}" ] && EXTRA+=(-e "OMUSE_E2E_ONLY=$OMUSE_E2E_ONLY")
  exec docker run --rm --user 0 ${EXTRA[@]+"${EXTRA[@]}"} ${OMUSE_TEST_NAME:+--name "$OMUSE_TEST_NAME"} \
    -v "$PWD":/repo:ro --entrypoint bash "$IMAGE" \
    -c 'cp /repo/tests/run_in_docker.sh /tmp/run_in_docker.sh && exec bash /tmp/run_in_docker.sh --inside "$@"' _ "$@"
fi
shift

# a writable copy of the tests (suites write next to themselves); the repo stays mounted read-only
# (retried: Docker's file sharing can report a file "replaced while being copied", and a failed copy ran the suite from /)
mkdir -p /src && for i in 1 2 3 4 5; do cp -r /repo/tests /repo/web /src/ && break; sleep 1; done; cd /src || exit 2
mkdir -p /tmp/claude-0
export PYTHONPATH=/src
# the suites on the app's timezone (tests/run_local.sh: Asia/Singapore): they say the same "today" as the app
export TZ=Asia/Singapore

if [ "$*" = real ] || [ "$*" = realdata ] || [ "$*" = realsites ]; then
  [ -n "${OMUSE_E2E_REAL:-}" ] || { echo "real: OMUSE_E2E_REAL is not set (pass OMUSE_E2E_ENV_FILE)"; exit 1; }
  case "$*" in real) SUITES="$REAL_SUITES" ;; realdata) SUITES="$REAL_DATA_SUITES" ;; *) SUITES="$REAL_SITES" ;; esac
elif [ $# -gt 0 ]; then
  SUITES="$*"
else
  SUITES="integration $(ls tests | grep -E '(_e2e|_ui)\.py$|^e2e_0|^takeover_burst|^i18n_check' | sed 's/\.py$//' | grep -v docker_browser_e2e)"
fi
failed=""
for f in $SUITES; do
  f="${f%.py}"
  bash tests/stop_local.sh >/dev/null 2>&1; pkill -f "uvicorn|chrome|omuse" >/dev/null 2>&1; sleep 1
  REALDATA=()
  # web_search_real reaches the search APIs: the browser's own engines are switched off for it (nothing listens there)
  [ -n "${OMUSE_E2E_REAL:-}" ] && [ "$f" = web_search_real ] && REALDATA=("OMUSE_E2E_SEARCH_ENGINES=off=http://127.0.0.1:9/?q=")
  # a suite's own settings for the stack: "# run_local: NAME=value …" lines in its file
  SUITEENV=($(grep -h '^# run_local: ' "tests/$f.py" 2>/dev/null | sed 's/^# run_local: //'))
  env ${REALDATA[@]+"${REALDATA[@]}"} ${SUITEENV[@]+"${SUITEENV[@]}"} bash tests/run_local.sh > /tmp/up.log 2>&1 < /dev/null
  # up to a minute: with several suites starting at once (-j) a stack can take longer than 20 s
  # every service of the stack answers (any HTTP status): the scripted model, the browser and Sentinel
  for i in $(seq 1 60); do
    up=1
    curl -s -o /dev/null "http://127.0.0.1:8090/" || up=0
    [ $up = 1 ] && curl -s -o /dev/null http://127.0.0.1:8082/ && curl -sf -o /dev/null http://127.0.0.1:8080/sentinel/api/health && break
    sleep 1
  done
  # a fresh install nudges the user to complete the profile (the first chat view opens Memory → Profile); the suites
  # start on the chat, so the reminder is off for them — profile_onboarding_ui.py switches it on itself
  curl -s -o /dev/null -X PUT -H 'X-Persona-UI: 1' -H 'Content-Type: application/json' -d '{"profile_reminder": false}' http://127.0.0.1:8080/api/settings
  t0=$(date +%s)
  rm -f /tmp/claude-0/travel_last.json
  # real-model runs take longer
  timeout "${OMUSE_SUITE_TIMEOUT:-$([ -n "${OMUSE_E2E_REAL:-}" ] && echo 1800 || echo 420)}" env ${REALDATA[@]+"${REALDATA[@]}"} python3 "tests/$f.py" > "/tmp/out-$f.log" 2>&1; rc=$?
  # every suite: no tool call ran with a price cap ≤ 0 or fewer than 1 traveller, no skill was refused as another bot's
  # (tests/args_guard.py)
  # (scripted-model suites only: a real-model suite checks only what the user sees, tests/user_common.py)
  if [ -z "${OMUSE_E2E_REAL:-}" ] && ! python3 tests/args_guard.py > "/tmp/guard-$f.log" 2>&1; then
    rc=1
    sed 's/^GUARD /FAIL args guard: /' "/tmp/guard-$f.log" | grep '^FAIL' >> "/tmp/out-$f.log"
  fi
  echo "SUITE $f rc=$rc $(( $(date +%s) - t0 ))s pass=$(grep -c '^PASS' "/tmp/out-$f.log") fail=$(grep -c '^FAIL' "/tmp/out-$f.log")"
  grep '^TIMING ' "/tmp/out-$f.log" | sed 's/^/  /'
  if [ -d /repo-out ]; then
    cp "/tmp/out-$f.log" /repo-out/
    # every model call of the suite (model, purpose, start / end), from the timing proxy of a real-model run
    [ -n "${OMUSE_E2E_REAL:-}" ] && curl -s -m 5 http://127.0.0.1:8098/_intervals > "/repo-out/models-$f.json" 2>/dev/null
    if [ -n "${OMUSE_E2E_REAL:-}" ] && [ -f /tmp/claude-0/travel_last.json ]; then
      # the task record (events, outcome) for reading the run afterwards
      for tid in $(jq -r '.tasks[]? | if type == "object" then .id else . end' /tmp/claude-0/travel_last.json); do
        curl -s -H 'X-Persona-UI: 1' "http://127.0.0.1:8080/api/tasks/$tid" > "/repo-out/task-$f-$tid.json"
      done
      timeout 180 python3 tests/screenshots_real.py /repo-out/shots > "/tmp/shots-$f.log" 2>&1 || echo "  screenshots failed: $(tail -1 /tmp/shots-$f.log)"
    fi
  fi
  if [ $rc -ne 0 ]; then
    failed="$failed $f"
    echo "  stack: $(tr '\n' ' ' < /tmp/up.log)"
    grep -E '^FAIL|Error|Traceback' "/tmp/out-$f.log" | head -8 | cut -c1-300 | sed 's/^/  /'
    tail -2 "/tmp/out-$f.log" | cut -c1-300 | sed 's/^/  /'
  fi
done
# OMUSE_E2E_KEEP=<seconds>: the last suite's stack stays up that long afterwards, to read its tasks and events
[ -n "${OMUSE_E2E_KEEP:-}" ] && { echo "KEEPING the stack for ${OMUSE_E2E_KEEP}s"; sleep "$OMUSE_E2E_KEEP"; }
[ -z "$failed" ] && echo "ALL SUITES PASS" || { echo "FAILED SUITES:$failed"; exit 1; }
