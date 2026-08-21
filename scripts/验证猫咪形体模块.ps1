param(
  [string]$Root = (Join-Path $PSScriptRoot '..\apps\desktop\public\cat-character-modules\cat-a-live2d-v1')
)

$ErrorActionPreference = 'Stop'
$expectedModuleIds = @('body-slender-v1', 'body-balanced-v1', 'body-rounded-v1')
$expectedCompatibility = [ordered]@{
  face = @('face-standard-v1')
  ears = @('ears-independent-v1')
  eyes = @('eyes-independent-v1')
  tail = @('tail-independent-v1')
}
$requiredParameters = @(
  'ParamEyeLOpen', 'ParamEyeROpen', 'ParamEarL', 'ParamEarR',
  'ParamTailAngle', 'ParamTailCurl', 'ParamTailTip', 'ParamBreath', 'ParamBodyStretch'
)
$fileRoles = @('moc3', 'model3', 'displayInfo', 'neutralTexture')
$requiredMotions = @(
  'breathing', 'blink', 'ear-twitch', 'tail-idle', 'pointer-focus', 'pet-happy',
  'sleepy-yawn', 'half-stand-stretch', 'edge-tail-left', 'edge-tail-right',
  'edge-tail-top', 'edge-tail-bottom'
)
$amplitudeSemantics = @('breath', 'blink', 'ear', 'tailAngle', 'tailCurl', 'tailTip', 'bodyStretch')
$issues = [System.Collections.Generic.List[string]]::new()
$profiles = @{}
$mocHashes = @{}

function Add-Issue([string]$Message) {
  $script:issues.Add($Message)
}

function Get-Properties($Value) {
  if ($null -eq $Value) { return @() }
  return @($Value.PSObject.Properties.Name)
}

