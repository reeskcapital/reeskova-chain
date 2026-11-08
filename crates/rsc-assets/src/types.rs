//! Identidad y metadatos de activos.

use crate::AssetsError;
use core::fmt;
use rsc_core::Hash;
use rsc_crypto::hash;

/// Versión de encoding del ledger de activos.
pub const ASSETS_SCHEMA_VERSION: u32 = 1;

/// Prefijo de dominio del activo nativo tipado.
const DOMAIN_NATIVE_ASSET: &[u8] = b"RSC-ASSETS-NATIVE/V1\0";

/// Identificador de activo de 32 bytes.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub Hash);

impl AssetId {
    /// Valor nulo.
    pub const ZERO: Self = Self(Hash::ZERO);

    /// Desde 32 bytes.
    pub fn from_slice(slice: &[u8]) -> Result<Self, AssetsError> {
        Hash::from_slice(slice)
            .map(Self)
            .map_err(|_| AssetsError::InvalidEncoding)
    }

    /// Bytes crudos.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0 .0
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId({})", self.0.to_hex())
    }
}

/// Metadatos de un activo registrado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetMetadata {
    /// Decimales de presentación / normalización (no afectan la aritmética u128).
    pub decimals: u8,
    /// Etiqueta canónica de hasta 8 bytes (relleno con ceros).
    pub symbol: [u8; 8],
}

impl AssetMetadata {
    /// Construye metadatos. `symbol` se trunca o rellena a 8 bytes ASCII.
    pub fn new(decimals: u8, symbol: &str) -> Self {
        let mut tag = [0u8; 8];
        let bytes = symbol.as_bytes();
        let n = bytes.len().min(8);
        tag[..n].copy_from_slice(&bytes[..n]);
        Self {
            decimals,
            symbol: tag,
        }
    }
}

/// Identificador canónico del activo nativo tipado.
///
/// `SHA-256(RSC-ASSETS-NATIVE/V1\0)`. Independiente del saldo `native_balance`
/// de `rsc-state`; el puente de génesis/mint lo fondea explícitamente.
pub fn native_asset_id() -> AssetId {
    let digest = hash(DOMAIN_NATIVE_ASSET);
    AssetId(Hash(*digest.as_bytes()))
}

/// Metadatos por defecto del activo nativo (18 decimales, símbolo RSK).
pub fn default_native_metadata() -> AssetMetadata {
    AssetMetadata::new(18, "RSK")
}
