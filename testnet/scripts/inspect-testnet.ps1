$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Var = Join-Path $Root "testnet\var"
Write-Host "testnet var: $Var"
if (-not (Test-Path $Var)) {
  Write-Host "not initialized; run reset-testnet.ps1"
  exit 0
}
1..4 | ForEach-Object {
  $ready = Join-Path $Var "validator-$_\READY"
  if (Test-Path $ready) {
    Write-Host "--- validator $_ ---"
    Get-Content $ready
  } else {
    Write-Host "validator ${_}: missing READY"
  }
}
