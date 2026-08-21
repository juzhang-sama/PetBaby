param(
    [ValidateSet("all", "slender-success", "balanced-success", "rounded-success")]
    [string]$Fixture = "all",
    [string]$Output = "",
    [string]$BaseUrl = "http://127.0.0.1:1420",
    [string]$FixtureBase = "",
    [string]$FixtureRoot = ""
)

$ErrorActionPreference = "Stop"
$CAT_BODY_MODULE_IDS = @("body-slender-v1", "body-balanced-v1", "body-rounded-v1")
$evidenceRelative = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String("ZG9jcy/pqozor4HorrDlvZUv6K+B5o2uL+eFp+eJh+WIhui6q+ehruWumuaAp+e6ueeQhuWQiOaIkC1mYWtl"))
if ([string]::IsNullOrWhiteSpace($Output)) { $Output = $evidenceRelative }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$outputRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot $Output))
$allowedRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot $evidenceRelative))
if (-not $outputRoot.StartsWith($allowedRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -and $outputRoot -ne $allowedRoot) {
    throw "Evidence output must stay below $allowedRoot"
}
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw "node is required" }
if (-not (Get-Command python -ErrorAction SilentlyContinue)) { throw "python is required" }

$selectedModules = switch ($Fixture) {
    "slender-success" { @("body-slender-v1") }
    "balanced-success" { @("body-balanced-v1") }
    "rounded-success" { @("body-rounded-v1") }
    default { $CAT_BODY_MODULE_IDS }
}
if (-not [string]::IsNullOrWhiteSpace($FixtureBase) -and $selectedModules.Count -ne 1) {
    throw "FixtureBase can only be used with one explicit body fixture"
}

function Get-Sha256Hex([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        $sha = [Security.Cryptography.SHA256]::Create()
        try { return ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace("-", "").ToLowerInvariant() }
        finally { $sha.Dispose() }
    }
    finally { $stream.Dispose() }
}

function Write-Utf8NoBom([string]$Path, [string]$Value) {
    [IO.File]::WriteAllText($Path, $Value, [Text.UTF8Encoding]::new($false))
}

