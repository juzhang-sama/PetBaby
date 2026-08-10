param(
  [string]$CubismSdkRoot = $env:CUBISM_SDK_ROOT
)
$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$vendorRoot = Join-Path $repoRoot "apps/desktop/.vendor/live2d-cubism-sdk"
if ([string]::IsNullOrWhiteSpace($CubismSdkRoot)) {
  Write-Error "CUBISM_SDK_ROOT is not set. Point it to the official Cubism SDK for Web directory."
}
$source = (Resolve-Path -LiteralPath $CubismSdkRoot -ErrorAction Stop).Path
$core = Join-Path $source "Core/live2dcubismcore.min.js"
$framework = Join-Path $source "Framework/src"
$frameworkEntry = Join-Path $framework "live2dcubismframework.ts"
$sampleModel = Join-Path $source "Samples/Resources/Wanko/Wanko.model3.json"
$shaderRoot = Join-Path $source "Framework/Shaders/WebGL"
$publicLive2D = Join-Path $repoRoot "apps/desktop/public/live2d"
if (-not (Test-Path -LiteralPath $core -PathType Leaf)) { Write-Error "SDK is missing Core/live2dcubismcore.min.js: $core" }
if (-not (Test-Path -LiteralPath $framework -PathType Container)) { Write-Error "SDK is missing Framework/src: $framework" }
if (-not (Test-Path -LiteralPath $frameworkEntry -PathType Leaf)) { Write-Error "SDK is missing Framework/src/live2dcubismframework.ts: $frameworkEntry" }
if (-not (Test-Path -LiteralPath $sampleModel -PathType Leaf)) { Write-Error "SDK is missing Samples/Resources/Wanko/Wanko.model3.json: $sampleModel" }
if (-not (Test-Path -LiteralPath $shaderRoot -PathType Container)) { Write-Error "SDK is missing Framework/Shaders/WebGL: $shaderRoot" }
New-Item -ItemType Directory -Force -Path $vendorRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $source "Core") -Destination $vendorRoot -Recurse -Force
Copy-Item -LiteralPath (Join-Path $source "Framework") -Destination $vendorRoot -Recurse -Force
New-Item -ItemType Directory -Force -Path $publicLive2D | Out-Null
Copy-Item -LiteralPath (Join-Path $source "Core") -Destination $publicLive2D -Recurse -Force
Copy-Item -LiteralPath (Join-Path $source "Samples/Resources/Wanko") -Destination $publicLive2D -Recurse -Force
New-Item -ItemType Directory -Force -Path (Join-Path $publicLive2D "Framework") | Out-Null
Copy-Item -LiteralPath (Join-Path $source "Framework/Shaders") -Destination (Join-Path $publicLive2D "Framework") -Recurse -Force
Write-Output "Cubism SDK prepared in $vendorRoot and $publicLive2D"
