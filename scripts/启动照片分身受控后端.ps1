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

# 探测 Python 解释器：按「能否 import 后端依赖」选，不写死版本目录。
# 原因：managed python 的版本目录会被运行时升级整体替换（旧目录留下 .old.<n> 后缀），
# 依赖不会跟着迁移。写死 versions\<ver>\python.exe 会在升级后选到没有依赖的空解释器，
# 表现为后端进程秒退、healthz 永远不就绪。
$pythonExe = $null
$managedRoot = Join-Path $env:USERPROFILE ".workbuddy\binaries\python\versions"
$candidates = @()
if (Test-Path -LiteralPath $managedRoot -PathType Container) {
    $candidates += Get-ChildItem -LiteralPath $managedRoot -Directory |
        Where-Object { $_.Name -match '^\d+\.' } |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName "python.exe" }
}
$resolved = Get-Command python -ErrorAction SilentlyContinue
if ($resolved) { $candidates += $resolved.Source }
foreach ($candidate in $candidates) {
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
    # scipy 是帧序列抠像（frames/matting.py 用 ndimage）的依赖。加进探测串之前
    # 必须先把 scipy 装进「已经有 fastapi 的那个解释器」——否则探测串会跳过它，
    # 落到没有 fastapi 的 Python312 上，后端直接起不来。
    & $candidate -c "import fastapi, uvicorn, httpx, PIL, numpy, scipy" 2>$null
    if ($LASTEXITCODE -eq 0) { $pythonExe = $candidate; break }
}
if (-not $pythonExe) {
    Write-Error "未找到已安装后端依赖的 Python 解释器（需要 fastapi/uvicorn/httpx/pillow/numpy/scipy，见 services/appearance-generation/requirements.txt）。" -ErrorAction Continue
    exit 15
}
Write-Output "使用 Python 解释器：$pythonExe"

# ⚠️ Start-Process 构造子进程环境用的是**区分大小写**的字典：同名不同大小写
# （最常见的就是 http_proxy / HTTP_PROXY）会被它判成重复键，直接抛
# 「已添加项。字典中的关键字…」——2026-09-15 实测：带代理变量的环境里这条脚本
# 根本起不来后端，报错停在下面 Start-Process 那一行，看起来像 Python 坏了。
# 拉进程前先把重复的那份去掉，只保留先出现的那个名字（Windows 上大小写不敏感，安全）。
$seenEnvNames = @{}
foreach ($envName in @([Environment]::GetEnvironmentVariables('Process').Keys)) {
    $folded = $envName.ToLowerInvariant()
    if ($seenEnvNames.ContainsKey($folded)) {
        Remove-Item -LiteralPath ("Env:" + $envName) -ErrorAction SilentlyContinue
    } else {
        $seenEnvNames[$folded] = $envName
    }
}

# ⚠️ `-u` 不能去掉：`-RedirectStandardOutput` 把 stdout 变成**块缓冲(8KB)**，
# 而写实风这条链的进度日志（`[1/4] [2/4] …`）一次只写几十字节 —— 进程还在跑时
# 它们**全卡在缓冲区里不落盘**。2026-09-15 就因此误判了失败阶段
# （stdout 里只看到 `[1/4]`，以为失败在第 1 步，其实卡在提交视频）。
$process = Start-Process -FilePath $pythonExe -ArgumentList @("-u", "-m", "photo_avatar_backend.app") -WorkingDirectory $serviceRoot -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru

$hostValue = $env:PHOTO_AVATAR_BACKEND_HOST
if (-not $hostValue) { $hostValue = "127.0.0.1" }
$portValue = $env:PHOTO_AVATAR_BACKEND_PORT
if (-not $portValue) { $portValue = "8787" }
$analysisModel = $env:LK888_ANALYSIS_MODEL
if (-not $analysisModel) { $analysisModel = "gpt-4o" }
$imageModel = $env:LK888_IMAGE_MODEL
if (-not $imageModel) { $imageModel = "gpt-image-2" }

Write-Output "PID=$($process.Id) host=$hostValue port=$portValue analysisModel=$analysisModel imageModel=$imageModel stateDir=$stateDir"
