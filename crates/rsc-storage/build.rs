//! Enlaza libs de sistema que RocksDB exige en MSVC cuando el harness de test
//! (u otros bins) linkea `rsc-storage` estáticamente.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows") {
        println!("cargo:rustc-link-lib=dylib=rpcrt4");
        println!("cargo:rustc-link-lib=dylib=shlwapi");
    }
}
