#!/bin/bash
# OMuse in one container. Settings come from config.toml ($OMUSE_CONFIG, default /omuse/config.toml on the volume).
# Container platforms that hand each instance its settings as environment variables (Fly secrets, Products Provisioner)
# may leave the file out: then this script writes the config from OMUSE_PASSWORD, OMUSE_MODEL_URL, ... (the table in
# README.md) into a private file for this start. The binary itself reads only the config file.
set -euo pipefail
CONFIG="${OMUSE_CONFIG:-/omuse/config.toml}"
DATA=/omuse
# started as root (Dockerfile.fly: Fly.io mounts its volume root-owned): hand the data dir to the service user, drop privileges
if [ "$(id -u)" = 0 ]; then
  mkdir -p "$DATA"
  # Fly file secrets are materialized before a volume is mounted.  A config file
  # outside the data volume therefore has to be staged while we are still root:
  # Fly gives injected files mode 0600 and the service user must be able to read
  # the staged copy after privileges are dropped.  The normal /omuse/config.toml
  # path is left alone for desktop Docker users who bind-mount it there.
  if [ "$CONFIG" != "$DATA/config.toml" ] && [ -s "$CONFIG" ]; then
    staged_config=$(mktemp /tmp/omuse-config.XXXXXX)
    cp -- "$CONFIG" "$staged_config"
    chown omuse:omuse "$staged_config"
    chmod 600 "$staged_config"
    CONFIG="$staged_config"
  fi
  # A read-only mount inside the volume (config.toml) keeps its owner; everything else goes to the service user.
  [ "$(stat -c %U "$DATA")" = omuse ] || find "$DATA" -xdev ! -user omuse -exec chown omuse:omuse {} + 2>/dev/null || true
  exec setpriv --reuid=omuse --regid=omuse --init-groups env HOME=/home/omuse OMUSE_CONFIG="$CONFIG" "$0" "$@"
fi

toml_str() {   # a TOML basic string: backslashes, quotes and control characters escaped
  local s=${1//\\/\\\\}
  s=${s//\"/\\\"}; s=${s//$'\n'/\\n}; s=${s//$'\r'/\\r}; s=${s//$'\t'/\\t}
  printf '"%s"' "$s"
}
kv() {   # kv <key> <env var>: one line when the variable is set
  [ -n "${!2:-}" ] && printf '%s = %s\n' "$1" "$(toml_str "${!2}")"
  return 0
}

if [ ! -s "$CONFIG" ]; then
  if [ "${OMUSE_AUTH:-}" != off ] && [ -z "${OMUSE_PASSWORD:-}" ]; then
    echo "No config file at $CONFIG and no OMUSE_PASSWORD. OMuse reads your mail and drives a browser: it needs a login password." >&2
    echo "Copy config.example.toml (in the image: /opt/omuse/config.example.toml), set [auth] password, and mount it:" >&2
    echo "  docker run ... -v \$PWD/config.toml:/omuse/config.toml:ro omuse" >&2
    echo "or pass -e OMUSE_PASSWORD=... (OMUSE_AUTH=off if another proxy in front already does the login)." >&2
    exit 64
  fi
  CONFIG=$(mktemp /tmp/omuse-config.XXXXXX)
  chmod 600 "$CONFIG"
  {
    echo "# written by the entrypoint from the environment"
    echo "[server]"; kv timezone TZ
    echo "[auth]"
    [ "${OMUSE_AUTH:-}" = off ] && echo "enabled = false"
    kv user OMUSE_USER; kv password OMUSE_PASSWORD
    echo "[model]"
    kv url OMUSE_MODEL_URL; kv api_key OMUSE_MODEL_API_KEY; kv name OMUSE_MODEL
    kv planner OMUSE_PLANNER_MODEL; kv vision OMUSE_VISION_MODEL; kv stt OMUSE_STT_MODEL
    kv image OMUSE_IMAGE_MODEL; kv image_url OMUSE_IMAGE_URL; kv image_api_key OMUSE_IMAGE_API_KEY
    kv video OMUSE_VIDEO_MODEL; kv video_url OMUSE_VIDEO_URL; kv video_api_key OMUSE_VIDEO_API_KEY
    case "${OMUSE_MODEL_FAST:-}" in 1|true|TRUE|yes|on) echo "fast = true";; esac
    kv reasoning_effort OMUSE_MODEL_REASONING
    if [ -n "${OMUSE_SKILLS:-}" ]; then
      echo "[skills]"
      printf 'initial = ['; sep=""
      IFS=, read -ra skills <<< "$OMUSE_SKILLS"
      for s in "${skills[@]}"; do s=$(echo "$s" | xargs); [ -n "$s" ] && { printf '%s%s' "$sep" "$(toml_str "$s")"; sep=", "; }; done
      echo "]"
    fi
    echo "[stripe]"
    kv api_key OMUSE_STRIPE_API_KEY; kv customer_id OMUSE_STRIPE_CUSTOMER_ID; kv subscription_id OMUSE_STRIPE_SUBSCRIPTION_ID
  } > "$CONFIG"
fi
exec /opt/omuse/bin/omuse --config "$CONFIG" "$@"
