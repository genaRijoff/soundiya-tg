#!/bin/sh
PIDFILE=/run/soundiya-tg.pid
touch /run/soundiya-tg.stop
if [ -f "$PIDFILE" ]; then
    pid=$(cat "$PIDFILE" 2>/dev/null)
    case "$pid" in
        '' | *[!0-9]*) ;;
        *) kill -TERM "$pid" 2>/dev/null || true ;;
    esac
fi
n=0
while [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null; do
    n=$((n + 1))
    if [ "$n" -gt 10 ]; then
        kill -KILL "$(cat "$PIDFILE")" 2>/dev/null || true
        for p in /proc/[0-9]*; do
            if [ "$(readlink "$p/exe" 2>/dev/null)" = /data/opt/soundiya-tg/soundiya-tg ]; then
                kill -KILL "${p#/proc/}" 2>/dev/null || true
            fi
        done
        break
    fi
    sleep 1
done
rm -f "$PIDFILE"
echo 'soundiya-tg stopped'
