//! Cuenta RSC v1.
//!
//! La identidad de cuenta no es una clave pública ni un [`KeyId`]. Se deriva
//! de forma determinista del propietario y solo cambia si una versión futura
//! del protocolo define una operación explícita para ello.

use crate::{domain_hash, StateError, ACCOUNT_ID_DOMAIN};
use core::fmt;
use rsc_crypto::KeyId;

/// Longitud del encoding canónico de [`Account`].
pub const ACCOUNT_ENCODED_LEN: usize = 92;

/// Única versión de cuenta implementada.
pub const ACCOUNT_VERSION_V1: u32 = 1;

/// Identificador de cuenta de 32 bytes.
///
/// No es un [`KeyId`] ni una clave pública. Dos propietarios distintos
/// producen identificadores distintos. El mismo propietario produce siempre
/// el mismo identificador.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId([u8; 32]);

impl AccountId {
    /// Rechaza longitudes distintas de 32. No comprueba la derivación.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StateError> {
        let array: [u8; 32] = bytes.try_into().map_err(|_| StateError::InvalidAccountId)?;
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

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountId({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl From<AccountId> for rsc_core::AccountId {
    fn from(value: AccountId) -> Self {
        rsc_core::AccountId(value.0)
    }
}

impl From<rsc_core::AccountId> for AccountId {
    fn from(value: rsc_core::AccountId) -> Self {
        Self(value.0)
    }
}

/// `AccountId = SHA-256(RSC/STATE/ACCOUNT-ID/V1 || owner_key_id)`.
///
/// El hash lo calcula `rsc-crypto`. La misma entrada produce la misma salida
/// en cualquier máquina.
pub fn derive_account_id(owner_key_id: &KeyId) -> AccountId {
    AccountId(domain_hash(ACCOUNT_ID_DOMAIN, &[owner_key_id.as_bytes()]))
}

/// Cuenta nativa de RSC.
///
/// `account_id` y `owner_key_id` quedan fijos al construir la cuenta.
/// El balance y el nonce solo cambian dentro de `rsc-state`, mediante una
/// transición o el génesis. El balance es un entero `u128` y el nonce un
/// `u64`; ninguno admite negativos ni aritmética con envoltura.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Account {
    account_id: AccountId,
    owner_key_id: KeyId,
    nonce: u64,
    native_balance: u128,
    version: u32,
}

impl Account {
    /// Cuenta nueva: balance 0, nonce 0, versión 1, id derivado.
    pub fn new(owner_key_id: KeyId) -> Self {
        Self::with_initial_balance(owner_key_id, 0)
    }

    /// Cuenta de génesis. No es una API de emisión para el resto del protocolo.
    pub(crate) fn with_initial_balance(owner_key_id: KeyId, initial_balance: u128) -> Self {
        Self {
            account_id: derive_account_id(&owner_key_id),
            owner_key_id,
            nonce: 0,
            native_balance: initial_balance,
            version: ACCOUNT_VERSION_V1,
        }
    }

    pub(crate) fn with_balance_and_nonce(self, native_balance: u128, nonce: u64) -> Self {
        Self {
            native_balance,
            nonce,
            ..self
        }
    }

    /// Identificador derivado. Inmutable.
    pub fn account_id(self) -> AccountId {
        self.account_id
    }

    /// Propietario criptográfico. Inmutable en v1.
    pub fn owner_key_id(self) -> KeyId {
        self.owner_key_id
    }

    /// Nonce actual. Copia; mutarlo no altera la cuenta.
    pub fn nonce(self) -> u64 {
        self.nonce
    }

    /// Balance nativo exacto.
    pub fn native_balance(self) -> u128 {
        self.native_balance
    }

    /// Versión de formato. En v1 es siempre 1.
    pub fn version(self) -> u32 {
        self.version
    }

    /// Comprueba versión, derivación del id y formato del propietario.
    pub fn validate(self) -> Result<(), StateError> {
        if self.version != ACCOUNT_VERSION_V1 {
            return Err(StateError::InvalidAccountVersion);
        }
        if self.account_id != derive_account_id(&self.owner_key_id) {
            return Err(StateError::InvalidAccountId);
        }
        if KeyId::from_bytes(self.owner_key_id.as_bytes()).is_err() {
            return Err(StateError::InvalidAccount);
        }
        Ok(())
    }

    /// Encoding canónico de 92 bytes, big-endian, sin campos opcionales.
    ///
    /// `version || account_id || owner_key_id || nonce || native_balance`.
    pub fn to_bytes(self) -> [u8; ACCOUNT_ENCODED_LEN] {
        let mut out = [0u8; ACCOUNT_ENCODED_LEN];
        out[0..4].copy_from_slice(&self.version.to_be_bytes());
        out[4..36].copy_from_slice(self.account_id.as_bytes());
        out[36..68].copy_from_slice(self.owner_key_id.as_bytes());
        out[68..76].copy_from_slice(&self.nonce.to_be_bytes());
        out[76..92].copy_from_slice(&self.native_balance.to_be_bytes());
        out
    }

    /// Inverso de [`Self::to_bytes`]. Rechaza longitud, versión e id incoherentes.
    ///
    /// No entra en pánico con entrada externa.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StateError> {
        if bytes.len() != ACCOUNT_ENCODED_LEN {
            return Err(StateError::Serialization);
        }
        let mut version_bytes = [0u8; 4];
        version_bytes.copy_from_slice(&bytes[0..4]);
        let version = u32::from_be_bytes(version_bytes);
        if version != ACCOUNT_VERSION_V1 {
            return Err(StateError::InvalidAccountVersion);
        }
        let account_id = AccountId::from_bytes(&bytes[4..36])?;
        let owner_key_id =
            KeyId::from_bytes(&bytes[36..68]).map_err(|_| StateError::InvalidAccount)?;
        let mut nonce_bytes = [0u8; 8];
        nonce_bytes.copy_from_slice(&bytes[68..76]);
        let nonce = u64::from_be_bytes(nonce_bytes);
        let mut balance_bytes = [0u8; 16];
        balance_bytes.copy_from_slice(&bytes[76..92]);
        let native_balance = u128::from_be_bytes(balance_bytes);
        let account = Self {
            account_id,
            owner_key_id,
            nonce,
            native_balance,
            version,
        };
        account.validate()?;
        Ok(account)
    }
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Account")
            .field("account_id", &self.account_id)
            .field("owner_key_id", &self.owner_key_id)
            .field("nonce", &self.nonce)
            .field("native_balance", &self.native_balance)
            .field("version", &self.version)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsc_crypto::hash;

    fn owner(byte: u8) -> KeyId {
        KeyId::from_bytes(&[byte; 32]).unwrap()
    }

    #[test]
    fn account_id_derivation_is_deterministic() {
        let key = owner(7);
        assert_eq!(derive_account_id(&key), derive_account_id(&key));
        assert_ne!(derive_account_id(&owner(1)), derive_account_id(&owner(2)));
        assert_ne!(derive_account_id(&key).to_bytes(), key.to_bytes());
        assert_ne!(
            derive_account_id(&key).to_bytes(),
            hash(key.as_bytes()).to_bytes()
        );
    }

    #[test]
    fn account_new_and_validation() {
        let account = Account::new(owner(4));
        assert_eq!(account.version(), ACCOUNT_VERSION_V1);
        assert_eq!(account.nonce(), 0);
        assert_eq!(account.native_balance(), 0);
        assert_eq!(account.account_id(), derive_account_id(&owner(4)));
        assert_eq!(account.owner_key_id(), owner(4));
        assert!(account.validate().is_ok());
        let nonce = account.nonce();
        assert_eq!(nonce, 0);
        assert_eq!(account.nonce(), 0);
    }

    #[test]
    fn account_roundtrip_and_bounds() {
        let account = Account::with_initial_balance(owner(9), u128::MAX)
            .with_balance_and_nonce(u128::MAX, u64::MAX);
        let bytes = account.to_bytes();
        assert_eq!(bytes.len(), ACCOUNT_ENCODED_LEN);
        assert_eq!(&bytes[0..4], &1u32.to_be_bytes());
        assert_eq!(&bytes[68..76], &u64::MAX.to_be_bytes());
        assert_eq!(&bytes[76..92], &u128::MAX.to_be_bytes());
        assert_eq!(Account::from_bytes(&bytes).unwrap(), account);
        assert_eq!(
            Account::from_bytes(&account.to_bytes()).unwrap().to_bytes(),
            bytes
        );
    }

    #[test]
    fn account_rejects_malformed_bytes() {
        let valid = Account::new(owner(3)).to_bytes();
        for len in [0usize, 1, 31, 91, 93, 200] {
            let buf = vec![0x11u8; len];
            assert!(Account::from_bytes(&buf).is_err());
        }
        let mut bad_version = valid;
        bad_version[0..4].copy_from_slice(&2u32.to_be_bytes());
        assert_eq!(
            Account::from_bytes(&bad_version).unwrap_err().to_string(),
            StateError::InvalidAccountVersion.to_string()
        );
        let mut bad_id = valid;
        bad_id[4] ^= 0xff;
        assert!(matches!(
            Account::from_bytes(&bad_id),
            Err(StateError::InvalidAccountId)
        ));
        let mut bad_owner = valid;
        bad_owner[36] ^= 0xff;
        assert!(matches!(
            Account::from_bytes(&bad_owner),
            Err(StateError::InvalidAccountId)
        ));
        assert_eq!(
            KeyId::from_bytes(&[0u8; 16]).unwrap_err(),
            rsc_crypto::CryptoError::InvalidHashLength
        );
        assert!(matches!(
            AccountId::from_bytes(&[0u8; 16]),
            Err(StateError::InvalidAccountId)
        ));
    }
}
