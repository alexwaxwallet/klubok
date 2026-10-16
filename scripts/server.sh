#!/usr/bin/env bash
# Локальный сервер «Былин» (BRus MUD) для тестов клиента «Клубок».
# Исходники и сборка живут отдельно: /root/projects/bylins-server
#
#   scripts/server.sh start    -- запустить в фоне (pid-файл, лог в файл)
#   scripts/server.sh stop     -- корректно остановить (SIGTERM -> сохранение игроков)
#   scripts/server.sh restart
#   scripts/server.sh status
#   scripts/server.sh log [N]  -- последние N строк syslog (по умолчанию 50); log -f -- следить
#   scripts/server.sh build    -- пересобрать (ninja)
#
# Переменные окружения: BYLINS_DIR, BYLINS_PORT (по умолчанию 4000).
set -euo pipefail

BYLINS_DIR="${BYLINS_DIR:-/root/projects/bylins-server}"
PORT="${BYLINS_PORT:-4000}"
BUILD_DIR="$BYLINS_DIR/build"
WORLD_DIR="$BYLINS_DIR/lib"
BIN="$BUILD_DIR/circle"
PIDFILE="$BUILD_DIR/circle.pid"
OUTFILE="$BUILD_DIR/circle.out"   # stdout/stderr процесса
SYSLOG="$BUILD_DIR/syslog"        # основной лог сервера (пишется в cwd = build/)

is_running() {
	[[ -f "$PIDFILE" ]] || return 1
	local pid
	pid="$(cat "$PIDFILE" 2>/dev/null || true)"
	[[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null && [[ "$(cat /proc/"$pid"/comm 2>/dev/null)" == "circle" ]]
}

port_listening() {
	ss -ltnH "sport = :$PORT" 2>/dev/null | grep -q .
}

cmd_start() {
	if is_running; then
		echo "Уже запущен (pid $(cat "$PIDFILE")), порт $PORT"
		return 0
	fi
	[[ -x "$BIN" ]] || { echo "Нет бинарника $BIN -- сначала: $0 build" >&2; exit 1; }
	if port_listening; then
		echo "Порт $PORT уже занят другим процессом" >&2
		exit 1
	fi
	cd "$BUILD_DIR"
	nohup "$BIN" -d "$WORLD_DIR" "$PORT" >>"$OUTFILE" 2>&1 &
	echo $! >"$PIDFILE"
	# Мир грузится несколько секунд -- ждём, пока порт начнёт слушаться
	for _ in $(seq 1 120); do
		if port_listening; then
			echo "Запущен: pid $(cat "$PIDFILE"), порт $PORT (telnet 127.0.0.1 $PORT)"
			return 0
		fi
		if ! is_running; then
			echo "Сервер завершился при старте. Хвост лога:" >&2
			tail -n 30 "$SYSLOG" >&2 || true
			rm -f "$PIDFILE"
			exit 1
		fi
		sleep 0.5
	done
	echo "Процесс жив (pid $(cat "$PIDFILE")), но порт $PORT за 60 с так и не открылся" >&2
	exit 1
}

cmd_stop() {
	if ! is_running; then
		echo "Не запущен"
		rm -f "$PIDFILE"
		return 0
	fi
	local pid
	pid="$(cat "$PIDFILE")"
	kill -TERM "$pid"
	for _ in $(seq 1 60); do
		kill -0 "$pid" 2>/dev/null || { rm -f "$PIDFILE"; echo "Остановлен (pid $pid)"; return 0; }
		sleep 0.5
	done
	echo "Не остановился за 30 с, шлю SIGKILL" >&2
	kill -KILL "$pid" 2>/dev/null || true
	rm -f "$PIDFILE"
}

cmd_status() {
	if is_running; then
		local pid
		pid="$(cat "$PIDFILE")"
		if port_listening; then
			echo "Работает: pid $pid, порт $PORT слушается"
		else
			echo "Процесс жив (pid $pid), но порт $PORT не слушается (идёт загрузка?)"
		fi
	else
		echo "Остановлен"
		return 3
	fi
}

cmd_log() {
	if [[ "${1:-}" == "-f" ]]; then
		exec tail -F "$SYSLOG"
	fi
	tail -n "${1:-50}" "$SYSLOG"
}

cmd_build() {
	ninja -C "$BUILD_DIR" -j"$(( $(nproc) / 2 ))"
}

case "${1:-}" in
	start) cmd_start ;;
	stop) cmd_stop ;;
	restart) cmd_stop; cmd_start ;;
	status) cmd_status ;;
	log) shift; cmd_log "$@" ;;
	build) cmd_build ;;
	*)
		echo "Использование: $0 {start|stop|restart|status|log [N|-f]|build}" >&2
		exit 2
		;;
esac