function Get-Value($Value, [string]$Name) {
  if ($null -eq $Value) { return $null }
  $property = $Value.PSObject.Properties[$Name]
  if ($null -eq $property) { return $null }
  return $property.Value
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

function Test-ExactProperties($Value, [string]$Path, [string[]]$Expected) {
  if ($null -eq $Value -or $Value -isnot [psobject]) {
    Add-Issue "$Path must be an object"
    return $false
  }
  $actual = @(Get-Properties $Value)
  foreach ($name in $Expected) {
    if ($actual -notcontains $name) { Add-Issue "$Path missing $name" }
  }
  foreach ($name in $actual) {
    if ($Expected -notcontains $name) { Add-Issue "$Path has unknown field $name" }
  }
  return $true
}

function Test-NormalizedNumber($Value, [string]$Path) {
  if ($Value -isnot [ValueType] -or $Value -is [bool]) {
    Add-Issue "$Path must be finite"
    return $false
  }
  $number = [double]$Value
  if ([double]::IsNaN($number) -or [double]::IsInfinity($number)) {
    Add-Issue "$Path must be finite"
    return $false
  }
  if ($number -lt 0 -or $number -gt 1) {
    Add-Issue "$Path must be within [0, 1] (actual $number)"
    return $false
  }
  return $true
}

function Test-Point($Value, [string]$Path) {
  Test-ExactProperties $Value $Path @('x', 'y') | Out-Null
  $xValid = Test-NormalizedNumber (Get-Value $Value 'x') "$Path.x"
  $yValid = Test-NormalizedNumber (Get-Value $Value 'y') "$Path.y"
  return $xValid -and $yValid
}

function Test-Rect($Value, [string]$Path) {
  Test-ExactProperties $Value $Path @('left', 'top', 'right', 'bottom') | Out-Null
  $valid = $true
  foreach ($field in @('left', 'top', 'right', 'bottom')) {
    if (-not (Test-NormalizedNumber (Get-Value $Value $field) "$Path.$field")) { $valid = $false }
  }
  if ($valid) {
    if ([double]$Value.left -ge [double]$Value.right -or [double]$Value.top -ge [double]$Value.bottom) {
      Add-Issue "$Path must have positive area"
      return $false
    }
  }
  return $valid
}

function Test-PointInside($Point, $Rect, [string]$PointPath, [string]$RectPath) {
  if (
    [double]$Point.x -lt [double]$Rect.left -or [double]$Point.x -gt [double]$Rect.right -or
    [double]$Point.y -lt [double]$Rect.top -or [double]$Point.y -gt [double]$Rect.bottom
  ) { Add-Issue "$PointPath must remain inside $RectPath" }
}

function Test-RectInside($Inner, $Outer, [string]$InnerPath, [string]$OuterPath) {
  if (
    [double]$Inner.left -lt [double]$Outer.left -or [double]$Inner.top -lt [double]$Outer.top -or
    [double]$Inner.right -gt [double]$Outer.right -or [double]$Inner.bottom -gt [double]$Outer.bottom
  ) { Add-Issue "$InnerPath must remain inside $OuterPath" }
}

function Test-Profile($Profile, [string]$ModuleId) {
  $path = "$ModuleId.motionSpatialProfile"
  $rootFields = @(
    'schemaVersion', 'bodyModuleId', 'canvas', 'alphaBounds', 'faceSafeZone', 'eyes',
    'earRoots', 'breathZone', 'stretchAxis', 'swayPivot', 'tailRoot', 'edgeTailBounds', 'amplitude'
  )
  Test-ExactProperties $Profile $path $rootFields | Out-Null
  if ((Get-Value $Profile 'schemaVersion') -ne 1) { Add-Issue "$path.schemaVersion must be 1" }
  if ((Get-Value $Profile 'bodyModuleId') -ne $ModuleId) {
    Add-Issue "$path.bodyModuleId expected $ModuleId but found $(Get-Value $Profile 'bodyModuleId')"
  }

  $canvas = Get-Value $Profile 'canvas'
  Test-ExactProperties $canvas "$path.canvas" @('width', 'height') | Out-Null
  foreach ($field in @('width', 'height')) {
    $value = Get-Value $canvas $field
    if ($value -isnot [ValueType] -or [double]$value -le 0 -or [double]$value % 1 -ne 0) {
      Add-Issue "$path.canvas.$field must be a positive integer"
    }
  }

  $alpha = Get-Value $Profile 'alphaBounds'
  $face = Get-Value $Profile 'faceSafeZone'
  $breath = Get-Value $Profile 'breathZone'
  $edgeTail = Get-Value $Profile 'edgeTailBounds'
  $alphaValid = Test-Rect $alpha "$path.alphaBounds"
  $faceValid = Test-Rect $face "$path.faceSafeZone"
  $breathValid = Test-Rect $breath "$path.breathZone"
  $edgeTailValid = Test-Rect $edgeTail "$path.edgeTailBounds"
  if ($alphaValid -and $faceValid) { Test-RectInside $face $alpha "$path.faceSafeZone" "$path.alphaBounds" }
  if ($alphaValid -and $breathValid) { Test-RectInside $breath $alpha "$path.breathZone" "$path.alphaBounds" }
  if ($alphaValid -and $edgeTailValid) { Test-RectInside $edgeTail $alpha "$path.edgeTailBounds" "$path.alphaBounds" }

  $eyes = Get-Value $Profile 'eyes'
  Test-ExactProperties $eyes "$path.eyes" @('left', 'right') | Out-Null
  foreach ($side in @('left', 'right')) {
    $eye = Get-Value $eyes $side
    Test-ExactProperties $eye "$path.eyes.$side" @('center', 'bounds') | Out-Null
    $center = Get-Value $eye 'center'
    $bounds = Get-Value $eye 'bounds'
    $centerValid = Test-Point $center "$path.eyes.$side.center"
    $boundsValid = Test-Rect $bounds "$path.eyes.$side.bounds"
    if ($boundsValid -and $faceValid) { Test-RectInside $bounds $face "$path.eyes.$side.bounds" "$path.faceSafeZone" }
    if ($centerValid -and $boundsValid) { Test-PointInside $center $bounds "$path.eyes.$side.center" "$path.eyes.$side.bounds" }
  }
  if ($null -ne $eyes.left -and $null -ne $eyes.right -and [double]$eyes.left.center.x -ge [double]$eyes.right.center.x) {
    Add-Issue "$path eyes must preserve left-to-right ordering"
  }

  $earRoots = Get-Value $Profile 'earRoots'
  Test-ExactProperties $earRoots "$path.earRoots" @('left', 'right') | Out-Null
  foreach ($side in @('left', 'right')) {
    $root = Get-Value $earRoots $side
    if ((Test-Point $root "$path.earRoots.$side") -and $alphaValid) {
      Test-PointInside $root $alpha "$path.earRoots.$side" "$path.alphaBounds"
      if ($faceValid -and [double]$root.y -gt [double]$face.bottom) {
        Add-Issue "$path.earRoots.$side must remain in the subject upper region"
      }
    }
  }
  if ($null -ne $earRoots.left -and $null -ne $earRoots.right -and [double]$earRoots.left.x -ge [double]$earRoots.right.x) {
    Add-Issue "$path earRoots must preserve left-to-right ordering"
  }

  $stretch = Get-Value $Profile 'stretchAxis'
  Test-ExactProperties $stretch "$path.stretchAxis" @('origin', 'direction') | Out-Null
  $origin = Get-Value $stretch 'origin'
  $direction = Get-Value $stretch 'direction'
  if ((Test-Point $origin "$path.stretchAxis.origin") -and $alphaValid) {
    Test-PointInside $origin $alpha "$path.stretchAxis.origin" "$path.alphaBounds"
  }
  if (Test-Point $direction "$path.stretchAxis.direction") {
    if ([double]$direction.x -eq 0 -and [double]$direction.y -eq 0) {
      Add-Issue "$path.stretchAxis.direction must be non-zero"
    }
  }
  foreach ($pointName in @('swayPivot', 'tailRoot')) {
    $point = Get-Value $Profile $pointName
    if ((Test-Point $point "$path.$pointName") -and $alphaValid) {
      Test-PointInside $point $alpha "$path.$pointName" "$path.alphaBounds"
      if ($pointName -eq 'tailRoot' -and $edgeTailValid) {
        Test-PointInside $point $edgeTail "$path.tailRoot" "$path.edgeTailBounds"
      }
    }
  }

  $amplitude = Get-Value $Profile 'amplitude'
  Test-ExactProperties $amplitude "$path.amplitude" $amplitudeSemantics | Out-Null
  foreach ($semantic in $amplitudeSemantics) {
    $range = Get-Value $amplitude $semantic
    Test-ExactProperties $range "$path.amplitude.$semantic" @('min', 'max') | Out-Null
    $min = Get-Value $range 'min'
    $max = Get-Value $range 'max'
    if ($min -isnot [ValueType] -or $max -isnot [ValueType]) {
      Add-Issue "$path.amplitude.$semantic min/max must be finite"
    } elseif ([double]$min -ge [double]$max) {
      Add-Issue "$path.amplitude.$semantic.min must be less than max"
    }
  }
}

$resolvedRoot = [System.IO.Path]::GetFullPath($Root)
$contractPath = Join-Path $resolvedRoot '模块合同.json'
if (-not (Test-Path -LiteralPath $contractPath -PathType Leaf)) {
  Add-Issue "missing 模块合同.json at $contractPath"
} else {
  try {
    $contract = Get-Content -Raw -Encoding UTF8 -LiteralPath $contractPath | ConvertFrom-Json
    Test-ExactProperties $contract '模块合同' @('schemaVersion', 'semanticVersion', 'readOnly', 'moduleIds') | Out-Null
    if ($contract.schemaVersion -ne 1) { Add-Issue '模块合同.schemaVersion must be 1' }
    if ($contract.semanticVersion -ne 'cat-a-live2d-v1') { Add-Issue '模块合同.semanticVersion must be cat-a-live2d-v1' }
    if ($contract.readOnly -ne $true) { Add-Issue '模块合同.readOnly must be true' }
    if (@($contract.moduleIds).Count -ne 3 -or (Compare-Object @($contract.moduleIds) $expectedModuleIds)) {
      Add-Issue '模块合同.moduleIds must be exactly body-slender-v1/body-balanced-v1/body-rounded-v1'
    }
  } catch { Add-Issue "invalid 模块合同.json: $($_.Exception.Message)" }
}

foreach ($moduleId in $expectedModuleIds) {
  $moduleDir = Join-Path $resolvedRoot $moduleId
  $manifestPath = Join-Path $moduleDir '模块.json'
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    Add-Issue "$moduleId missing 模块.json"
    continue
  }
  try { $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath | ConvertFrom-Json }
  catch { Add-Issue "$moduleId invalid 模块.json: $($_.Exception.Message)"; continue }

  Test-ExactProperties $manifest "$moduleId manifest" @(
    'schemaVersion', 'moduleId', 'semanticVersion', 'readOnly', 'compatibleModules',
    'requiredParameters', 'tailArtMesh', 'files', 'hashes', 'motions',
    'approvedAmplitude', 'motionSpatialProfile'
  ) | Out-Null
  if ($manifest.schemaVersion -ne 1) { Add-Issue "$moduleId schemaVersion must be 1" }
  if ($manifest.moduleId -ne $moduleId) { Add-Issue "$moduleId moduleId expected $moduleId but found $($manifest.moduleId)" }
  if ($manifest.semanticVersion -ne 'cat-a-live2d-v1') { Add-Issue "$moduleId semanticVersion must be cat-a-live2d-v1" }
  if ($manifest.readOnly -ne $true) { Add-Issue "$moduleId readOnly must be true" }
  if ($manifest.tailArtMesh -ne 'ArtMeshTail') { Add-Issue "$moduleId tailArtMesh must be ArtMeshTail" }
  if (@($manifest.requiredParameters).Count -ne $requiredParameters.Count -or (Compare-Object @($manifest.requiredParameters) $requiredParameters)) {
    Add-Issue "$moduleId requiredParameters must preserve independent eyes/ears/tail and body parameters"
  }

  Test-ExactProperties $manifest.compatibleModules "$moduleId.compatibleModules" @('face', 'ears', 'eyes', 'tail') | Out-Null
  foreach ($role in $expectedCompatibility.Keys) {
    if (@($manifest.compatibleModules.$role).Count -ne 1 -or @($manifest.compatibleModules.$role)[0] -ne $expectedCompatibility[$role][0]) {
      Add-Issue "$moduleId compatibleModules.$role must be $($expectedCompatibility[$role][0])"
    }
  }

  Test-ExactProperties $manifest.files "$moduleId.files" $fileRoles | Out-Null
  Test-ExactProperties $manifest.hashes "$moduleId.hashes" $fileRoles | Out-Null
  foreach ($role in $fileRoles) {
    $relative = Get-Value $manifest.files $role
    if ([string]::IsNullOrWhiteSpace([string]$relative)) { Add-Issue "$moduleId files.$role missing"; continue }
    if ([System.IO.Path]::IsPathRooted([string]$relative) -or ([string]$relative).Contains('..')) {
      Add-Issue "$moduleId files.$role has unsafe path $relative"
      continue
    }
    $assetPath = Join-Path $moduleDir ([string]$relative)
    if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
      Add-Issue "$moduleId missing $relative"
      continue
    }
    $expectedHash = [string](Get-Value $manifest.hashes $role)
    if ($expectedHash -notmatch '^[0-9a-f]{64}$') {
      Add-Issue "$moduleId hash missing or invalid for $role"
      continue
    }
    $actualHash = Get-Sha256 $assetPath
    if ($actualHash -ne $expectedHash) { Add-Issue "$moduleId hash mismatch for $relative" }
    if ($role -eq 'moc3') {
      $bytes = [System.IO.File]::ReadAllBytes($assetPath)
      if ($bytes.Length -le 1024 -or [Text.Encoding]::ASCII.GetString($bytes, 0, [Math]::Min(4, $bytes.Length)) -ne 'MOC3') {
        Add-Issue "$moduleId $relative is not a real Cubism moc3"
      }
      $mocHashes[$moduleId] = $actualHash
    }
  }

  Test-ExactProperties $manifest.motions "$moduleId.motions" $requiredMotions | Out-Null
  foreach ($motionName in $requiredMotions) {
    $motion = Get-Value $manifest.motions $motionName
    Test-ExactProperties $motion "$moduleId.motions.$motionName" @('relativePath', 'sha256') | Out-Null
    $relative = [string](Get-Value $motion 'relativePath')
    if ($relative -ne "motions/$motionName.motion3.json") {
      Add-Issue "$moduleId motions.$motionName has unexpected path $relative"
      continue
    }
    $motionPath = Join-Path $moduleDir $relative
    if (-not (Test-Path -LiteralPath $motionPath -PathType Leaf)) {
      Add-Issue "$moduleId missing $relative"
      continue
    }
    $expectedHash = [string](Get-Value $motion 'sha256')
    if ($expectedHash -notmatch '^[0-9a-f]{64}$' -or (Get-Sha256 $motionPath) -ne $expectedHash) {
      Add-Issue "$moduleId motion hash mismatch for $relative"
    }
  }

  $modelRelative = [string](Get-Value $manifest.files 'model3')
  $modelPath = Join-Path $moduleDir $modelRelative
  if (Test-Path -LiteralPath $modelPath -PathType Leaf) {
    try {
      $model = Get-Content -Raw -Encoding UTF8 -LiteralPath $modelPath | ConvertFrom-Json
      if ($model.FileReferences.Moc -ne $manifest.files.moc3) { Add-Issue "$moduleId model Moc reference mismatch" }
      if ($model.FileReferences.DisplayInfo -ne $manifest.files.displayInfo) { Add-Issue "$moduleId model DisplayInfo reference mismatch" }
      if (@($model.FileReferences.Textures).Count -ne 1 -or @($model.FileReferences.Textures)[0] -ne $manifest.files.neutralTexture) {
        Add-Issue "$moduleId model neutral texture reference mismatch"
      }
      Test-ExactProperties $model.FileReferences.Motions "$moduleId.model.Motions" $requiredMotions | Out-Null
      foreach ($motionName in $requiredMotions) {
        $references = @($model.FileReferences.Motions.$motionName)
        if ($references.Count -ne 1 -or $references[0].File -ne $manifest.motions.$motionName.relativePath) {
          Add-Issue "$moduleId model motion reference mismatch for $motionName"
        }
      }
    } catch { Add-Issue "$moduleId invalid model3 JSON: $($_.Exception.Message)" }
  }

  $displayRelative = [string](Get-Value $manifest.files 'displayInfo')
  $displayPath = Join-Path $moduleDir $displayRelative
  if (Test-Path -LiteralPath $displayPath -PathType Leaf) {
    try {
      $display = Get-Content -Raw -Encoding UTF8 -LiteralPath $displayPath | ConvertFrom-Json
      $ids = @($display.Parameters | ForEach-Object { $_.Id })
      foreach ($parameter in $requiredParameters) {
        if ($ids -notcontains $parameter) { Add-Issue "$moduleId display info missing $parameter" }
      }
    } catch { Add-Issue "$moduleId invalid display info JSON: $($_.Exception.Message)" }
  }

  Test-Profile $manifest.motionSpatialProfile $moduleId
  $profiles[$moduleId] = $manifest.motionSpatialProfile
  if ((ConvertTo-Json $manifest.approvedAmplitude -Compress -Depth 8) -ne (ConvertTo-Json $manifest.motionSpatialProfile.amplitude -Compress -Depth 8)) {
    Add-Issue "$moduleId approvedAmplitude must equal motionSpatialProfile.amplitude"
  }
}

