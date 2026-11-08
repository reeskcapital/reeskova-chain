//! Capa criptográfica de RSC Chain v1.
//!
//! Proporciona hash, claves, firmas e identidades. No conoce bloques,
//! consenso, estado, red ni economía.

#![forbid(unsafe_code)]

pub mod aggregate;
pub mod hash;
pub mod keys;
pub mod signatures;

pub use aggregate::{verify_signature_bundle, SignatureBundle};
pub use hash::{
    encode_domain_payload, hash, hash_domain, CryptoDomain, CryptoVersion, Hash256, HashBuilder,
};
pub use keys::{KeyId, PrivateKey, PublicKey};
pub use signatures::{
    sign, sign_with_version, verify, verify_with_version, Signature, SignatureScheme,
};

use core::fmt;

/// Error de material o verificación criptográfica.
///
/// No incluye secretos en el mensaje.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// El slice no tiene 32 bytes.
    InvalidHashLength,
    /// Clave pública inválida o de longitud incorrecta.
    InvalidPublicKey,
    /// Clave privada de longitud incorrecta.
    InvalidPrivateKey,
    /// Firma de longitud incorrecta o bytes no canónicos.
    InvalidSignature,
    /// La firma no corresponde a clave + dominio + mensaje.
    SignatureVerificationFailed,
    /// Esquema no soportado en v1.
    UnsupportedAlgorithm,
    /// Versión criptográfica distinta de [`CryptoVersion::V1`].
    UnsupportedVersion,
    /// Identificador de dominio desconocido.
    InvalidDomain,
    /// Encoding de dominio mal formado.
    InvalidEncoding,
    /// El CSPRNG del sistema falló.
    RandomnessFailure,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidHashLength => "invalid hash length",
            Self::InvalidPublicKey => "invalid public key",
            Self::InvalidPrivateKey => "invalid private key",
            Self::InvalidSignature => "invalid signature",
            Self::SignatureVerificationFailed => "signature verification failed",
            Self::UnsupportedAlgorithm => "unsupported algorithm",
            Self::UnsupportedVersion => "unsupported crypto version",
            Self::InvalidDomain => "invalid crypto domain",
            Self::InvalidEncoding => "invalid crypto encoding",
            Self::RandomnessFailure => "randomness failure",
        })
    }
}

impl std::error::Error for CryptoError {}
