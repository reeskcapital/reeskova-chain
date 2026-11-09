# Arranca la API universal detrás de rpc.reeskova-chain.xyz
# - rsc-node      127.0.0.1:8545
# - mobile-node   127.0.0.1:8546
# - community     127.0.0.1:8547
# - Caddy         80/443 (dominio) + :8880 (prueba local)
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

$Var = Join-Path $PSScriptRoot "var"
$Data = Join-Path $Var "data"
$Genesis = Join-Path $Var "genesis.json"
$Caddy = Join-Path $Var "caddy\caddy.exe"
$Logs = Join-Path $Var "logs"
New-Item -ItemType Directory -Force -Path $Data, $Logs | Out-Null

$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$env:PATH = "C:\Program Files\LLVM\bin;" + $env:PATH

# Dominio = puerta universal (los nodos se ligan aquí, no a localhost)
$Api = "https://rpc.reeskova-chain.xyz/rpc"
$NodeBin = Join-Path $Root "target\debug\rsc-node.exe"
$MobileBin = Join-Path $Root "target\debug\rsc-mobile-node.exe"
$CommunityBin = Join-Path $Root "target\debug\rsc-community-node.exe"

if (-not (Test-Path $Genesis)) {
  & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "generate-genesis.ps1")
}
if (-not (Test-Path $NodeBin)) { cargo build -p rsc-node }
if (-not (Test-Path $MobileBin)) { cargo build -p rsc-mobile-node }
if (-not (Test-Path $CommunityBin)) { cargo build -p rsc-community-node }
if (-not (Test-Path $Caddy)) { throw "falta $Caddy — descargá Caddy en deploy\public-rpc\var\caddy\" }

# Parar instancias previas de este stack
Get-Process rsc-node, rsc-mobile-node, rsc-community-node, caddy -ErrorAction SilentlyContinue |
  Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep 1

@'
{
  "endpoints": [
    "https://rpc.reeskova-chain.xyz/rpc"
  ]
}
'@ | Set-Content -Path (Join-Path $Data "rpc-seeds.json") -Encoding utf8

function Start-Logged([string]$Name, [string]$File, [string[]]$ArgumentList) {
  $out = Join-Path $Logs "$Name.out.log"
  $err = Join-Path $Logs "$Name.err.log"
  $p = Start-Process -FilePath $File -ArgumentList $ArgumentList -PassThru -WindowStyle Minimized `
    -RedirectStandardOutput $out -RedirectStandardError $err
  Write-Host ("started {0} pid={1}" -f $Name, $p.Id)
  return $p
}

Start-Logged "rsc-node" $NodeBin @(
  "--data-dir", $Data,
  "--network-id", "rsc-public",
  "--chain-id", "3",
  "--genesis", $Genesis,
  "--rpc-bind", "127.0.0.1:8545",
  "--rpc-public-url", $Api,
  "--rpc-seed", $Api,
  "--rpc-cors", "http://rpc.reeskova-chain.xyz"
) | Out-Null

Start-Logged "rsc-mobile-node" $MobileBin @(
  "--api", $Api,
  "--bind", "127.0.0.1:8546",
  "--state", (Join-Path $Var "mobile-state.json")
) | Out-Null

Start-Logged "rsc-community-node" $CommunityBin @(
  "--api", $Api,
  "--bind", "127.0.0.1:8547",
  "--state", (Join-Path $Var "community-state.json")
) | Out-Null

Start-Sleep 2

Start-Logged "caddy" $Caddy @(
  "run", "--config", (Join-Path $PSScriptRoot "Caddyfile"), "--adapter", "caddyfile"
) | Out-Null

Start-Sleep 2

Write-Host ""
Write-Host "API UNIVERSAL (HTTP, sin certificado)"
Write-Host "  curl http://rpc.reeskova-chain.xyz/community"
Write-Host "  curl -X POST http://rpc.reeskova-chain.xyz/community/link -H 'Content-Type: application/json' -d '{`"user`":`"rsc…`"}'"
Write-Host "  curl -X POST http://rpc.reeskova-chain.xyz/community/unlink -H 'Content-Type: application/json' -d '{`"user`":`"rsc…`"}'"
Write-Host "  rpc: http://rpc.reeskova-chain.xyz/rpc"
Write-Host ""
Write-Host "Para que otros en internet lleguen: router TCP 80 → 192.168.1.10"
Write-Host "Logs: deploy\public-rpc\var\logs\"
