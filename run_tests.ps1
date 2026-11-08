# Ejecutar tests desde D:\rsc_chain (todo en D:)
Set-Location D:\rsc_chain
$env:CARGO_TARGET_DIR = "D:\rsc_chain\target"
$env:RUSTUP_HOME = "D:\.rustup"
$env:CARGO_HOME = "D:\.cargo"
# Limpiar y actualizar si falla thiserror_impl u otras dependencias
cargo clean
cargo update
cargo test --lib chain
