# Установка своей сборки soundiya (новейший soundiya-mini1-*.tar.gz из папки dist) на Станцию
# Мини 1 и возврат на официальную версию. По умолчанию через SSH (хост 'station'), -Adb — по USB.
#   powershell -ExecutionPolicy Bypass -File .\install-soundiya-build.ps1            поставить сборку
#   powershell -ExecutionPolicy Bypass -File .\install-soundiya-build.ps1 -Restore   официальная версия
#                                                                                    + автообновление
# Перед установкой автообновление останавливается и убирается из автозапуска: иначе оно вернуло бы
# официальную версию. -Restore возвращает и официальную версию, и автозапуск автообновления.
param([switch]$Restore, [switch]$Adb, [string]$SshHost = 'station',
      [string]$PackageDir = (Join-Path $PSScriptRoot '..\dist'))
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'station.ps1')
Initialize-Station -Adb:$Adb -SshHost $SshHost
$au = '/data/opt/soundiya-autoupdate'

if ($Restore) {
    Write-Host 'Возврат на официальную версию и включение автообновления...'
    $code = Invoke-Station ('if [ ! -e /etc/init.d/S98soundiya-update ] && [ -f {0}/S98soundiya-update.off ]; then mount -o remount,rw / && cp {0}/S98soundiya-update.off /etc/init.d/S98soundiya-update && chmod 755 /etc/init.d/S98soundiya-update; mount -o remount,ro /; fi; /etc/init.d/S98soundiya-update start >/dev/null 2>&1; sh {0}/soundiya-autoupdate.sh once' -f $au)
    exit $code
}

$pkg = Get-ChildItem -Path $PackageDir -Filter 'soundiya-mini1-*.tar.gz' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $pkg) { throw "в $PackageDir нет soundiya-mini1-*.tar.gz: сначала собери пакет" }
Write-Host "Пакет: $($pkg.Name)"

# Автообновление не только останавливается, но и убирается из автозапуска (init-скрипт уходит
# в {0}/S98soundiya-update.off): иначе после перезагрузки оно вернуло бы официальную версию.
$null = Invoke-Station ('if [ -e /etc/init.d/S98soundiya-update ]; then /etc/init.d/S98soundiya-update stop >/dev/null 2>&1; mount -o remount,rw / && mv /etc/init.d/S98soundiya-update {0}/S98soundiya-update.off; mount -o remount,ro /; fi; true' -f $au)
Copy-ToStation $pkg.FullName '/tmp/sdy-build.tgz'
$code = Invoke-Station 'cd /tmp && rm -rf soundiya-armv7 && gzip -dc sdy-build.tgz | tar -x && cd soundiya-armv7 && sha256sum -c SHA256SUMS && sh install.sh >/dev/null; r=$?; cd /tmp; rm -rf soundiya-armv7 sdy-build.tgz; exit $r'
if ($code -ne 0) { throw "установка не удалась (код $code): пакет повреждён или install.sh завершился с ошибкой" }
Write-Host 'Установлено.'
Start-Sleep -Seconds 3
$null = Invoke-Station 'sha256sum /data/opt/soundiya/soundiya | cut -c1-12; soundiya tone; soundiya status'
# Без двойных кавычек и кириллицы внутри команды: Windows PowerShell 5.1 портит и то, и другое.
$null = Invoke-Station 'ls /etc/init.d/ | grep -q S98soundiya-update && echo autoupdate: ON || echo autoupdate: OFF; grep -q '' / ubifs ro'' /proc/mounts && echo root: ro || echo root: rw'
Write-Host "`nАвтообновление выключено и после перезагрузки не вернётся. Вернуть официальную версию и автообновление: -Restore"
