param(
    [Parameter(Mandatory)][string]$PhotoRoot,
    [Parameter(Mandatory)][string]$AuthorizationRecord,
    [switch]$DryRun,
    [switch]$Execute,
    [string]$SampleId,
    [ValidateRange(1, 65535)][int]$BackendPort = 8788
)

$ErrorActionPreference = "Stop"
if ($DryRun -eq $Execute) { throw "choose exactly one of DryRun or Execute" }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$photoRootPath = (Resolve-Path -LiteralPath $PhotoRoot).Path
$authorizationPath = (Resolve-Path -LiteralPath $AuthorizationRecord).Path
$evidenceRelative = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String("ZG9jcy/pqozor4HorrDlvZUv6K+B5o2uL+eFp+eJh+WIhui6q+ecn+WunjIw5qC35pys"))
$matrixName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String("5qC35pys55+p6Zi1Lmpzb24="))
$guideIndexName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String("57Si5byVLmpzb24="))
$moduleContractName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String("5qih5Z2XLmpzb24="))
$evidenceRoot = Join-Path $repoRoot $evidenceRelative
$matrixPath = Join-Path $evidenceRoot $matrixName
$bodyModuleIds = @("body-slender-v1", "body-balanced-v1", "body-rounded-v1")
$task8CommitName = "4855e9d"
$minimumChangeRatio = 0.95
$frozenMatrixSha256 = "a05a35c3b94c1881e29467f44faf725622041ef5c6db14c455f9e68a7be0f24a"

function Get-Sha256([string]$Path) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $stream = [IO.File]::OpenRead($Path)
        try { return ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace("-", "").ToLowerInvariant() }
        finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

function Assert-ExactProperties($Value, [string[]]$Expected, [string]$Label) {
    $actual = @($Value.PSObject.Properties.Name)
    $difference = @(Compare-Object -ReferenceObject $Expected -DifferenceObject $actual)
    if ($difference.Count -ne 0) { throw "$Label must contain exactly: $($Expected -join ', ')" }
}

function Get-GitValue([string[]]$Arguments) {
    $value = & git -C $repoRoot @Arguments
    if ($LASTEXITCODE -ne 0) { throw "git command failed: git $($Arguments -join ' ')" }
    return ($value -join "`n").Trim()
}

$authorizationBytes = [IO.File]::ReadAllBytes($authorizationPath)
try {
    $strictUtf8 = [Text.UTF8Encoding]::new($false, $true)
    $authorizationText = $strictUtf8.GetString($authorizationBytes)
    $authorization = $authorizationText | ConvertFrom-Json
} catch {
    throw "authorization record must be valid UTF-8 JSON"
}
$authorizationFields = @(
    "authorized",
    "material",
    "provider",
    "model",
    "sampleCount",
    "maximumAttemptsPerSample"
)
Assert-ExactProperties $authorization $authorizationFields "authorization record"
if (
    $authorization.authorized -ne $true -or
    $authorization.material -ne "non-human-pet" -or
    $authorization.provider -ne "lk888.ai" -or
    $authorization.model -ne "gpt-image-2" -or
    [int]$authorization.sampleCount -ne 20 -or
    [int]$authorization.maximumAttemptsPerSample -ne 3
) {
    throw "authorization record does not match the approved Task 9 boundary"
}

$paths = [string[]]@(Get-ChildItem -LiteralPath $photoRootPath -File -ErrorAction Stop | ForEach-Object FullName)
[Array]::Sort($paths, [StringComparer]::Ordinal)
if ($paths.Count -ne 8) { throw "PhotoRoot must contain exactly eight files" }

Add-Type -AssemblyName System.Drawing
$photos = [Collections.Generic.List[object]]::new()
for ($index = 0; $index -lt $paths.Count; $index++) {
    $path = $paths[$index]
    if ([IO.Path]::GetExtension($path).ToLowerInvariant() -notin @(".jpg", ".jpeg", ".png")) {
        throw "PhotoRoot contains a non-JPEG/PNG file"
    }
    try {
        $image = [Drawing.Image]::FromFile($path)
        try {
            if ($image.Width -le 0 -or $image.Height -le 0) { throw "empty image" }
            $width = $image.Width
            $height = $image.Height
        } finally { $image.Dispose() }
    } catch {
        throw "PhotoRoot contains an undecodable image at ordinal $($index + 1)"
    }
    $file = Get-Item -LiteralPath $path
    $photos.Add([ordered]@{
        ordinal = $index + 1
        sha256 = Get-Sha256 $path
        bytes = $file.Length
        width = $width
        height = $height
    })
}

$head = Get-GitValue @("rev-parse", "HEAD")
$task8Commit = Get-GitValue @("rev-parse", "$task8CommitName^{commit}")
& git -C $repoRoot merge-base --is-ancestor $task8Commit $head
if ($LASTEXITCODE -ne 0) { throw "HEAD must descend from the Task 8 release commit" }

