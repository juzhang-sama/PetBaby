[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$EnvFile
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Import-PhotoAvatarEnv([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "EnvFile does not exist"
    }
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        if ($line -match '^\s*$' -or $line -match '^\s*#') {
            continue
        }
        if ($line -notmatch '^\s*([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
            throw "EnvFile contains an invalid entry"
        }
        Set-Item -Path ("Env:" + $Matches[1]) -Value $Matches[2].Trim()
    }
}

Import-PhotoAvatarEnv (Resolve-Path -LiteralPath $EnvFile)
$serviceRoot = Join-Path $PSScriptRoot "..\services\appearance-generation"
$serviceRoot = (Resolve-Path -LiteralPath $serviceRoot).Path
$sourceRoot = Join-Path $serviceRoot "src"
$env:PYTHONPATH = if ($env:PYTHONPATH) {
    $sourceRoot + [IO.Path]::PathSeparator + $env:PYTHONPATH
} else {
    $sourceRoot
}

$stateDir = $env:PHOTO_AVATAR_BACKEND_STATE_DIR
if (-not $stateDir) { $stateDir = "output/photo-avatar-backend" }
if (-not [IO.Path]::IsPathRooted($stateDir)) {
    $stateDir = Join-Path $serviceRoot $stateDir
}
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
$stdout = Join-Path $stateDir "photo-avatar-backend.stdout.log"
$stderr = Join-Path $stateDir "photo-avatar-backend.stderr.log"

# 探测 Python 解释器：优先 managed python（后端依赖装在其下），回退到系统 PATH
$managedPython = Join-Path $env:USERPROFILE ".workbuddy\binaries\python\versions\3.13.12\python.exe"
$pythonExe = $null
if (Test-Path -LiteralPath $managedPython -PathType Leaf) {
    $pythonExe = $managedPython
} else {
    $resolved = Get-Command python -ErrorAction SilentlyContinue
    if ($resolved) { $pythonExe = $resolved.Source }
}
if (-not $pythonExe) {
    Write-Error "未找到 Python 解释器（后端依赖需已安装）。" -ErrorAction Continue
    exit 15
}

$process = Start-Process -FilePath $pythonExe -ArgumentList @("-m", "photo_avatar_backend.app") -WorkingDirectory $serviceRoot -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru

$hostValue = $env:PHOTO_AVATAR_BACKEND_HOST
if (-not $hostValue) { $hostValue = "127.0.0.1" }
$portValue = $env:PHOTO_AVATAR_BACKEND_PORT
if (-not $portValue) { $portValue = "8787" }
$analysisModel = $env:LK888_ANALYSIS_MODEL
if (-not $analysisModel) { $analysisModel = "gpt-4o" }
$imageModel = $env:LK888_IMAGE_MODEL
if (-not $imageModel) { $imageModel = "gpt-image-2" }

Write-Output "PID=$($process.Id) host=$hostValue port=$portValue analysisModel=$analysisModel imageModel=$imageModel stateDir=$stateDir"
