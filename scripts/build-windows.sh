#!/usr/bin/env bash
# Портативная Windows-сборка из WSL через cargo-xwin.
#   scripts/build-windows.sh [папка]   -- по умолчанию dist/windows
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"

OUT="${1:-dist/windows}"
TARGET=x86_64-pc-windows-msvc

cargo xwin build --release --target "$TARGET" -p klubok-cli
mkdir -p "$OUT"
cp "target/$TARGET/release/klubok-cli.exe" "$OUT/"
echo "Готово: $OUT/klubok-cli.exe"
