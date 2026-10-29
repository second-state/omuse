#!/bin/bash
# Run the local e2e suites (integration.py, *_e2e.py, *_ui.py, ...) inside a container of the test image
# (Dockerfile.test: Playwright's Python image + the `omuse` binary), so they need nothing on the host but Docker.
# Each suite gets a freshly started stack (tests/run_local.sh: fake LLM + all services), because several suites
# expect a fresh install.
#   bash tests/run_in_docker.sh                 # every suite
#   bash tests/run_in_docker.sh e2e_027 mcp_e2e # only these
# OMUSE_IMAGE picks the image (default: omuse-test, built from Dockerfile.test when missing). Exit code 1 if a suite fails.
# docker_browser_e2e.py is not part of this: it needs a real model (see its docstring).
set -u
if [ "${1:-}" != "--inside" ]; then
  cd "$(dirname "$0")/.."
  IMAGE="${OMUSE_IMAGE:-omuse-test}"
  docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -f Dockerfile.test -t "$IMAGE" . || exit 1
  # root: Dovecot. shop.test / opentable.test: the hostnames the test pages are served under.
  # OMUSE_E2E_ENV_FILE: real-model runs of the travel suites (endpoint, key and model names; the key stays in that file).
  # OMUSE_E2E_OUT: a writable host directory, mounted at /repo-out (suite logs, TIMING lines, screenshots_real.py pictures).
  EXTRA=()
  # OMUSE_E2E_{BRAVE,TAVILY,SERPAPI}_EP: the real search endpoints instead of the fakes (web_search_real.py).
  [ -n "${OMUSE_E2E_ENV_FILE:-}" ] && EXTRA+=(--env-file "$OMUSE_E2E_ENV_FILE" -e OMUSE_E2E_REAL=1 -e OMUSE_E2E_MODELS -e OMUSE_E2E_BRAVE_EP -e OMUSE_E2E_TAVILY_EP -e OMUSE_E2E_EXA_EP -e OMUSE_E2E_EXA_MCP_EP -e OMUSE_E2E_KIWI_MCP_EP -e OMUSE_E2E_SERPAPI_EP ${OMUSE_E2E_SEARCH_ENGINES+-e OMUSE_E2E_SEARCH_ENGINES} ${SEARCH_TIMING_BURST:+-e SEARCH_TIMING_BURST})
  [ -n "${OMUSE_E2E_OUT:-}" ] && { mkdir -p "$OMUSE_E2E_OUT"; EXTRA+=(-v "$(cd "$OMUSE_E2E_OUT" && pwd)":/repo-out); }
  exec docker run --rm --user 0 ${EXTRA[@]+"${EXTRA[@]}"} ${OMUSE_TEST_NAME:+--name "$OMUSE_TEST_NAME"} \
    --add-host shop.test:127.0.0.1 --add-host opentable.test:127.0.0.1 \
    -v "$PWD":/repo:ro --entrypoint bash "$IMAGE" /repo/tests/run_in_docker.sh --inside "$@"
fi
shift

# a writable copy of the tests (suites write next to themselves); the repo stays mounted read-only
mkdir -p /src && cp -r /repo/tests /repo/web /src/ && cd /src
# test-only services baked into the image: IMAP server for the mail suites
mkdir -p /tmp/claude-0 /srv/dove/run /srv/dove/mail
cp tests/dovecot.conf /srv/dove/; cp tests/dovecot-oauth2.conf /srv/dove/oauth2.conf
echo "tester@example.com:{PLAIN}secretpass123" > /srv/dove/users; chown -R nobody:nogroup /srv/dove/mail
dovecot -c /srv/dove/dovecot.conf || echo "WARNING: dovecot did not start"
export PYTHONPATH=/src

if [ $# -gt 0 ]; then
  SUITES="$*"
else
  SUITES="integration $(ls tests | grep -E '(_e2e|_ui)\.py$|^e2e_0|^browser_parallel|^popup_takeover|^takeover_burst|^i18n_check' | sed 's/\.py$//' | grep -v docker_browser_e2e)"
fi
failed=""
for f in $SUITES; do
  f="${f%.py}"
  bash tests/stop_local.sh >/dev/null 2>&1; pkill -f "uvicorn|http.server|chrome|omuse" >/dev/null 2>&1; sleep 1
  bash tests/run_local.sh > /tmp/up.log 2>&1 < /dev/null
  for i in $(seq 1 20); do
    curl -s -o /dev/null http://127.0.0.1:8082/ && curl -sf -o /dev/null http://127.0.0.1:8080/sentinel/api/health && break
    sleep 1
  done
  # a fresh install nudges the user to complete the profile (the first chat view opens Memory → Profile); the suites
  # start on the chat, so the reminder is off for them — profile_onboarding_ui.py switches it on itself
  curl -s -o /dev/null -X PUT -H 'X-Persona-UI: 1' -H 'Content-Type: application/json' -d '{"profile_reminder": false}' http://127.0.0.1:8080/api/settings
  t0=$(date +%s)
  rm -f /tmp/claude-0/travel_last.json
  # real-model runs take longer
  timeout "${OMUSE_SUITE_TIMEOUT:-$([ -n "${OMUSE_E2E_REAL:-}" ] && echo 1800 || echo 420)}" python3 "tests/$f.py" > "/tmp/out-$f.log" 2>&1; rc=$?
  # every suite: no search ran with a price cap ≤ 0 or fewer than 1 traveller (tests/args_guard.py)
  if ! python3 tests/args_guard.py > "/tmp/guard-$f.log" 2>&1; then
    rc=1
    sed 's/^GUARD /FAIL args guard: /' "/tmp/guard-$f.log" | grep '^FAIL' >> "/tmp/out-$f.log"
  fi
  echo "SUITE $f rc=$rc $(( $(date +%s) - t0 ))s pass=$(grep -c '^PASS' "/tmp/out-$f.log") fail=$(grep -c '^FAIL' "/tmp/out-$f.log")"
  grep '^TIMING ' "/tmp/out-$f.log" | sed 's/^/  /'
  if [ -d /repo-out ]; then
    cp "/tmp/out-$f.log" /repo-out/
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
[ -z "$failed" ] && echo "ALL SUITES PASS" || { echo "FAILED SUITES:$failed"; exit 1; }
