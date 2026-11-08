//! El binario que el smoke de proceso ejecuta.
//!
//! `cargo test` construye este target y, con él, `rsc-validator`.

#[test]
fn validator_binary_rejects_missing_args() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rsc-validator"))
        .output()
        .expect("binario");
    assert!(!output.status.success());
    let err = String::from_utf8(output.stderr).expect("stderr");
    assert!(err.contains("falta key-file"), "{err}");
}
