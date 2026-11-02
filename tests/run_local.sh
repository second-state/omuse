#!/bin/bash
# Start the scripted model (tests/fake_llm.py) and the OMuse binary (all three services in one process) locally. The
# binary reads one config.toml, written here. No fake services: every third-party endpoint is the real one (the
# product's defaults); the scripted suites never need them, the real-model suites use them as a user's install does.
set -e
cd "$(dirname "$0")/.."
T=/tmp/claude-0/persona-test
rm -rf $T; mkdir -p $T/{sdata,data,workspace,bprofile}
BIN="${OMUSE_BIN:-$PWD/target/release/omuse}"
export PYTHONPATH=$PWD
python3 -m uvicorn tests.fake_llm:app --port 8090 >$T/llm.log 2>&1 &
# the scripted suites stay off the internet: web search, travel data and market data point at a port nothing listens on
# (a fast, certain failure — no fake answers); the real-model suites (OMUSE_E2E_REAL) use the real services
OFF="http://127.0.0.1:9"
if [ -z "${OMUSE_E2E_REAL:-}" ]; then
  ENGINES="${OMUSE_E2E_SEARCH_ENGINES-off=$OFF/?q=}"
  ENDPOINTS="serpapi = \"$OFF\"
brave = \"$OFF\"
tavily = \"$OFF\"
exa = \"$OFF\"
exa_mcp = \"$OFF/mcp\"
kiwi_mcp = \"$OFF/\"
kayak = \"$OFF\"
rentalcars = \"$OFF\"
market_data_hosts = [\"$OFF\"]"
  FREE_EXA=false
else
  ENGINES="${OMUSE_E2E_SEARCH_ENGINES-}"
  ENDPOINTS=""
  FREE_EXA=true
fi
cat > $T/config.toml <<TOML
[server]
host = "0.0.0.0"
port = 8080
runtime_port = 8081
browser_port = 8082
timezone = "Asia/Singapore"
scheduler_loop = ${OMUSE_E2E_SCHEDULER_LOOP:-true}   # a suite's "# run_local: OMUSE_E2E_SCHEDULER_LOOP=false" line (run_in_docker.sh)

[data]
sentinel = "$T/sdata"
runtime = "$T/data"
workspace = "$T/workspace"
browser_profile = "$T/bprofile"

[auth]
enabled = true
user = "omuse"
password = "local-test"
# no login page in front: the suites call the API directly; the gate has its own tests (docker_image_smoke.sh)
gate = false

[model]
url = "http://127.0.0.1:8090/v1"
name = "fake"
fast = true                # every request to the system model carries service_tier "fast" (models_ui.py checks it)
reasoning_effort = "low"

[browser]
auto_approve = false       # the suites check the approval flow; browser_auto_real.py turns it on itself
search_engines = "$ENGINES"   # empty: the real DuckDuckGo / Bing

[search]
free_exa = $FREE_EXA

[endpoints]
$ENDPOINTS

[skills]
dir = "${OMUSE_E2E_SKILLS_DIR-}"   # OMUSE_E2E_SKILLS_DIR=/repo/skills (run_in_docker.sh): the working tree's skills, no rebuild

[internal]
runtime_token = "rt-test"
browser_token = "bt-test"
TOML
env -u HTTPS_PROXY -u HTTP_PROXY -u https_proxy -u http_proxy "$BIN" --config $T/config.toml >$T/omuse.log 2>&1 &
sleep 4
for p in 8080 8081 8082 8090; do curl -s --noproxy '*' -o /dev/null -w "$p %{http_code}\n" http://127.0.0.1:$p/ ; done
