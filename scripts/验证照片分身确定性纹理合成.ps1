param(
    [switch]$Revision13Only
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$revision13 = Join-Path $repoRoot "services/appearance-generation/output/task-11-runtime/artifacts/a3eb6a61778c48cd85136aa4e5f67f83.png"
$expectedRevision13Sha256 = "5503e493829423f316a348c9c96a54681383d0efb7a76417c9fa2ac0c653e9a0"
$moduleTexture = Join-Path $repoRoot "apps/desktop/public/cat-character-modules/cat-a-live2d-v1/body-balanced-v1/body-balanced-v1.2048/texture_00.png"
$stateRoot = Join-Path $repoRoot "services/appearance-generation/output/task-11-runtime"
$pythonRoot = Join-Path $repoRoot "services/appearance-generation"

function Get-Sha256([string]$Path) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $stream = [IO.File]::OpenRead($Path)
        try { return ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
        finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

function Get-FileState([string]$Root) {
    if (-not (Test-Path -LiteralPath $Root)) { return @{} }
    $state = @{}
    foreach ($file in Get-ChildItem -LiteralPath $Root -Recurse -File -ErrorAction Stop) {
        $key = $file.FullName.ToLowerInvariant()
        $state[$key] = "{0}|{1}|{2}" -f $file.Length, $file.LastWriteTimeUtc.Ticks, (Get-Sha256 $file.FullName)
    }
    return $state
}

function Invoke-Revision13ReadOnly {
    if (-not (Test-Path -LiteralPath $revision13)) { throw "revision 13 artifact is missing" }
    if ((Get-Sha256 $revision13) -ne $expectedRevision13Sha256) { throw "revision 13 artifact changed" }
    $beforeJobs = Get-FileState (Join-Path $stateRoot "jobs")

    $analysis = @'
import hashlib
import json
import os
from pathlib import Path
from PIL import Image

from photo_avatar_backend.contracts import ContractError
from photo_avatar_backend.texture_compositor import compose_canonical_texture

repo = Path(os.environ["PHOTO_AVATAR_REPO_ROOT"])
raw_path = repo / "services/appearance-generation/output/task-11-runtime/artifacts/a3eb6a61778c48cd85136aa4e5f67f83.png"
module_path = repo / "apps/desktop/public/cat-character-modules/cat-a-live2d-v1/body-balanced-v1/body-balanced-v1.2048/texture_00.png"
raw = Image.open(raw_path).convert("RGBA")
module = Image.open(module_path).convert("RGBA")
raw_alpha = raw.getchannel("A").tobytes()
module_alpha = module.getchannel("A").tobytes()
alpha_mismatch = sum(left != right for left, right in zip(raw_alpha, module_alpha))
module_transparent_missing = sum(left == 0 and right > 0 for left, right in zip(raw_alpha, module_alpha))
missing_rgb_nonzero = sum(
    pixel[3] == 0 and guide[3] > 0 and pixel[:3] != (0, 0, 0)
    for pixel, guide in zip(raw.getdata(), module.getdata())
)
try:
    compose_canonical_texture(
        provider_png=raw_path.read_bytes(),
        work_canvas_png=module_path.read_bytes(),
        region_map_png=module_path.read_bytes(),
        module_alpha=module_alpha,
        minimum_change_ratio=0.95,
    )
except ContractError:
    opaque_provider_rejected = True
else:
    opaque_provider_rejected = False
print(json.dumps({
    "alphaMismatch": alpha_mismatch,
    "moduleTransparentMissing": module_transparent_missing,
    "missingRgbNonzero": missing_rgb_nonzero,
    "opaqueProviderRejected": opaque_provider_rejected,
}, separators=(",", ":")))
'@
    $tempScript = Join-Path ([IO.Path]::GetTempPath()) "photo-avatar-revision13-$PID.py"
    try {
        Set-Content -LiteralPath $tempScript -Value $analysis -Encoding utf8
        $env:PYTHONPATH = Join-Path $repoRoot "services/appearance-generation/src"
        $env:PHOTO_AVATAR_REPO_ROOT = $repoRoot
        $result = & python $tempScript
        if ($LASTEXITCODE -ne 0) { throw "revision 13 analysis failed" }
        $report = ($result -join "`n") | ConvertFrom-Json
        if ($report.alphaMismatch -ne 3390353) { throw "unexpected revision 13 alpha mismatch: $($report.alphaMismatch)" }
        if ($report.moduleTransparentMissing -ne 54911) { throw "unexpected revision 13 transparent missing count: $($report.moduleTransparentMissing)" }
        if ($report.missingRgbNonzero -ne 0) { throw "revision 13 missing RGB is not zero" }
        if (-not $report.opaqueProviderRejected) { throw "revision 13 raw artifact passed the opaque provider gate" }
    } finally {
        Remove-Item Env:PYTHONPATH -ErrorAction SilentlyContinue
        Remove-Item Env:PHOTO_AVATAR_REPO_ROOT -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $tempScript -Force -ErrorAction SilentlyContinue
    }

    $afterJobs = Get-FileState (Join-Path $stateRoot "jobs")
    if ($beforeJobs.Count -ne $afterJobs.Count) { throw "revision 13 job files changed" }
    foreach ($key in $beforeJobs.Keys) {
        if (-not $afterJobs.ContainsKey($key) -or $beforeJobs[$key] -ne $afterJobs[$key]) {
            throw "revision 13 job file changed: $key"
        }
    }
    if ((Get-Sha256 $revision13) -ne $expectedRevision13Sha256) { throw "revision 13 artifact changed during analysis" }
    Write-Output "revision13-only: PASS; sha256=$expectedRevision13Sha256; alphaMismatch=3390353; moduleTransparentMissing=54911"
}

function Invoke-CheckedProcess([string]$FilePath, [string[]]$ArgumentList, [string]$WorkingDirectory) {
    $logRoot = Join-Path ([IO.Path]::GetTempPath()) "photo-avatar-offline-gate-$PID"
    New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
    $name = [IO.Path]::GetRandomFileName()
    $stdoutPath = Join-Path $logRoot "$name.out.log"
    $stderrPath = Join-Path $logRoot "$name.err.log"
    $specPath = Join-Path $logRoot "$name.spec.json"
    $wrapperPath = Join-Path $logRoot "$name.wrapper.ps1"
    $exitCodePath = Join-Path $logRoot "$name.exitcode"
    [ordered]@{
        file = $FilePath
        args = @($ArgumentList)
        workingDirectory = $WorkingDirectory
        exitCodePath = $exitCodePath
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $specPath -Encoding utf8
    $wrapper = @'
param([Parameter(Mandatory = $true)][string]$SpecPath)
$spec = Get-Content -LiteralPath $SpecPath -Raw | ConvertFrom-Json
Set-Location -LiteralPath $spec.workingDirectory
& $spec.file @($spec.args)
$code = if ($null -eq $LASTEXITCODE) { 0 } else { [int]$LASTEXITCODE }
[IO.File]::WriteAllText($spec.exitCodePath, [string]$code)
exit $code
'@
    Set-Content -LiteralPath $wrapperPath -Value $wrapper -Encoding utf8
    $process = Start-Process -FilePath "powershell" -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $wrapperPath, "-SpecPath", $specPath) -WorkingDirectory $WorkingDirectory -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -WindowStyle Hidden
    $lastSize = 0L
    $lastProgress = [DateTime]::UtcNow
    try {
        while (-not $process.HasExited) {
            Start-Sleep -Seconds 2
            $size = 0L
            foreach ($path in @($stdoutPath, $stderrPath)) {
                if (Test-Path -LiteralPath $path) { $size += (Get-Item -LiteralPath $path).Length }
            }
            if ($size -gt $lastSize) { $lastSize = $size; $lastProgress = [DateTime]::UtcNow }
            if (([DateTime]::UtcNow - $lastProgress).TotalSeconds -gt 120) {
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                throw "offline gate command stalled for 120 seconds: $FilePath $($ArgumentList -join ' ')"
            }
        }
        $process.WaitForExit()
        $process.Refresh()
        $exitCode = $null
        if (Test-Path -LiteralPath $exitCodePath) {
            $exitCode = [int](Get-Content -LiteralPath $exitCodePath -Raw).Trim()
        }
        if ($null -eq $exitCode -or $exitCode -ne 0) {
            $output = @()
            if (Test-Path -LiteralPath $stdoutPath) { $output += Get-Content -LiteralPath $stdoutPath -Tail 80 }
            if (Test-Path -LiteralPath $stderrPath) { $output += Get-Content -LiteralPath $stderrPath -Tail 80 }
            $command = "$FilePath $($ArgumentList -join ' ')"
            $details = $output -join "`n"
            throw "offline gate command failed ($exitCode): $command`n$details"
        }
    } finally {
        Remove-Item -LiteralPath $stdoutPath, $stderrPath, $specPath, $wrapperPath, $exitCodePath -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $logRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Invoke-Revision13ReadOnly
if ($Revision13Only) { exit 0 }

Invoke-CheckedProcess "python" @("-m", "pytest", "src/photo_avatar_backend", "-q") $pythonRoot
Invoke-CheckedProcess "npm" @("--prefix", "apps/desktop", "test", "--", "--run") $repoRoot
Invoke-CheckedProcess "npm" @("--prefix", "apps/desktop", "run", "typecheck") $repoRoot
Invoke-CheckedProcess "npm" @("--prefix", "apps/desktop", "run", "build") $repoRoot
Invoke-CheckedProcess "cargo" @("test", "--manifest-path", "apps/desktop/src-tauri/Cargo.toml") $repoRoot
Invoke-CheckedProcess "cargo" @("fmt", "--manifest-path", "apps/desktop/src-tauri/Cargo.toml", "--check") $repoRoot
Invoke-CheckedProcess "git" @("diff", "--check") $repoRoot
Write-Output "offline-gate: PASS; revision13 immutable; no provider call performed"
