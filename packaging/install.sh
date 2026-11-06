#!/bin/sh
# soundiya-tg на колонке. Запуск от root из распакованного пакета или из /data/opt/soundiya-tg:
#   sh install.sh               установить или обновить (настройки сохраняются), запустить
#   sh install.sh setup         ввести токен бота и свой Telegram id, проверить, запустить
#   sh install.sh uninstall     удалить (config.json остаётся; --purge удаляет и его)
set -eu
SRC=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
DIR=/data/opt/soundiya-tg
INIT=/etc/init.d/S99soundiya-tg
CONFIG=$DIR/config.json

die() {
    echo "ошибка: $*" >&2
    exit 1
}
[ "$(id -u)" -eq 0 ] || die 'запусти от root на колонке'

# Корень на стоковой прошивке только для чтения: открываем его только ради init-скрипта.
WAS_RO=0
root_rw() {
    if grep -qE '^[^ ]+ / [^ ]+ ro[ ,]' /proc/mounts; then
        WAS_RO=1
        mount -o remount,rw / || die 'не удалось открыть корень на запись'
    fi
}
root_restore() {
    if [ "$WAS_RO" -eq 1 ]; then
        sync
        mount -o remount,ro / || echo 'предупреждение: корень остался открытым на запись' >&2
        WAS_RO=0
    fi
}
restore_tty() { stty echo 2>/dev/null || true; }
trap 'restore_tty; root_restore' EXIT
trap 'exit 130' INT TERM

setup() {
    [ -x "$DIR/soundiya-tg" ] || die "сначала установи: sh install.sh"
    [ -t 0 ] || die 'setup нужен в интерактивной SSH-сессии (ssh -t)'
    echo 'Токен бота от @BotFather (ввод не отображается), Enter:'
    stty -echo 2>/dev/null || true
    IFS= read -r token || token=
    restore_tty
    echo
    token=$(printf '%s' "$token" | tr -d ' \r\t')
    printf '%s\n' "$token" | grep -qE '^[0-9]+:[A-Za-z0-9_-]{30,}$' \
        || die 'это не похоже на токен бота (вид 123456:ABC...)'
    printf 'Твой числовой Telegram id (бот @userinfobot): '
    IFS= read -r user || user=
    user=$(printf '%s' "$user" | tr -d ' \r\t')
    case "$user" in
        '' | *[!0-9]*) die 'id — только цифры' ;;
    esac
    printf 'Уведомлять со звуком о каждом новом треке? [Y/n]: '
    IFS= read -r answer || answer=
    case "$answer" in
        n | N | н | Н) notify=false ;;
        *) notify=true ;;
    esac
    umask 077
    printf '{\n  "token": "%s",\n  "users": [%s],\n  "notify": %s\n}\n' "$token" "$user" "$notify" >"$CONFIG.new"
    chmod 600 "$CONFIG.new"
    if ! "$DIR/soundiya-tg" check --config "$CONFIG.new"; then
        rm -f "$CONFIG.new"
        die 'проверка не прошла, config.json не изменён'
    fi
    mv "$CONFIG.new" "$CONFIG"
    "$DIR/stop.sh" >/dev/null 2>&1 || true
    "$DIR/start.sh"
    echo
    echo 'Готово. Открой своего бота в Telegram и нажми Start (или отправь /start).'
}

action=${1:-install}
case "$action" in
    setup)
        setup
        exit 0
        ;;
    uninstall)
        [ ! -x "$DIR/stop.sh" ] || "$DIR/stop.sh" || true
        root_rw
        rm -f "$INIT"
        root_restore
        if [ "${2:-}" = --purge ]; then
            rm -rf "$DIR"
            echo 'soundiya-tg удалён вместе с настройками'
        else
            rm -f "$DIR/soundiya-tg" "$DIR/run.sh" "$DIR/start.sh" "$DIR/stop.sh" "$DIR/install.sh" "$DIR/S99soundiya-tg"
            echo "soundiya-tg удалён; настройки остались в $CONFIG (--purge удалит их)"
        fi
        exit 0
        ;;
    install) ;;
    *) die "неизвестное действие '$action'" ;;
esac

[ -f "$SRC/soundiya-tg" ] || die "рядом с install.sh нет файла soundiya-tg"
chmod 755 "$SRC/soundiya-tg"
"$SRC/soundiya-tg" version >/dev/null 2>&1 || die 'бинарник soundiya-tg не запускается на этой колонке'

mkdir -p "$DIR"
chmod 700 "$DIR"
[ ! -x "$DIR/stop.sh" ] || "$DIR/stop.sh" >/dev/null 2>&1 || true
if [ "$SRC" != "$DIR" ]; then
    for f in soundiya-tg run.sh start.sh stop.sh install.sh S99soundiya-tg; do
        cp "$SRC/$f" "$DIR/$f.new"
        chmod 755 "$DIR/$f.new"
        mv "$DIR/$f.new" "$DIR/$f"
    done
fi
root_rw
cp "$SRC/S99soundiya-tg" "$INIT.new"
chmod 755 "$INIT.new"
mv "$INIT.new" "$INIT"
root_restore

"$DIR/soundiya-tg" version
if [ -f "$CONFIG" ]; then
    "$DIR/start.sh"
else
    echo "Теперь настрой бота: sh $DIR/install.sh setup"
fi
