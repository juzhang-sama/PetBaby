param(
  [string]$LayerSource,
  [string]$CharacterPackage
)

$ErrorActionPreference = 'Stop'

$requiredLayers = @(
  'body', 'bodyUnderTail', 'chest', 'head', 'earLeft', 'earRight',
  'eyeWhiteLeft', 'eyeWhiteRight', 'irisLeft', 'irisRight',
  'upperLidLeft', 'upperLidRight', 'lowerLidLeft', 'lowerLidRight',
  'muzzle', 'frontLegLeft', 'frontLegRight', 'tail', 'tailReserve',
  'occlusionReserve'
)

if (-not [string]::IsNullOrWhiteSpace($CharacterPackage)) {
  if (-not [string]::IsNullOrWhiteSpace($LayerSource)) { throw 'Choose either -LayerSource or -CharacterPackage' }
  $packageRoot = [IO.Path]::GetFullPath($CharacterPackage)
  $manifestPath = Join-Path $packageRoot 'manifest.json'
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Character manifest not found: $manifestPath" }
  $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath | ConvertFrom-Json
  $packageErrors = [System.Collections.Generic.List[string]]::new()
  if ($manifest.schemaVersion -ne 4) { $packageErrors.Add('schemaVersion must be 4') }
  if ($manifest.renderer -ne 'cat-live2d-v1') { $packageErrors.Add('renderer must be cat-live2d-v1') }
  if ($manifest.skeletonVersion -ne 'cat-a-live2d-v1') { $packageErrors.Add('skeletonVersion must be cat-a-live2d-v1') }
  $requiredMotions = @('breathing','blink','ear-twitch','tail-idle','pointer-focus','pet-happy','sleepy-yawn','half-stand-stretch')
  $requiredParameters = @('eyeOpenLeft','eyeOpenRight','eyeBallX','eyeBallY','earLeft','earRight','tailAngle','tailCurl','tailTip','bodyBreath','bodyStretch','mouthOpen')
  foreach ($name in $requiredMotions) { if ($null -eq $manifest.motions.$name) { $packageErrors.Add("missing motion: $name") } }
  foreach ($name in $requiredParameters) { if ([string]::IsNullOrWhiteSpace([string]$manifest.parameters.$name)) { $packageErrors.Add("missing parameter: $name") } }
  foreach ($name in @('body','edgeTail')) { if ([string]::IsNullOrWhiteSpace([string]$manifest.hitAreas.$name)) { $packageErrors.Add("missing hit area: $name") } }
  $tailMeshes = @()
  foreach ($edge in @('left','right','top','bottom')) {
    $state = $manifest.edgeTailStates.$edge
    if ($null -eq $state) { $packageErrors.Add("missing edge-tail state: $edge") }
    else { $tailMeshes += [string]$state.tailArtMesh }
  }
  if (@($tailMeshes | Select-Object -Unique).Count -ne 1) { $packageErrors.Add('all edge-tail states must reuse the same tail ArtMesh') }
  if ($manifest.license.redistributable -ne $true) { $packageErrors.Add('license must be redistributable') }
  $seen = @{}
  foreach ($file in @($manifest.files)) {
    $relative = ([string]$file.relativePath).Replace('\','/')
    if ([string]::IsNullOrWhiteSpace($relative) -or $relative.StartsWith('/') -or $relative.Contains(':') -or @($relative.Split('/') | Where-Object { $_ -eq '' -or $_ -eq '.' -or $_ -eq '..' }).Count -gt 0) {
      $packageErrors.Add("unsafe file path: $relative")
      continue
    }
    $key = $relative.ToLowerInvariant()
    if ($seen.ContainsKey($key)) { $packageErrors.Add("duplicate file path: $relative") } else { $seen[$key] = $true }
    $absolute = Join-Path $packageRoot ($relative.Replace('/', [IO.Path]::DirectorySeparatorChar))
    if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) { $packageErrors.Add("missing package file: $relative") }
    elseif ((Get-FileHash -Algorithm SHA256 -LiteralPath $absolute).Hash.ToLowerInvariant() -ne ([string]$file.sha256).ToLowerInvariant()) { $packageErrors.Add("hash mismatch: $relative") }
  }
  foreach ($entry in @([string]$manifest.modelEntry, [string]$manifest.previewImage)) {
    if (-not $seen.ContainsKey($entry.ToLowerInvariant())) { $packageErrors.Add("entry not listed in files: $entry") }
  }
  if ($packageErrors.Count -gt 0) {
    foreach ($message in $packageErrors) { [Console]::Error.WriteLine($message) }
    exit 1
  }
  Write-Output "RuntimeAssetManifestV4 valid: $manifestPath"
  exit 0
}

