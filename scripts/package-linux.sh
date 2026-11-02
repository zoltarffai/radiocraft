#!/usr/bin/env bash
# Build RadioCraft deb + rpm bundles inside the rc-tauri container
# (fedora:44 with webkit2gtk4.1 + tauri CLI). The host cannot compile the
# window shell, so everything bundle-related happens in the image:
#   podman build -t localhost/rc-tauri -f .github/Dockerfile.fedora-tauri .
#
# Flow (mirrors docs/06 §10 container rules — never share host target/):
#   1. Host: build radiocraft-server (release) + web/dist.
#   2. Stage a clean context; drop the server binary next to the window crate
#      (tauri.conf.json linux.{deb,rpm}.files maps it to /usr/bin/).
#   3. Container: cargo tauri build -b deb rpm in crates/radiocraft-window,
#      cargo cache in the named volume rc-window-target:/src/target.
#   4. Copy the produced bundles back to dist/.
set -euo pipefail
IMAGE=${IMAGE:-localhost/rc-tauri}
CONTAINER=${CONTAINER:-podman}
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
stage=/tmp/rc-pkg
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --release -p radiocraft-server
( cd "$ROOT/web" && if [ -f package.json ]; then
    if command -v npm >/dev/null 2>&1; then npm run build
    else echo "npm not found; expecting pre-built web/dist" >&2; test -d dist || exit 1; fi
  fi )
test -d "$ROOT/web/dist" || { echo "web/dist missing" >&2; exit 1; }
test -d "$ROOT/crates/radiocraft-window/icons" || { echo "window icons missing" >&2; exit 1; }

rm -rf "$stage" && mkdir -p "$stage"
tar -C "$ROOT" Cargo.toml Cargo.lock crates web scripts .github \
  | tar -C "$stage" -xf -
rm -rf "$stage/web/node_modules"
cp "$ROOT/target/release/radiocraft-server" \
   "$stage/crates/radiocraft-window/radiocraft-server"

"$CONTAINER" run --rm -v "$stage:/src:Z" -v rc-window-target:/src/target:Z "$IMAGE" bash -e -c '
  cd /src/crates/radiocraft-window
  cargo tauri build -b deb rpm
  mkdir -p /src/bundles
  cp target/release/bundle/deb/*.deb target/release/bundle/rpm/*.rpm /src/bundles/
'

mkdir -p "$ROOT/dist"
cp "$stage"/bundles/*.deb "$stage"/bundles/*.rpm "$ROOT/dist/"
echo "bundles -> $ROOT/dist/"

# Sanity: both packages must ship the server sibling (spawn_server looks for
# it next to radiocraft-window) and the web assets (resolve_web_dist looks at
# <prefix>/lib/RadioCraft/web/dist).
"$CONTAINER" run --rm -v "$ROOT/dist:/d:Z" "$IMAGE" bash -e -c '
  dpkg-deb -c /d/*.deb | grep -q usr/bin/radiocraft-server
  dpkg-deb -c /d/*.deb | grep -q usr/lib/RadioCraft/web/dist/index.html
  rpm -qlp /d/*.rpm | grep -q usr/bin/radiocraft-server
  rpm -qlp /d/*.rpm | grep -q usr/lib/RadioCraft/web/dist/index.html
  echo "layout verified: server + web assets present in deb and rpm"
'
