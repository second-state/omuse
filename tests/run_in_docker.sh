#!/bin/bash
# Run the local e2e suites (integration.py, *_e2e.py, *_ui.py, ...) inside a container of the test image
# (Dockerfile.test: Playwright's Python image + the `omuse` binary), so they need nothing on the host but Docker.
# Each suite gets a freshly started stack (tests/run_local.sh: fake LLM + all services), because several suites
# expect a fresh install.
#   bash tests/run_in_docker.sh                 # every suite
#   bash tests/run_in_docker.sh e2e_027 mcp_e2e # only these
#   bash tests/run_in_docker.sh real            # the real-model suites (REAL_SUITES below; billed), with the env file
#                                                 OMUSE_E2E_ENV_FILE (default ~/.config/omuse/e2e.env)
# OMUSE_IMAGE picks the image (default: omuse-test, built from Dockerfile.test when missing). Exit code 1 if a suite fails.
# docker_browser_e2e.py is not part of this: it needs a real model (see its docstring).
set -u
# the real-model suites: plain requests to Main (tests/real_common.py) and the older real ones; each needs OMUSE_E2E_REAL
REAL_SUITES="bots_real arrival_real arrival_cards_real arrival_cards2_real arrival_cards3_real arrival_cards4_real arrival_address_real arrival_address2_real insurance_real cards_real photos_real mail_real calendar_real research_real docs_real \
travel_ask_real travel_flight_real travel_hotel_real travel_michelin_real city_guides_real browser_auto_real stream_real travel_nomail_real \
travel_user_real travel_kiwi_real travel_brave_real travel_connect_real media_real web_search_real \
insurance_hk_real claims_real emergency_real refunds_real tax_refund_real \
cards_markets_real cards_pay_real cards_claim_real cards_interruption_real apdt_real \
trip_hotel_real travel_info_real travel_receipts_real city_guides2_real \
shopping_real watch_real support_chat_real pdf_forms_real brief_real gcal_real assistant_real \
drive_storybook_real drive_video_real drive_cleanup_real attach_media_real chart_real market_real \
apps_real automations_real telegram_real mcp_real phone_real memory_real chat_real library_real"
# the real-data suites use the real services the way a user's install does — SerpApi, Kiwi.com, Kayak, Rentalcars.com
# and Exa's keyless web search — instead of the stand-ins (the user enters their own SerpApi key in the UI)
# read-only use cases run on the real web; the ones that would submit, pay or book something stay on stand-ins
REAL_DATA_SUITES=" travel_user_real travel_kiwi_real travel_brave_real travel_connect_real web_search_real travel_info_real emergency_real tax_refund_real market_real research_real library_real cards_real cards_markets_real "

# bash reads a script as it runs: a copy keeps a run going while the file is edited (another suite added to the lists)
if [ -z "${OMUSE_RUNNER_COPY:-}" ] && [ "${1:-}" != "--inside" ]; then
  cp "$0" "${TMPDIR:-/tmp}/omuse_run_in_docker.$$.sh"
  OMUSE_RUNNER_COPY="$(cd "$(dirname "$0")/.." && pwd)" exec bash "${TMPDIR:-/tmp}/omuse_run_in_docker.$$.sh" "$@"
