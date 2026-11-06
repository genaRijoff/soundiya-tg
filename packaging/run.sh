#!/bin/sh
# Keeps soundiya-tg running: restarts it after a crash, exits when stop.sh asks.
DIR=/data/opt/soundiya-tg
PIDFILE=/run/soundiya-tg.pid
STOP=/run/soundiya-tg.stop
LOG=/tmp/soundiya-tg.log
umask 077
echo $$ >"$PIDFILE"
child=
trap 'touch "$STOP"; [ -z "$child" ] || kill "$child" 2>/dev/null' TERM INT
rm -f "$STOP"
while [ ! -f "$STOP" ]; do
    # /tmp is RAM; keep the log small.
    if [ -f "$LOG" ] && [ "$(wc -c <"$LOG")" -gt 262144 ]; then
        : >"$LOG"
    fi
    "$DIR/soundiya-tg" --config "$DIR/config.json" run >>"$LOG" 2>&1 &
    child=$!
    wait "$child"
    code=$?
    child=
    [ -f "$STOP" ] && break
    # 3: wrong config or token; no point retrying every few seconds.
    if [ "$code" -eq 3 ]; then delay=120; else delay=5; fi
    sleep "$delay" &
    child=$!
    wait "$child"
    child=
done
rm -f "$PIDFILE"
