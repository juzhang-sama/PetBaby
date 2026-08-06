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
if (-not (Test-Path -LiteralPath $core -PathType Leaf)) { Write-Error "SDK is missing Core/live2dcubismcore.min.js: $core" }
if (-not (Test-Path -LiteralPath $framework -PathType Container)) { Write-Error "SDK is missing Framework/src: $framework" }
New-Item -ItemType Directory -Force -Path $vendorRoot | Out-Null
Copy-Item -LiteralPath (Join-Path $source "Core") -Destination $vendorRoot -Recurse -Force
Copy-Item -LiteralPath (Join-Path $source "Framework") -Destination $vendorRoot -Recurse -Force
Write-Output "Cubism SDK copied to $vendorRoot"
