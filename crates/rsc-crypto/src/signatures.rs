//! Firmas Ed25519 sobre el payload canónico de dominio.
//!
//! Se usa `verify_strict` de `ed25519-dalek`: rechaza puntos de orden
//! pequeño y encodings no canónicos. No se pre-hashea el mensaje con
//! SHA-256; Ed25519 aplica su propio hashing interno.

use crate::hash::{encode_domain_payload, CryptoDomain, CryptoVersion};
use crate::keys::{PrivateKey, PublicKey};
use crate::CryptoError;
use ed25519_dalek::{Signature as DalekSignature, Signer};

/// Esquema de firma. v1 solo [`Self::Ed25519`].
///
/// `#[non_exhaustive]` deja sitio a post-quantum sin inventar el algoritmo.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureScheme {
    /// Ed25519 (RSC Crypto v1).
    Ed25519,
}

/// Firma Ed25519: 64 bytes crudos.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature([u8; 64]);

impl Signature {
    /// Rechaza longitud distinta de 64. Los 64 bytes se aceptan aquí;
    /// la validez criptográfica se comprueba en [`verify`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let array = <[u8; 64]>::try_from(bytes).map_err(|_| CryptoError::InvalidSignature)?;
        DalekSignature::from_bytes(&array);
        Ok(Self(array))
    }

    /// 64 bytes.
    pub const fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    pub(crate) fn dalek(self) -> DalekSignature {
        DalekSignature::from_bytes(&self.0)
    }
}

impl core::fmt::Debug for Signature {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Signature({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

fn check_v1(version: CryptoVersion) -> Result<(), CryptoError> {
    if version != CryptoVersion::V1 {
        return Err(CryptoError::UnsupportedVersion);
    }
    Ok(())
}

/// Firma `message` en `domain` con versión V1.
pub fn sign(
    key: &PrivateKey,
    domain: CryptoDomain,
    message: &[u8],
) -> Result<Signature, CryptoError> {
    sign_with_version(key, domain, CryptoVersion::V1, message)
}

/// Firma con versión explícita. Solo V1 está implementada.
pub fn sign_with_version(
    key: &PrivateKey,
    domain: CryptoDomain,
    version: CryptoVersion,
    message: &[u8],
) -> Result<Signature, CryptoError> {
    check_v1(version)?;
    let payload = encode_domain_payload(domain, version, message);
    let sig = key.signing().sign(&payload);
    Ok(Signature(sig.to_bytes()))
}

/// Verifica una firma V1.
pub fn verify(
    key: &PublicKey,
    domain: CryptoDomain,
    message: &[u8],
    signature: &Signature,
) -> Result<(), CryptoError> {
    verify_with_version(key, domain, CryptoVersion::V1, message, signature)
}

/// Verifica con versión explícita.
///
/// `verify_strict` evita claves/firmas de orden pequeño que `verify`
/// podría aceptar. Es la modalidad de protocolo RSC v1.
pub fn verify_with_version(
    key: &PublicKey,
    domain: CryptoDomain,
    version: CryptoVersion,
    message: &[u8],
    signature: &Signature,
) -> Result<(), CryptoError> {
    check_v1(version)?;
    let payload = encode_domain_payload(domain, version, message);
    let vk = key.verifying()?;
    vk.verify_strict(&payload, &signature.dalek())
        .map_err(|_| CryptoError::SignatureVerificationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::PrivateKey;

    fn pair() -> (PrivateKey, PublicKey) {
        let sk = PrivateKey::from_bytes(&[11u8; 32]).unwrap();
        let pk = sk.public_key();
        (sk, pk)
    }

    #[test]
    fn test_sign() {
        let (sk, _) = pair();
        assert!(sign(&sk, CryptoDomain::Generic, b"m").is_ok());
    }

    #[test]
    fn test_verify() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Transaction, b"pay").unwrap();
        assert_eq!(verify(&pk, CryptoDomain::Transaction, b"pay", &sig), Ok(()));
    }

    #[test]
    fn test_wrong_message() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Generic, b"a").unwrap();
        assert_eq!(
            verify(&pk, CryptoDomain::Generic, b"b", &sig),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn test_wrong_public_key() {
        let (sk, _) = pair();
        let other = PrivateKey::from_bytes(&[12u8; 32]).unwrap().public_key();
        let sig = sign(&sk, CryptoDomain::Generic, b"m").unwrap();
        assert_eq!(
            verify(&other, CryptoDomain::Generic, b"m", &sig),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn test_modified_signature() {
        let (sk, pk) = pair();
        let mut bytes = sign(&sk, CryptoDomain::Generic, b"m").unwrap().to_bytes();
        bytes[10] ^= 0xff;
        let sig = Signature::from_bytes(&bytes).unwrap();
        assert_eq!(
            verify(&pk, CryptoDomain::Generic, b"m", &sig),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn test_signature_roundtrip() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Block, b"h").unwrap();
        let again = Signature::from_bytes(&sig.to_bytes()).unwrap();
        assert_eq!(verify(&pk, CryptoDomain::Block, b"h", &again), Ok(()));
    }

    #[test]
    fn test_invalid_signature_length() {
        assert_eq!(
            Signature::from_bytes(&[0u8; 32]).unwrap_err(),
            CryptoError::InvalidSignature
        );
    }

    #[test]
    fn test_same_message_different_domain() {
        let (sk, _) = pair();
        let a = sign(&sk, CryptoDomain::Transaction, b"m").unwrap();
        let b = sign(&sk, CryptoDomain::ConsensusVote, b"m").unwrap();
        assert_ne!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn test_signature_cannot_cross_domain() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::ConsensusVote, b"m").unwrap();
        assert_eq!(
            verify(&pk, CryptoDomain::Transaction, b"m", &sig),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn test_modified_domain_rejected() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Evidence, b"e").unwrap();
        assert!(verify(&pk, CryptoDomain::AccountIdentity, b"e", &sig).is_err());
    }

    #[test]
    fn test_modified_version_rejected() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Generic, b"m").unwrap();
        assert_eq!(
            verify_with_version(
                &pk,
                CryptoDomain::Generic,
                CryptoVersion::new(2),
                b"m",
                &sig
            ),
            Err(CryptoError::UnsupportedVersion)
        );
    }

    #[test]
    fn test_malformed_signature_rejected() {
        assert!(Signature::from_bytes(&[]).is_err());
    }

    #[test]
    fn test_wrong_signature_rejected() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Generic, b"x").unwrap();
        assert!(verify(&pk, CryptoDomain::Generic, b"y", &sig).is_err());
    }

    #[test]
    fn test_modified_message_rejected() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::NodeIdentity, b"n1").unwrap();
        assert!(verify(&pk, CryptoDomain::NodeIdentity, b"n2", &sig).is_err());
    }

    #[test]
    fn test_signature_verification_determinism() {
        let (sk, pk) = pair();
        let sig = sign(&sk, CryptoDomain::Generic, b"d").unwrap();
        assert_eq!(
            verify(&pk, CryptoDomain::Generic, b"d", &sig),
            verify(&pk, CryptoDomain::Generic, b"d", &sig)
        );
    }
}
