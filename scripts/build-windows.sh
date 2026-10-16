#!/usr/bin/env bash
# Портативная Windows-сборка из WSL через cargo-xwin.
#   scripts/build-windows.sh [папка]   -- по умолчанию dist/windows
#
# Получается:
#   klubok.exe      -- окно (интерфейс встроен в exe; нужен WebView2, он есть в Windows 10/11)
#   klubok-cli.exe  -- консольный клиент для отладки
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"

OUT="${1:-dist/windows}"
TARGET=x86_64-pc-windows-msvc

(cd app && npm install --silent && npx tauri build --no-bundle --runner cargo-xwin --target "$TARGET")
cargo xwin build --release --target "$TARGET" -p klubok-cli

mkdir -p "$OUT"
cp "target/$TARGET/release/klubok.exe" "target/$TARGET/release/klubok-cli.exe" "$OUT/"
echo "Готово: $OUT/klubok.exe, $OUT/klubok-cli.exe"
