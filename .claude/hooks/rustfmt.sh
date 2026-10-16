#!/usr/bin/env bash
# PostToolUse-хук: форматирует .rs-файл после того, как Claude его записал или отредактировал.
set -euo pipefail

file=$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("tool_input",{}).get("file_path",""))')

if [[ "$file" == *.rs && -f "$file" ]]; then
    [[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
    rustfmt --edition 2024 "$file" 2>&1 || true
fi