$guideRoot = Join-Path $repoRoot "services/appearance-generation/src/photo_avatar_backend/assets/uv-guides"
$guideIndexPath = Join-Path $guideRoot $guideIndexName
$guideIndex = Get-Content -LiteralPath $guideIndexPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($guideIndex.schemaVersion -ne 2) { throw "UV guide index must be schema version 2" }
$moduleBindings = [Collections.Generic.List[object]]::new()
foreach ($bodyModuleId in $bodyModuleIds) {
    $guide = @($guideIndex.guides | Where-Object { $_.moduleId -eq $bodyModuleId })
    if ($guide.Count -ne 1 -or $guide[0].visualReview.status -ne "passed") {
        throw "$bodyModuleId UV guide must have one passed visual review"
    }
    $guide = $guide[0]
    $workCanvasPath = Join-Path $guideRoot $guide.workCanvasPath
    $regionMapPath = Join-Path $guideRoot $guide.regionMapPath
    $moduleContractPath = Join-Path $repoRoot "apps/desktop/public/cat-character-modules/cat-a-live2d-v1/$bodyModuleId/$moduleContractName"
    $workCanvasSha256 = Get-Sha256 $workCanvasPath
    $regionMapSha256 = Get-Sha256 $regionMapPath
    $moduleContractSha256 = Get-Sha256 $moduleContractPath
    if (
        $workCanvasSha256 -ne $guide.workCanvasSha256 -or
        $regionMapSha256 -ne $guide.regionMapSha256 -or
        $moduleContractSha256 -ne $guide.moduleContractSha256
    ) { throw "UV guide or module contract hash changed for $bodyModuleId" }
    $moduleBindings.Add([ordered]@{
        bodyModuleId = $bodyModuleId
        workCanvasSha256 = $workCanvasSha256
        regionMapSha256 = $regionMapSha256
        moduleContractSha256 = $moduleContractSha256
    })
}

$samples = [Collections.Generic.List[object]]::new()
$sequence = 1
foreach ($photoCount in @(1, 2, 4, 8)) {
    for ($rotation = 0; $rotation -lt 5; $rotation++) {
        $samplePhotos = [Collections.Generic.List[object]]::new()
        for ($offset = 0; $offset -lt $photoCount; $offset++) {
            $photo = $photos[($rotation + $offset) % $photos.Count]
            $samplePhotos.Add([ordered]@{
                ordinal = $photo.ordinal
                sha256 = $photo.sha256
            })
        }
        $samples.Add([ordered]@{
            sampleId = ("sample-{0:D2}-{1}p-{2:D2}" -f $sequence, $photoCount, ($rotation + 1))
            photoCount = $photoCount
            rotationStartOrdinal = $rotation + 1
            photos = $samplePhotos
            codeCommit = $task8Commit
            provider = "lk888.ai"
            model = "gpt-image-2"
            apiContractVersion = "lk888-media-generate-v1"
            allowedBodyModuleIds = $bodyModuleIds
            minimumChangeRatio = $minimumChangeRatio
            maximumAttempts = 3
        })
        $sequence++
    }
}

$matrix = [ordered]@{
    schemaVersion = 2
    mode = "dry-run"
    task8ReleaseCommit = $task8Commit
    codeCommit = $task8Commit
    authorizationSha256 = Get-Sha256 $authorizationPath
    authorization = [ordered]@{
        material = $authorization.material
        provider = $authorization.provider
        model = $authorization.model
        sampleCount = [int]$authorization.sampleCount
        maximumAttemptsPerSample = [int]$authorization.maximumAttemptsPerSample
    }
    moduleBindings = $moduleBindings
    photos = $photos
    samples = $samples
}

if ($DryRun) {
    New-Item -ItemType Directory -Force -Path $evidenceRoot | Out-Null
    $json = ($matrix | ConvertTo-Json -Depth 10).Replace("`r`n", "`n")
    [IO.File]::WriteAllText($matrixPath, $json + "`n", [Text.UTF8Encoding]::new($false))
}

