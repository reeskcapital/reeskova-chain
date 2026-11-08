$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Var = Join-Path $Root "testnet\var"
if (Test-Path $Var) {
  Remove-Item -Recurse -Force $Var
}
New-Item -ItemType Directory -Force -Path $Var | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Var "pids") | Out-Null
1..4 | ForEach-Object {
  New-Item -ItemType Directory -Force -Path (Join-Path $Var "validator-$_") | Out-Null
}
Write-Host "reset ok: $Var"
