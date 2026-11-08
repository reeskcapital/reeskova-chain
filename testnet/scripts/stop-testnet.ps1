$ErrorActionPreference = "Continue"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$PidDir = Join-Path $Root "testnet\var\pids"
if (Test-Path $PidDir) {
  Get-ChildItem $PidDir -Filter "*.pid" | ForEach-Object {
    $procId = Get-Content $_.FullName
    if ($procId) {
      Stop-Process -Id ([int]$procId) -Force -ErrorAction SilentlyContinue
      Write-Host "stopped pid $procId"
    }
    Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue
  }
}
Write-Host "stop complete"
