#!/usr/bin/env bash
# Build RadioCraft AppImage inside the manylinux container (glibc 2.28+).
# Prereq: podman/ docker + image built from .github/Dockerfile.manyrust:
#   podman build -t manyrust -f .github/Dockerfile.manyrust .github
set -euo pipefail
IMAGE=${IMAGE:-manyrust}
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
stage=/tmp/rc-build
rm -rf "$stage" && mkdir -p "$stage"
tar -C "$ROOT" --exclude=.git --exclude=target -cf - . | tar -C "$stage" -xf -
podman run --rm --userns=keep-id -v "$stage:/src:Z" localhost/manyrust bash -e -c '
  dnf -q install -y linuxdeploy.appimage 2>/dev/null || (curl -sL -o /usr/local/bin/linuxdeploy \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous && chmod +x /usr/local/bin/linuxdeploy)
  cd /src
  cargo build --release -p radiocraft-app
  stage=/src/out/radiocraft.AppDir
  mkdir -p $stage/usr/bin $stage/usr/share/applications
  target/release/radiocraft-app $stage/usr/bin/radiocraft-app || cp target/release/radiocraft-app $stage/usr/bin/radiocraft-app
  cat > $stage/radiocraft.desktop <<EOF
[Desktop Entry]
Type=Application
Name=RadioCraft
Exec=radiocraft-app
Icon=radiocraft
Categories=AudioVideo;Audio;
EOF
  echo "ICON_PLACEHOLDER: add Icon.png + Fusion icon theme (see 03-pc-hall-of-fame)"
  export LD_LIBRARY_PATH=/usr/lib64:$LD_LIBRARY_PATH
  export VERSION=0.1.0
  linuxdeploy --appdir $stage --output appimage --desktop-file $stage/radiocraft.desktop
  cp radiocraft*.AppImage /src/dist/ 2>/dev/null || (mkdir -p /src/dist && cp radiocraft*.AppImage /src/dist/)
'
echo "AppImage -> $ROOT/dist/"
