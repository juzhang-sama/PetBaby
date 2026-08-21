param(
  [Parameter(Mandatory = $true)]
  [string]$ExportDir,
  [Parameter(Mandatory = $true)]
  [string]$PreviewSource
)

$ErrorActionPreference = 'Stop'
$ExportDir = [IO.Path]::GetFullPath($ExportDir)
$PreviewSource = [IO.Path]::GetFullPath($PreviewSource)
$modelPath = Join-Path $ExportDir 'cat-a-standard-v1.model3.json'
$displayInfoPath = Join-Path $ExportDir 'cat-a-standard-v1.cdi3.json'
$motionDir = Join-Path $ExportDir 'motions'
$previewPath = Join-Path $ExportDir 'preview.png'
$utf8 = [Text.UTF8Encoding]::new($false)

foreach ($requiredFile in @($modelPath, $displayInfoPath, $PreviewSource)) {
  if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
    throw "Missing standard cat export input: $requiredFile"
  }
}

$requiredParameters = @(
  'ParamEyeLOpen', 'ParamEyeROpen', 'ParamEyeBallX', 'ParamEyeBallY',
  'ParamEarL', 'ParamEarR', 'ParamTailAngle', 'ParamTailCurl', 'ParamTailTip',
  'ParamBreath', 'ParamBodyStretch', 'ParamMouthOpenY'
)
$displayInfo = Get-Content -Raw -Encoding UTF8 -LiteralPath $displayInfoPath | ConvertFrom-Json
$actualParameters = @($displayInfo.Parameters | ForEach-Object { [string]$_.Id })
$missingParameters = @($requiredParameters | Where-Object { $actualParameters -notcontains $_ })
if ($missingParameters.Count -gt 0) {
  throw "Missing required Cubism parameters: $($missingParameters -join ', ')"
}

function New-LinearCurve([string]$Id, [double[]]$Points) {
  if ($Points.Count -lt 4 -or $Points.Count % 2 -ne 0) {
    throw "Motion curve '$Id' needs at least two time/value points"
  }
  $segments = [System.Collections.Generic.List[double]]::new()
  $segments.Add($Points[0])
  $segments.Add($Points[1])
  for ($index = 2; $index -lt $Points.Count; $index += 2) {
    $segments.Add(0) # Linear segment.
    $segments.Add($Points[$index])
    $segments.Add($Points[$index + 1])
  }
  return [ordered]@{ Target = 'Parameter'; Id = $Id; Segments = @($segments) }
}

function New-Motion([double]$Duration, [bool]$Loop, [object[]]$Curves) {
  $segmentCount = 0
  $pointCount = 0
  foreach ($curve in $Curves) {
    $segmentCount += [math]::Floor(($curve.Segments.Count - 2) / 3)
    $pointCount += 1 + [math]::Floor(($curve.Segments.Count - 2) / 3)
  }
  return [ordered]@{
    Version = 3
    Meta = [ordered]@{
      Duration = $Duration
      Fps = 30.0
      Loop = $Loop
      AreBeziersRestricted = $true
      CurveCount = $Curves.Count
      TotalSegmentCount = $segmentCount
      TotalPointCount = $pointCount
      UserDataCount = 0
      TotalUserDataSize = 0
    }
    Curves = $Curves
  }
}

