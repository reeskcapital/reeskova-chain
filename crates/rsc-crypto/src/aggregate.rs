//! Haz ordenado de firmas Ed25519 sobre un mismo mensaje.
//!
//! Ed25519 no comprime varias firmas en una sola. El haz conserva cada
//! firma de 64 bytes. [`verify_signature_bundle`] comprueba el par
//! (clave, firma) en el orden que entrega el llamador.

use crate::hash::CryptoDomain;
use crate::keys::PublicKey;
use crate::signatures::{verify, Signature};
use crate::CryptoError;

const SIGNATURE_LEN: usize = 64;

/// Firmas sobre un mismo mensaje, en un orden fijado por el llamador.
#[derive(Clone, PartialEq, Eq)]
pub struct SignatureBundle {
    signatures: Vec<Signature>,
}

impl SignatureBundle {
    /// Rechaza un haz vacío y un conteo que no quepa en `u32`.
    pub fn from_signatures(signatures: &[Signature]) -> Result<Self, CryptoError> {
        if signatures.is_empty() || u32::try_from(signatures.len()).is_err() {
            return Err(CryptoError::InvalidSignature);
        }
        Ok(Self {
            signatures: signatures.to_vec(),
        })
    }

    /// Firmas, en el orden almacenado.
    pub fn as_slice(&self) -> &[Signature] {
        &self.signatures
    }

    /// `count` u32 little-endian, luego cada firma de 64 bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = u32::try_from(self.signatures.len()).expect("count checked at construction");
        let mut out = Vec::with_capacity(4 + self.signatures.len() * SIGNATURE_LEN);
        out.extend_from_slice(&count.to_le_bytes());
        for signature in &self.signatures {
            out.extend_from_slice(&signature.to_bytes());
        }
        out
    }

    /// Invierte [`Self::to_bytes`]. El conteo tiene que coincidir con los bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let count_bytes: [u8; 4] = bytes
            .get(..4)
            .ok_or(CryptoError::InvalidEncoding)?
            .try_into()
            .map_err(|_| CryptoError::InvalidEncoding)?;
        let count = u32::from_le_bytes(count_bytes);
        if count == 0 {
            return Err(CryptoError::InvalidSignature);
        }
        let rest = &bytes[4..];
        let expected = usize::try_from(count)
            .ok()
            .and_then(|n| n.checked_mul(SIGNATURE_LEN))
            .ok_or(CryptoError::InvalidEncoding)?;
        if rest.len() != expected {
            return Err(CryptoError::InvalidEncoding);
        }
        let mut signatures = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        for chunk in rest.chunks(SIGNATURE_LEN) {
            signatures.push(Signature::from_bytes(chunk)?);
        }
        Ok(Self { signatures })
    }
}

impl core::fmt::Debug for SignatureBundle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SignatureBundle")
            .field("count", &self.signatures.len())
            .finish()
    }
}

/// Verifica cada firma contra la clave de la misma posición.
///
/// El mensaje y el dominio son únicos para todo el haz. Un orden distinto
/// de claves o de firmas no verifica.
pub fn verify_signature_bundle(
    keys: &[PublicKey],
    domain: CryptoDomain,
    message: &[u8],
    bundle: &SignatureBundle,
) -> Result<(), CryptoError> {
    if keys.len() != bundle.signatures.len() {
        return Err(CryptoError::InvalidEncoding);
    }
    for (key, signature) in keys.iter().zip(bundle.signatures.iter()) {
        verify(key, domain, message, signature)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::PrivateKey;
    use crate::sign;

    fn key(byte: u8) -> (PrivateKey, PublicKey) {
        let secret = PrivateKey::from_bytes(&[byte; 32]).unwrap();
        let public = secret.public_key();
        (secret, public)
    }

    fn bundle(message: &[u8]) -> (Vec<PublicKey>, SignatureBundle) {
        let signers = [key(1), key(2), key(3)];
        let signatures: Vec<Signature> = signers
            .iter()
            .map(|(secret, _)| sign(secret, CryptoDomain::ConsensusVote, message).unwrap())
            .collect();
        let keys = signers.into_iter().map(|(_, public)| public).collect();
        (keys, SignatureBundle::from_signatures(&signatures).unwrap())
    }

    #[test]
    fn bundle_verifies_in_signer_order() {
        let message = b"vote";
        let (keys, packed) = bundle(message);
        assert_eq!(
            verify_signature_bundle(&keys, CryptoDomain::ConsensusVote, message, &packed),
            Ok(())
        );
        let again = SignatureBundle::from_bytes(&packed.to_bytes()).unwrap();
        assert_eq!(again, packed);
        assert_eq!(
            verify_signature_bundle(&keys, CryptoDomain::ConsensusVote, message, &again),
            Ok(())
        );
    }

    #[test]
    fn swapped_order_is_rejected() {
        let message = b"vote";
        let (mut keys, packed) = bundle(message);
        keys.swap(0, 1);
        assert_eq!(
            verify_signature_bundle(&keys, CryptoDomain::ConsensusVote, message, &packed),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn altered_signature_is_rejected() {
        let message = b"vote";
        let (keys, packed) = bundle(message);
        let mut signatures = packed.as_slice().to_vec();
        let mut raw = signatures[1].to_bytes();
        raw[3] ^= 0x01;
        signatures[1] = Signature::from_bytes(&raw).unwrap();
        let packed = SignatureBundle::from_signatures(&signatures).unwrap();
        assert_eq!(
            verify_signature_bundle(&keys, CryptoDomain::ConsensusVote, message, &packed),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn empty_and_truncated_bytes_are_rejected() {
        assert_eq!(
            SignatureBundle::from_signatures(&[]).unwrap_err(),
            CryptoError::InvalidSignature
        );
        assert_eq!(
            SignatureBundle::from_bytes(&[]).unwrap_err(),
            CryptoError::InvalidEncoding
        );
        assert_eq!(
            SignatureBundle::from_bytes(&0u32.to_le_bytes()).unwrap_err(),
            CryptoError::InvalidSignature
        );
        let (keys, packed) = bundle(b"vote");
        let mut bytes = packed.to_bytes();
        bytes.pop();
        assert_eq!(
            SignatureBundle::from_bytes(&bytes).unwrap_err(),
            CryptoError::InvalidEncoding
        );
        assert_eq!(
            verify_signature_bundle(&keys[..2], CryptoDomain::ConsensusVote, b"vote", &packed),
            Err(CryptoError::InvalidEncoding)
        );
    }

    #[test]
    fn wrong_message_or_domain_is_rejected() {
        let (keys, packed) = bundle(b"vote");
        assert_eq!(
            verify_signature_bundle(&keys, CryptoDomain::ConsensusVote, b"other", &packed),
            Err(CryptoError::SignatureVerificationFailed)
        );
        assert_eq!(
            verify_signature_bundle(&keys, CryptoDomain::ConsensusQc, b"vote", &packed),
            Err(CryptoError::SignatureVerificationFailed)
        );
    }
}
