# Общие функции скриптов для Станции Мини 1: SSH (по умолчанию, хост 'station' из ~/.ssh/config)
# или USB-ADB (-Adb). Подключается из других скриптов этой папки:
#   . (Join-Path $PSScriptRoot 'station.ps1'); Initialize-Station -Adb:$Adb -SshHost $SshHost

function Initialize-Station([switch]$Adb, [string]$SshHost = 'station') {
    $script:StationAdb = [bool]$Adb
    $script:StationHost = $SshHost
    # Колонка отвечает в UTF-8; без этого вывод ssh через PowerShell читается как CP866.
    [Console]::OutputEncoding = [Text.Encoding]::UTF8
    if ($Adb) { $env:ADB_VENDOR_KEYS = "$env:LOCALAPPDATA\StationMiniRoot\work\keys\adbkey" }
}

# Выполняет команду на колонке, выводит её ответ и возвращает код завершения.
# Внутри команды лучше обходиться без двойных кавычек: Windows PowerShell 5.1 их портит.
function Invoke-Station([string]$Command) {
    if ($script:StationAdb) { adb shell $Command | Out-Host } else { ssh $script:StationHost $Command | Out-Host }
    return $LASTEXITCODE
}

# Размер файла на колонке в байтах (-1, если его нет).
function Get-StationSize([string]$Remote) {
    $size = if ($script:StationAdb) { adb shell "wc -c < $Remote 2>/dev/null" } else { ssh $script:StationHost "wc -c < $Remote 2>/dev/null" }
    $n = 0L
    if ([long]::TryParse(("$size").Trim(), [ref]$n)) { return $n } else { return -1L }
}

# SFTP-сервер SSH-плагина Homebrew отдаёт scp код 1 даже после полной передачи, поэтому успех
# проверяется сравнением размеров. Вторая попытка через 2 с — на случай занятой сессии.
function Invoke-Scp([string]$From, [string]$To, [string]$Local, [string]$Remote) {
    foreach ($attempt in 1..2) {
        scp $From $To
        if ((Test-Path $Local) -and (Get-Item $Local).Length -eq (Get-StationSize $Remote)) { return }
        if ($attempt -eq 1) { Start-Sleep -Seconds 2 }
    }
    throw "scp $From -> $To не удался; если выше «session limit reached», закрой MobaXterm"
}

function Copy-ToStation([string]$Local, [string]$Remote) {
    if ($script:StationAdb) {
        adb push $Local $Remote | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "не удалось передать $Local на колонку" }
    } else {
        # Старый файл того же размера не должен сойти за успешную передачу.
        ssh $script:StationHost "rm -f $Remote" | Out-Null
        Invoke-Scp $Local "$($script:StationHost):$Remote" $Local $Remote
    }
}

function Copy-FromStation([string]$Remote, [string]$Local) {
    if ($script:StationAdb) {
        adb pull $Remote $Local | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "не удалось забрать $Remote с колонки" }
    } else {
        Remove-Item $Local -ErrorAction SilentlyContinue
        Invoke-Scp "$($script:StationHost):$Remote" $Local $Local $Remote
    }
}

# Скрипт из этой папки на колонку: CRLF → LF (shell колонки не переносит CRLF).
function Send-ScriptToStation([string]$Name, [string]$Remote) {
    $src = Join-Path $PSScriptRoot $Name
    if (-not (Test-Path $src)) { throw "нет ${src}" }
    $tmp = Join-Path $env:TEMP (Split-Path $Name -Leaf)
    [IO.File]::WriteAllText($tmp, ([IO.File]::ReadAllText($src) -replace "`r`n", "`n"))
    try { Copy-ToStation $tmp $Remote } finally { Remove-Item $tmp -ErrorAction SilentlyContinue }
}
