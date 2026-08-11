param(
    [string] $AnalyzerPath
)

$ErrorActionPreference = "Stop"
if (-not $AnalyzerPath) {
    $AnalyzerPath = Get-ChildItem -LiteralPath $PSScriptRoot -File -Filter "*.ps1" |
        Where-Object FullName -ne $PSCommandPath |
        Sort-Object Length -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
$source = Get-Content -LiteralPath $AnalyzerPath -Raw

if ($source -notmatch '\[string\]\s+\$ReferencePath') {
    throw "analyzer must require an explicit neutral reference path"
}
if ($source -match 'Raster\s+reference\s*=\s*Load\(paths\[0\]\)') {
    throw "analyzer must not treat the random-phase first capture as neutral"
}
if ($source -notmatch 'Raster\s+reference\s*=\s*Load\(referencePath\)') {
    throw "analyzer must load the explicit neutral reference"
}
if ($source -notmatch 'referencePath,\s*double\s+faceLeft') {
    throw "analyzer must pass the explicit reference through the analysis boundary"
}
if ($source -match 'Raster\s+current\s*=\s*index\s*==\s*0\s*\?\s*reference') {
    throw "analyzer must not substitute the neutral reference for captured frame 0"
}
if ($source -notmatch 'Raster\s+current\s*=\s*Load\(paths\[index\]\)') {
    throw "analyzer must load every captured frame including index 0"
}
if ($source -match 'Select-Object\s+-Skip\s+1') {
    throw "analyzer must include all 92 captured frames in summary metrics"
}
if ($source -notmatch 'const\s+int\s+SameSwayOpposedBreathStride\s*=\s*39') {
    throw "analyzer must predeclare the 39-frame same-sway opposed-breath stride"
}
if ($source -notmatch 'Raster\s+comparison\s*=\s*Load\(paths\[comparisonIndex\]\)') {
    throw "analyzer must compare each metric frame with another real captured frame"
}
if ($source -notmatch 'CompareActualFrames\(comparison,\s*current') {
    throw "analyzer must measure local residuals between real captured frames"
}
if ($source -match 'BoundaryResidual\(reference,\s*current') {
    throw "analyzer must use neutral only for pose fitting, not as a local residual sample"
}

Write-Output "neutral reference and real-frame contract passed"
