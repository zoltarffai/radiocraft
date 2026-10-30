#!/usr/bin/env bash
# Build RadioCraft AppImage inside the manylinux container (glibc 2.28+ baseline).
# Prereq: an image built from .github/Dockerfile.manyrust:
#   podman build -t manyrust -f .github/Dockerfile.manyrust .github
set -euo pipefail
IMAGE=${IMAGE:-localhost/manyrust}
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
stage=/tmp/rc-build
rm -rf "$stage" && mkdir -p "$stage/dist"
tar -C "$ROOT" --exclude=.git --exclude=target --exclude=out -cf - . | tar -C "$stage" -xf -
podman run --rm --userns=keep-id -v "$stage:/src:Z" "$IMAGE" bash -e -c '
  set -e
  if ! command -v linuxdeploy >/dev/null 2>&1; then
    curl -sL -o /usr/local/bin/linuxdeploy \
      https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
    chmod +x /usr/local/bin/linuxdeploy
  fi
  cd /src
  cargo build --release -p radiocraft-app
  appdir=/src/out/radiocraft.AppDir
  mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/dist"
  cp target/release/radiocraft-app "$appdir/usr/bin/radiocraft-app"
  cat > "$appdir/radiocraft.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=RadioCraft
Exec=radiocraft-app %f
Icon=radiocraft
Terminal=false
Categories=AudioVideo;Audio;
EOF
  # Placeholder icon (radiocraft.png) — replace with a real logo before release.
  test -f "$appdir/radiocraft.png" || python3 - <<'PY'
import struct, zlib
w = h = 256
raw = b"".join(b"\x00" + b"\x12\x22\x36\x255\x99\xff" * w for _ in range(h))
def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
png = (b"\x89PNG\r\n\x1a\n"
       + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
       + chunk(b"IDAT", zlib.compress(raw, 9))
       + chunk(b"IEND", b""))
open("/src/out/radiocraft.AppDir/radiocraft.png", "wb").write(png)
PY
  export VERSION=0.1.0
  linuxdeploy --appdir "$appdir" --desktop-file "$appdir/radiocraft.desktop" \
    --icon-file "$appdir/radiocraft.png" --output appimage
  mkdir -p /src/dist && mv radiocraft-*.AppImage /src/dist/ 2>/dev/null || \
    find /src -maxdepth 1 -name "*.AppImage" -exec mv {} /src/dist/ \;
'
mkdir -p "$ROOT/dist"
cp "$stage"/dist/*.AppImage "$ROOT/dist/" 2>/dev/null || true
echo "AppImage -> $ROOT/dist/"
