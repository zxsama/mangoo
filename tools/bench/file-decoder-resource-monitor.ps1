param([int]$CoreId,[int]$DecoderId,[string]$Ready,[string]$Stop,[string]$Output)
$ErrorActionPreference = 'Stop'
$rows = @{}
foreach($processId in @($CoreId,$DecoderId)) {
  $process = Get-Process -Id $processId
  $process.Refresh()
  $rows[$processId] = @{pid=$processId; baselineWorkingSet=$process.WorkingSet64; baselinePrivate=$process.PrivateMemorySize64; maximumWorkingSet=$process.WorkingSet64; maximumPrivate=$process.PrivateMemorySize64; peakWorkingSet=$process.PeakWorkingSet64; samples=0}
}
Set-Content -LiteralPath $Ready -Value 'ready'
while(-not (Test-Path -LiteralPath $Stop)) {
  foreach($processId in @($CoreId,$DecoderId)) {
    $process = Get-Process -Id $processId
    $process.Refresh()
    $row = $rows[$processId]
    $row.maximumWorkingSet = [Math]::Max($row.maximumWorkingSet,$process.WorkingSet64)
    $row.maximumPrivate = [Math]::Max($row.maximumPrivate,$process.PrivateMemorySize64)
    $row.peakWorkingSet = [Math]::Max($row.peakWorkingSet,$process.PeakWorkingSet64)
    $row.samples += 1
  }
  Start-Sleep -Milliseconds 10
}
@{core=$rows[$CoreId];decoder=$rows[$DecoderId]} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Output -Encoding utf8
