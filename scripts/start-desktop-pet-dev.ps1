[CmdletBinding()]
param(
  [string]$DesktopRootOverride,
  [string]$NpmCommandOverride,
  [string]$CargoCommandOverride,
  [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$desktopRoot = if ([string]::IsNullOrWhiteSpace($DesktopRootOverride)) {
  Join-Path $repoRoot "apps/desktop"
} else {
  [System.IO.Path]::GetFullPath($DesktopRootOverride)
}
$tauriCommand = Join-Path $desktopRoot "node_modules/.bin/tauri.cmd"
# 不再校验 Live2D 运行时：该路线已废弃，前端在缺 SDK 时会自动 alias 到降级实现，
# 构建与开发都不需要它（干净克隆因此可以直接一键启动）。

$npm = if ([string]::IsNullOrWhiteSpace($NpmCommandOverride)) {
  Get-Command npm.cmd -ErrorAction SilentlyContinue
} elseif (Test-Path -LiteralPath $NpmCommandOverride -PathType Leaf) {
  Get-Command $NpmCommandOverride -ErrorAction SilentlyContinue
}
if (-not $npm) {
  Write-Error "未找到 npm。请安装包含 npm 的 Node.js 后重试。" -ErrorAction Continue
  exit 10
}
$npmCommand = $npm.Source

$cargo = if ([string]::IsNullOrWhiteSpace($CargoCommandOverride)) {
  Get-Command cargo.exe -ErrorAction SilentlyContinue
} elseif (Test-Path -LiteralPath $CargoCommandOverride -PathType Leaf) {
  Get-Command $CargoCommandOverride -ErrorAction SilentlyContinue
}
if (-not $cargo) {
  Write-Error "未找到 cargo。请安装 Rust 工具链后重试。" -ErrorAction Continue
  exit 11
}

Push-Location $desktopRoot
try {
  & $npmCommand ls --depth=0 --silent
  $npmDependenciesComplete = $LASTEXITCODE -eq 0
  if (-not $npmDependenciesComplete -or -not (Test-Path -LiteralPath $tauriCommand -PathType Leaf)) {
    Write-Host "正在安装 npm 依赖..."
    & $npmCommand install
    if ($LASTEXITCODE -ne 0) {
      $code = $LASTEXITCODE
      Write-Error "npm 依赖安装失败，退出码：$code" -ErrorAction Continue
      exit $code
    }

    & $npmCommand ls --depth=0 --silent
    $npmDependenciesComplete = $LASTEXITCODE -eq 0
    if (-not $npmDependenciesComplete -or -not (Test-Path -LiteralPath $tauriCommand -PathType Leaf)) {
      Write-Error "npm 依赖安装完成，但依赖树或本地 Tauri CLI 仍不完整。" -ErrorAction Continue
      exit 13
    }
  }

  if ($ValidateOnly) {
    Write-Host "开发环境校验通过。"
    exit 0
  }

  # 注入写实风生成后端环境变量（照片 → 桌宠这条链必需）。
  # 后端地址固定为本机 127.0.0.1:8787；token 从 services/appearance-generation/.env 读取。
  $backendEnv = Join-Path $repoRoot "services/appearance-generation/.env"
  $backendConfigured = $false
  if (Test-Path -LiteralPath $backendEnv -PathType Leaf) {
    $tokenLine = Select-String -Path $backendEnv -Pattern '^PHOTO_AVATAR_BACKEND_TOKEN=' | Select-Object -First 1
    if ($tokenLine) {
      $token = $tokenLine.Line.Substring($tokenLine.Line.IndexOf('=') + 1).Trim()
      if (-not [string]::IsNullOrWhiteSpace($token)) {
        $env:PHOTO_AVATAR_BACKEND_BASE_URL = "http://127.0.0.1:8787"
        $env:PHOTO_AVATAR_BACKEND_TOKEN = $token
        $env:PHOTO_AVATAR_ALLOW_INSECURE_LOOPBACK = "1"
        $backendConfigured = $true
      }
    }
  }
  if ($backendConfigured) {
    Write-Host "已注入照片分身受控后端环境变量（127.0.0.1:8787）。"
  } else {
    Write-Host "未找到 PHOTO_AVATAR_BACKEND_TOKEN，照片分身生成将不可用（前端会提示后端未配置）。"
  }

  # 一键启动：先启动照片分身受控后端（若未运行），等 healthz 就绪后再起桌宠窗口。
  $backendHealthUrl = "http://127.0.0.1:8787/healthz"
  $backendAlreadyRunning = $false
  try {
    $backendProbe = Invoke-WebRequest -Uri $backendHealthUrl -TimeoutSec 3 -UseBasicParsing -ErrorAction Stop
    $backendAlreadyRunning = ($backendProbe.StatusCode -eq 200)
  } catch {
    $backendAlreadyRunning = $false
  }

  if ($backendAlreadyRunning) {
    Write-Host "照片分身受控后端已在运行（127.0.0.1:8787）。"
  } elseif (Test-Path -LiteralPath $backendEnv -PathType Leaf) {
    Write-Host "正在启动照片分身受控后端..."
    $backendLauncher = Join-Path $PSScriptRoot "启动照片分身受控后端.ps1"
    & $backendLauncher -EnvFile $backendEnv

    $backendDeadline = (Get-Date).AddSeconds(30)
    $backendReady = $false
    while ((Get-Date) -lt $backendDeadline) {
      try {
        $backendProbe = Invoke-WebRequest -Uri $backendHealthUrl -TimeoutSec 2 -UseBasicParsing -ErrorAction Stop
        if ($backendProbe.StatusCode -eq 200) {
          $backendReady = $true
          break
        }
      } catch {}
      Start-Sleep -Milliseconds 500
    }
    if ($backendReady) {
      Write-Host "照片分身受控后端已就绪（127.0.0.1:8787）。"
    } else {
      Write-Host "警告：后端未在 30 秒内就绪，请检查 services/appearance-generation/output/photo-avatar-backend/photo-avatar-backend.stderr.log。"
    }
  } else {
    Write-Host "警告：未找到 services/appearance-generation/.env，后端未启动，照片生成将不可用。"
    Write-Host "      复制 services/appearance-generation/.env.example 为 .env 并填入其中的必填项即可启用。"
  }

  Write-Host "正在启动 PetBaby 前端、Rust 后端和桌宠窗口..."
  & $npmCommand run tauri -- dev
  exit $LASTEXITCODE
} finally {
  Pop-Location
}
