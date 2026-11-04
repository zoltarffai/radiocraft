#!/usr/bin/env bash
# Build RadioCraft AppImage inside the manylinux container (glibc 2.28+ baseline).
# Prereq: an image built from .github/Dockerfile.manyrust:
#   podman build -t manyrust -f .github/Dockerfile.manyrust .github
set -euo pipefail
IMAGE=${IMAGE:-localhost/manyrust}
CONTAINER=${CONTAINER:-podman}
VERSION=${VERSION:-$(grep -m1 '^version' "$(dirname "$0")/../Cargo.toml" | sed 's/.*"\(.*\)"/\1/')}
export VERSION
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
stage=/tmp/rc-build
rm -rf "$stage" && mkdir -p "$stage/dist"
tar -C "$ROOT" --exclude=.git --exclude=target --exclude=out -cf - . | tar -C "$stage" -xf -
"$CONTAINER" run --rm -e VERSION -v "$stage:/src:Z" "$IMAGE" bash -e -c '
  set -e
  if ! command -v linuxdeploy >/dev/null 2>&1; then
    curl -sL -o /usr/local/bin/linuxdeploy \
      https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
    chmod +x /usr/local/bin/linuxdeploy
  fi
  cd /src
  cargo build --release -p radiocraft-server
  ( cd web && if [ -f package.json ]; then
      if command -v npm >/dev/null 2>&1; then npm ci && npm run build
      else echo "npm not found; expecting pre-built web/dist" >&2; test -d dist || exit 1; fi
    fi )
  appdir=/src/out/radiocraft.AppDir
  # Web assets go under <bindir>/../lib/RadioCraft/web/dist, one of the
  # exe-relative fallbacks the server itself resolves (see main.rs), so the
  # desktop Exec can name the binary directly — linuxdeploy rejects an Exec
  # whose argv0 (`sh`) has no matching executable in the AppDir.
  mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/lib/RadioCraft/web/dist"
  cp target/release/radiocraft-server "$appdir/usr/bin/radiocraft-server"
  cp -r web/dist/. "$appdir/usr/lib/RadioCraft/web/dist/"
  cat > "$appdir/radiocraft.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=RadioCraft
Exec=radiocraft-server
Icon=radiocraft
Terminal=false
Categories=AudioVideo;Audio;
EOF
  # Placeholder icon (radiocraft.png) — replace with a real logo before release.
  test -f "$appdir/radiocraft.png" || python3 - <<'PY'
import struct, zlib
w = h = 256
pixel = b"\x12\x22\x36\xff"  # RGBA
raw = b"".join(b"\x00" + pixel * w for _ in range(h))
def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
png = (b"\x89PNG\r\n\x1a\n"
       + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
       + chunk(b"IDAT", zlib.compress(raw, 9))
       + chunk(b"IEND", b""))
open("/src/out/radiocraft.AppDir/radiocraft.png", "wb").write(png)
PY
  export VERSION=${VERSION:-0.1.0}
  # linuxdeploy ships as an AppImage; containers have no FUSE, so run it
  # extracted-to-temp instead of mounted.
  export APPIMAGE_EXTRACT_AND_RUN=1
  linuxdeploy --appdir "$appdir" --desktop-file "$appdir/radiocraft.desktop" \
    --icon-file "$appdir/radiocraft.png" --output appimage
  mkdir -p /src/dist && mv radiocraft-*.AppImage /src/dist/ 2>/dev/null || \
    find /src -maxdepth 1 -name "*.AppImage" -exec mv {} /src/dist/ \;
'
mkdir -p "$ROOT/dist"
cp "$stage"/dist/*.AppImage "$ROOT/dist/" 2>/dev/null || true
echo "AppImage -> $ROOT/dist/"
