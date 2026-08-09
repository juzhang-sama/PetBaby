param(
  [string]$SourceDir = 'D:\PetBabyAssets\first-pet\06-runtime-export\pet-live2d-v1',
  [string]$OutputDir = (Join-Path $PSScriptRoot '..\apps\desktop\public\builtin-pets\pet-live2d-v1'),
  [string]$LicenseDir = 'D:\PetBabyAssets\first-pet\07-license-record'
)

$ErrorActionPreference = 'Stop'
$SourceDir = [IO.Path]::GetFullPath($SourceDir)
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
$LicenseDir = [IO.Path]::GetFullPath($LicenseDir)

$modelName = 'pet-live2d-v1.model3.json'
$mocName = 'pet-live2d-v1.moc3'
$previewName = 'preview.png'
$required = @($modelName, $mocName, $previewName)
foreach ($name in $required) {
  if (-not (Test-Path -LiteralPath (Join-Path $SourceDir $name) -PathType Leaf)) {
    throw "Missing exported file: $name"
  }
}

$modelJson = Get-Content -LiteralPath (Join-Path $SourceDir $modelName) -Raw -Encoding UTF8 | ConvertFrom-Json
if ($modelJson.FileReferences.Moc -ne $mocName) {
  throw "Unexpected Moc reference in model3: $($modelJson.FileReferences.Moc)"
}
$texturePaths = @($modelJson.FileReferences.Textures)
if ($texturePaths.Count -eq 0) {
  throw 'model3 does not declare a texture'
}
foreach ($texturePath in $texturePaths) {
  $normalizedTexturePath = ([string]$texturePath).Replace('/', [string][IO.Path]::DirectorySeparatorChar)
  if ($normalizedTexturePath.Contains('..') -or [IO.Path]::IsPathRooted($normalizedTexturePath)) {
    throw "Unsafe texture path in model3: $texturePath"
  }
  if (-not (Test-Path -LiteralPath (Join-Path $SourceDir $normalizedTexturePath) -PathType Leaf)) {
    throw "Missing texture referenced by model3: $texturePath"
  }
}

$allowedSuffixes = @('.moc3', '.png', '.model3.json', '.cdi3.json', '.physics3.json', '.pose3.json', '.userdata3.json')
$sourceFiles = @(Get-ChildItem -LiteralPath $SourceDir -Recurse -File | Where-Object {
  $lowerName = $_.Name.ToLowerInvariant()
  @($allowedSuffixes | Where-Object { $lowerName.EndsWith($_) }).Count -gt 0
} | Sort-Object FullName)

New-Item -ItemType Directory -Force -Path $OutputDir, $LicenseDir | Out-Null
$copiedRelativePaths = New-Object System.Collections.Generic.List[string]
foreach ($sourceFile in $sourceFiles) {
  $relativePath = $sourceFile.FullName.Substring($SourceDir.Length).TrimStart([IO.Path]::DirectorySeparatorChar)
  $destination = Join-Path $OutputDir $relativePath
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
  Copy-Item -LiteralPath $sourceFile.FullName -Destination $destination -Force
  $copiedRelativePaths.Add($relativePath.Replace([IO.Path]::DirectorySeparatorChar, [char]'/'))
}

$licenseSource = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('55So5oi36Ieq5pyJ5a6g54mp54Wn54mH5LiO55So5oi35ZyoIENoYXRHUFQg5Lit55Sf5oiQ5bm256Gu6K6k55qE6KGN55Sf5Zu+5YOP'))
$license = [ordered]@{
  modelId = 'pet-live2d-v1'
  author = 'juzhang-sama'
  source = $licenseSource
  commercialUse = $true
  redistribution = $true
  reviewedAt = (Get-Date).ToString('yyyy-MM-dd')
}
$utf8 = New-Object Text.UTF8Encoding($false)
$licenseJson = $license | ConvertTo-Json -Depth 10
$licenseName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('6K645Y+v6K+BLmpzb24='))
$licenseRecordName = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('5a6g54mp5qih5Z6L5o6I5p2D6K6w5b2VLmpzb24='))
$licensePath = Join-Path $OutputDir $licenseName
[IO.File]::WriteAllText($licensePath, $licenseJson, $utf8)
[IO.File]::WriteAllText((Join-Path $LicenseDir $licenseRecordName), $licenseJson, $utf8)
$copiedRelativePaths.Add($licenseName)

function Get-Role([string]$Path) {
  $lower = $Path.ToLowerInvariant()
  if ($Path -eq $licenseName) { return 'license' }
  if ($lower.EndsWith('.model3.json')) { return 'model' }
  if ($lower.EndsWith('.moc3')) { return 'moc' }
  if ($lower.EndsWith('preview.png')) { return 'preview' }
  if ($lower.EndsWith('.png')) { return 'texture' }
  return 'metadata'
}

$files = @($copiedRelativePaths | Sort-Object | ForEach-Object {
  $relativePath = $_
  $absolutePath = Join-Path $OutputDir $relativePath.Replace('/', [string][IO.Path]::DirectorySeparatorChar)
  [ordered]@{
    role = Get-Role $relativePath
    relativePath = $relativePath
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $absolutePath).Hash.ToLowerInvariant()
  }
})

$manifest = [ordered]@{
  schemaVersion = 2
  renderer = 'live2d-v1'
  petId = 'pet-live2d-v1'
  variantId = 'front-sitting-motion-v1'
  modelEntry = $modelName
  previewImage = $previewName
  files = $files
  semantics = [ordered]@{
    motions = [ordered]@{}
    expressions = [ordered]@{}
    hitAreas = [ordered]@{}
    parameters = [ordered]@{
      bodyBreath = 'ParamBreath'
      bodySway = 'ParamBodyAngleX'
    }
  }
  license = [ordered]@{
    id = 'pet-live2d-v1'
    author = $license.author
    source = $license.source
    commercialUse = $license.commercialUse
    redistributable = $license.redistribution
  }
}
[IO.File]::WriteAllText(
  (Join-Path $OutputDir 'manifest.json'),
  ($manifest | ConvertTo-Json -Depth 12),
  $utf8
)

Write-Output "Built Live2D pet package: $OutputDir"