fi
if [ "${1:-}" = "-j" ]; then
  # -j N: the suites spread over N containers at once (each has its own stack, browser, fakes and stand-ins), then one
  # summary; e.g. `bash tests/run_in_docker.sh -j 4 real`
  JOBS="$2"; shift 2
  cd "$OMUSE_RUNNER_COPY"
  if [ "$*" = real ]; then
    export OMUSE_E2E_ENV_FILE="${OMUSE_E2E_ENV_FILE:-$HOME/.config/omuse/e2e.env}"
    ALL="$REAL_SUITES"
  elif [ $# -gt 0 ]; then
    ALL="$*"
  else
    ALL="integration $(ls tests | grep -E '(_e2e|_ui)\.py$|^e2e_0|^browser_parallel|^popup_takeover|^takeover_burst|^i18n_check' | sed 's/\.py$//' | grep -v docker_browser_e2e)"
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
  if [ "${1:-}" = real ] && [ -z "${OMUSE_E2E_ENV_FILE:-}" ]; then
    OMUSE_E2E_ENV_FILE="$HOME/.config/omuse/e2e.env"
    [ -f "$OMUSE_E2E_ENV_FILE" ] || { echo "real: needs OMUSE_E2E_ENV_FILE (the model endpoint, key and names)"; exit 1; }
  fi
  IMAGE="${OMUSE_IMAGE:-omuse-test}"
  # built from the working tree on every run (cached: seconds when nothing changed); OMUSE_IMAGE names a ready image
  if [ -z "${OMUSE_IMAGE:-}" ]; then docker build -q -f Dockerfile.test -t "$IMAGE" . >/dev/null || exit 1; fi
  # root: Dovecot. shop.test / opentable.test: the hostnames the test pages are served under. eservices.ica.gov.sg and
  # www.fwd.com.sg: the official sites' stand-ins (tests/https_standins.py), so a real model sees the real site names.
  # OMUSE_E2E_ENV_FILE: real-model runs of the travel suites (endpoint, key and model names; the key stays in that file).
  # OMUSE_E2E_EXECUTOR / OMUSE_E2E_PLANNER set on the command line override that file's model names.
  # OMUSE_E2E_OUT: a writable host directory, mounted at /repo-out (suite logs, TIMING lines, screenshots_real.py pictures).
  # the official sites served by tests/https_standins.py (its ROUTES): they resolve to the container itself
  STANDIN_HOSTS="eservices.ica.gov.sg www.fwd.com.sg tdac.immigration.go.th imigresen-online.imi.gov.my www.e-arrivalcard.go.kr
    vjw-lp.digital.go.jp s.nia.gov.cn niaspeedy.immigration.gov.tw twac.immigration.gov.tw allindonesia.imigrasi.go.id etravel.gov.ph arrival.gov.kh
    www.gov.uk app.apdtrefund.hk www.fwd.com.hk www.singaporeair.com wtrweb.worldtracer.aero japan.embassy.gov.au
    www.mlit.go.jp www.eclaimsline.com www.thaievisa.go.th www.ica.gov.sg www.jreast.co.jp"
  EXTRA=()
  # OMUSE_E2E_{BRAVE,TAVILY,SERPAPI}_EP: the real search endpoints instead of the fakes (web_search_real.py).
  [ -n "${OMUSE_E2E_ENV_FILE:-}" ] && EXTRA+=(--env-file "$OMUSE_E2E_ENV_FILE" -e OMUSE_E2E_REAL=1 -e OMUSE_E2E_MODELS ${OMUSE_E2E_EXECUTOR:+-e OMUSE_E2E_EXECUTOR} ${OMUSE_E2E_PLANNER:+-e OMUSE_E2E_PLANNER} -e OMUSE_E2E_BRAVE_EP -e OMUSE_E2E_TAVILY_EP -e OMUSE_E2E_EXA_EP -e OMUSE_E2E_EXA_MCP_EP -e OMUSE_E2E_KIWI_MCP_EP -e OMUSE_E2E_SERPAPI_EP ${OMUSE_E2E_SEARCH_ENGINES+-e OMUSE_E2E_SEARCH_ENGINES} ${OMUSE_E2E_KEEP:+-e OMUSE_E2E_KEEP} ${SEARCH_TIMING_BURST:+-e SEARCH_TIMING_BURST})
  # OMUSE_E2E_SKILLS_DIR=1: the skills come from the working tree (mounted at /repo) instead of the binary, so a skill
  # edit is tested without rebuilding the image (bots stay compiled in)
  [ -n "${OMUSE_E2E_SKILLS_DIR:-}" ] && EXTRA+=(-e OMUSE_E2E_SKILLS_DIR=/repo/skills)
  [ -n "${OMUSE_E2E_OUT:-}" ] && { mkdir -p "$OMUSE_E2E_OUT"; EXTRA+=(-v "$(cd "$OMUSE_E2E_OUT" && pwd)":/repo-out); }
  exec docker run --rm --user 0 ${EXTRA[@]+"${EXTRA[@]}"} ${OMUSE_TEST_NAME:+--name "$OMUSE_TEST_NAME"} \
    --add-host shop.test:127.0.0.1 --add-host opentable.test:127.0.0.1 \
    $(for h in $STANDIN_HOSTS; do printf -- '--add-host %s:127.0.0.1 ' "$h"; done) \
    -v "$PWD":/repo:ro --entrypoint bash "$IMAGE" \
    -c 'cp /repo/tests/run_in_docker.sh /tmp/run_in_docker.sh && exec bash /tmp/run_in_docker.sh --inside "$@"' _ "$@"
fi
shift

# a writable copy of the tests (suites write next to themselves); the repo stays mounted read-only
# (retried: Docker's file sharing can report a file "replaced while being copied", and a failed copy ran the suite from /)
mkdir -p /src && for i in 1 2 3 4 5; do cp -r /repo/tests /repo/web /src/ && break; sleep 1; done; cd /src || exit 2
# test-only services baked into the image: IMAP server for the mail suites
mkdir -p /tmp/claude-0 /srv/dove/run /srv/dove/mail
cp tests/dovecot.conf /srv/dove/; cp tests/dovecot-oauth2.conf /srv/dove/oauth2.conf
echo "tester@example.com:{PLAIN}secretpass123" > /srv/dove/users; chown -R nobody:nogroup /srv/dove/mail
dovecot -c /srv/dove/dovecot.conf || echo "WARNING: dovecot did not start"
export PYTHONPATH=/src
# the whole test world on the app's timezone (tests/run_local.sh: Asia/Singapore): the suites, the fake services' pages
# and mail all say the same "today" as the app (claims_real 2026-10-31: a status page made in UTC was a day behind)
export TZ=Asia/Singapore
# the stand-ins' test CA, trusted by this container's Chromium only (its NSS database); started per suite by run_local.sh
export OMUSE_STANDINS=1
python3 -c "from tests.https_standins import certificate; certificate()" \
  && mkdir -p "$HOME/.pki/nssdb" && { [ -f "$HOME/.pki/nssdb/cert9.db" ] || certutil -d "sql:$HOME/.pki/nssdb" -N --empty-password; } \
  && certutil -d "sql:$HOME/.pki/nssdb" -A -t "C,," -n omuse-standins -i /tmp/claude-0/standins/ca.pem \
  || echo "WARNING: the HTTPS stand-ins' CA is not trusted"

if [ "$*" = real ]; then
  [ -n "${OMUSE_E2E_REAL:-}" ] || { echo "real: OMUSE_E2E_REAL is not set (pass OMUSE_E2E_ENV_FILE)"; exit 1; }
  SUITES="$REAL_SUITES"
elif [ $# -gt 0 ]; then
  SUITES="$*"
else
  SUITES="integration $(ls tests | grep -E '(_e2e|_ui)\.py$|^e2e_0|^browser_parallel|^popup_takeover|^takeover_burst|^i18n_check' | sed 's/\.py$//' | grep -v docker_browser_e2e)"
fi
failed=""
for f in $SUITES; do
  f="${f%.py}"
  bash tests/stop_local.sh >/dev/null 2>&1; pkill -f "uvicorn|http.server|https_standins|chrome|omuse" >/dev/null 2>&1; sleep 1
  REALDATA=()
  if [ -n "${OMUSE_E2E_REAL:-}" ] && [ -n "${OMUSE_E2E_SERPAPI_KEY:-}" ] && [[ "$REAL_DATA_SUITES" == *" $f "* ]]; then
    REALDATA=(OMUSE_E2E_SERPAPI_EP=https://serpapi.com OMUSE_E2E_KIWI_MCP_EP=https://mcp.kiwi.com/ OMUSE_E2E_KAYAK_EP=https://www.kayak.com
              OMUSE_E2E_RENTALCARS_EP=https://www.rentalcars.com OMUSE_E2E_EXA_MCP_EP=https://mcp.exa.ai/mcp
              OMUSE_E2E_BRAVE_EP=https://api.search.brave.com/res/v1 OMUSE_E2E_TAVILY_EP=https://api.tavily.com
              "OMUSE_E2E_MARKET_HOSTS=[]")
    # the browser's own engines for real too, except where the suite blocks them to reach the search APIs
    [ "$f" != web_search_real ] && REALDATA+=(OMUSE_E2E_SEARCH_ENGINES=)
    # the real web: the official sites' names resolve to the real sites for this suite, not to the stand-ins
    # (tests/https_standins.py HOSTS); /etc/hosts is put back after the suite
    cp /etc/hosts /tmp/hosts.standins
    python3 -c "from tests.https_standins import HOSTS; print('\n'.join(HOSTS))" > /tmp/standin_hosts
    grep -v -w -F -f /tmp/standin_hosts /tmp/hosts.standins > /tmp/hosts.realweb && cat /tmp/hosts.realweb > /etc/hosts
  fi
  # a suite's own settings for the stack: "# run_local: NAME=value …" lines in its file
  SUITEENV=($(grep -h '^# run_local: ' "tests/$f.py" 2>/dev/null | sed 's/^# run_local: //'))
  env ${REALDATA[@]+"${REALDATA[@]}"} ${SUITEENV[@]+"${SUITEENV[@]}"} bash tests/run_local.sh > /tmp/up.log 2>&1 < /dev/null
  # up to a minute: with several suites starting at once (-j) a stack can take longer than 20 s
  # every service of the stack answers (any HTTP status): the browser, Sentinel, and the fakes run_local.sh starts
  for i in $(seq 1 60); do
    up=1
    for port in 8090 8091 8093 8094 8095 8096 8097; do curl -s -o /dev/null "http://127.0.0.1:$port/" || up=0; done
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
  # every suite: no search ran with a price cap ≤ 0 or fewer than 1 traveller, no skill was refused as another bot's
  # (tests/args_guard.py)
  # (scripted-model suites only: a real-model suite checks only what the user sees, tests/user_common.py)
  if [ -z "${OMUSE_E2E_REAL:-}" ] && ! python3 tests/args_guard.py > "/tmp/guard-$f.log" 2>&1; then
    rc=1
    sed 's/^GUARD /FAIL args guard: /' "/tmp/guard-$f.log" | grep '^FAIL' >> "/tmp/out-$f.log"
  fi
  [ -f /tmp/hosts.standins ] && { cat /tmp/hosts.standins > /etc/hosts; rm -f /tmp/hosts.standins; }
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