if (-not (Test-Path -LiteralPath $matrixPath -PathType Leaf)) { throw "frozen matrix is missing" }
if ($Execute -and (Get-Sha256 $matrixPath) -ne $frozenMatrixSha256) { throw "frozen matrix hash changed" }
$written = Get-Content -LiteralPath $matrixPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ([int]$written.schemaVersion -ne 2) { throw "matrix must use schema version 2" }
if (@($written.samples).Count -ne 20) { throw "matrix must contain 20 samples" }
if (@($written.samples.sampleId | Select-Object -Unique).Count -ne 20) { throw "sample IDs must be unique" }
foreach ($photoCount in @(1, 2, 4, 8)) {
    if (@($written.samples | Where-Object { $_.photoCount -eq $photoCount }).Count -ne 5) {
        throw "matrix must contain five samples with $photoCount photos"
    }
}
$expectedPhotoHashes = @($written.photos.sha256 | Sort-Object -Unique)
$usedPhotoHashes = @($written.samples.photos.sha256 | Sort-Object -Unique)
if ($expectedPhotoHashes.Count -ne 8 -or @(Compare-Object $expectedPhotoHashes $usedPhotoHashes).Count -ne 0) {
    throw "all eight photo hashes must appear in the sample matrix"
}
foreach ($sample in $written.samples) {
    $allowed = @($sample.allowedBodyModuleIds)
    if (
        $sample.codeCommit -ne $task8Commit -or
        $allowed.Count -ne $bodyModuleIds.Count -or
        @(Compare-Object $bodyModuleIds $allowed).Count -ne 0 -or
        [double]$sample.minimumChangeRatio -ne $minimumChangeRatio
    ) {
        throw "sample matrix binding changed for $($sample.sampleId)"
    }
}
$writtenBindings = @($written.moduleBindings)
if ($writtenBindings.Count -ne $moduleBindings.Count) { throw "matrix module bindings changed" }
foreach ($binding in $moduleBindings) {
    $writtenBinding = @($writtenBindings | Where-Object { $_.bodyModuleId -eq $binding.bodyModuleId })
    if (
        $writtenBinding.Count -ne 1 -or
        $writtenBinding[0].workCanvasSha256 -ne $binding.workCanvasSha256 -or
        $writtenBinding[0].regionMapSha256 -ne $binding.regionMapSha256 -or
        $writtenBinding[0].moduleContractSha256 -ne $binding.moduleContractSha256
    ) { throw "matrix module binding changed for $($binding.bodyModuleId)" }
}

if ($DryRun) {
    Write-Output "dry-run: PASS; samples=20; distribution=1/2/4/8x5; photos=8; modules=3; network=0; matrixSha256=$(Get-Sha256 $matrixPath); matrix=$matrixPath"
    exit 0
}

if ([string]::IsNullOrWhiteSpace($SampleId)) { throw "Execute requires SampleId" }
$selected = @($written.samples | Where-Object { $_.sampleId -eq $SampleId })
if ($selected.Count -ne 1) { throw "SampleId must identify exactly one frozen sample" }
if ($written.authorizationSha256 -ne (Get-Sha256 $authorizationPath)) {
    throw "authorization record hash changed after DryRun"
}

$frozenPhotos = @($written.photos | Sort-Object ordinal)
if ($frozenPhotos.Count -ne $photos.Count) { throw "photo inventory changed after DryRun" }
for ($index = 0; $index -lt $photos.Count; $index++) {
    $actual = $photos[$index]
    $frozen = $frozenPhotos[$index]
    if (
        [int]$actual.ordinal -ne [int]$frozen.ordinal -or
        $actual.sha256 -ne $frozen.sha256 -or
        [long]$actual.bytes -ne [long]$frozen.bytes -or
        [int]$actual.width -ne [int]$frozen.width -or
        [int]$actual.height -ne [int]$frozen.height
    ) { throw "photo inventory changed after DryRun" }
}

$listener = @(Get-NetTCPConnection -State Listen -LocalAddress "127.0.0.1" -LocalPort $BackendPort -ErrorAction SilentlyContinue)
if ($listener.Count -ne 1) { throw "exactly one loopback backend listener is required" }
$backendProcess = Get-Process -Id $listener[0].OwningProcess -ErrorAction Stop
if ($backendProcess.ProcessName -ne "python") { throw "backend listener must belong to python" }

$desktopExe = Join-Path $repoRoot "apps/desktop/src-tauri/target/debug/desktop-pet.exe"
$desktopProcesses = @(Get-Process -Name "desktop-pet" -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -and [IO.Path]::GetFullPath($_.Path) -eq [IO.Path]::GetFullPath($desktopExe)
})
if ($desktopProcesses.Count -ne 1) { throw "exactly one current-worktree desktop process is required" }

$result = [ordered]@{
    status = "ready-for-ui"
    sampleId = $selected[0].sampleId
    photoCount = [int]$selected[0].photoCount
    photoOrdinals = @($selected[0].photos.ordinal)
    allowedBodyModuleIds = @($selected[0].allowedBodyModuleIds)
    maximumAttempts = [int]$selected[0].maximumAttempts
    matrixSha256 = $frozenMatrixSha256
    authorizationSha256 = $written.authorizationSha256
    codeCommit = $head
    backendPort = $BackendPort
    backendPid = $backendProcess.Id
    desktopPid = $desktopProcesses[0].Id
    networkCalls = 0
}
Write-Output ($result | ConvertTo-Json -Compress)
