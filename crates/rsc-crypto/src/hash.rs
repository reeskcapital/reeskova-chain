//! SHA-256, [`Hash256`] y separación de dominio.
//!
//! No implementa SHA-256; usa `sha2`. No conoce bloques ni votos.

use crate::CryptoError;
use sha2::{Digest, Sha256};

/// Versión del encoding criptográfico. Única constante de versión: V1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CryptoVersion(u16);

impl CryptoVersion {
    /// RSC Crypto v1.
    pub const V1: Self = Self(1);

    /// Valor numérico. Serialización: little-endian de 2 bytes.
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Construye una versión. Solo [`Self::V1`] está implementada para firmar.
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }
}

/// Identificador de contexto criptográfico. No es un tipo de blockchain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CryptoDomain {
    /// Uso genérico.
    Generic,
    /// Transacción.
    Transaction,
    /// Bloque.
    Block,
    /// Propuesta de consenso.
    ConsensusProposal,
    /// Voto de consenso.
    ConsensusVote,
    /// Quorum certificate.
    ConsensusQc,
    /// Identidad de validator.
    ValidatorIdentity,
    /// Identidad de nodo.
    NodeIdentity,
    /// Identidad de cuenta.
    AccountIdentity,
    /// Evidencia.
    Evidence,
    /// Beacon determinista y verificable de consenso.
    ConsensusBeacon,
}

impl CryptoDomain {
    /// Byte estable del dominio. No es endianness de un entero de plataforma.
    pub const fn id(self) -> u8 {
        match self {
            Self::Generic => 0,
            Self::Transaction => 1,
            Self::Block => 2,
            Self::ConsensusProposal => 3,
            Self::ConsensusVote => 4,
            Self::ConsensusQc => 5,
            Self::ValidatorIdentity => 6,
            Self::NodeIdentity => 7,
            Self::AccountIdentity => 8,
            Self::Evidence => 9,
            Self::ConsensusBeacon => 10,
        }
    }

    /// Inverse de [`Self::id`].
    pub const fn from_id(id: u8) -> Result<Self, CryptoError> {
        Ok(match id {
            0 => Self::Generic,
            1 => Self::Transaction,
            2 => Self::Block,
            3 => Self::ConsensusProposal,
            4 => Self::ConsensusVote,
            5 => Self::ConsensusQc,
            6 => Self::ValidatorIdentity,
            7 => Self::NodeIdentity,
            8 => Self::AccountIdentity,
            9 => Self::Evidence,
            10 => Self::ConsensusBeacon,
            _ => return Err(CryptoError::InvalidDomain),
        })
    }
}

/// Digest SHA-256 de 32 bytes. Tipo fuerte; no es un `BlockHash`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash256([u8; 32]);

impl Hash256 {
    /// Digest nulo.
    pub const fn zero() -> Self {
        Self([0u8; 32])
    }

