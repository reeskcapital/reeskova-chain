# Hace que rpc.reeskova-chain.xyz sea alcanzable en TODO el mundo
# sin abrir puertos del router (túnel Cloudflare + CNAME en Porkbun).
$ErrorActionPreference = "Stop"
$cf = Join-Path $PSScriptRoot "var\caddy\cloudflared.exe"
$cfgDir = Join-Path $env:USERPROFILE ".cloudflared"
$cert = Join-Path $cfgDir "cert.pem"
New-Item -ItemType Directory -Force -Path $cfgDir,(Join-Path $PSScriptRoot "var") | Out-Null

if (-not (Test-Path $cf)) { throw "falta cloudflared: $cf" }

if (-not (Test-Path $cert)) {
  Write-Host "1) Se abre el login de Cloudflare. Entrá y dale Authorize."
  Write-Host "2) Cuando termine, volvé a correr este script."
  & $cf tunnel login
  if (-not (Test-Path $cert)) { throw "login incompleto: no hay cert.pem" }
}

# Crear túnel si no existe
$list = & $cf tunnel list 2>&1 | Out-String
$tunnelName = "rsc-rpc"
if ($list -notmatch $tunnelName) {
  & $cf tunnel create $tunnelName
}

$tunnelId = $null
$json = & $cf tunnel list --output json 2>$null
if ($json) {
  $rows = $json | ConvertFrom-Json
  $tunnelId = ($rows | Where-Object { $_.name -eq $tunnelName } | Select-Object -First 1).id
}
if (-not $tunnelId) {
  # parse table fallback
  $line = (& $cf tunnel list 2>&1 | Select-String $tunnelName | Select-Object -First 1).ToString()
  if ($line -match "([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})") {
    $tunnelId = $Matches[1]
  }
}
if (-not $tunnelId) { throw "no pude leer tunnel id" }

$cred = Join-Path $cfgDir "$tunnelId.json"
$configPath = Join-Path $PSScriptRoot "var\cloudflared-config.yml"
@"
tunnel: $tunnelId
credentials-file: $cred

ingress:
  - hostname: rpc.reeskova-chain.xyz
    service: http://127.0.0.1:80
  - service: http_status:404
"@ | Set-Content $configPath -Encoding utf8

Set-Content (Join-Path $PSScriptRoot "var\DOMAIN_CNAME.txt") -Encoding utf8 -Value @"
EN PORKBUN → DNS de reeskova-chain.xyz:

1) BORRAR los registros A y AAAA de host "rpc"
2) CREAR:
   Type: CNAME
   Host: rpc
   Answer: $tunnelId.cfargotunnel.com
   TTL: 600

Eso hace que rpc.reeskova-chain.xyz apunte al túnel mundial.
Cualquiera en el mundo usa:
  https://rpc.reeskova-chain.xyz/
  https://rpc.reeskova-chain.xyz/rpc
  https://rpc.reeskova-chain.xyz/community/link
"@

Write-Host ""
Write-Host "TUNNEL_ID=$tunnelId"
Write-Host "CNAME Answer = $tunnelId.cfargotunnel.com"
Write-Host "Config = $configPath"
Write-Host ""
Get-Content (Join-Path $PSScriptRoot "var\DOMAIN_CNAME.txt")
Write-Host ""
Write-Host "Arrancando túnel (dejalo corriendo)..."
Get-Process cloudflared -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep 1
Start-Process -FilePath $cf -ArgumentList @("tunnel","--config",$configPath,"run") -WindowStyle Minimized
Write-Host "cloudflared run OK. Cuando el CNAME esté en Porkbun, el dominio es mundial."
