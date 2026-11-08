@echo off
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" || exit /b 1
set "LIBCLANG_PATH=C:\Program Files\LLVM\bin"
set "PATH=C:\Program Files\LLVM\bin;%PATH%"
cd /d D:\rsc_chain
cargo test -p rsc-genesis --lib changing_amm_permissionless
exit /b %ERRORLEVEL%
