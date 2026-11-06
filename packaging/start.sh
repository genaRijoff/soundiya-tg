#!/bin/sh
DIR=/data/opt/soundiya-tg
PIDFILE=/run/soundiya-tg.pid
if [ ! -f "$DIR/config.json" ]; then
    echo "soundiya-tg is not set up yet: sh $DIR/install.sh setup" >&2
    exit 1
fi
if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null; then
    echo 'soundiya-tg is already running'
    exit 0
fi
rm -f /run/soundiya-tg.stop
nohup setsid "$DIR/run.sh" </dev/null >/dev/null 2>&1 &
n=0
while [ ! -f "$PIDFILE" ]; do
    n=$((n + 1))
    if [ "$n" -gt 20 ]; then
        echo 'soundiya-tg failed to start' >&2
        exit 1
    fi
    sleep 1
done
echo 'soundiya-tg started; log: /tmp/soundiya-tg.log'