if ($mocHashes.Count -eq 3 -and @($mocHashes.Values | Select-Object -Unique).Count -ne 3) {
  Add-Issue 'three modules must use independently exported moc3 binaries; duplicate hash found'
}
if ($profiles.Count -eq 3) {
  $slenderWidth = [double]$profiles['body-slender-v1'].breathZone.right - [double]$profiles['body-slender-v1'].breathZone.left
  $balancedWidth = [double]$profiles['body-balanced-v1'].breathZone.right - [double]$profiles['body-balanced-v1'].breathZone.left
  $roundedWidth = [double]$profiles['body-rounded-v1'].breathZone.right - [double]$profiles['body-rounded-v1'].breathZone.left
  if (-not ($slenderWidth -lt $balancedWidth -and $balancedWidth -lt $roundedWidth)) {
    Add-Issue 'breathZone width must increase slender < balanced < rounded'
  }
  if ([double]$profiles['body-slender-v1'].amplitude.breath.max -ge [double]$profiles['body-balanced-v1'].amplitude.breath.max) {
    Add-Issue 'slender breath max must be lower than balanced'
  }
  if ([double]$profiles['body-rounded-v1'].amplitude.bodyStretch.max -ge [double]$profiles['body-balanced-v1'].amplitude.bodyStretch.max) {
    Add-Issue 'rounded bodyStretch max must be lower than balanced'
  }
}

if ($issues.Count -gt 0) {
  foreach ($issue in $issues) { Write-Output "FAIL: $issue" }
  Write-Output "0/3 FAIL ($($issues.Count) concrete issue(s))"
  exit 1
}

Write-Output '3/3 PASS - cat-a-live2d-v1 body modules are complete, bound, distinct, and hash-verified'
