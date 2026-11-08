$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Var = Join-Path $PSScriptRoot "var"
$Data = Join-Path $Var "data"
$Genesis = Join-Path $Var "genesis.json"
$CaddyDir = Join-Path $Var "caddy"
$Cloudflared = Join-Path $CaddyDir "cloudflared.exe"
$Node = Join-Path $Root "target\debug\rsc-node.exe"

New-Item -ItemType Directory -Force -Path $Data,$CaddyDir | Out-Null
if (-not (Test-Path $Genesis)) {
  & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "generate-genesis.ps1")
}
if (-not (Test-Path $Node)) {
  $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
  Push-Location $Root
  cargo build -p rsc-node
  Pop-Location
}
if (-not (Test-Path $Cloudflared)) {
  curl.exe -L --retry 3 -o $Cloudflared "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe"
}

Get-Process rsc-node,cloudflared,caddy -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep 1

# Node local first
$nodeProc = Start-Process -FilePath $Node -PassThru -WindowStyle Minimized -ArgumentList @(
  "--data-dir", $Data,
  "--network-id", "rsc-public",
  "--chain-id", "3",
  "--genesis", $Genesis,
  "--rpc-bind", "127.0.0.1:8545",
  "--rpc-public-url", "http://127.0.0.1:8545/rpc",
  "--rpc-seed", "http://127.0.0.1:8545/rpc"
)
Start-Sleep 3

# Quick tunnel (HTTPS publico sin abrir router)
$log = Join-Path $Var "cloudflared.log"
Remove-Item $log -ErrorAction SilentlyContinue
$cfProc = Start-Process -FilePath $Cloudflared -PassThru -WindowStyle Minimized `
  -RedirectStandardError $log -RedirectStandardOutput $log `
  -ArgumentList @("tunnel","--url","http://127.0.0.1:8545","--no-autoupdate")

$public = $null
for ($i = 0; $i -lt 60; $i++) {
  Start-Sleep 1
  if (Test-Path $log) {
    $m = Select-String -Path $log -Pattern "https://[a-z0-9-]+\.trycloudflare\.com" | Select-Object -Last 1
    if ($m) {
      $public = ([regex]::Match($m.Line, "https://[a-z0-9-]+\.trycloudflare\.com")).Value
      break
    }
  }
}
if (-not $public) { throw "cloudflared no publicó URL" }

$rpc = "$public/rpc"
Set-Content (Join-Path $Var "CURRENT_PUBLIC_RPC.txt") -Value $rpc -Encoding utf8
@"
{
  "endpoints": [
    "$rpc",
    "https://rpc.reeskova-chain.xyz/rpc",
    "http://127.0.0.1:8545/rpc"
  ]
}
"@ | Set-Content (Join-Path $Data "rpc-seeds.json") -Encoding utf8

# Reinicia nodo anunciando la URL publica real
Stop-Process -Id $nodeProc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep 1
$nodeProc = Start-Process -FilePath $Node -PassThru -WindowStyle Minimized -ArgumentList @(
  "--data-dir", $Data,
  "--network-id", "rsc-public",
  "--chain-id", "3",
  "--genesis", $Genesis,
  "--rpc-bind", "127.0.0.1:8545",
  "--rpc-public-url", $rpc,
  "--rpc-seed", $rpc
)

Write-Host ""
Write-Host "BOOTSTRAP PUBLICO LISTO"
Write-Host "  health : $public/health"
Write-Host "  rpc    : $rpc"
Write-Host "  network: $public/network"
Write-Host "  node pid=$($nodeProc.Id)  cloudflared pid=$($cfProc.Id)"
Write-Host "Dejá esta ventana/procesos vivos las 48h. El dominio rpc.reeskova-chain.xyz sigue sin TLS hasta abrir 80/443 en el router."