$motions = [ordered]@{
  breathing = New-Motion 4.0 $true @(
    (New-LinearCurve 'ParamBreath' @(0, 0.15, 2, 0.15, 4, 0.15)),
    (New-LinearCurve 'ParamBodyStretch' @(0, 0, 2, 1, 4, 0))
  )
  blink = New-Motion 0.32 $false @(
    (New-LinearCurve 'ParamEyeLOpen' @(0, 1, 0.12, 0, 0.2, 0, 0.32, 1)),
    (New-LinearCurve 'ParamEyeROpen' @(0, 1, 0.12, 0, 0.2, 0, 0.32, 1))
  )
  'ear-twitch' = New-Motion 0.8 $false @(
    (New-LinearCurve 'ParamEarL' @(0, 0, 0.18, -0.75, 0.42, 0.35, 0.8, 0)),
    (New-LinearCurve 'ParamEarR' @(0, 0, 0.22, 0.65, 0.48, -0.25, 0.8, 0))
  )
  'tail-idle' = New-Motion 3.2 $true @(
    (New-LinearCurve 'ParamTailAngle' @(0, -16, 1.6, 16, 3.2, -16)),
    (New-LinearCurve 'ParamTailCurl' @(0, -0.25, 1.6, 0.4, 3.2, -0.25)),
    (New-LinearCurve 'ParamTailTip' @(0, 0.35, 0.8, -0.45, 1.6, 0.25, 2.4, -0.35, 3.2, 0.35))
  )
  'pointer-focus' = New-Motion 1.2 $false @(
    (New-LinearCurve 'ParamEyeBallX' @(0, 0, 0.35, 0.65, 1.2, 0)),
    (New-LinearCurve 'ParamEyeBallY' @(0, 0, 0.35, 0.35, 1.2, 0))
  )
  'pet-happy' = New-Motion 1.8 $false @(
    (New-LinearCurve 'ParamEyeLOpen' @(0, 1, 0.5, 0.55, 1.3, 0.55, 1.8, 1)),
    (New-LinearCurve 'ParamEyeROpen' @(0, 1, 0.5, 0.55, 1.3, 0.55, 1.8, 1)),
    (New-LinearCurve 'ParamTailAngle' @(0, 0, 0.35, 18, 0.7, -14, 1.05, 16, 1.4, -10, 1.8, 0)),
    (New-LinearCurve 'ParamTailTip' @(0, 0, 0.35, -0.8, 0.7, 0.8, 1.05, -0.7, 1.8, 0))
  )
  'sleepy-yawn' = New-Motion 2.6 $false @(
    (New-LinearCurve 'ParamEyeLOpen' @(0, 1, 0.7, 0.35, 1.8, 0.25, 2.6, 1)),
    (New-LinearCurve 'ParamEyeROpen' @(0, 1, 0.7, 0.35, 1.8, 0.25, 2.6, 1)),
    (New-LinearCurve 'ParamMouthOpenY' @(0, 0, 0.8, 0.85, 1.7, 1, 2.6, 0))
  )
  'half-stand-stretch' = New-Motion 2.4 $false @(
    (New-LinearCurve 'ParamBodyStretch' @(0, 0, 0.7, 1, 1.65, 1, 2.4, 0)),
    (New-LinearCurve 'ParamBreath' @(0, 0.2, 0.9, 0.8, 1.8, 0.45, 2.4, 0.2)),
    (New-LinearCurve 'ParamTailAngle' @(0, 0, 0.8, -10, 1.7, 12, 2.4, 0))
  )
  'edge-tail-left' = New-Motion 2.8 $true @(
    (New-LinearCurve 'ParamTailAngle' @(0, -18, 1.4, 8, 2.8, -18)),
    (New-LinearCurve 'ParamTailCurl' @(0, 0.2, 1.4, 0.6, 2.8, 0.2)),
    (New-LinearCurve 'ParamTailTip' @(0, -0.5, 0.7, 0.5, 1.4, -0.35, 2.1, 0.45, 2.8, -0.5))
  )
  'edge-tail-right' = New-Motion 2.8 $true @(
    (New-LinearCurve 'ParamTailAngle' @(0, 18, 1.4, -8, 2.8, 18)),
    (New-LinearCurve 'ParamTailCurl' @(0, 0.2, 1.4, 0.6, 2.8, 0.2)),
    (New-LinearCurve 'ParamTailTip' @(0, 0.5, 0.7, -0.5, 1.4, 0.35, 2.1, -0.45, 2.8, 0.5))
  )
  'edge-tail-top' = New-Motion 2.4 $true @(
    (New-LinearCurve 'ParamTailAngle' @(0, -8, 1.2, 8, 2.4, -8)),
    (New-LinearCurve 'ParamTailCurl' @(0, 0.55, 1.2, 0.85, 2.4, 0.55)),
    (New-LinearCurve 'ParamTailTip' @(0, -0.4, 0.6, 0.45, 1.2, -0.3, 1.8, 0.4, 2.4, -0.4))
  )
  'edge-tail-bottom' = New-Motion 3.0 $true @(
    (New-LinearCurve 'ParamTailAngle' @(0, 10, 1.5, -10, 3, 10)),
    (New-LinearCurve 'ParamTailCurl' @(0, -0.15, 1.5, 0.25, 3, -0.15)),
    (New-LinearCurve 'ParamTailTip' @(0, 0.35, 0.75, -0.45, 1.5, 0.3, 2.25, -0.4, 3, 0.35))
  )
}

New-Item -ItemType Directory -Force -Path $motionDir | Out-Null
$motionReferences = [ordered]@{}
foreach ($entry in $motions.GetEnumerator()) {
  $relative = "motions/$($entry.Key).motion3.json"
  $absolute = Join-Path $ExportDir ($relative.Replace('/', [IO.Path]::DirectorySeparatorChar))
  [IO.File]::WriteAllText($absolute, ($entry.Value | ConvertTo-Json -Depth 20), $utf8)
  $motionReferences[$entry.Key] = @([ordered]@{ File = $relative })
}

$model = Get-Content -Raw -Encoding UTF8 -LiteralPath $modelPath | ConvertFrom-Json
if ($null -eq $model.FileReferences) { throw 'model3 is missing FileReferences' }
$model.FileReferences | Add-Member -Force -NotePropertyName Motions -NotePropertyValue $motionReferences
$hitAreas = @(
  [ordered]@{ Name = 'body'; Id = 'ArtMeshBody' },
  [ordered]@{ Name = 'edgeTail'; Id = 'ArtMeshTail' }
)
$model | Add-Member -Force -NotePropertyName HitAreas -NotePropertyValue $hitAreas
[IO.File]::WriteAllText($modelPath, ($model | ConvertTo-Json -Depth 20), $utf8)
Copy-Item -Force -LiteralPath $PreviewSource -Destination $previewPath

Write-Output "Generated standard cat motion resources: $motionDir"
