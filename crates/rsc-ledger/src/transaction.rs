//! Transacción canónica de RSC Chain v1.
//!
//! El encoding es big-endian y el orden de campos es fijo. La firma cubre el
//! cuerpo sin la propia firma. La identidad es un hash de ese mismo cuerpo
//! bajo otro dominio. Este módulo no consulta saldos, nonces de estado ni
//! existencia de cuentas.

use rsc_core::AccountId;
use rsc_crypto::{
    encode_domain_payload, hash, sign, verify, CryptoDomain, CryptoError, CryptoVersion, Hash256,
    KeyId, PrivateKey, PublicKey, Signature,
};
use thiserror::Error;

/// Única versión de transacción que [`Transaction::validate`] acepta.
pub const TRANSACTION_VERSION_V1: u32 = 1;

/// Dominio del identificador. No es el sobre de firma de `CryptoDomain`.
const DOMAIN_TRANSACTION_ID: &[u8] = b"RSC-LEDGER-TX-ID\0";

const PAYLOAD_CREATE_ACCOUNT: u8 = 0x01;
const PAYLOAD_TRANSFER: u8 = 0x02;
const PAYLOAD_AMM: u8 = 0x03;

/// Tamaño máximo del cuerpo opaco de una instrucción AMM (bytes canónicos).
pub const AMM_INSTRUCTION_MAX_LEN: usize = 512;

/// Carga útil de una transacción v1.
///
/// Alta de cuenta, transferencia nativa e instrucción AMM opaca. El ledger no
/// interpreta el cuerpo AMM: lo decodifica `rsc-amm` en el ejecutor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionPayload {
    /// Alta de cuenta. El identificador de clave ya viene derivado.
    ///
    /// No crea la cuenta en el estado.
    CreateAccount {
        /// Propietario criptográfico. No es una clave privada.
        owner_key_id: KeyId,
    },
    /// Transferencia de valor nativo.
    ///
    /// El ledger no comprueba si el emisor tiene saldo.
    Transfer {
        /// Cuenta destino. No se comprueba que exista.
        recipient: AccountId,
        /// Importe. Cero se rechaza en [`Transaction::validate`].
        amount: u128,
    },
    /// Instrucción AMM (encoding canónico de `rsc_amm::native::AmmInstruction`).
    ///
    /// El ledger solo valida longitud y no vaciedad. Firmas, nonces y saldos
    /// los comprueba el ejecutor / `rsc-state` / `rsc-assets`.
    Amm {
        /// Cuerpo canónico de la instrucción.
        instruction: Vec<u8>,
    },
}

/// Identidad canónica de una transacción.
///
/// Es el SHA-256 del cuerpo firmado bajo [`DOMAIN_TRANSACTION_ID`]. La firma
/// no entra. No es un hash de bloque ni un `StateRoot`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransactionId(pub Hash256);

impl core::fmt::Debug for TransactionId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TransactionId({:02x}{:02x}..)",
            self.0.as_bytes()[0],
            self.0.as_bytes()[1]
        )
    }
}

/// Transacción firmada.
///
/// Construirla no valida. La aceptación local es [`Self::validate`]. El
/// orden en el ledger lo decide quien llama a [`crate::Ledger::append`].
///
/// `sender` es un [`AccountId`], no una clave pública. La verificación de
/// firma recibe la [`PublicKey`] aparte: este crate no la busca en el estado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// Versión de formato. v1 solo acepta [`TRANSACTION_VERSION_V1`].
    pub version: u32,
    /// Cuenta que emite la transacción.
    pub sender: AccountId,
    /// Contador declarado por el emisor.
    ///
    /// Cualquier `u64` es representable. Que coincida con el nonce de estado
    /// lo decide `rsc-state`, no este crate.
    pub nonce: u64,
    /// Qué pide la transacción.
    pub payload: TransactionPayload,
    /// Firma Ed25519 del cuerpo canónico. No forma parte del cuerpo.
    pub signature: Signature,
}

