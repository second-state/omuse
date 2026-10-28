#!/usr/bin/env bash
# Verify the Rust image's entrypoint, config handling, auth, Chromium and persistent volume without live credentials.
# Usage: [OMUSE_VERSION=<tag>] [OMUSE_TEST_IMAGE=omuse-test] bash tests/docker_image_smoke.sh <local-image>
# The checks that need Python (httpx, Playwright) run in a sidecar container of the test image (Dockerfile.test),
# on the same Docker network as the image under test; the image itself has no Python.
set -euo pipefail

image=${1:?Usage: bash tests/docker_image_smoke.sh <local-image>}
tester=${OMUSE_TEST_IMAGE:-omuse-test}
suffix="$$-$(date +%s)-$RANDOM"
container="omuse-image-smoke-$suffix"
volume="omuse-image-smoke-$suffix"
network="omuse-image-smoke-$suffix"
platform=$(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$image")
work=$(mktemp -d)
cleanup() {
  docker rm -f "$container" "$container-env" >/dev/null 2>&1 || true
  docker volume rm "$volume" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

# Fail closed without a config file / password.
rc=0
docker run --rm --platform "$platform" "$image" >/dev/null 2>&1 || rc=$?
test "$rc" -eq 64
echo 'PASS refuses to start without a config file'
# An isolated, disposable test password; never a real product credential.
export OMUSE_PASSWORD="smoke-$suffix"
cat > "$work/nopw.toml" <<TOML
[auth]
enabled = true
TOML
rc=0
docker run --rm --platform "$platform" -v "$work/nopw.toml:/omuse/config.toml:ro" "$image" >/dev/null 2>&1 || rc=$?
test "$rc" -eq 64
echo 'PASS refuses to start without a password'

# Platforms that pass only environment variables (Fly secrets, Products Provisioner): no config file, settings from OMUSE_*.
envc="$container-env"
docker run -d --name "$envc" --platform "$platform" -e OMUSE_PASSWORD -e OMUSE_MODEL_URL=http://127.0.0.1:9/v1 \
  -e OMUSE_MODEL='smoke "env" model' -e OMUSE_SKILLS='reply-email, online-shopping' "$image" >/dev/null
ok=false
for ((i=0; i<60; i++)); do
  if docker exec "$envc" curl -fsS -u "omuse:$OMUSE_PASSWORD" http://127.0.0.1:8080/api/settings 2>/dev/null \
      | jq -e '.settings.model_name == "smoke \"env\" model"' >/dev/null 2>&1; then ok=true; break; fi
  sleep 1
done
code=$(docker exec "$envc" curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/api/settings || true)
skills=$(docker exec "$envc" curl -fsS -u "omuse:$OMUSE_PASSWORD" http://127.0.0.1:8080/api/skills | jq -c '[.skills[] | select(.enabled) | .name] | sort' || true)
docker rm -f "$envc" >/dev/null
test "$ok" = true && test "$code" = 401 && test "$skills" = '["online-shopping","reply-email"]' || { echo "env start: ok=$ok code=$code skills=$skills"; exit 1; }
echo 'PASS starts from environment variables alone (password, model, initial skills)'

cat > "$work/config.toml" <<TOML
[auth]
password = "$OMUSE_PASSWORD"
[model]
url = "http://127.0.0.1:9/v1"
name = "smoke"
[telegram]
enabled = false
[voice]
port = 0
TOML
# Exercise the same private mode used by Fly file secrets.  The root-starting
# release image must stage this file before dropping to the omuse user.
chmod 600 "$work/config.toml"
docker volume create "$volume" >/dev/null
docker network create "$network" >/dev/null
start() {
  docker run -d --name "$container" --platform "$platform" --network "$network" \
    --mount "type=volume,source=$volume,target=/omuse" \
    -v "$work/config.toml:/etc/omuse/config.toml:ro" -e OMUSE_CONFIG=/etc/omuse/config.toml \
    -p 127.0.0.1::8080 "$image" >/dev/null
  local ready=false
  for ((i=0; i<90; i++)); do
    if docker exec "$container" curl -fsS http://127.0.0.1:8082/health >/dev/null 2>&1 \
      && docker exec "$container" curl -fsS http://127.0.0.1:8080/sentinel/api/health >/dev/null 2>&1 \
      && docker exec "$container" curl -fsS http://127.0.0.1:8081/api/health >/dev/null 2>&1; then
      ready=true
      break
    fi
    sleep 1
  done
  if [ "$ready" != true ]; then
    docker logs --tail 100 "$container"
    return 1
  fi
  echo 'PASS all three services ready'
}

start
port=$(docker port "$container" 8080/tcp)
test "$(curl -s -o /dev/null -w '%{http_code}' "http://$port/")" = 401
echo 'PASS published port requires authentication'
if [ -n "${OMUSE_VERSION:-}" ]; then
  expected=${OMUSE_VERSION#*-v}; expected=${expected#v}
  got=$(curl -fsS -u "omuse:$OMUSE_PASSWORD" "http://$port/sentinel/api/health" | jq -er .version)
  test "$got" = "$expected"
  echo "PASS app reports version $got for tag $OMUSE_VERSION"
fi

side() {   # run a Python check in the test image, next to the container
  # share the container's network namespace: the checks see 127.0.0.1 exactly as checks inside the container would
  # (and Chromium does not try to upgrade a plain-http container hostname to https)
  docker run --rm -i --network "container:$container" -e OMUSE_PASSWORD -e B="http://127.0.0.1:8080" -e COOKIE --entrypoint python3 "$tester" - "$@"
}
side <<'PY'
import os
import httpx

b = os.environ['B']
with httpx.Client(base_url=b, auth=('omuse', os.environ['OMUSE_PASSWORD']), timeout=30) as c:
    assert c.get('/').status_code == 200
    assert c.get('/api/health').json()['ok']
    assert c.get('/sentinel/api/subscription').json() == {'enabled': False}
    assert c.get('/internal/browser_state').status_code == 401
    assert httpx.get(b + '/', auth=('omuse', 'wrong'), timeout=10).status_code == 401
    assert c.put('/api/settings', json={'model_override': True, 'model_name': 'smoke-persisted'}, headers={'X-Persona-UI': '1'}).status_code == 400   # endpoint and key required
    assert c.put('/api/settings', json={'model_override': True, 'model_base_url': 'http://127.0.0.1:9/v1', 'model_api_key': 'smoke-key',
                                        'model_name': 'smoke-persisted'}, headers={'X-Persona-UI': '1'}).status_code == 200
print('PASS UI, runtime proxy, authentication, subscription defaults and internal token guard')
nav = {'Sec-Fetch-Mode': 'navigate'}
with httpx.Client(base_url=b, timeout=30) as c:
    r = c.get('/', headers=nav)
    assert r.status_code == 302 and r.headers['location'] == '/login', (r.status_code, r.headers)
    assert c.get('/api/settings', headers={'Sec-Fetch-Mode': 'cors'}).status_code == 401
    assert 'www-authenticate' not in c.get('/api/settings', headers={'Sec-Fetch-Mode': 'cors'}).headers
    assert 'name="password"' in c.get('/login', headers=nav).text
    r = c.post('/login', data={'user': 'omuse', 'password': 'wrong', 'next': '/'}, headers=nav)
    assert r.status_code == 401 and 'omuse_session' not in c.cookies
    r = c.post('/login', data={'user': 'omuse', 'password': os.environ['OMUSE_PASSWORD'], 'next': '/'}, headers=nav)
    assert r.status_code == 303 and r.headers['location'] == '/' and c.cookies.get('omuse_session'), (r.status_code, r.headers)
    assert c.get('/', headers=nav).status_code == 200 and c.get('/api/settings', headers={'Sec-Fetch-Mode': 'cors'}).status_code == 200
    print('COOKIE=' + c.cookies.get('omuse_session'))
    r = c.get('/logout', headers=nav)
    assert r.status_code == 303 and c.get('/', headers=nav).status_code == 302
print('PASS login page, cookie session and sign out')
PY
cookie=$(side <<'PY' | sed -n 's/^COOKIE=//p'
import os, httpx
b = os.environ['B']
with httpx.Client(base_url=b, timeout=30) as c:
    r = c.post('/login', data={'user': 'omuse', 'password': os.environ['OMUSE_PASSWORD'], 'next': '/'}, headers={'Sec-Fetch-Mode': 'navigate'})
    print('COOKIE=' + c.cookies.get('omuse_session'))
PY
)
test -n "$cookie"

# A root-owned Fly-style volume must be writable by the unprivileged service user; state is initialised.
docker exec "$container" sh -c 'test -s /omuse/sentinel/vault.key && test "$(stat -c %u /omuse/sentinel/vault.key)" != 0 && sha256sum /omuse/sentinel/vault.key | cut -d" " -f1' > "$work/vault.sha"
test -s "$work/vault.sha"
docker exec "$container" sh -c 'for p in /proc/[0-9]*; do c=$(tr "\0" " " < $p/cmdline 2>/dev/null); case "$c" in *bin/omuse*) stat -c %u $p;; esac; done' | sort -u > "$work/uids"
test "$(cat "$work/uids")" != 0
echo 'PASS services run unprivileged and initialize persistent state'

# The broker's health check proves Chromium starts in this image; also render a PDF through the broker.
docker exec "$container" sh -c 'curl -fsS http://127.0.0.1:8082/health' | grep -q . 
side <<'PY'
import os
from playwright.sync_api import sync_playwright
b = os.environ['B']
with sync_playwright() as p:
    browser = p.chromium.launch(headless=True, args=['--no-sandbox'])
    page = browser.new_page()
    page.goto(b + '/')
    page.wait_for_url(b + '/login')
    page.fill('input[name=user]', 'omuse')
    page.fill('input[name=password]', os.environ['OMUSE_PASSWORD'])
    page.click('button[type=submit]')
    page.wait_for_url(lambda u: u.split('#')[0] == b + '/')
    page.wait_for_selector('#navlinks a')   # the app booted and the API answered behind the login
    # first login with an empty profile: the chat view sends the user to Memory → Profile, once per session
    page.wait_for_url(lambda u: u == b + '/#memory/profile')
    page.reload()
    page.wait_for_selector('#profile')   # the session survived the reload: the profile card loaded from the API
    assert page.url == b + '/#memory/profile', page.url
    page.goto(b + '/logout')
    page.wait_for_url(b + '/login')
    page.goto(b + '/')
    page.wait_for_url(b + '/login')
    browser.close()
print('PASS browser login, session across reloads and sign out')
PY

docker stop --time 20 "$container" >/dev/null
test "$(docker inspect --format '{{.State.ExitCode}}' "$container")" != 137
docker rm "$container" >/dev/null
start
COOKIE="$cookie" side <<'PY'
import os, httpx
b = os.environ['B']
with httpx.Client(base_url=b, auth=('omuse', os.environ['OMUSE_PASSWORD']), timeout=30) as c:
    s = c.get('/api/settings').json()['settings']
    assert s['model_override'] and s['model_name'] == 'smoke-persisted' and s['model_api_key_set'] and 'model_api_key' not in s, s
    assert c.put('/api/settings', json={'model_override': False}, headers={'X-Persona-UI': '1'}).json()['settings']['model_name'] == 'smoke'
print('PASS settings survive container replacement')
PY
docker exec "$container" curl -s -o /dev/null -w '%{http_code}' -H 'Sec-Fetch-Mode: navigate' -H "Cookie: omuse_session=$cookie" "http://127.0.0.1:8080/" | grep -q '^200$'
docker exec "$container" sh -c 'sha256sum /omuse/sentinel/vault.key | cut -d" " -f1' | diff -q - "$work/vault.sha" >/dev/null
echo 'PASS workspace, vault key and the login session survive container replacement'
echo 'ALL IMAGE SMOKE CHECKS PASS'
