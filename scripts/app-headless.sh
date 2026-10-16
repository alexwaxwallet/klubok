#!/usr/bin/env bash
# Запуск окна Клубка на виртуальном дисплее (Xvfb) — чтобы Claude мог делать скриншоты и кликать.
#   scripts/app-headless.sh start           -- собрать (debug) и запустить
#   scripts/app-headless.sh shot [файл.png] -- скриншот (по умолчанию /tmp/klubok-shot.png)
#   scripts/app-headless.sh stop
# Кликать и печатать: DISPLAY=:99 xdotool ... (окно ищется по имени «Клубок»).
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"

export DISPLAY=:99
RUNTIME="${TMPDIR:-/tmp}/klubok-xdg"

case "${1:-}" in
	start)
		pgrep -x Xvfb >/dev/null || { Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 & sleep 1; }
		(cd app && npx tauri build --debug --no-bundle >/dev/null)
		pkill -x klubok || true
		mkdir -p "$RUNTIME" && chmod 700 "$RUNTIME"
		env -u WAYLAND_DISPLAY GDK_BACKEND=x11 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
			WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 XDG_RUNTIME_DIR="$RUNTIME" \
			./target/debug/klubok >"$RUNTIME/app.log" 2>&1 &
		sleep 6
		echo "Окно: $(xdotool search --onlyvisible --name 'Клубок' | head -1), лог: $RUNTIME/app.log"
		;;
	shot)
		import -window root "${2:-/tmp/klubok-shot.png}" && echo "${2:-/tmp/klubok-shot.png}"
		;;
	stop)
		pkill -x klubok || true
		pkill -x Xvfb || true
		;;
	*)
		echo "использование: $0 start|shot [файл]|stop" >&2
		exit 1
		;;
esac
