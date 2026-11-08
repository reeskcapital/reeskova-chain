//! Wallet que genera la cadena.
//!
//! La dirección siempre empieza por `rsc` y sigue con la cuenta derivada de
//! la clave. Junto con la clave se entrega la frase semilla de 12 palabras.

#![forbid(unsafe_code)]

mod phrase;

use rsc_crypto::{HashBuilder, KeyId, PrivateKey};
use zeroize::Zeroize;

/// Mismo dominio que `derive_account_id` en el estado.
const ACCOUNT_ID_DOMAIN: &[u8] = b"RSC/STATE/ACCOUNT-ID/V1";

/// Prefijo de toda dirección de wallet.
pub const ADDRESS_PREFIX: &str = "rsc";

/// La clave no se pudo generar.
#[derive(Debug)]
pub struct WalletError;

impl std::fmt::Display for WalletError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("clave inválida")
    }
}

/// Wallet generada. El secreto no sale en `Debug`.
pub struct Wallet {
    address: String,
    account_id: [u8; 32],
    key_id: KeyId,
    public_key: [u8; 32],
    seed: [u8; 32],
    phrase: String,
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Wallet")
            .field("address", &self.address)
            .finish()
    }
}

impl Drop for Wallet {
    fn drop(&mut self) {
        self.seed.zeroize();
        self.phrase.zeroize();
    }
}

impl Wallet {
    /// Genera una clave nueva y su frase de 12 palabras.
    pub fn generate() -> Result<Self, WalletError> {
        let entropy = phrase::random_entropy()?;
        let words = phrase::phrase_from_entropy(&entropy)?;
        Self::from_phrase(&words)
    }

    /// Recupera la wallet desde la frase semilla.
    pub fn from_phrase(words: &str) -> Result<Self, WalletError> {
        let mut derived = phrase::seed_from_phrase(words)?;
        let key = PrivateKey::from_bytes(&derived[..32]).map_err(|_| WalletError)?;
        derived.zeroize();
        Self::from_key(key, words)
    }

    fn from_key(key: PrivateKey, words: &str) -> Result<Self, WalletError> {
        let public_key = key.public_key();
        let key_id = key.key_id();
        let account_id = account_id_of(&key_id);
        let address = format!("{ADDRESS_PREFIX}{}", hex(&account_id));
        Ok(Self {
            address,
            account_id,
            key_id,
            public_key: public_key.to_bytes(),
            seed: key.to_bytes(),
            phrase: words.to_string(),
        })
    }

    /// Dirección que empieza por `rsc`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Cuenta de 32 bytes derivada de la clave.
    pub fn account_id(&self) -> &[u8; 32] {
        &self.account_id
    }

    /// Cuenta en hexadecimal minúsculo, sin el prefijo `rsc`.
    pub fn account_id_hex(&self) -> String {
        hex(&self.account_id)
    }

    /// Propietario criptográfico de la cuenta.
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// Clave pública de 32 bytes.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// Clave pública en hexadecimal minúsculo.
    pub fn public_key_hex(&self) -> String {
        hex(&self.public_key)
    }

    /// Semilla de firma, en hexadecimal. Quien recibe la wallet la guarda.
    pub fn signing_key_hex(&self) -> String {
        hex(&self.seed)
    }

    /// Semilla de 32 bytes. El llamador la trata como secreto.
    pub fn signing_key_bytes(&self) -> [u8; 32] {
        self.seed
    }

    /// Frase semilla de 12 palabras. Quien recibe la wallet la guarda.
    pub fn seed_phrase(&self) -> &str {
        &self.phrase
    }
}

fn account_id_of(key_id: &KeyId) -> [u8; 32] {
    let mut builder = HashBuilder::new();
    builder.update(ACCOUNT_ID_DOMAIN);
    builder.update(key_id.as_bytes());
    builder.finalize().to_bytes()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Wallet;

    #[test]
    fn every_address_starts_with_rsc_and_is_the_account() {
        let wallet = Wallet::generate().unwrap();
        assert!(wallet.address().starts_with("rsc"));
        assert_eq!(
            wallet.address(),
            format!("rsc{}", wallet.account_id_hex())
        );
        assert_ne!(wallet.address(), "rsc000000000");
        let other = Wallet::generate().unwrap();
        assert_ne!(wallet.address(), other.address());
        let rendered = format!("{wallet:?}");
        assert!(!rendered.contains(&wallet.signing_key_hex()));
        assert!(!rendered.contains(wallet.seed_phrase()));
        assert_eq!(wallet.seed_phrase().split_whitespace().count(), 12);
        let restored = Wallet::from_phrase(wallet.seed_phrase()).unwrap();
        assert_eq!(restored.address(), wallet.address());
        assert_eq!(restored.signing_key_hex(), wallet.signing_key_hex());
    }

    #[test]
    fn the_abandon_phrase_derives_the_bip39_seed() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let wallet = Wallet::from_phrase(phrase).unwrap();
        assert_eq!(
            crate::phrase::phrase_from_entropy(&[0u8; 16]).unwrap(),
            phrase
        );
        assert_eq!(
            wallet.signing_key_hex(),
            "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1"
        );
        assert_eq!(wallet.seed_phrase(), phrase);
        assert!(Wallet::from_phrase("abandon abandon abandon").is_err());
    }
}
