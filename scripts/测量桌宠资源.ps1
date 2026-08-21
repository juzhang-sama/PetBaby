param(
  [Parameter(Mandatory=$true)][int]$RootPid,
  [ValidateSet('idle','interaction','hidden','idle-m1','hidden-m1','v4-companion','v4-interaction')][string]$State = 'idle',
  [int]$DurationSeconds = 300,
  [string]$OutputPath = "docs/验证记录/性能采样/$State.csv",
  [string]$FrameLogPath = ''
)

$logicalProcessors = [Environment]::ProcessorCount
$resolved = [IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputPath))
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($resolved)) | Out-Null
$previousCpu = @{}
$rows = New-Object System.Collections.Generic.List[object]
$previousSampleAt = $null
$resolvedFrameLog = if ([string]::IsNullOrWhiteSpace($FrameLogPath)) { $null } else { [IO.Path]::GetFullPath((Join-Path (Get-Location) $FrameLogPath)) }
$frameLogStartLine = if ($resolvedFrameLog -and (Test-Path -LiteralPath $resolvedFrameLog)) { @(Get-Content -LiteralPath $resolvedFrameLog).Count } else { 0 }

function Get-ProcessTree([int]$ParentPid) {
  $all = Get-CimInstance Win32_Process
  $ids = New-Object System.Collections.Generic.HashSet[int]
  [void]$ids.Add($ParentPid)
  do {
    $changed = $false
    foreach ($process in $all) {
      if ($ids.Contains([int]$process.ParentProcessId) -and $ids.Add([int]$process.ProcessId)) { $changed = $true }
    }
  } while ($changed)
  return $ids
}

for ($second = 0; $second -lt $DurationSeconds; $second++) {
  $sampleStartedAt = Get-Date
  $elapsedSeconds = if ($null -eq $previousSampleAt) { 0.0 } else { [Math]::Max(0.001, ($sampleStartedAt - $previousSampleAt).TotalSeconds) }
  $ids = Get-ProcessTree $RootPid
  $processes = @($ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
  $cpuPercent = 0.0
  foreach ($process in $processes) {
    $current = [double]$process.CPU
    if ($elapsedSeconds -gt 0 -and $previousCpu.ContainsKey($process.Id)) {
      $cpuPercent += (($current - $previousCpu[$process.Id]) / $elapsedSeconds / $logicalProcessors) * 100.0
    }
    $previousCpu[$process.Id] = $current
  }
  $rows.Add([PSCustomObject]@{
    timestamp = (Get-Date).ToString('o')
    state = $State
    totalCpuPercent = [Math]::Round($cpuPercent,3)
    totalPrivateMb = [Math]::Round((($processes | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB),2)
    processCount = $processes.Count
    handleCount = ($processes | Measure-Object Handles -Sum).Sum
    threadCount = ($processes | ForEach-Object { $_.Threads.Count } | Measure-Object -Sum).Sum
  })
  $previousSampleAt = $sampleStartedAt
  $workElapsedMs = ((Get-Date) - $sampleStartedAt).TotalMilliseconds
  $remainingMs = [Math]::Max(0, 1000 - $workElapsedMs)
  if ($remainingMs -gt 0) { Start-Sleep -Milliseconds ([int]$remainingMs) }
}

$rows | Export-Csv -LiteralPath $resolved -NoTypeInformation -Encoding UTF8
$sortedCpu = @($rows.totalCpuPercent | Sort-Object)
$p95Index = [Math]::Max(0,[Math]::Ceiling($sortedCpu.Count * 0.95) - 1)
$frameDeltas = New-Object System.Collections.Generic.List[double]
if ($resolvedFrameLog -and (Test-Path -LiteralPath $resolvedFrameLog)) {
  $newLogLines = @(Get-Content -LiteralPath $resolvedFrameLog | Select-Object -Skip $frameLogStartLine)
  foreach ($line in $newLogLines) {
    if ($line -notmatch 'frame-sample: deltas=([0-9.,]+)') { continue }
    foreach ($value in $Matches[1].Split(',')) {
      $delta = 0.0
      if ([double]::TryParse($value, [Globalization.NumberStyles]::Float, [Globalization.CultureInfo]::InvariantCulture, [ref]$delta) -and $delta -gt 0) {
        $frameDeltas.Add($delta)
      }
    }
  }
}

$averageFps = 'N/A'
$onePercentLowFps = 'N/A'
if ($frameDeltas.Count -gt 0) {
  $totalFrameMs = ($frameDeltas | Measure-Object -Sum).Sum
  $averageFps = [Math]::Round(($frameDeltas.Count * 1000.0) / $totalFrameMs, 2)
  $slowFrameCount = [Math]::Max(1, [Math]::Ceiling($frameDeltas.Count * 0.01))
  $slowFrameAverageMs = (@($frameDeltas | Sort-Object -Descending | Select-Object -First $slowFrameCount) | Measure-Object -Average).Average
  $onePercentLowFps = [Math]::Round(1000.0 / $slowFrameAverageMs, 2)
}

[PSCustomObject]@{
  State = $State
  Samples = $rows.Count
  CpuP95 = $sortedCpu[$p95Index]
  PrivateMbPeak = ($rows.totalPrivateMb | Measure-Object -Maximum).Maximum
  FrameSamples = $frameDeltas.Count
  AverageFps = $averageFps
  OnePercentLowFps = $onePercentLowFps
  Output = $resolved
} | Format-List
