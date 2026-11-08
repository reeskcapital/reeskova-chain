//! Tipos primitivos de RSC. No calculan hashes, no firman y no leen el reloj.

use crate::constants::*;
use crate::CoreError;
use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Serialize};

macro_rules! bytes {
    ($(#[$doc:meta])* $name:ident, $n:expr, $err:expr) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; $n]);

        impl $name {
            /// Valor nulo.
            pub const ZERO: Self = Self([0; $n]);

            /// Construye desde una rebanada de longitud exacta.
            pub fn from_slice(slice: &[u8]) -> Result<Self, CoreError> {
                <[u8; $n]>::try_from(slice)
                    .map(Self)
                    .map_err(|_| $err(slice.len()))
            }

            /// Hex con o sin `0x`.
            pub fn from_hex(s: &str) -> Result<Self, CoreError> {
                match decode_hex::<$n>(s) {
                    Ok(b) => Ok(Self(b)),
                    Err(CoreError::InvalidLength { actual, .. }) => Err($err(actual)),
                    Err(e) => Err(e),
                }
            }

            /// Hex en minúsculas con prefijo `0x`.
            pub fn to_hex(&self) -> String {
                encode_hex(&self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::ZERO
            }
        }

        impl FromStr for $name {
            type Err = CoreError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::from_hex(s)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_hex())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d)
                    .and_then(|s| Self::from_hex(&s).map_err(serde::de::Error::custom))
            }
        }
    };
}

macro_rules! wrap_hash {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Hash);

        impl $name {
            /// Valor nulo.
            pub const ZERO: Self = Self(Hash::ZERO);

            /// Rebanada de 32 bytes.
            pub fn from_slice(slice: &[u8]) -> Result<Self, CoreError> {
                Hash::from_slice(slice).map(Self)
            }

            /// Hex con o sin `0x`.
            pub fn from_hex(s: &str) -> Result<Self, CoreError> {
                Hash::from_hex(s).map(Self)
            }

            /// Hex con prefijo `0x`.
            pub fn to_hex(&self) -> String {
                self.0.to_hex()
            }
        }

        impl From<Hash> for $name {
            fn from(hash: Hash) -> Self {
                Self(hash)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_hex())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }
    };
}

bytes!(
    /// Digest de 32 bytes. Internamente no es un `String`.
    Hash,
    HASH_SIZE,
    |_| CoreError::InvalidHash
);
bytes!(
    /// Identificador de cuenta. No es una clave privada.
    AccountId,
    ACCOUNT_ID_SIZE,
    |_| CoreError::InvalidAccountId
);
bytes!(
    /// Contenedor de clave pública. Sin `sign`/`verify` aquí.
    PublicKey,
    PUBLIC_KEY_SIZE,
    |actual| CoreError::InvalidLength {
        expected: PUBLIC_KEY_SIZE,
        actual
    }
);
bytes!(
    /// Firma de 64 bytes. Sin verificación aquí.
    Signature,
    SIGNATURE_SIZE,
    |actual| CoreError::InvalidLength {
        expected: SIGNATURE_SIZE,
        actual
    }
);

wrap_hash!(
    /// Id de transacción. Distinto de [`BlockHash`].
    TransactionId
);
wrap_hash!(
    /// Id de bloque. Distinto de [`TransactionId`].
    BlockHash
);
wrap_hash!(
    /// Raíz de estado. Este crate no calcula el árbol.
    StateRoot
);

/// Unidad mínima como `u128`. Nunca float.
///
/// No define cuántas unidades son un RSC. Esa escala la declara el génesis.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Amount(pub u128);

impl Amount {
    /// Cero.
    pub const ZERO: Self = Self(0);
    /// Máximo del tipo, no un tope económico.
    pub const MAX: Self = Self(u128::MAX);

    /// Suma sin wrapping.
    pub fn checked_add(self, other: Self) -> Result<Self, CoreError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(CoreError::Overflow)
    }

    /// Resta sin ir a negativo.
    pub fn checked_sub(self, other: Self) -> Result<Self, CoreError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or(CoreError::Underflow)
    }
}

impl FromStr for Amount {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self).map_err(|_| CoreError::InvalidAmount)
    }
}

/// Altura de bloque. Génesis = 0. Nunca negativa.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct BlockHeight(pub u64);

impl BlockHeight {
    /// Génesis.
    pub const GENESIS: Self = Self(GENESIS_HEIGHT);

    /// Siguiente altura, o overflow en `u64::MAX`.
    pub fn checked_next(self) -> Result<Self, CoreError> {
        self.0.checked_add(1).map(Self).ok_or(CoreError::Overflow)
    }
}

/// Identificador de red (`u32`).
///
/// El número lo elige el documento de génesis. Este tipo no nombra redes.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ChainId(pub u32);

/// Contador por cuenta contra replay.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Nonce(pub u64);

impl Nonce {
    /// Cero.
    pub const ZERO: Self = Self(0);

    /// Siguiente nonce, o error en `u64::MAX`.
    pub fn checked_next(self) -> Result<Self, CoreError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(CoreError::InvalidNonce)
    }
}

/// Segundos Unix. No llama a `SystemTime::now()`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// Construye desde segundos Unix.
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::from("0x");
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex<const N: usize>(s: &str) -> Result<[u8; N], CoreError> {
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if !s.len().is_multiple_of(2) {
        return Err(CoreError::InvalidHex);
    }
    if s.len() != N * 2 {
        return Err(CoreError::InvalidLength {
            expected: N,
            actual: s.len() / 2,
        });
    }
    let raw = s.as_bytes();
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (hex_nibble(raw[i * 2])? << 4) | hex_nibble(raw[i * 2 + 1])?;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, CoreError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(CoreError::InvalidHex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_ids_and_keys() {
        let h = Hash([0xab; 32]);
        assert_eq!(Hash::from_hex(&h.to_hex()).unwrap(), h);
        assert_eq!(
            Hash::from_slice(&[0; 12]).unwrap_err(),
            CoreError::InvalidHash
        );
        assert_eq!(
            AccountId::from_slice(&[0; 3]).unwrap_err(),
            CoreError::InvalidAccountId
        );
        assert_eq!(TransactionId::from(h).0, h);
        assert_ne!(format!("{:?}", BlockHash(h)), format!("{:?}", StateRoot(h)));
        assert!(PublicKey::from_slice(&[0; 16]).is_err());
        assert!(Signature::from_slice(&[0; 32]).is_err());
    }

    #[test]
    fn amounts_and_counters() {
        assert_eq!(
            "1.5".parse::<Amount>().unwrap_err(),
            CoreError::InvalidAmount
        );
        assert_eq!(Amount(0), Amount::ZERO);
        assert!(Amount::MAX.checked_add(Amount(1)).is_err());
        assert!(Amount::ZERO.checked_sub(Amount(1)).is_err());
        assert_eq!(BlockHeight::GENESIS.checked_next().unwrap(), BlockHeight(1));
        assert!(BlockHeight(u64::MAX).checked_next().is_err());
        assert_eq!(Nonce::ZERO.checked_next().unwrap(), Nonce(1));
        assert!(Nonce(u64::MAX).checked_next().is_err());
        assert_eq!(Timestamp::from_secs(9).0, 9);
        assert_eq!(ChainId(7).0, 7);
    }

    #[test]
    fn same_bytes_same_result() {
        let a = Hash([7; 32]);
        let b = Hash([7; 32]);
        assert_eq!(a, b);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&Amount(1_500_000)).unwrap(),
            "1500000"
        );
        assert_eq!(TransactionId(a), TransactionId(b));
    }
}