/// Fallo local de una transacción.
///
/// No incluye saldo, cuenta ausente, nonce de estado, almacenamiento,
/// consenso, ejecución ni red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransactionError {
    /// `version` distinta de [`TRANSACTION_VERSION_V1`].
    #[error("unsupported transaction version")]
    UnsupportedVersion,
    /// `Transfer.amount` es cero.
    #[error("transfer amount must be greater than zero")]
    InvalidAmount,
    /// Discriminante de payload desconocido.
    #[error("invalid transaction payload")]
    InvalidPayload,
    /// Bytes canónicos incompletos o de longitud inesperada.
    #[error("invalid transaction encoding")]
    InvalidEncoding,
    /// La firma no corresponde a la clave y al cuerpo canónico.
    #[error("invalid transaction signature")]
    InvalidSignature,
    /// Un campo fijo no tiene la longitud que exige el tipo.
    #[error("invalid transaction field")]
    InvalidField,
}

impl Transaction {
    /// Firma el cuerpo canónico y sustituye [`Self::signature`].
    ///
    /// El mensaje firmado es `version || sender || nonce || payload`. El
    /// sobre de dominio lo aplica `rsc-crypto` con [`CryptoDomain::Transaction`].
    /// No comprueba saldos ni que la clave sea dueña de `sender`.
    pub fn sign(&mut self, key: &PrivateKey) -> Result<(), TransactionError> {
        self.signature = sign(key, CryptoDomain::Transaction, &self.unsigned_body())
            .map_err(map_crypto_error)?;
        Ok(())
    }

    /// Bytes que cubre la firma, incluido el sobre de `rsc-crypto`.
    ///
    /// Forma: `RSC || version_le || CryptoDomain::Transaction || cuerpo`.
    /// El cuerpo es `version || sender || nonce || payload`, big-endian.
    /// La firma no entra. Dos transacciones con los mismos campos firmados
    /// producen el mismo vector.
    pub fn signing_bytes(&self) -> Vec<u8> {
        encode_domain_payload(
            CryptoDomain::Transaction,
            CryptoVersion::V1,
            &self.unsigned_body(),
        )
    }

    /// Identidad determinista. No incluye la firma.
    pub fn transaction_id(&self) -> TransactionId {
        let body = self.unsigned_body();
        let mut preimage = Vec::with_capacity(DOMAIN_TRANSACTION_ID.len() + body.len());
        preimage.extend_from_slice(DOMAIN_TRANSACTION_ID);
        preimage.extend_from_slice(&body);
        TransactionId(hash(&preimage))
    }

    /// Igual que [`Self::transaction_id`].
    pub fn id(&self) -> TransactionId {
        self.transaction_id()
    }

