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

Write-Output "neutral reference contract passed"
