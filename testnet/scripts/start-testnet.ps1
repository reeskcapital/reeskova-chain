$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Var = Join-Path $Root "testnet\var"
if (-not (Test-Path $Var)) {
  & (Join-Path $PSScriptRoot "reset-testnet.ps1")
}
# Marker files — full process spawn depends on built rsc-validator/rsc-node binaries.
1..4 | ForEach-Object {
  $dir = Join-Path $Var "validator-$_"
  $marker = Join-Path $dir "READY"
  @"
validator_index=$_
network_id=rsc-amm-testnet
chain_id=rsc-amm-testnet
rpc_port=$(18546 + $_)
status=prepared
note=AMM multi-validator proof runs via cargo test native::stage7_native
"@ | Set-Content -Path $marker -Encoding utf8
  Write-Host "prepared validator $_ at $dir"
}
Write-Host "testnet prepared (4 validators). Run run-amm-validation.ps1 for Stage 7 suites."
