@echo off
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" || exit /b 1
set "LIBCLANG_PATH=C:\Program Files\LLVM\bin"
set "PATH=C:\Program Files\LLVM\bin;%PATH%"
cd /d D:\rsc_chain
echo === rsc-genesis amm tests ===
cargo test -p rsc-genesis --lib amm_
if errorlevel 1 exit /b 1
echo === rsc-amm native closed policy ===
cargo test -p rsc-amm closed_policy_governor_creates
if errorlevel 1 exit /b 1
echo === rsc-node amm governor test ===
cargo test -p rsc-node amm_governor_create_pool_ok_outsider_rejected_survives_restart
if errorlevel 1 exit /b 1
echo === ALL OK ===
exit /b 0
