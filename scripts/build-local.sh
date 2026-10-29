#!/usr/bin/env bash
# Local development build/check for RadioCraft.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
cargo check --workspace
cargo test --workspace --exclude radiocraft-app