function New-BodyModuleFixture([string]$BodyModuleId, [string]$Root) {
    $source = Join-Path $repoRoot "apps/desktop/public/cat-character-modules/cat-a-live2d-v1/$BodyModuleId"
    if (-not (Test-Path -LiteralPath $source)) { throw "body module is unavailable: $BodyModuleId" }
    $destination = Join-Path $Root $BodyModuleId
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Copy-Item -Path (Join-Path $source "*") -Destination $destination -Recurse -Force

    $module = Get-Content -Raw -Encoding UTF8 (Join-Path $destination "模块.json") | ConvertFrom-Json
    if ($module.moduleId -ne $BodyModuleId) { throw "body module manifest mismatch: $BodyModuleId" }

    $profilePath = Join-Path $destination "motion-spatial-profile.json"
    Write-Utf8NoBom $profilePath ($module.motionSpatialProfile | ConvertTo-Json -Depth 16)
    $files = [Collections.Generic.List[object]]::new()
    foreach ($entry in @(
        @("moc", $module.files.moc3),
        @("model", $module.files.model3),
        @("metadata", $module.files.displayInfo),
        @("texture", $module.files.neutralTexture)
    )) {
        $path = Join-Path $destination $entry[1]
        $files.Add([ordered]@{ role = $entry[0]; relativePath = $entry[1]; sha256 = Get-Sha256Hex $path }) | Out-Null
    }
    foreach ($motionProperty in $module.motions.PSObject.Properties) {
        $path = Join-Path $destination $motionProperty.Value.relativePath
        $files.Add([ordered]@{ role = "motion"; relativePath = $motionProperty.Value.relativePath; sha256 = Get-Sha256Hex $path }) | Out-Null
    }
    $files.Add([ordered]@{ role = "motion-spatial-profile"; relativePath = "motion-spatial-profile.json"; sha256 = Get-Sha256Hex $profilePath }) | Out-Null

    $motions = [ordered]@{}
    foreach ($motion in @("breathing", "blink", "ear-twitch", "tail-idle", "pointer-focus", "pet-happy", "sleepy-yawn", "half-stand-stretch")) {
        $motions[$motion] = [ordered]@{ group = $motion; index = 0 }
    }
    $manifest = [ordered]@{
        schemaVersion = 5
        renderer = "cat-spatial-live2d-v1"
        petId = "photo-avatar-fixture-$BodyModuleId"
        variantId = "photo-avatar-fixture-$BodyModuleId"
        skeletonVersion = "cat-a-live2d-v1"
        bodyModuleId = $BodyModuleId
        modelEntry = $module.files.model3
        previewImage = $module.files.neutralTexture
        motionSpatialProfile = "motion-spatial-profile.json"
        files = $files
        motions = $motions
        parameters = [ordered]@{
            eyeOpenLeft = "ParamEyeLOpen"; eyeOpenRight = "ParamEyeROpen"
            eyeBallX = "ParamEyeBallX"; eyeBallY = "ParamEyeBallY"
            earLeft = "ParamEarL"; earRight = "ParamEarR"
            tailAngle = "ParamTailAngle"; tailCurl = "ParamTailCurl"; tailTip = "ParamTailTip"
            bodyBreath = "ParamBreath"; bodyStretch = "ParamBodyStretch"; mouthOpen = "ParamMouthOpenY"
        }
        hitAreas = [ordered]@{ body = "ArtMeshBody"; edgeTail = "ArtMeshTail" }
        edgeTailStates = [ordered]@{
            left = [ordered]@{ group = "edge-tail-left"; index = 0; tailArtMesh = "ArtMeshTail" }
            right = [ordered]@{ group = "edge-tail-right"; index = 0; tailArtMesh = "ArtMeshTail" }
            top = [ordered]@{ group = "edge-tail-top"; index = 0; tailArtMesh = "ArtMeshTail" }
            bottom = [ordered]@{ group = "edge-tail-bottom"; index = 0; tailArtMesh = "ArtMeshTail" }
        }
        license = [ordered]@{
            id = "project-owned-$BodyModuleId"; author = "PetBaby"; source = "Project-owned prebound body module"
            commercialUse = $true; redistributable = $true
        }
    }
    Write-Utf8NoBom (Join-Path $destination "manifest.json") ($manifest | ConvertTo-Json -Depth 16)
}

if (Test-Path -LiteralPath $outputRoot) { Remove-Item -LiteralPath $outputRoot -Recurse -Force }
New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null

