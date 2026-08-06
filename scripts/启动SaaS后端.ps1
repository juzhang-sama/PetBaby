$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$service = Join-Path $root 'services\saas-backend'
$envFile = Join-Path $service '.env'

if (-not (Test-Path $envFile)) {
    Copy-Item (Join-Path $service '.env.example') $envFile
    Write-Host 'Created .env from .env.example. Fill in LK888_API_KEY and run again.'
    exit 1
}

$hasKeyInEnvFile = Select-String -Path $envFile -Pattern '^LK888_API_KEY=.+' -Quiet
if (-not $hasKeyInEnvFile -and -not $env:LK888_API_KEY) {
    Write-Warning 'LK888_API_KEY is not configured: health check works, but real generation jobs will fail.'
}

Push-Location $service
try {
    Write-Host 'SaaS backend: http://127.0.0.1:8787 (Ctrl+C to stop)'
    python -m uvicorn src.app:app --host 127.0.0.1 --port 8787
} finally {
    Pop-Location
}
