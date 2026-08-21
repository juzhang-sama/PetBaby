param(
  [string]$SourceDir = (
    'D:\PetBabyAssets\cat-a-live2d-v1\' +
    [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('5qCH5YeG54yr')) + '\' +
    [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('MDMtQ3ViaXNt5a+85Ye6'))
  ),
  [string]$OutputDir = (Join-Path $PSScriptRoot '..\apps\desktop\public\builtin-pets\cat-a-standard-v1'),
  [string]$BodyModuleRoot = (Join-Path $PSScriptRoot '..\apps\desktop\public\cat-character-modules\cat-a-live2d-v1')
)

$ErrorActionPreference = 'Stop'
$SourceDir = [IO.Path]::GetFullPath($SourceDir)
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
$BodyModuleRoot = [IO.Path]::GetFullPath($BodyModuleRoot)

$modelName = 'cat-a-standard-v1.model3.json'
$mocName = 'cat-a-standard-v1.moc3'
$displayInfoName = 'cat-a-standard-v1.cdi3.json'
$textureName = 'cat-a-standard-v1.2048/texture_00.png'
$previewName = 'preview.png'
$profileName = 'motion-spatial-profile.json'
$bodyModuleId = 'body-balanced-v1'
$requiredMotions = @(
  'breathing', 'blink', 'ear-twitch', 'tail-idle',
  'pointer-focus', 'pet-happy', 'sleepy-yawn', 'half-stand-stretch'
)
$requiredEdges = @('left', 'right', 'top', 'bottom')
$parameterMap = [ordered]@{
  eyeOpenLeft = 'ParamEyeLOpen'
  eyeOpenRight = 'ParamEyeROpen'
  eyeBallX = 'ParamEyeBallX'
  eyeBallY = 'ParamEyeBallY'
  earLeft = 'ParamEarL'
  earRight = 'ParamEarR'
  tailAngle = 'ParamTailAngle'
  tailCurl = 'ParamTailCurl'
  tailTip = 'ParamTailTip'
  bodyBreath = 'ParamBreath'
  bodyStretch = 'ParamBodyStretch'
  mouthOpen = 'ParamMouthOpenY'
}

function Resolve-PackageReference([string]$Root, [string]$RelativePath, [string]$Label) {
  if ([string]::IsNullOrWhiteSpace($RelativePath)) { throw "$Label reference is empty" }
  $normalized = $RelativePath.Replace('\', '/')
  if ($normalized.StartsWith('/') -or $normalized.Contains(':')) {
    throw "Unsafe $Label reference: $RelativePath"
  }
  $parts = @($normalized.Split('/'))
  if ($parts.Count -eq 0 -or @($parts | Where-Object { $_ -eq '' -or $_ -eq '.' -or $_ -eq '..' }).Count -gt 0) {
    throw "Unsafe $Label reference: $RelativePath"
  }
  $absolute = [IO.Path]::GetFullPath((Join-Path $Root ($normalized.Replace('/', [IO.Path]::DirectorySeparatorChar))))
  $prefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
  if (-not $absolute.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe $Label reference: $RelativePath"
  }
  if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) {
    throw "Missing $Label reference: $RelativePath"
  }
  return [pscustomobject]@{ Relative = $normalized; Absolute = $absolute }
}

function Get-FileRole([string]$RelativePath) {
  $lower = $RelativePath.ToLowerInvariant()
  if ($lower -eq $modelName) { return 'model' }
  if ($lower -eq $previewName) { return 'preview' }
  if ($lower -eq $profileName) { return 'motion-spatial-profile' }
  if ($lower.EndsWith('.moc3')) { return 'moc' }
  if ($lower.EndsWith('.motion3.json')) { return 'motion' }
  if ($lower.EndsWith('.exp3.json')) { return 'expression' }
  if ($lower.EndsWith('.physics3.json')) { return 'physics' }
  if ($lower.EndsWith('.png')) { return 'texture' }
  return 'metadata'
}

