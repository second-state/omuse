#!/usr/bin/env bash
# The Olares chart without Olares: render it with Helm (in Docker), then run its three containers the way the pod does
# (one network namespace, one `omuse --role …` each, unprivileged, the rendered config.toml) and check that they find
# each other: the UI, the runtime behind Sentinel's proxy and the browser broker behind Sentinel.
# Usage: bash tests/olares_chart_smoke.sh <release-image>      (e.g. omuse-fly, built from Dockerfile.fly)
set -euo pipefail
image=${1:?Usage: bash tests/olares_chart_smoke.sh <release-image>}
cd "$(dirname "$0")/.."
suffix="$$-$RANDOM"
pod="omuse-chart-smoke-$suffix"
work=$(mktemp -d)
cleanup() {
  for r in browser runtime sentinel; do docker rm -f "$pod-$r" >/dev/null 2>&1 || true; done
  docker run --rm -v "$work:/w" alpine:3.20 rm -rf /w/data >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
fail() { for r in sentinel runtime browser; do echo "--- $r"; docker logs --tail 20 "$pod-$r" 2>&1; done; exit 1; }

OMUSE_IMAGE="$image" sh chart/build.sh omuse >/dev/null
docker run --rm -v "$PWD/dist:/d:ro" alpine/helm:3.16.4 template t /d/omuse \
  --set bfl.username=smoke --set olaresEnv.OMUSE_MODEL=smoke-model > "$work/manifest.yaml"
# the config.toml of the rendered Secret (stringData, indented four spaces)
awk '/^  config.toml: \|/{on=1; next} on && /^[^ ]/{exit} on {sub(/^    /, ""); print}' \
  "$work/manifest.yaml" > "$work/config.toml"
grep -q '^name = "smoke-model"' "$work/config.toml" && grep -q '^\[internal\]' "$work/config.toml"
grep -q "image: \"$image\"" "$work/manifest.yaml"
echo 'PASS chart renders: config.toml in a Secret, the release image in every container'

# the init container's job: data directories owned by uid 1000
docker run --rm -v "$work:/w" alpine:3.20 sh -c 'mkdir -p /w/data/sdata /w/data/rdata /w/data/workspace /w/data/bprofile && chown -R 1000:1000 /w/data && chmod 700 /w/data/sdata'
run() {   # run <role> <docker args...>
  local role=$1; shift
  docker run -d --name "$pod-$role" --user 1000:1000 -e HOME=/tmp -e TZ=Asia/Singapore \
    -v "$work/config.toml:/config/config.toml:ro" "$@" --entrypoint tini "$image" \
    -- /opt/omuse/bin/omuse --config /config/config.toml --role "$role" >/dev/null
}
run sentinel -p 127.0.0.1::8080 -v "$work/data/sdata:/sdata"
sleep 1; [ "$(docker inspect --format "{{.State.Running}}" "$pod-sentinel")" = true ] || fail
run runtime --network "container:$pod-sentinel" -v "$work/data/rdata:/data" -v "$work/data/workspace:/workspace"
run browser --network "container:$pod-sentinel" -v "$work/data/bprofile:/bprofile" -v "$work/data/workspace:/workspace" \
  --shm-size 512m
port=$(docker port "$pod-sentinel" 8080/tcp)
for ((i=0; i<60; i++)); do
  curl -fsS "http://$port/api/health" >/dev/null 2>&1 && curl -fsS "http://$port/sentinel/api/health" >/dev/null 2>&1 && break
  sleep 1
done
test "$(curl -s -o /dev/null -w '%{http_code}' "http://$port/")" = 200   # the Olares entrance does the login
echo 'PASS UI served without a login page (the entrance logs in)'
curl -fsS "http://$port/api/settings" -H 'X-Persona-UI: 1' | jq -e '.settings.model_name == "smoke-model"' >/dev/null
echo 'PASS runtime reached through Sentinel, model from the chart values'
for ((i=0; i<30; i++)); do
  st=$(curl -s "http://$port/sentinel/api/browser/state" -H 'X-Persona-UI: 1' || true)
  echo "$st" | jq -e 'has("mode") and .mode != "offline" and (has("detail") | not)' >/dev/null 2>&1 && break
  sleep 1
done
echo "$st" | jq -e 'has("mode") and .mode != "offline" and (has("detail") | not)' >/dev/null || {
  echo "browser state: $st"; docker logs --tail 30 "$pod-browser"; exit 1; }
docker exec "$pod-sentinel" curl -fsS http://127.0.0.1:8082/health >/dev/null
echo 'PASS browser broker reached through Sentinel with the shared token'
for r in sentinel runtime browser; do
  test "$(docker inspect --format '{{.State.Running}}' "$pod-$r")" = true || { docker logs --tail 30 "$pod-$r"; exit 1; }
done
test -s "$work/data/sdata/vault.key"
echo 'PASS all three containers running unprivileged, state in the mounted volumes'
echo 'ALL CHART SMOKE CHECKS PASS'
