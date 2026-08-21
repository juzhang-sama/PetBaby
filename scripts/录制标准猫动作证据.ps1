param(
    [string]$BaseUrl = "http://localhost:1420",
    [string]$OutputRoot = "",
    [string[]]$Motion = @(),
    [switch]$IncludeInterruptions
)

$ErrorActionPreference = "Stop"
$sessionName = "cat-motion-evidence-$PID"
$allowedMotions = @(
    "breathing",
    "blink",
    "ear-twitch",
    "tail-idle",
    "pointer-focus",
    "pet-happy",
    "sleepy-yawn",
    "half-stand-stretch"
)
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = [Text.Encoding]::UTF8.GetString(
        [Convert]::FromBase64String("ZG9jcy/pqozor4HorrDlvZUv6K+B5o2uL+agh+WHhueMq+WFq+e7hOWKqOS9nA==")
    )
}
if ($Motion.Count -eq 0) {
    $Motion = $allowedMotions
}
$durationMs = @{
    "breathing" = 4000
    "blink" = 220
    "ear-twitch" = 800
    "tail-idle" = 3200
    "pointer-focus" = 1200
    "pet-happy" = 1800
    "sleepy-yawn" = 2600
    "half-stand-stretch" = 2400
}

foreach ($name in $Motion) {
    if ($name -notin $allowedMotions) {
        throw "Unsupported standard cat motion: $name"
    }
}

if (-not (Get-Command npx -ErrorAction SilentlyContinue)) {
    throw "npx is required to invoke Playwright CLI"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$resolvedOutput = Join-Path $repoRoot $OutputRoot
New-Item -ItemType Directory -Force -Path $resolvedOutput | Out-Null
$selectedMotionNames = [System.Collections.Generic.HashSet[string]]::new(
    [string[]]$Motion,
    [StringComparer]::Ordinal
)
foreach ($name in $Motion) {
    $motionOutput = Join-Path $resolvedOutput $name
    if (Test-Path -LiteralPath $motionOutput) {
        Remove-Item -LiteralPath $motionOutput -Recurse -Force
    }
}
$manifestPath = Join-Path $repoRoot "apps/desktop/public/builtin-pets/cat-a-standard-v1/manifest.json"
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$metadata = [System.Collections.Generic.List[object]]::new()
$totalEntries = $Motion.Count * 8
if ($IncludeInterruptions) {
    $totalEntries += 2
}

function Invoke-PlaywrightCli {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$CliArguments)
    $output = & npx --yes --package "@playwright/cli" playwright-cli "-s=$sessionName" @CliArguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Playwright CLI failed: $($CliArguments -join ' ')`n$($output -join "`n")"
    }
    return ($output -join "`n")
}

function Read-EvidenceState {
    $expression = 'async (page) => { const selector = String.fromCharCode(109, 97, 105, 110); await page.waitForFunction(value => document.querySelector(value)?.dataset.evidenceFrozen === String(1), selector); return page.locator(selector).evaluate(el => ({ state: el.dataset.catMotionEvidence, motion: el.dataset.evidenceMotion, phase: el.dataset.evidencePhase, atMs: el.dataset.evidenceAtMs || null, fps: Number(el.dataset.evidenceFps || 0), frozen:Number(el.dataset.evidenceFrozen) === 1 })); }'
    $raw = Invoke-PlaywrightCli --json run-code $expression
    $outer = $raw | ConvertFrom-Json
    return $outer.result | ConvertFrom-Json
}

