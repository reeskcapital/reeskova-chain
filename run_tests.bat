@echo off
REM Ejecutar tests desde D:\rsc_chain (todo en D:)
cd /d D:\rsc_chain
set CARGO_TARGET_DIR=D:\rsc_chain\target
set RUSTUP_HOME=D:\.rustup
set CARGO_HOME=D:\.cargo
cargo clean
cargo update
cargo test --lib chain
pause
