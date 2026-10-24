#!/bin/sh
# Package the Olares chart as dist/<app>-<version>.tgz (upload it in Olares Market > Upload custom chart).
# The pod runs the release image (`omuse`, one role per container); nothing is downloaded at start.
#   sh chart/build.sh            # app id "omuse" (current install)
#   sh chart/build.sh persona    # the old pre-2026-10 install id
# OMUSE_IMAGE overrides the image (default: the release image of SOURCE_VERSION in crates/omuse-common/src/util.rs).
set -eu
cd "$(dirname "$0")/.."
APP=$(printf '%s' "${1:-omuse}" | tr 'A-Z' 'a-z')
printf '%s' "$APP" | grep -Eq '^[a-z][a-z0-9]{1,29}$' || { echo "app id must be lowercase letters/digits" >&2; exit 1; }
VER=$(sed -n 's/^pub const SOURCE_VERSION: &str = "\(.*\)";$/\1/p' crates/omuse-common/src/util.rs)
[ -n "$VER" ] || { echo "SOURCE_VERSION not found" >&2; exit 1; }
IMAGE="${OMUSE_IMAGE:-ghcr.io/second-state/omuse:locius-v$VER}"
DIST="dist/$APP"
rm -rf "$DIST"; mkdir -p "$DIST/templates"
cp -R chart/persona/. "$DIST/"

# the chart sources use the id "persona"; rename every resource, label and path for another app id
rename_tpl() {
  if [ "$APP" = persona ]; then cat; return; fi
  sed -E -e "s/persona-(voice|config)/$APP-\1/g" \
    -e "s/\.Values\.workloads\.persona\./.Values.workloads.$APP./g" \
    -e "s/^([[:space:]]*(name|app): )persona\$/\1$APP/" \
    -e "s/^([[:space:]]*app_id = )\"persona\"\$/\1\"$APP\"/"
}
rename_tpl < chart/deployment.tpl.yaml > "$DIST/templates/$APP.yaml"
for f in Chart.yaml OlaresManifest.yaml values.yaml; do
  p="$DIST/$f"
  if [ "$APP" != persona ]; then
    sed -E -e "s/^([[:space:]]*(-[[:space:]]+)?(name|appid|host): )persona\$/\1$APP/" \
      -e "s/^([[:space:]]+)persona:( 1)?\$/\1$APP:\2/" \
      -e "s/persona-(voice|appdata|appcache)/$APP-\1/g" "$p" > "$p.tmp" && mv "$p.tmp" "$p"
  fi
  sed -E -e "s/^version: .*/version: $VER/" -e "s/^appVersion: .*/appVersion: \"$VER\"/" \
    -e "s/^  version: '.*'\$/  version: '$VER'/" -e "s/^  versionName: '.*'\$/  versionName: '$VER'/" \
    -e "s|^  omuse: [^ ]*|  omuse: $IMAGE|" "$p" > "$p.tmp" && mv "$p.tmp" "$p"
done
tar -czf "dist/$APP-$VER.tgz" -C dist "$APP"
echo "chart dist/$APP-$VER.tgz (image $IMAGE)"
