//! Tamaños de encoding y el origen de altura.
//!
//! Los límites de bloque, la escala de `Amount` y las identidades de red
//! no viven aquí. Los fija el documento de génesis.

/// 32 bytes = 256 bits.
pub const HASH_SIZE: usize = 32;
/// Identificador de cuenta.
pub const ACCOUNT_ID_SIZE: usize = 32;
/// Contenedor de clave pública. La cripto está en `rsc-crypto`.
pub const PUBLIC_KEY_SIZE: usize = 32;
/// Firma del esquema adoptado.
pub const SIGNATURE_SIZE: usize = 64;
/// Altura del génesis. Es el origen del tipo, no un parámetro de red.
pub const GENESIS_HEIGHT: u64 = 0;
