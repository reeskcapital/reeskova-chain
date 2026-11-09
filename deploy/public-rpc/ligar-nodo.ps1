# Liga un nodo comunitario.
# Sin wallet en este dispositivo, pide crear una o iniciar sesión.
# La sesión queda en %USERPROFILE%\.rsc-chain\session.json y no se cierra sola.
$ErrorActionPreference = "Stop"
$SessionDir = Join-Path $env:USERPROFILE ".rsc-chain"
$Session = Join-Path $SessionDir "session.json"
New-Item -ItemType Directory -Force -Path $SessionDir | Out-Null

function Invoke-Rsc([string]$Url, [string]$Method, [string]$BodyFile) {
    $args = @("-s", "-6", "--max-time", "20", "-X", $Method, $Url, "-H", "Content-Type: application/json")
    if ($BodyFile) { $args += @("--data-binary", "@$BodyFile") }
    $public = & curl.exe @args
    if ($LASTEXITCODE -eq 0 -and $public -and $public -notmatch "404 Not Found") { return $public }
    $local = $Url -replace "http://rpc\.reeskova-chain\.xyz", "http://127.0.0.1:8545"
    if ($local -match "/community") { $local = $local -replace "http://127.0.0.1:8545", "http://127.0.0.1:8547" }
    $args = @("-s", "--max-time", "20", "-X", $Method, $local, "-H", "Content-Type: application/json")
    if ($BodyFile) { $args += @("--data-binary", "@$BodyFile") }
    & curl.exe @args
}

function Read-Secret([string]$Prompt) {
    $secure = Read-Host $Prompt -AsSecureString
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr) }
}

function Save-Session($wallet, [string]$Name) {
    $obj = [pscustomobject]@{
        name         = $Name
        address      = $wallet.result.address
        account_id   = $wallet.result.account_id
        public_key   = $wallet.result.public_key
        signing_key  = $wallet.result.signing_key
        seed_phrase  = $wallet.result.seed_phrase
    }
    $obj | ConvertTo-Json | Set-Content -Path $Session -Encoding utf8
}

function New-Wallet {
    $name = Read-Host "Nombre completo"
    $password = Read-Secret "Contrasena (minimo 8)"
    $again = Read-Secret "Repite la contrasena"
    if ($password -ne $again) { throw "las contrasenas no coinciden" }
    if ($password.Length -lt 8 -or $password.Length -gt 128) {
        throw "la contrasena debe tener entre 8 y 128 caracteres"
    }
    $file = Join-Path $env:TEMP "rsc-wallet-create.json"
    $json = @{ name = $name; password = $password } | ConvertTo-Json -Compress
    [System.IO.File]::WriteAllText($file, $json)
    $raw = Invoke-Rsc "http://rpc.reeskova-chain.xyz/wallet/create" "POST" $file
    Remove-Item $file -Force -ErrorAction SilentlyContinue
    $wallet = $raw | ConvertFrom-Json
    if (-not $wallet.ok) { throw "no se pudo crear: $raw" }
    Save-Session $wallet $name
    Write-Host ""
    Write-Host "Wallet creada: $($wallet.result.address)"
    Write-Host "Frase secreta (guardala; es la unica forma de firmar):"
    Write-Host $wallet.result.seed_phrase
    Write-Host ""
}

function Open-Wallet {
    $name = Read-Host "Nombre completo"
    $password = Read-Secret "Contrasena"
    $phrase = Read-Host "Frase secreta"
    $file = Join-Path $env:TEMP "rsc-wallet-login.json"
    $json = @{ name = $name; seed_phrase = $phrase; password = $password } | ConvertTo-Json -Compress
    [System.IO.File]::WriteAllText($file, $json)
    $raw = Invoke-Rsc "http://rpc.reeskova-chain.xyz/wallet/login" "POST" $file
    Remove-Item $file -Force -ErrorAction SilentlyContinue
    $wallet = $raw | ConvertFrom-Json
    if (-not $wallet.ok) { throw "nombre, contrasena o frase no coinciden: $raw" }
    Save-Session $wallet $name
    Write-Host "Sesion de $name abierta."
}

if (Test-Path $Session) {
    $saved = Get-Content $Session -Raw | ConvertFrom-Json
    Write-Host "Sesion local: $($saved.name)  $($saved.address)"
} else {
    Write-Host "Esta dispositivo no tiene wallet."
    Write-Host "1 crear wallet"
    Write-Host "2 iniciar sesion"
    $choice = Read-Host "elige"
    if ($choice -eq "1") { New-Wallet }
    elseif ($choice -eq "2") { Open-Wallet }
    else { throw "opcion invalida" }
    $saved = Get-Content $Session -Raw | ConvertFrom-Json
}

$file = Join-Path $env:TEMP "rsc-link-user.json"
[System.IO.File]::WriteAllText($file, (@{ user = $saved.address } | ConvertTo-Json -Compress))
$linked = Invoke-Rsc "http://rpc.reeskova-chain.xyz/community/link" "POST" $file
Write-Host $linked
