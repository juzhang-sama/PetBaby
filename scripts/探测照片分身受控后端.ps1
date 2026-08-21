[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$BaseUrl,

    [Parameter(Mandatory)]
    [string]$EnvFile
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http

function Read-BackendToken([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "EnvFile does not exist"
    }
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        if ($line -match '^\s*PHOTO_AVATAR_BACKEND_TOKEN=(.*)$') {
            $token = $Matches[1].Trim()
            if ($token) { return $token }
        }
    }
    throw "EnvFile does not contain PHOTO_AVATAR_BACKEND_TOKEN"
}

function Invoke-BackendRequest([System.Net.Http.HttpClient]$Client, [string]$Method, [string]$Uri, [string]$Token = "") {
    $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::new($Method.ToUpperInvariant()), $Uri)
    if ($Token) {
        $request.Headers.Authorization = [System.Net.Http.Headers.AuthenticationHeaderValue]::new("Bearer", $Token)
    }
    $response = $Client.SendAsync($request).GetAwaiter().GetResult()
    $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    return @{ Status = [int]$response.StatusCode; Body = $body }
}

$token = Read-BackendToken (Resolve-Path -LiteralPath $EnvFile)
$base = $BaseUrl.TrimEnd("/")
$client = [System.Net.Http.HttpClient]::new()
try {
    $health = Invoke-BackendRequest $client "Get" "$base/healthz"
    $unauthorized = Invoke-BackendRequest $client "Get" "$base/v1/photo-avatar/jobs/probe-unauthorized"
    $deleted = Invoke-BackendRequest $client "Delete" "$base/v1/photo-avatar/sessions/probe-empty-session" $token
} finally {
    $client.Dispose()
}

if ($health.Status -ne 200 -or $health.Body -ne '{"status":"ok"}') {
    throw "health probe failed"
}
if ($unauthorized.Status -ne 401 -or $unauthorized.Body -ne '{"code":"auth","message":"unauthorized"}') {
    throw "authorization probe failed"
}
if ($deleted.Status -ne 200 -or $deleted.Body -ne '{"backendCleanup":"deleted","upstreamCleanup":"unsupported","provider":"lk888"}') {
    throw "delete probe failed"
}

Write-Output "health=200 unauthorized=401 delete=200 backendCleanup=deleted upstreamCleanup=unsupported lk888GenerationCalls=0"