function Capture-Evidence {
    param(
        [string]$MotionName,
        [string]$Phase,
        [string]$FileName,
        [Nullable[int]]$AtMs = $null,
        [string]$Trigger
    )
    $query = "catMotionEvidence=1&motion=$MotionName&phase=$Phase"
    if ($null -ne $AtMs) {
        $query += "&atMs=$AtMs"
    }
    $url = "$BaseUrl/?$query"
    Invoke-PlaywrightCli goto $url | Out-Null
    $state = Read-EvidenceState
    if ($state.motion -ne $MotionName) {
        throw "Evidence motion mismatch: expected $MotionName, got $($state.motion)"
    }
    $expectedState = switch ($Phase) {
        "neutral" { "ready" }
        "peak" { "peak" }
        "fallback" { "fallback" }
        "frame" { "frame" }
        "interrupt-pet" { "interrupted-pet" }
        "interrupt-drag" { "interrupted-drag" }
    }
    if ($state.state -ne $expectedState) {
        throw "Evidence runtime state mismatch: expected $expectedState, got $($state.state)"
    }
    if (-not $state.frozen) {
        throw "Evidence frame was not frozen before capture: $MotionName/$Phase"
    }
    if ($Phase -eq "frame" -and [int]$state.atMs -ne [int]$AtMs) {
        throw "Evidence frame offset mismatch: expected $AtMs, got $($state.atMs)"
    }
    $targetPath = Join-Path (Join-Path $resolvedOutput $MotionName) $FileName
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $targetPath) | Out-Null
    Invoke-PlaywrightCli screenshot --filename $targetPath | Out-Null
    if (-not (Test-Path -LiteralPath $targetPath)) {
        throw "Evidence screenshot was not generated: $targetPath"
    }
    $metadata.Add([ordered]@{
        motion = $MotionName
        phase = $Phase
        atMs = if ($null -eq $state.atMs) { $null } else { [int]$state.atMs }
        trigger = $Trigger
        runtimeState = $state.state
        fps = [double]$state.fps
        frozen = [bool]$state.frozen
        file = "$MotionName/$($FileName.Replace('\', '/'))"
    })
    Write-Output ("[{0}/{1}] {2} {3} FPS={4}" -f ($metadata.Count), $totalEntries, $MotionName, $Phase, $state.fps)
}

try {
    Invoke-PlaywrightCli open "about:blank" | Out-Null
    Invoke-PlaywrightCli resize 420 520 | Out-Null

    foreach ($motionName in $Motion) {
        Capture-Evidence -MotionName $motionName -Phase "neutral" -FileName "00-neutral.png" -Trigger "fixed neutral first frame"
        Capture-Evidence -MotionName $motionName -Phase "peak" -FileName "01-peak.png" -Trigger "direct action at fixed peak"
        Capture-Evidence -MotionName $motionName -Phase "fallback" -FileName "02-fallback.png" -Trigger "completed action returns to breathing idle"

        $duration = [int]$durationMs[$motionName]
        for ($index = 0; $index -lt 5; $index += 1) {
            $atMs = [int][math]::Round(($duration - 1) * $index / 4)
            Capture-Evidence -MotionName $motionName -Phase "frame" -AtMs $atMs -FileName ("sequence/{0:D2}-{1:D4}ms.png" -f $index, $atMs) -Trigger "fixed five-frame sequence"
        }
    }

    if ($IncludeInterruptions) {
        Capture-Evidence -MotionName "half-stand-stretch" -Phase "interrupt-pet" -FileName "03-interrupt-pet.png" -Trigger "user pet interrupts autonomous stretch"
        Capture-Evidence -MotionName "sleepy-yawn" -Phase "interrupt-drag" -FileName "03-interrupt-drag.png" -Trigger "user drag interrupts autonomous yawn"
    }

    $result = [ordered]@{
        schemaVersion = 1
        generatedAt = (Get-Date).ToString("o")
        petId = $manifest.petId
        packageSchemaVersion = $manifest.schemaVersion
        skeletonVersion = $manifest.skeletonVersion
        modelSha256 = ($manifest.files | Where-Object role -eq "moc").sha256
        browser = "Playwright Chromium"
        viewport = @{ width = 420; height = 520 }
        entries = $metadata
    }
    $indexName = [Text.Encoding]::UTF8.GetString(
        [Convert]::FromBase64String("6K+B5o2u57Si5byVLmpzb24=")
    )
    $indexPath = Join-Path $resolvedOutput $indexName
    $existing = @()
    if (Test-Path -LiteralPath $indexPath) {
        $existing = @((Get-Content -Raw -LiteralPath $indexPath | ConvertFrom-Json).entries)
    }
    $byFile = @{}
    foreach ($entry in $existing) {
        if (-not $selectedMotionNames.Contains([string]$entry.motion)) {
            $byFile[$entry.file] = $entry
        }
    }
    foreach ($entry in $metadata) { $byFile[$entry.file] = $entry }
    $result.entries = @($byFile.Values | Sort-Object file)
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $indexPath -Encoding utf8
    Write-Output "Evidence recording complete: $resolvedOutput"
}
finally {
    try { Invoke-PlaywrightCli close | Out-Null } catch { Write-Warning $_ }
}
