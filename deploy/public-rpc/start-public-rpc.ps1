$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

$Var = Join-Path $PSScriptRoot "var"
$Data = Join-Path $Var "data"
$Genesis = Join-Path $Var "genesis.json"
New-Item -ItemType Directory -Force -Path $Data | Out-Null

$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$env:PATH = "C:\Program Files\LLVM\bin;" + $env:PATH

Write-Host "=== IPs públicas de este host (para Porkbun) ==="
try { Write-Host ("IPv4 = " + (Invoke-RestMethod https://api.ipify.org -TimeoutSec 8)) } catch { Write-Host "IPv4 = (falló)" }
try { Write-Host ("IPv6 = " + (Invoke-RestMethod https://api64.ipify.org -TimeoutSec 8)) } catch { Write-Host "IPv6 = (falló)" }
Write-Host "Poné esas IPs en el DNS A / AAAA de rpc.reeskova-chain.xyz y borrá el AAAA viejo."
Write-Host ""

if (-not (Test-Path $Genesis)) {
  Write-Host "Generando génesis bootstrap..."
  & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "generate-genesis.ps1")
  if (-not (Test-Path $Genesis)) {
    throw "no se pudo crear genesis.json"
  }
}

Write-Host "Compilando rsc-node (puede tardar)..."
cargo build -p rsc-node
$bin = Join-Path $Root "target\debug\rsc-node.exe"
if (-not (Test-Path $bin)) { throw "falta $bin" }

$seedsFile = Join-Path $Data "rpc-seeds.json"
@'
{
  "endpoints": [
    "https://rpc.reeskova-chain.xyz/rpc",
    "http://127.0.0.1:8545/rpc"
  ]
}
'@ | Set-Content -Path $seedsFile -Encoding utf8

Write-Host "Arrancando rsc-node en 127.0.0.1:8545 (Ctrl+C para parar)..."
Write-Host "Luego, en otra terminal: caddy run --config deploy\public-rpc\Caddyfile"
& $bin `
  --data-dir $Data `
  --network-id rsc-public `
  --chain-id 3 `
  --genesis $Genesis `
  --rpc-bind 127.0.0.1:8545 `
  --rpc-public-url https://rpc.reeskova-chain.xyz/rpc `
  --rpc-seed https://rpc.reeskova-chain.xyz/rpc