$fixtureServer = $null
$pythonScriptPath = $null
$createdFixtureRoot = $false
$fixtureRootPath = $null
try {
    if ([string]::IsNullOrWhiteSpace($FixtureBase)) {
        if ([string]::IsNullOrWhiteSpace($FixtureRoot)) {
            $FixtureRoot = Join-Path ([IO.Path]::GetTempPath()) "desktop-pet-task4-fixtures-$PID-$([guid]::NewGuid().ToString('N'))"
            $createdFixtureRoot = $true
        }
        $fixtureRootPath = [IO.Path]::GetFullPath($FixtureRoot)
        if (Test-Path -LiteralPath $fixtureRootPath) { throw "FixtureRoot must not already exist: $fixtureRootPath" }
        New-Item -ItemType Directory -Path $fixtureRootPath | Out-Null
        foreach ($bodyModuleId in $selectedModules) { New-BodyModuleFixture $bodyModuleId $fixtureRootPath }

        $fixturePort = 18700 + ($PID % 200)
        $pythonScriptPath = Join-Path ([IO.Path]::GetTempPath()) "desktop-pet-task4-server-$PID.py"
        Write-Utf8NoBom $pythonScriptPath @"
import http.server
import os
import sys
os.chdir(sys.argv[1])
class H(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        super().end_headers()
http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[2])), H).serve_forever()
"@
        $fixtureServer = Start-Process -WindowStyle Hidden -PassThru -FilePath python -ArgumentList @($pythonScriptPath, $fixtureRootPath, $fixturePort)
        $serverDeadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            Start-Sleep -Milliseconds 100
            $listening = Get-NetTCPConnection -State Listen -LocalPort $fixturePort -ErrorAction SilentlyContinue
        } while ($null -eq $listening -and [DateTime]::UtcNow -lt $serverDeadline)
        if ($null -eq $listening) { throw "fixture server did not listen on $fixturePort" }
    }

    $nodePath = (Get-Command node -ErrorAction Stop).Source
    $playwrightCoreRoot = "D:\DevTools\npm-cache\_npx\31e32ef8478fbf80\node_modules"
    if (-not (Test-Path (Join-Path $playwrightCoreRoot "playwright-core"))) { throw "playwright-core runtime is unavailable" }
    $driverScript = Join-Path $repoRoot "scripts/照片分身动作证据驱动.mjs"
    $indexes = [Collections.Generic.List[object]]::new()
    $env:NODE_PATH = $playwrightCoreRoot
    foreach ($bodyModuleId in $selectedModules) {
        $moduleOutput = Join-Path $outputRoot $bodyModuleId
        $moduleFixtureBase = if ([string]::IsNullOrWhiteSpace($FixtureBase)) {
            "http://127.0.0.1:$fixturePort/$bodyModuleId"
        } else { $FixtureBase.TrimEnd('/') }
        & $nodePath $driverScript $moduleFixtureBase $moduleOutput $BaseUrl $bodyModuleId
        if ($LASTEXITCODE -ne 0) { throw "Chromium evidence driver failed for $bodyModuleId with exit code $LASTEXITCODE" }
        $indexPath = Join-Path $moduleOutput "证据索引.json"
        $index = Get-Content -Raw -Encoding UTF8 $indexPath | ConvertFrom-Json
        if ($index.bodyModuleId -ne $bodyModuleId -or $index.runtimeEvidence.frames.Count -ne 24) {
            throw "incomplete runtime evidence for $bodyModuleId"
        }
        $states = @($index.runtimeEvidence.interruptions | ForEach-Object { $_.state })
        if (($states -join ",") -ne "interrupted-pet,interrupted-drag") {
            throw "interruption evidence mismatch for $bodyModuleId"
        }
        $indexes.Add([ordered]@{
            bodyModuleId = $bodyModuleId
            manifestSha256 = $index.manifestSha256
            evidenceIndex = "$bodyModuleId/证据索引.json"
            frameCount = $index.runtimeEvidence.frames.Count
            interruptionStates = $states
        }) | Out-Null
    }
    Write-Utf8NoBom (Join-Path $outputRoot "证据索引.json") ([ordered]@{
        schemaVersion = 1
        generatedAt = (Get-Date).ToString("o")
        provider = "local deterministic module fixtures; no network"
        bodyModules = $indexes
        checks = [ordered]@{ allThreeBodyModules = ($indexes.Count -eq 3); framesPerModule = 24; interruptionStates = $true }
    } | ConvertTo-Json -Depth 12)
}
finally {
    Remove-Item Env:NODE_PATH -ErrorAction SilentlyContinue
    if ($null -ne $fixtureServer) { Stop-Process -Id $fixtureServer.Id -Force -ErrorAction SilentlyContinue }
    if ($null -ne $pythonScriptPath -and (Test-Path -LiteralPath $pythonScriptPath)) {
        Remove-Item -LiteralPath $pythonScriptPath -Force -ErrorAction SilentlyContinue
    }
    if ($createdFixtureRoot -and $null -ne $fixtureRootPath -and (Test-Path -LiteralPath $fixtureRootPath)) {
        Remove-Item -LiteralPath $fixtureRootPath -Recurse -Force -ErrorAction SilentlyContinue
    }
}