if ([string]::IsNullOrWhiteSpace($LayerSource)) {
  throw 'LayerSource mode requires -LayerSource <图层合同.json>'
}
if (-not (Test-Path -LiteralPath $LayerSource -PathType Leaf)) {
  throw "Layer contract not found: $LayerSource"
}

$contract = Get-Content -Raw -Encoding UTF8 -LiteralPath $LayerSource | ConvertFrom-Json
$errors = [System.Collections.Generic.List[string]]::new()
if ($contract.contractVersion -ne 'LayerContractV1') {
  $errors.Add("contractVersion must be LayerContractV1 (got '$($contract.contractVersion)')")
}
if (-not $contract.canvas -or $contract.canvas.width -le 0 -or $contract.canvas.height -le 0) {
  $errors.Add('canvas.width and canvas.height must be positive')
}
foreach ($assetField in @('sourceMaster', 'layerSource')) {
  if ([string]::IsNullOrWhiteSpace($contract.$assetField)) {
    $errors.Add("$assetField must be declared")
  } elseif (-not (Test-Path -LiteralPath $contract.$assetField -PathType Leaf)) {
    $errors.Add("$assetField not found: $($contract.$assetField)")
  }
}
$layerMap = @{}
foreach ($layer in @($contract.layers)) {
  if ($null -ne $layer.name) { $layerMap[$layer.name] = $layer }
}
foreach ($name in $requiredLayers) {
  if (-not $layerMap.ContainsKey($name)) {
    $errors.Add("missing required layer: $name")
  }
}
foreach ($name in $layerMap.Keys) {
  $layer = $layerMap[$name]
  foreach ($field in @('canvas','anchor','deformBounds','occludes','kraPath','kraLayerPath')) {
    if ($null -eq $layer.$field) { $errors.Add("layer '$name' missing field: $field") }
  }
  if ($layer.kraPath -and -not (Test-Path -LiteralPath $layer.kraPath -PathType Leaf)) {
    $errors.Add("layer '$name' kraPath not found: $($layer.kraPath)")
  }
  if ($layer.kraPath -and $contract.layerSource -and $layer.kraPath -ne $contract.layerSource) {
    $errors.Add("layer '$name' kraPath does not match layerSource")
  }
}
if ($null -eq $contract.occlusionReserve -or @($contract.occlusionReserve).Count -eq 0) {
  $errors.Add('occlusionReserve must contain at least one reserve layer')
}
if ($errors.Count -eq 0) {
  try {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($contract.layerSource)
    try {
      $entry = $archive.GetEntry('maindoc.xml')
      if ($null -eq $entry) {
        $errors.Add('layerSource KRA is missing maindoc.xml')
      } else {
        $reader = [System.IO.StreamReader]::new($entry.Open(), [System.Text.Encoding]::UTF8)
        try { [xml]$document = $reader.ReadToEnd() } finally { $reader.Dispose() }
        $actualNames = @($document.SelectNodes("//*[local-name()='layer']") | ForEach-Object { $_.GetAttribute('name') } | Where-Object { $_ })
        foreach ($name in $requiredLayers) {
          if ($actualNames -notcontains $name) {
            $errors.Add("layerSource KRA missing layer: $name")
          }
        }
        foreach ($name in $layerMap.Keys) {
          $layerPath = [string]$layerMap[$name].kraLayerPath
          $pathParts = $layerPath -split '/', 2
          if ($pathParts.Count -ne 2 -or $pathParts[1] -ne $name) {
            $errors.Add("layer '$name' kraLayerPath must end with /$name")
          }
        }
      }
    } finally { $archive.Dispose() }
  } catch {
    $errors.Add("layerSource KRA could not be read: $($_.Exception.Message)")
  }
}
if ($errors.Count -gt 0) {
  foreach ($errorMessage in $errors) {
    [Console]::Error.WriteLine($errorMessage)
  }
  exit 1
}
Write-Output "LayerContractV1 valid: $LayerSource"
exit 0
