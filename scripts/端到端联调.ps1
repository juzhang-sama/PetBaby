param(
    [switch]$SkipDesktop
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$service = Join-Path $root 'services\saas-backend'
$desktop = Join-Path $root 'apps\desktop'
$envFile = Join-Path $service '.env'

if (-not (Test-Path $envFile)) {
    Copy-Item (Join-Path $service '.env.example') $envFile
    Write-Error 'Created .env from .env.example. Fill in LK888_API_KEY and run again.'
}

$backendOut = Join-Path $env:TEMP 'desktop-pet-saas-backend.out.log'
$backendErr = Join-Path $env:TEMP 'desktop-pet-saas-backend.err.log'
$backend = Start-Process -FilePath 'python' `
    -ArgumentList '-m', 'uvicorn', 'src.app:app', '--host', '127.0.0.1', '--port', '8787' `
    -WorkingDirectory $service `
    -WindowStyle Hidden `
    -RedirectStandardOutput $backendOut `
    -RedirectStandardError $backendErr `
    -PassThru

try {
    $healthy = $false
    for ($i = 0; $i -lt 30; $i += 1) {
        try {
            $resp = Invoke-RestMethod -Uri 'http://127.0.0.1:8787/healthz' -TimeoutSec 2
            if ($resp.status -eq 'ok') {
                $healthy = $true
                break
            }
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    if (-not $healthy) {
        throw "SaaS backend did not become ready in 30s. Log: $backendErr"
    }
    Write-Host 'SaaS backend ready: http://127.0.0.1:8787'

    & (Join-Path $PSScriptRoot 'saas-smoke-check.ps1')

    if (-not $SkipDesktop) {
        Push-Location $desktop
        try {
            npm run tauri dev
        } finally {
            Pop-Location
        }
    }
} finally {
    if ($backend -and -not $backend.HasExited) {
        Stop-Process -Id $backend.Id -Force
    }
}