function Get-Sha256([string]$LiteralPath) {
  $stream = [IO.File]::OpenRead($LiteralPath)
  try {
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
      return ([BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    } finally {
      $sha256.Dispose()
    }
  } finally {
    $stream.Dispose()
  }
}

function Invoke-WindowsPowerShellScript([string]$ScriptPath, [string[]]$Arguments) {
  $originalModulePath = $env:PSModulePath
  try {
    $env:PSModulePath = @(
      (Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'WindowsPowerShell\Modules')
      (Join-Path $env:ProgramFiles 'WindowsPowerShell\Modules')
      (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules')
    ) -join [IO.Path]::PathSeparator
    & powershell -NoProfile -ExecutionPolicy Bypass -File $ScriptPath @Arguments | Out-Host
    return $LASTEXITCODE
  } finally {
    $env:PSModulePath = $originalModulePath
  }
}

function Set-MotionCurveValues(
  [string]$MotionPath,
  [string]$ParameterId,
  [int[]]$ValueIndices,
  [double[]]$Values,
  [Text.Encoding]$Encoding
) {
  if ($ValueIndices.Count -ne $Values.Count) { throw "Motion calibration count mismatch: $ParameterId" }
  $motion = Get-Content -Raw -Encoding UTF8 -LiteralPath $MotionPath | ConvertFrom-Json
  $curves = @($motion.Curves | Where-Object { $_.Target -eq 'Parameter' -and $_.Id -eq $ParameterId })
  if ($curves.Count -ne 1) { throw "Motion must contain exactly one $ParameterId curve: $MotionPath" }
  $segments = @($curves[0].Segments)
  for ($index = 0; $index -lt $ValueIndices.Count; $index += 1) {
    $segmentIndex = $ValueIndices[$index]
    if ($segmentIndex -lt 0 -or $segmentIndex -ge $segments.Count) {
      throw "Motion curve shape changed for ${ParameterId}: $MotionPath"
    }
    $segments[$segmentIndex] = $Values[$index]
  }
  $curves[0].Segments = $segments
  [IO.File]::WriteAllText($MotionPath, ($motion | ConvertTo-Json -Depth 20), $Encoding)
}

if (-not (Test-Path -LiteralPath $SourceDir -PathType Container)) {
  throw "Cubism export directory not found: $SourceDir"
}
if (-not (Test-Path -LiteralPath $BodyModuleRoot -PathType Container)) {
  throw "Cat body module root not found: $BodyModuleRoot"
}

$moduleValidatorName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('6aqM6K+B54yr5ZKq5b2i5L2T5qih5Z2XLnBzMQ=='))
$moduleValidatorExitCode = Invoke-WindowsPowerShellScript `
  (Join-Path $PSScriptRoot $moduleValidatorName) `
  @('-Root', $BodyModuleRoot)
if ($moduleValidatorExitCode -ne 0) {
  throw "Cat body module validation failed with exit code $moduleValidatorExitCode"
}

$moduleDir = Join-Path $BodyModuleRoot $bodyModuleId
$moduleManifestName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('5qih5Z2XLmpzb24='))
$moduleManifestPath = Join-Path $moduleDir $moduleManifestName
if (-not (Test-Path -LiteralPath $moduleManifestPath -PathType Leaf)) {
  throw "Balanced body module manifest not found: $moduleManifestPath"
}
$moduleManifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $moduleManifestPath | ConvertFrom-Json
if ($moduleManifest.moduleId -ne $bodyModuleId) { throw "Expected body module $bodyModuleId" }
if ($moduleManifest.semanticVersion -ne 'cat-a-live2d-v1') { throw 'Balanced body module semanticVersion mismatch' }
if ($moduleManifest.readOnly -ne $true) { throw 'Balanced body module must be read-only' }

$moduleFiles = @{}
foreach ($role in @('moc3', 'model3', 'displayInfo', 'neutralTexture')) {
  $resolved = Resolve-PackageReference $moduleDir ([string]$moduleManifest.files.$role) "body module $role"
  $expectedHash = ([string]$moduleManifest.hashes.$role).ToLowerInvariant()
  if ((Get-Sha256 $resolved.Absolute) -ne $expectedHash) {
    throw "Balanced body module hash mismatch: $role"
  }
  $moduleFiles[$role] = $resolved
}
if ($moduleManifest.motionSpatialProfile.bodyModuleId -ne $bodyModuleId) {
  throw 'Balanced body module spatial profile mismatch'
}

$modelPath = Join-Path $SourceDir $modelName
$previewPath = Join-Path $SourceDir $previewName
foreach ($requiredFile in @($modelPath, $previewPath)) {
  if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
    throw "Missing exported file: $([IO.Path]::GetFileName($requiredFile))"
  }
}

$model = Get-Content -Raw -Encoding UTF8 -LiteralPath $modelPath | ConvertFrom-Json
if ($null -eq $model.FileReferences) { throw 'model3 is missing FileReferences' }
$model.FileReferences.Moc = $mocName
$model.FileReferences.Textures = @($textureName)
$model.FileReferences.DisplayInfo = $displayInfoName
$references = [System.Collections.Generic.Dictionary[string, object]]::new([StringComparer]::OrdinalIgnoreCase)
function Add-Reference([string]$Path, [string]$Label) {
  $resolved = Resolve-PackageReference $SourceDir $Path $Label
  if (-not $references.ContainsKey($resolved.Relative)) { $references.Add($resolved.Relative, $resolved) }
}

