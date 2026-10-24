#!/bin/bash
# Start fake LLM, test pages, fake third-party services and the OMuse binary (all three services in one process)
# locally. The binary reads one config.toml, written here.
set -e
cd "$(dirname "$0")/.."
T=/tmp/claude-0/persona-test
rm -rf $T; mkdir -p $T/{sdata,data,workspace,bprofile}
BIN="${OMUSE_BIN:-$PWD/target/release/omuse}"
export PYTHONPATH=$PWD
(cd tests/pages && env -u HTTPS_PROXY -u HTTP_PROXY python3 -m http.server 8099 >$T/pages.log 2>&1 &)
TEST_PAGE=http://shop.test:8099/page.html python3 -m uvicorn tests.fake_llm:app --port 8090 >$T/llm.log 2>&1 &
python3 -m uvicorn tests.fake_telegram:app --port 8091 >$T/tg.log 2>&1 &
python3 -m uvicorn tests.fake_mcp:app --port 8093 >$T/mcp.log 2>&1 &
python3 -m uvicorn tests.fake_apps:app --port 8094 >$T/apps.log 2>&1 &
python3 -m uvicorn tests.fake_dial:app --port 8096 >$T/dial.log 2>&1 &
python3 -m uvicorn tests.fake_travel:app --port 8097 >$T/travel.log 2>&1 &
env -u HTTPS_PROXY -u HTTP_PROXY -u https_proxy -u http_proxy python3 -m uvicorn tests.fake_voice:app --port 8095 >$T/voice.log 2>&1 &
cat > $T/config.toml <<TOML
[server]
host = "0.0.0.0"
port = 8080
runtime_port = 8081
browser_port = 8082
timezone = "Asia/Singapore"

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

[browser]
search_engines = "empty=http://shop.test:8094/emptysearch?q=,duckduckgo=http://shop.test:8094/ddg?q="

[telegram]
api = "http://127.0.0.1:8091"

[voice]
port = 8083
allow_http = true
telnyx_api = "http://127.0.0.1:8095"
openai_api = "http://127.0.0.1:8095"
openai_realtime_url = "ws://127.0.0.1:8095/v1/realtime"
dialmcp_url = "http://127.0.0.1:8096/mcp"

[stripe]
api_key = "sk_test_fake_omuse"
customer_id = "cus_test123"
subscription_id = "sub_test123"
api = "http://127.0.0.1:8094/stripe"

[google]
auth_url = "http://127.0.0.1:8094/g/auth"
token_url = "http://127.0.0.1:8094/g/token"
userinfo_url = "http://127.0.0.1:8094/g/userinfo"
gcal_api = "http://127.0.0.1:8094/cal"
relay_url = "http://127.0.0.1:8094/relay/callback"
managed_file = "$T/google_managed.json"

[endpoints]
notion = "http://127.0.0.1:8094"
slack = "http://127.0.0.1:8094/api"
github_raw = "http://127.0.0.1:8094/ghraw"
ms_login = "http://127.0.0.1:18999"
outlook_imap = "127.0.0.1:1143:none"
outlook_smtp = "127.0.0.1:1025:none"
market_data_hosts = ["http://127.0.0.1:8094/yahoo1", "http://127.0.0.1:8094/yahoo2"]
ical_allow_http = true

[internal]
runtime_token = "rt-test"
browser_token = "bt-test"
TOML
env -u HTTPS_PROXY -u HTTP_PROXY -u https_proxy -u http_proxy "$BIN" --config $T/config.toml >$T/omuse.log 2>&1 &
sleep 4
for p in 8080 8081 8082 8090 8091 8093 8094; do curl -s --noproxy '*' -o /dev/null -w "$p %{http_code}\n" http://127.0.0.1:$p/ ; done
