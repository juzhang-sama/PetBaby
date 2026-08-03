param(
  [Parameter(Mandatory=$true)][int]$RootPid,
  [ValidateSet('idle','interaction','hidden','idle-m1','hidden-m1')][string]$State = 'idle',
  [int]$DurationSeconds = 300,
  [string]$OutputPath = "docs/验证记录/性能采样/$State.csv"
)

$logicalProcessors = [Environment]::ProcessorCount
$resolved = [IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputPath))
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($resolved)) | Out-Null
$previousCpu = @{}
$rows = New-Object System.Collections.Generic.List[object]

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
  $ids = Get-ProcessTree $RootPid
  $processes = @($ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
  $cpuPercent = 0.0
  foreach ($process in $processes) {
    $current = [double]$process.CPU
    if ($previousCpu.ContainsKey($process.Id)) {
      $cpuPercent += (($current - $previousCpu[$process.Id]) / $logicalProcessors) * 100.0
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
  Start-Sleep -Seconds 1
}

$rows | Export-Csv -LiteralPath $resolved -NoTypeInformation -Encoding UTF8
$sortedCpu = @($rows.totalCpuPercent | Sort-Object)
$p95Index = [Math]::Max(0,[Math]::Ceiling($sortedCpu.Count * 0.95) - 1)
[PSCustomObject]@{
  State = $State
  Samples = $rows.Count
  CpuP95 = $sortedCpu[$p95Index]
  PrivateMbPeak = ($rows.totalPrivateMb | Measure-Object -Maximum).Maximum
  Output = $resolved
} | Format-List