foreach ($field in @('Physics', 'Pose', 'UserData')) {
  $value = [string]$model.FileReferences.$field
  if (-not [string]::IsNullOrWhiteSpace($value)) { Add-Reference $value $field }
}

$motionMappings = [ordered]@{}
foreach ($name in $requiredMotions) {
  $entries = @($model.FileReferences.Motions.$name)
  if ($entries.Count -ne 1) { throw "Motion '$name' must contain exactly one entry" }
  $file = [string]$entries[0].File
  Add-Reference $file "motion '$name'"
  $motionMappings[$name] = [ordered]@{ group = $name; index = 0 }
}

$edgeMappings = [ordered]@{}
foreach ($edge in $requiredEdges) {
  $group = "edge-tail-$edge"
  $entries = @($model.FileReferences.Motions.$group)
  if ($entries.Count -ne 1) { throw "Edge-tail motion '$group' must contain exactly one entry" }
  Add-Reference ([string]$entries[0].File) "edge-tail motion '$group'"
  $edgeMappings[$edge] = [ordered]@{ group = $group; index = 0; tailArtMesh = 'ArtMeshTail' }
}

$hitAreaMap = @{}
foreach ($hitArea in @($model.HitAreas)) { $hitAreaMap[[string]$hitArea.Name] = [string]$hitArea.Id }
if ($hitAreaMap.body -ne 'ArtMeshBody') { throw "model3 body hit area must map to ArtMeshBody" }
if ($hitAreaMap.edgeTail -ne 'ArtMeshTail') { throw "model3 edgeTail hit area must map to ArtMeshTail" }

$displayInfo = Get-Content -Raw -Encoding UTF8 -LiteralPath $moduleFiles.displayInfo.Absolute | ConvertFrom-Json
$actualParameters = @(@($displayInfo.Parameters) | ForEach-Object { [string]$_.Id })
foreach ($parameterId in $parameterMap.Values) {
  if ($actualParameters -notcontains $parameterId) { throw "Missing required Cubism parameter: $parameterId" }
}

$parent = Split-Path -Parent $OutputDir
$leaf = Split-Path -Leaf $OutputDir
New-Item -ItemType Directory -Force -Path $parent | Out-Null
$staging = Join-Path $parent (".{0}.{1}.{2}.staging" -f $leaf, $PID, [DateTime]::UtcNow.Ticks)
$backup = Join-Path $parent (".{0}.{1}.{2}.backup" -f $leaf, $PID, [DateTime]::UtcNow.Ticks)
$committed = $false

