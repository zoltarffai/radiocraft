#!/usr/bin/env bash
# Cross-build RadioCraft for Windows (x86_64-pc-windows-gnu) with mingw.
# Requires: mingw-w64 installed (dnf install mingw-w64-gcc x86_64-w64-mingw32-... or distro equivalent).
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
TRIPLE=x86_64-pc-windows-gnu
rustup target add $TRIPLE >/dev/null 2>&1 || true
mkdir -p out
RCC_TARGET=$(pwd)/out/cargo-win-target
CARGO_TARGET_DIR=$RCC_TARGET cargo build --release -p radiocraft-app --target $TRIPLE \
  --config "target.$TRIPLE.linker=\"x86_64-w64-mingw32-gcc\"" \
  --config "target.$TRIPLE.ar=\"x86_64-w64-mingw32-ar\""
cp $RCC_TARGET/$TRIPLE/release/radiocraft-app.exe out/
echo "Windows exe: out/radiocraft-app.exe"