    /// Bytes que un bloque puede guardar: el cuerpo de [`Self::transaction_id`] y después la firma.
    ///
    /// La firma ocupa los últimos 64 bytes y no entra en el identificador.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = self.unsigned_body();
        out.extend_from_slice(&self.signature.to_bytes());
        out
    }

    /// Recupera una transacción escrita con [`Self::canonical_bytes`].
    ///
    /// No verifica la firma: la [`PublicKey`] no viaja en estos bytes.
    /// Una longitud distinta de la del payload es [`TransactionError::InvalidEncoding`].
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, TransactionError> {
        const SIGNATURE_LEN: usize = 64;
        const PREFIX_LEN: usize = 4 + 32 + 8;
        if bytes.len() < PREFIX_LEN + 1 + SIGNATURE_LEN {
            return Err(TransactionError::InvalidEncoding);
        }
        let (body, signature_bytes) = bytes.split_at(bytes.len() - SIGNATURE_LEN);
        let version = u32::from_be_bytes(
            body[0..4]
                .try_into()
                .map_err(|_| TransactionError::InvalidEncoding)?,
        );
        let sender =
            AccountId::from_slice(&body[4..36]).map_err(|_| TransactionError::InvalidField)?;
        let mut nonce_bytes = [0u8; 8];
        nonce_bytes.copy_from_slice(&body[36..44]);
        let payload = decode_payload(&body[44..])?;
        let signature = Signature::from_bytes(signature_bytes)
            .map_err(|_| TransactionError::InvalidSignature)?;
        Ok(Self {
            version,
            sender,
            nonce: u64::from_be_bytes(nonce_bytes),
            payload,
            signature,
        })
    }

    /// Comprueba la firma contra `signer`.
    ///
    /// Cubre exactamente versión, emisor, nonce y payload. No consulta estado.
    pub fn verify_signature(&self, signer: &PublicKey) -> Result<(), TransactionError> {
        verify(
            signer,
            CryptoDomain::Transaction,
            &self.unsigned_body(),
            &self.signature,
        )
        .map_err(map_crypto_error)
    }

    /// Validación local: versión, payload y firma.
    ///
    /// Rechaza versión distinta de 1 y un importe de transferencia igual a
    /// cero. No decide si `sender` existe, si `recipient` existe, si el nonce
    /// es el actual ni si hay saldo.
    pub fn validate(&self, signer: &PublicKey) -> Result<(), TransactionError> {
        if self.version != TRANSACTION_VERSION_V1 {
            return Err(TransactionError::UnsupportedVersion);
        }
        self.validate_payload()?;
        self.verify_signature(signer)
    }

    fn validate_payload(&self) -> Result<(), TransactionError> {
        match &self.payload {
            TransactionPayload::CreateAccount { .. } => Ok(()),
            TransactionPayload::Transfer { amount, .. } => {
                if *amount == 0 {
                    Err(TransactionError::InvalidAmount)
                } else {
                    Ok(())
                }
            }
            TransactionPayload::Amm { instruction } => {
                if instruction.is_empty() || instruction.len() > AMM_INSTRUCTION_MAX_LEN {
                    Err(TransactionError::InvalidPayload)
                } else {
                    Ok(())
                }
            }
        }
    }

    fn unsigned_body(&self) -> Vec<u8> {
        let payload = encode_payload(&self.payload);
        let mut out = Vec::with_capacity(4 + 32 + 8 + payload.len());
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.sender.0);
        out.extend_from_slice(&self.nonce.to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }
}

fn encode_payload(payload: &TransactionPayload) -> Vec<u8> {
    match payload {
        TransactionPayload::CreateAccount { owner_key_id } => {
            let mut out = Vec::with_capacity(1 + 32);
            out.push(PAYLOAD_CREATE_ACCOUNT);
            out.extend_from_slice(owner_key_id.as_bytes());
            out
        }
        TransactionPayload::Transfer { recipient, amount } => {
            let mut out = Vec::with_capacity(1 + 32 + 16);
            out.push(PAYLOAD_TRANSFER);
            out.extend_from_slice(&recipient.0);
            out.extend_from_slice(&amount.to_be_bytes());
            out
        }
        TransactionPayload::Amm { instruction } => {
            let mut out = Vec::with_capacity(1 + 4 + instruction.len());
            out.push(PAYLOAD_AMM);
            out.extend_from_slice(&(instruction.len() as u32).to_be_bytes());
            out.extend_from_slice(instruction);
            out
        }
    }
}

fn map_crypto_error(error: CryptoError) -> TransactionError {
    match error {
        CryptoError::SignatureVerificationFailed | CryptoError::InvalidSignature => {
            TransactionError::InvalidSignature
        }
        CryptoError::InvalidPublicKey | CryptoError::InvalidPrivateKey => {
            TransactionError::InvalidField
        }
        CryptoError::InvalidHashLength
        | CryptoError::InvalidEncoding
        | CryptoError::InvalidDomain
        | CryptoError::UnsupportedAlgorithm
        | CryptoError::UnsupportedVersion
        | CryptoError::RandomnessFailure => TransactionError::InvalidEncoding,
    }
}

