[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$sourceRoot = Join-Path $repoRoot 'services\appearance-generation\src'
$previousPythonPath = $env:PYTHONPATH

try {
  if ([string]::IsNullOrEmpty($previousPythonPath)) {
    $env:PYTHONPATH = $sourceRoot
  } else {
    $env:PYTHONPATH = "$sourceRoot;$previousPythonPath"
  }
  Write-Host 'Generating photo-avatar work canvases and region maps...'
  & python -m photo_avatar_backend.uv_guides --repo-root $repoRoot
  if ($LASTEXITCODE -ne 0) {
    throw "UV guide generation failed with exit code $LASTEXITCODE"
  }
} finally {
  $env:PYTHONPATH = $previousPythonPath
}