try {
  New-Item -ItemType Directory -Path $staging | Out-Null
  $utf8 = [Text.UTF8Encoding]::new($false)
  [IO.File]::WriteAllText((Join-Path $staging $modelName), ($model | ConvertTo-Json -Depth 30), $utf8)
  Copy-Item -LiteralPath $previewPath -Destination (Join-Path $staging $previewName)
  Copy-Item -LiteralPath $moduleFiles.moc3.Absolute -Destination (Join-Path $staging $mocName)
  Copy-Item -LiteralPath $moduleFiles.displayInfo.Absolute -Destination (Join-Path $staging $displayInfoName)
  $textureDestination = Join-Path $staging ($textureName.Replace('/', [IO.Path]::DirectorySeparatorChar))
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $textureDestination) | Out-Null
  Copy-Item -LiteralPath $moduleFiles.neutralTexture.Absolute -Destination $textureDestination
  foreach ($reference in $references.Values) {
    $destination = Join-Path $staging ($reference.Relative.Replace('/', [IO.Path]::DirectorySeparatorChar))
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath $reference.Absolute -Destination $destination
  }

  $profile = $moduleManifest.motionSpatialProfile
  $breathingMotionPath = Join-Path $staging (([string]@($model.FileReferences.Motions.breathing)[0].File).Replace('/', [IO.Path]::DirectorySeparatorChar))
  Set-MotionCurveValues $breathingMotionPath 'ParamBreath' @(1, 4, 7) @(0.15, 0.15, 0.15) $utf8
  Set-MotionCurveValues $breathingMotionPath 'ParamBodyStretch' @(1, 4, 7) @(
    0,
    [double]$profile.amplitude.bodyStretch.max,
    0
  ) $utf8

  $earMotionPath = Join-Path $staging (([string]@($model.FileReferences.Motions.'ear-twitch')[0].File).Replace('/', [IO.Path]::DirectorySeparatorChar))
  $earMin = [double]$profile.amplitude.ear.min
  $earMax = [double]$profile.amplitude.ear.max
  Set-MotionCurveValues $earMotionPath 'ParamEarL' @(1, 4, 7, 10) @(0, $earMin, $earMax, 0) $utf8
  Set-MotionCurveValues $earMotionPath 'ParamEarR' @(1, 4, 7, 10) @(0, $earMax, $earMin, 0) $utf8

  $textureRepairName = [Text.Encoding]::UTF8.GetString(
    [Convert]::FromBase64String('5L+u5aSNQ3ViaXNt57q555CG6YCP5piO5a2ULnB5')
  )
  $textureRepair = Join-Path $PSScriptRoot $textureRepairName
  & python $textureRepair $textureDestination `
    --sample 1436,105,13 `
    --sample 17,1227,7 `
    --sample 246,576,7 `
    --sample 801,58,7 `
    --sample 918,1691,7 `
    --sample 892,1778,7 `
    --sample 1062,816,2 `
    --sample 752,957,3 `
    --sample 756,1192,2
  if ($LASTEXITCODE -ne 0) { throw "Cubism texture UV repair failed: $textureName" }

  [IO.File]::WriteAllText(
    (Join-Path $staging $profileName),
    ($moduleManifest.motionSpatialProfile | ConvertTo-Json -Depth 20),
    $utf8
  )

  $license = [ordered]@{
    id = 'cat-a-standard-v1-project-owned'
    author = 'PetBaby'
    source = 'Project-owned standard cat artwork and Cubism binding'
    commercialUse = $true
    redistributable = $true
  }
  [IO.File]::WriteAllText((Join-Path $staging 'license.json'), ($license | ConvertTo-Json -Depth 8), $utf8)

  $files = @(Get-ChildItem -LiteralPath $staging -Recurse -File | Sort-Object FullName | ForEach-Object {
    $relative = $_.FullName.Substring($staging.Length).TrimStart([IO.Path]::DirectorySeparatorChar).Replace([IO.Path]::DirectorySeparatorChar, '/')
    [ordered]@{
      role = Get-FileRole $relative
      relativePath = $relative
      sha256 = Get-Sha256 $_.FullName
    }
  })
  $manifest = [ordered]@{
    schemaVersion = 5
    renderer = 'cat-spatial-live2d-v1'
    petId = 'cat-a-standard-v1'
    variantId = 'standard-v1'
    skeletonVersion = 'cat-a-live2d-v1'
    bodyModuleId = $bodyModuleId
    modelEntry = $modelName
    previewImage = $previewName
    motionSpatialProfile = $profileName
    files = $files
    motions = $motionMappings
    parameters = $parameterMap
    hitAreas = [ordered]@{ body = 'ArtMeshBody'; edgeTail = 'ArtMeshTail' }
    edgeTailStates = $edgeMappings
    license = $license
  }
  [IO.File]::WriteAllText((Join-Path $staging 'manifest.json'), ($manifest | ConvertTo-Json -Depth 20), $utf8)

  $profileFile = @($manifest.files | Where-Object {
    $_.role -eq 'motion-spatial-profile' -and $_.relativePath -eq $profileName
  })
  if ($profileFile.Count -ne 1) { throw 'v5 package must list exactly one motion spatial profile' }
  foreach ($file in $manifest.files) {
    $absolute = Join-Path $staging ($file.relativePath.Replace('/', [IO.Path]::DirectorySeparatorChar))
    if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) {
      throw "v5 package file missing after staging: $($file.relativePath)"
    }
    if ((Get-Sha256 $absolute) -ne $file.sha256) {
      throw "v5 package hash mismatch after staging: $($file.relativePath)"
    }
  }
  foreach ($entry in @($modelName, $previewName, $profileName)) {
    if (@($manifest.files | Where-Object { $_.relativePath -eq $entry }).Count -ne 1) {
      throw "v5 package entry must be listed exactly once: $entry"
    }
  }

  if (Test-Path -LiteralPath $OutputDir) { Move-Item -LiteralPath $OutputDir -Destination $backup }
  try {
    Move-Item -LiteralPath $staging -Destination $OutputDir
    $committed = $true
  } catch {
    if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $OutputDir }
    throw
  }
  if (Test-Path -LiteralPath $backup) { Remove-Item -LiteralPath $backup -Recurse -Force }
  Write-Output "Built standard cat v5 package: $OutputDir"
} finally {
  if (-not $committed -and (Test-Path -LiteralPath $staging)) { Remove-Item -LiteralPath $staging -Recurse -Force }
}
