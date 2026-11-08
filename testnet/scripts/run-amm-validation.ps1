$ErrorActionPreference = "Continue"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root
$Report = Join-Path $Root "testnet\var\last-validation.txt"
New-Item -ItemType Directory -Force -Path (Join-Path $Root "testnet\var") | Out-Null

function Invoke-CargoSuite {
  param(
    [string]$Title,
    [string[]]$CargoArgs
  )
  Write-Host "`n=== $Title ==="
  Add-Content -Path $Report -Value "`n=== $Title ===" -Encoding utf8
  $prev = $ErrorActionPreference
  $ErrorActionPreference = "SilentlyContinue"
  $lines = & cargo @CargoArgs 2>&1 | ForEach-Object { $_.ToString() }
  $ErrorActionPreference = $prev
  $lines | ForEach-Object {
    Write-Host $_
    Add-Content -Path $Report -Value $_ -Encoding utf8
  }
  if ($LASTEXITCODE -ne 0) {
    throw "suite failed: $Title (exit $LASTEXITCODE)"
  }
}

Set-Content -Path $Report -Value "" -Encoding utf8
Invoke-CargoSuite -Title "rsc-amm Stage7 (no native)" -CargoArgs @("test","-p","rsc-amm","--lib","--no-default-features","--","stage7_")
Invoke-CargoSuite -Title "rsc-amm unit (no native)" -CargoArgs @("test","-p","rsc-amm","--lib","--no-default-features")
Invoke-CargoSuite -Title "rsc-ledger" -CargoArgs @("test","-p","rsc-ledger","--lib")

Write-Host "`nAttempting native suite (may fail if RocksDB toolchain broken)..."
Add-Content -Path $Report -Value "`n=== native attempt ===" -Encoding utf8
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$prev = $ErrorActionPreference
$ErrorActionPreference = "SilentlyContinue"
$nativeLines = & cargo test -p rsc-amm --lib --features native -- native::stage7_native 2>&1 | ForEach-Object { $_.ToString() }
$ErrorActionPreference = $prev
$nativeLines | ForEach-Object {
  Write-Host $_
  Add-Content -Path $Report -Value $_ -Encoding utf8
}
if ($LASTEXITCODE -ne 0) {
  Add-Content -Path $Report -Value "NATIVE: NOT EXECUTED / FAILED (do not mark PASS)" -Encoding utf8
  Write-Host "NATIVE suite did not pass — recorded as not passed."
} else {
  Add-Content -Path $Report -Value "NATIVE: PASS" -Encoding utf8
}

Write-Host "`nReport: $Report"
