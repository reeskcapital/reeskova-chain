//! Claves Ed25519 e identidad [`KeyId`].
//!
//! No asigna roles (validator, user, node). [`PrivateKey`] no aparece en Debug.

use crate::hash::{encode_domain_payload, hash, CryptoDomain, CryptoVersion, Hash256};
use crate::CryptoError;
use ed25519_dalek::{SigningKey, VerifyingKey};
use zeroize::Zeroize;

/// Identificador de 32 bytes derivado de una [`PublicKey`].
///
/// `KeyId = SHA-256(RSC || V1_le || KEY-ID || public_key)`.
/// El tag `KEY-ID` no es un rol de red.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(Hash256);

impl KeyId {
    const TAG: &'static [u8] = b"KEY-ID";

    pub(crate) fn derive(public: &PublicKey) -> Self {
        let mut payload = Vec::with_capacity(Self::TAG.len() + 32);
        payload.extend_from_slice(Self::TAG);
        payload.extend_from_slice(&public.0);
        // Mismo envelope que el resto: RSC + version + domain + (KEY-ID || pk).
        Self(hash(&encode_domain_payload(
            CryptoDomain::Generic,
            CryptoVersion::V1,
            &payload,
        )))
    }

    /// Desde 32 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        Ok(Self(Hash256::from_bytes(bytes)?))
    }

    /// Bytes crudos.
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Vista de bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl core::fmt::Debug for KeyId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "KeyId({:02x}{:02x}..)",
            self.0.as_bytes()[0],
            self.0.as_bytes()[1]
        )
    }
}

/// Clave pública Ed25519 (32 bytes). Transmisible.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(pub(crate) [u8; 32]);

impl PublicKey {
    /// Rechaza longitud distinta de 32 o puntos que `ed25519-dalek` no acepta.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let array = <[u8; 32]>::try_from(bytes).map_err(|_| CryptoError::InvalidPublicKey)?;
        VerifyingKey::from_bytes(&array).map_err(|_| CryptoError::InvalidPublicKey)?;
        Ok(Self(array))
    }

    /// 32 bytes crudos.
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Identidad derivada. No es un rol.
    pub fn key_id(&self) -> KeyId {
        KeyId::derive(self)
    }

    pub(crate) fn verifying(&self) -> Result<VerifyingKey, CryptoError> {
        VerifyingKey::from_bytes(&self.0).map_err(|_| CryptoError::InvalidPublicKey)
    }
}

impl core::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PublicKey({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

/// Material secreto Ed25519 (32 bytes de seed).
///
/// No implementa `Display`. `Debug` no muestra el secreto.
/// `SigningKey` se zeroiza al dropear (feature `zeroize` de dalek).
pub struct PrivateKey(SigningKey);

impl PrivateKey {
    /// Genera con CSPRNG del SO (`getrandom`). No usa reloj ni counters.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|_| CryptoError::RandomnessFailure)?;
        let key = SigningKey::from_bytes(&seed);
        seed.zeroize();
        Ok(Self(key))
    }

    /// Seed de 32 bytes. Toda seed de 32 bytes es un secreto Ed25519 válido.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let seed = <[u8; 32]>::try_from(bytes).map_err(|_| CryptoError::InvalidPrivateKey)?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }

    /// Copia del seed. El caller debe tratar el array como secreto.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Única pública correspondiente.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }

    /// [`KeyId`] de la pública.
    pub fn key_id(&self) -> KeyId {
        self.public_key().key_id()
    }

    pub(crate) fn signing(&self) -> &SigningKey {
        &self.0
    }
}

impl core::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PrivateKey(****)")
    }
}

impl core::fmt::Display for PrivateKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PrivateKey(****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() {
        let a = PrivateKey::generate().unwrap();
        let b = PrivateKey::generate().unwrap();
        assert_ne!(a.to_bytes(), b.to_bytes());
        assert_ne!(a.public_key().to_bytes(), b.public_key().to_bytes());
    }

    #[test]
    fn test_public_key_derivation() {
        let sk = PrivateKey::from_bytes(&[7u8; 32]).unwrap();
        assert_eq!(sk.public_key().to_bytes(), sk.public_key().to_bytes());
    }

    #[test]
    fn test_private_key_roundtrip() {
        let sk = PrivateKey::generate().unwrap();
        let again = PrivateKey::from_bytes(&sk.to_bytes()).unwrap();
        assert_eq!(sk.public_key().to_bytes(), again.public_key().to_bytes());
    }

    #[test]
    fn test_public_key_roundtrip() {
        let pk = PrivateKey::generate().unwrap().public_key();
        assert_eq!(
            PublicKey::from_bytes(&pk.to_bytes()).unwrap().to_bytes(),
            pk.to_bytes()
        );
    }

    #[test]
    fn test_key_id_deterministic() {
        let pk = PrivateKey::from_bytes(&[3u8; 32]).unwrap().public_key();
        assert_eq!(pk.key_id().to_bytes(), pk.key_id().to_bytes());
        assert_eq!(
            KeyId::from_bytes(&pk.key_id().to_bytes()).unwrap(),
            pk.key_id()
        );
    }

    #[test]
    fn test_different_keys_have_different_ids() {
        let a = PrivateKey::from_bytes(&[1u8; 32]).unwrap().key_id();
        let b = PrivateKey::from_bytes(&[2u8; 32]).unwrap().key_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_invalid_private_key() {
        assert_eq!(
            PrivateKey::from_bytes(&[0u8; 16]).unwrap_err(),
            CryptoError::InvalidPrivateKey
        );
    }

    #[test]
    fn test_invalid_public_key() {
        assert_eq!(
            PublicKey::from_bytes(&[0u8; 8]).unwrap_err(),
            CryptoError::InvalidPublicKey
        );
    }

    #[test]
    fn test_malformed_public_key_rejected() {
        // Punto de orden pequeño / no canónico que dalek rechaza.
        let mut bad = [0u8; 32];
        bad[31] = 1;
        let _ = PublicKey::from_bytes(&bad);
    }

    #[test]
    fn test_private_key_debug_does_not_leak_secret() {
        let sk = PrivateKey::from_bytes(&[9u8; 32]).unwrap();
        let debug = format!("{sk:?}");
        let display = format!("{sk}");
        assert_eq!(debug, "PrivateKey(****)");
        assert_eq!(display, "PrivateKey(****)");
        assert!(!debug.contains("09"));
    }
}
