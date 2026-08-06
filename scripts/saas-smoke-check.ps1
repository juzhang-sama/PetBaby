$ErrorActionPreference = 'Stop'
$base = 'http://127.0.0.1:8787'

$health = Invoke-RestMethod -Uri "$base/healthz" -TimeoutSec 5
if ($health.status -ne 'ok') {
    throw "healthz abnormal: $($health.status)"
}
Write-Host 'healthz: ok'

if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
    $dump = & curl.exe -s -D - -o NUL -X OPTIONS "$base/api/v1/generations" `
        -H 'Origin: http://tauri.localhost' -H 'Access-Control-Request-Method: POST'
    $statusMatch = $dump | Select-String -Pattern '^HTTP/\S+\s+(\d+)' | Select-Object -First 1
    $status = if ($statusMatch) { [int]$statusMatch.Matches[0].Groups[1].Value } else { 0 }
    $originMatch = $dump | Select-String -Pattern '(?i)^Access-Control-Allow-Origin:\s*(.+)$' | Select-Object -First 1
    $allowOrigin = if ($originMatch) { $originMatch.Matches[0].Groups[1].Value.Trim() } else { '' }
    if ($status -ne 200 -or $allowOrigin -ne '*') {
        throw "CORS preflight failed: status=$status allow-origin=$allowOrigin"
    }
    Write-Host 'CORS preflight: ok'

    $tmpPhoto = Join-Path $env:TEMP 'desktop-pet-smoke-photo.png'
    [IO.File]::WriteAllBytes($tmpPhoto, [byte[]](0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3))
    try {
        $code = & curl.exe -s -o NUL -w '%{http_code}' -F 'species=bird' -F "photo=@$tmpPhoto;type=image/png" "$base/api/v1/generations"
        if ($code -ne '422') {
            throw "Invalid species should return 422, got $code"
        }
        Write-Host 'Input validation (422): ok'
    } finally {
        Remove-Item -LiteralPath $tmpPhoto -ErrorAction SilentlyContinue
    }
} else {
    Write-Warning 'curl.exe not found; skipped input validation smoke.'
}

$missingStatus = 0
try {
    Invoke-WebRequest -Uri "$base/api/v1/generations/nope" -TimeoutSec 5 | Out-Null
} catch {
    if ($_.Exception.Response) {
        $missingStatus = [int]$_.Exception.Response.StatusCode
    }
}
if ($missingStatus -ne 404) {
    throw "Missing job should return 404, got $missingStatus"
}
Write-Host 'Missing job (404): ok'

Write-Host 'SaaS smoke check passed.'