    /// Desde 32 bytes exactos.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let array = <[u8; 32]>::try_from(bytes).map_err(|_| CryptoError::InvalidHashLength)?;
        Ok(Self(array))
    }

    /// Vista de los 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Copia de los 32 bytes.
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl core::fmt::Debug for Hash256 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Hash256({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

/// SHA-256 de `data`. Determinista. Sin reloj ni aleatoriedad.
pub fn hash(data: &[u8]) -> Hash256 {
    let digest = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Hash256(out)
}

/// SHA-256 incremental. `finalize` ≡ `hash(concat updates)`.
pub struct HashBuilder {
    hasher: Sha256,
}

impl HashBuilder {
    /// Estado vacío.
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    /// Añade bytes al stream.
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    /// Cierra el digest.
    pub fn finalize(self) -> Hash256 {
        let digest = self.hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Hash256(out)
    }
}

impl Default for HashBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Magic ASCII `RSC` (3 bytes).
pub const MAGIC: &[u8; 3] = b"RSC";

/// Payload canónico: `RSC || version_le_u16 || domain_id || data`.
///
/// `version` se escribe little-endian de forma explícita.
pub fn encode_domain_payload(domain: CryptoDomain, version: CryptoVersion, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + 2 + 1 + data.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&version.as_u16().to_le_bytes());
    out.push(domain.id());
    out.extend_from_slice(data);
    out
}

/// SHA-256 del payload de dominio.
pub fn hash_domain(domain: CryptoDomain, version: CryptoVersion, data: &[u8]) -> Hash256 {
    hash(&encode_domain_payload(domain, version, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_known_vector() {
        // Vector de la documentación de `sha2` para "hello world".
        let got = hash(b"hello world");
        let expect = hex_array("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        assert_eq!(got.to_bytes(), expect);
    }

    #[test]
    fn test_hash_empty() {
        let empty = hash(b"");
        assert_eq!(
            empty.to_bytes(),
            hex_array("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn test_hash_small_input() {
        assert_ne!(hash(b"a"), hash(b"b"));
        assert_eq!(hash(b"a"), hash(b"a"));
    }

    #[test]
    fn test_hash_large_input() {
        let data = vec![0x5au8; 100_000];
        assert_eq!(hash(&data), hash(&data));
        assert_ne!(hash(&data), hash(b""));
    }

    #[test]
    fn test_hash_deterministic() {
        assert_eq!(hash(b"rsc"), hash(b"rsc"));
    }

    #[test]
    fn test_hash_bytes_roundtrip() {
        let h = hash(b"round");
        assert_eq!(Hash256::from_bytes(h.as_bytes()).unwrap(), h);
        assert_eq!(h.to_bytes(), *h.as_bytes());
        assert_eq!(
            Hash256::from_bytes(&[0u8; 16]).unwrap_err(),
            CryptoError::InvalidHashLength
        );
    }

    #[test]
    fn test_hash_builder() {
        let mut b = HashBuilder::new();
        b.update(b"ab");
        b.update(b"cd");
        assert_eq!(b.finalize(), hash(b"abcd"));
    }

    #[test]
    fn test_hash_builder_matches_direct_hash() {
        assert_eq!(HashBuilder::new().finalize(), hash(b""));
    }

    #[test]
    fn test_hash_builder_multiple_updates() {
        let mut b = HashBuilder::new();
        b.update(b"xy");
        b.update(&[]);
        b.update(b"z");
        assert_eq!(b.finalize(), hash(b"xyz"));
    }

    #[test]
    fn test_hash_builder_empty() {
        assert_eq!(HashBuilder::new().finalize(), hash(&[]));
    }

    #[test]
    fn test_hash_cannot_cross_domain() {
        let a = hash_domain(CryptoDomain::Transaction, CryptoVersion::V1, b"m");
        let b = hash_domain(CryptoDomain::ConsensusVote, CryptoVersion::V1, b"m");
        assert_ne!(a, b);
    }

    #[test]
    fn test_version_changes_domain_payload() {
        let v1 = encode_domain_payload(CryptoDomain::Block, CryptoVersion::V1, b"x");
        let v2 = encode_domain_payload(CryptoDomain::Block, CryptoVersion::new(2), b"x");
        assert_ne!(v1, v2);
        assert_eq!(
            encode_domain_payload(CryptoDomain::Block, CryptoVersion::V1, b"x"),
            v1
        );
    }

    #[test]
    fn test_domain_encoding_determinism() {
        let a = encode_domain_payload(CryptoDomain::Evidence, CryptoVersion::V1, b"e");
        let b = encode_domain_payload(CryptoDomain::Evidence, CryptoVersion::V1, b"e");
        assert_eq!(a, b);
        assert!(a.starts_with(b"RSC"));
        assert_eq!(&a[3..5], &1u16.to_le_bytes());
        assert_eq!(
            CryptoDomain::from_id(99).unwrap_err(),
            CryptoError::InvalidDomain
        );
        assert_eq!(CryptoDomain::ConsensusBeacon.id(), 10);
        assert_eq!(CryptoDomain::from_id(10).unwrap(), CryptoDomain::ConsensusBeacon);
        assert_ne!(CryptoDomain::ConsensusBeacon.id(), CryptoDomain::ConsensusVote.id());
    }

    fn hex_array(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }
}