fn decode_payload(bytes: &[u8]) -> Result<TransactionPayload, TransactionError> {
    let (tag, rest) = bytes
        .split_first()
        .ok_or(TransactionError::InvalidEncoding)?;
    match *tag {
        PAYLOAD_CREATE_ACCOUNT => {
            if rest.len() != 32 {
                return Err(TransactionError::InvalidEncoding);
            }
            let owner_key_id =
                KeyId::from_bytes(rest).map_err(|_| TransactionError::InvalidField)?;
            Ok(TransactionPayload::CreateAccount { owner_key_id })
        }
        PAYLOAD_TRANSFER => {
            if rest.len() != 48 {
                return Err(TransactionError::InvalidEncoding);
            }
            let recipient =
                AccountId::from_slice(&rest[..32]).map_err(|_| TransactionError::InvalidField)?;
            let mut amount_bytes = [0u8; 16];
            amount_bytes.copy_from_slice(&rest[32..48]);
            Ok(TransactionPayload::Transfer {
                recipient,
                amount: u128::from_be_bytes(amount_bytes),
            })
        }
        PAYLOAD_AMM => {
            if rest.len() < 4 {
                return Err(TransactionError::InvalidEncoding);
            }
            let mut len_bytes = [0u8; 4];
            len_bytes.copy_from_slice(&rest[..4]);
            let len = u32::from_be_bytes(len_bytes) as usize;
            if len == 0 || len > AMM_INSTRUCTION_MAX_LEN || rest.len() != 4 + len {
                return Err(TransactionError::InvalidEncoding);
            }
            Ok(TransactionPayload::Amm {
                instruction: rest[4..].to_vec(),
            })
        }
        _ => Err(TransactionError::InvalidPayload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    fn key(byte: u8) -> PrivateKey {
        PrivateKey::from_bytes(&[byte; 32]).unwrap()
    }

    fn blank_signature() -> Signature {
        Signature::from_bytes(&[0u8; 64]).unwrap()
    }

    fn signed(mut tx: Transaction, sk: &PrivateKey) -> Transaction {
        tx.sign(sk).unwrap();
        tx
    }

    fn transfer(
        sender: u8,
        nonce: u64,
        recipient: u8,
        amount: u128,
        sk: &PrivateKey,
    ) -> Transaction {
        signed(
            Transaction {
                version: TRANSACTION_VERSION_V1,
                sender: account(sender),
                nonce,
                payload: TransactionPayload::Transfer {
                    recipient: account(recipient),
                    amount,
                },
                signature: blank_signature(),
            },
            sk,
        )
    }

    fn create_account(sender: u8, nonce: u64, owner: u8, sk: &PrivateKey) -> Transaction {
        signed(
            Transaction {
                version: TRANSACTION_VERSION_V1,
                sender: account(sender),
                nonce,
                payload: TransactionPayload::CreateAccount {
                    owner_key_id: key(owner).key_id(),
                },
                signature: blank_signature(),
            },
            sk,
        )
    }

    #[test]
    fn valid_create_account_transaction() {
        let sk = key(1);
        let tx = create_account(0x11, 0, 0x22, &sk);
        assert_eq!(tx.validate(&sk.public_key()), Ok(()));
    }

    #[test]
    fn valid_transfer_transaction() {
        let sk = key(2);
        let tx = transfer(0x11, 4, 0x22, 100, &sk);
        assert_eq!(tx.validate(&sk.public_key()), Ok(()));
    }

    #[test]
    fn unsupported_version_rejected() {
        let sk = key(3);
        let mut tx = transfer(0x11, 1, 0x22, 5, &sk);
        tx.version = 2;
        tx.sign(&sk).unwrap();
        assert_eq!(
            tx.validate(&sk.public_key()),
            Err(TransactionError::UnsupportedVersion)
        );
    }

    #[test]
    fn zero_transfer_amount_rejected() {
        let sk = key(4);
        let tx = transfer(0x11, 1, 0x22, 0, &sk);
        assert_eq!(
            tx.validate(&sk.public_key()),
            Err(TransactionError::InvalidAmount)
        );
    }

    #[test]
    fn deterministic_signing_bytes() {
        let sk = key(5);
        let tx = transfer(0x11, 7, 0x33, 9, &sk);
        assert_eq!(tx.signing_bytes(), tx.signing_bytes());
    }

    #[test]
    fn deterministic_transaction_id() {
        let sk = key(6);
        let tx = transfer(0x11, 7, 0x33, 9, &sk);
        assert_eq!(tx.transaction_id(), tx.id());
        assert_eq!(tx.id(), tx.id());
    }

    #[test]
    fn canonical_bytes_roundtrip_keeps_the_id() {
        let sk = key(6);
        let tx = transfer(0x11, 7, 0x33, 9, &sk);
        let restored = Transaction::from_canonical_bytes(&tx.canonical_bytes()).unwrap();
        assert_eq!(restored, tx);
        assert_eq!(restored.id(), tx.id());
        let created = create_account(0x11, 0, 0x22, &sk);
        let restored = Transaction::from_canonical_bytes(&created.canonical_bytes()).unwrap();
        assert_eq!(restored.id(), created.id());
        assert_eq!(
            Transaction::from_canonical_bytes(&[0u8; 10]).unwrap_err(),
            TransactionError::InvalidEncoding
        );
    }

    #[test]
    fn same_transaction_same_id() {
        let sk = key(7);
        let a = transfer(0x11, 3, 0x44, 8, &sk);
        let b = transfer(0x11, 3, 0x44, 8, &sk);
        assert_eq!(a.id(), b.id());
        assert_eq!(a.signing_bytes(), b.signing_bytes());
    }

    #[test]
    fn nonce_change_changes_id() {
        let sk = key(8);
        let a = transfer(0x11, 1, 0x44, 8, &sk);
        let b = transfer(0x11, 2, 0x44, 8, &sk);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn sender_change_changes_id() {
        let sk = key(9);
        let a = transfer(0x11, 1, 0x44, 8, &sk);
        let b = transfer(0x12, 1, 0x44, 8, &sk);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn payload_change_changes_id() {
        let sk = key(10);
        let transfer_tx = transfer(0x11, 1, 0x44, 8, &sk);
        let create_tx = create_account(0x11, 1, 0x44, &sk);
        assert_ne!(transfer_tx.id(), create_tx.id());
    }

    #[test]
    fn recipient_change_changes_id() {
        let sk = key(11);
        let a = transfer(0x11, 1, 0x44, 8, &sk);
        let b = transfer(0x11, 1, 0x45, 8, &sk);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn amount_change_changes_id() {
        let sk = key(12);
        let a = transfer(0x11, 1, 0x44, 8, &sk);
        let b = transfer(0x11, 1, 0x44, 9, &sk);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn version_change_changes_id() {
        let sk = key(13);
        let a = transfer(0x11, 1, 0x44, 8, &sk);
        let mut b = a.clone();
        b.version = 2;
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn invalid_signature_rejected() {
        let sk = key(14);
        let mut tx = transfer(0x11, 1, 0x44, 8, &sk);
        let mut raw = tx.signature.to_bytes();
        raw[0] ^= 0xff;
        tx.signature = Signature::from_bytes(&raw).unwrap();
        assert_eq!(
            tx.verify_signature(&sk.public_key()),
            Err(TransactionError::InvalidSignature)
        );
        assert_eq!(
            tx.validate(&sk.public_key()),
            Err(TransactionError::InvalidSignature)
        );
    }

    #[test]
    fn modified_payload_invalidates_signature() {
        let sk = key(15);
        let mut tx = transfer(0x11, 1, 0x44, 8, &sk);
        tx.payload = TransactionPayload::Transfer {
            recipient: account(0x44),
            amount: 9,
        };
        assert_eq!(
            tx.verify_signature(&sk.public_key()),
            Err(TransactionError::InvalidSignature)
        );
    }

    #[test]
    fn modified_sender_invalidates_signature() {
        let sk = key(16);
        let mut tx = transfer(0x11, 1, 0x44, 8, &sk);
        tx.sender = account(0x99);
        assert_eq!(
            tx.verify_signature(&sk.public_key()),
            Err(TransactionError::InvalidSignature)
        );
    }

    #[test]
    fn modified_nonce_invalidates_signature() {
        let sk = key(17);
        let mut tx = transfer(0x11, 1, 0x44, 8, &sk);
        tx.nonce = 99;
        assert_eq!(
            tx.verify_signature(&sk.public_key()),
            Err(TransactionError::InvalidSignature)
        );
    }

    #[test]
    fn signature_change_does_not_change_signing_preimage_or_id() {
        let sk = key(18);
        let tx = transfer(0x11, 1, 0x44, 8, &sk);
        let mut other = tx.clone();
        let mut raw = other.signature.to_bytes();
        raw[3] ^= 0x01;
        other.signature = Signature::from_bytes(&raw).unwrap();
        assert_eq!(tx.signing_bytes(), other.signing_bytes());
        assert_eq!(tx.id(), other.id());
    }

    #[test]
    fn amm_payload_roundtrip_and_bounds() {
        let sk = key(21);
        let body = vec![0x01u8; 10];
        let tx = signed(
            Transaction {
                version: TRANSACTION_VERSION_V1,
                sender: account(0x11),
                nonce: 1,
                payload: TransactionPayload::Amm {
                    instruction: body.clone(),
                },
                signature: blank_signature(),
            },
            &sk,
        );
        assert_eq!(tx.validate(&sk.public_key()), Ok(()));
        let restored = Transaction::from_canonical_bytes(&tx.canonical_bytes()).unwrap();
        assert_eq!(restored.payload, TransactionPayload::Amm { instruction: body });
        let empty = signed(
            Transaction {
                version: TRANSACTION_VERSION_V1,
                sender: account(0x11),
                nonce: 2,
                payload: TransactionPayload::Amm {
                    instruction: Vec::new(),
                },
                signature: blank_signature(),
            },
            &sk,
        );
        assert_eq!(
            empty.validate(&sk.public_key()),
            Err(TransactionError::InvalidPayload)
        );
    }

    #[test]
    fn canonical_payload_encoding_is_deterministic() {
        let sk = key(19);
        let owner = key(0x20).key_id();
        let tx = signed(
            Transaction {
                version: TRANSACTION_VERSION_V1,
                sender: account(0x11),
                nonce: 0x0102,
                payload: TransactionPayload::CreateAccount {
                    owner_key_id: owner,
                },
                signature: blank_signature(),
            },
            &sk,
        );
        let body = tx.unsigned_body();
        assert_eq!(body, tx.unsigned_body());
        assert_eq!(&body[0..4], &1u32.to_be_bytes());
        assert_eq!(&body[4..36], &[0x11; 32]);
        assert_eq!(&body[36..44], &0x0102u64.to_be_bytes());
        assert_eq!(body[44], PAYLOAD_CREATE_ACCOUNT);
        assert_eq!(&body[45..77], owner.as_bytes());
        assert_eq!(decode_payload(&body[44..]).unwrap(), tx.payload);

        let transfer_tx = transfer(0x01, 1, 0x02, 1, &sk);
        let payload = encode_payload(&transfer_tx.payload);
        assert_eq!(payload, encode_payload(&transfer_tx.payload));
        assert_eq!(payload[0], PAYLOAD_TRANSFER);
        assert_eq!(&payload[1..33], &[0x02; 32]);
        assert_eq!(&payload[33..49], &1u128.to_be_bytes());
        assert_eq!(decode_payload(&payload).unwrap(), transfer_tx.payload);
        assert_eq!(
            decode_payload(&[0x09]).unwrap_err(),
            TransactionError::InvalidPayload
        );
        assert_eq!(
            decode_payload(&[]).unwrap_err(),
            TransactionError::InvalidEncoding
        );
    }
}
