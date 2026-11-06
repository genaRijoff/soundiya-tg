# Установка бота soundiya-tg (новейший soundiya-tg-armv7-*.tar.gz из папки dist) на Станцию Мини 1
# по SSH (хост 'station' из ~/.ssh/config).
#   powershell -ExecutionPolicy Bypass -File .\install-soundiya-tg.ps1              установить/обновить
#   powershell -ExecutionPolicy Bypass -File .\install-soundiya-tg.ps1 -Setup       заново ввести токен
#   powershell -ExecutionPolicy Bypass -File .\install-soundiya-tg.ps1 -Uninstall   удалить
# Токен вводится прямо в SSH-сессии на колонке и не проходит через этот скрипт.
param([switch]$Setup, [switch]$Uninstall, [string]$SshHost = 'station',
      [string]$PackageDir = (Join-Path $PSScriptRoot '..\dist'))
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'station.ps1')
Initialize-Station -SshHost $SshHost
$dir = '/data/opt/soundiya-tg'

function Invoke-Setup {
    # -t: интерактивный терминал, чтобы ввод токена не отображался.
    ssh -t $SshHost "sh $dir/install.sh setup"
    if ($LASTEXITCODE -ne 0) { throw "настройка не завершена (код $LASTEXITCODE)" }
}

if ($Uninstall) { exit (Invoke-Station "sh $dir/install.sh uninstall") }
if ($Setup) { Invoke-Setup; exit 0 }

$pkg = Get-ChildItem -Path $PackageDir -Filter 'soundiya-tg-armv7-*.tar.gz' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $pkg) { throw "в $PackageDir нет soundiya-tg-armv7-*.tar.gz: сначала собери пакет" }
Write-Host "Пакет: $($pkg.Name)"
Copy-ToStation $pkg.FullName '/tmp/sdy-tg.tgz'
$code = Invoke-Station 'cd /tmp && rm -rf soundiya-tg && gzip -dc sdy-tg.tgz | tar -x && cd soundiya-tg && sha256sum -c SHA256SUMS >/dev/null && sh install.sh; r=$?; cd /tmp; rm -rf soundiya-tg sdy-tg.tgz; exit $r'
if ($code -ne 0) { throw "установка не удалась (код $code)" }

$configured = Invoke-Station "[ -f $dir/config.json ]"
if ($configured -ne 0) {
    Write-Host "`nНастройка бота: вставь токен от @BotFather и свой id от @userinfobot."
    Invoke-Setup
} else {
    $null = Invoke-Station "$dir/soundiya-tg check"
}
