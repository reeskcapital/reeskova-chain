//! Frase semilla de 12 palabras.
//!
//! La lista es la inglesa de BIP-0039. La frase se convierte en la semilla
//! de firma con PBKDF2-HMAC-SHA512, 2048 rondas y sal `mnemonic`.

use sha2::{Digest, Sha256, Sha512};
use std::sync::OnceLock;

use crate::WalletError;

const ROUNDS: u32 = 2048;
const BLOCK: usize = 128;

fn wordlist() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        include_str!("english.txt")
            .lines()
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .collect()
    })
}

/// 12 palabras separadas por un espacio, a partir de 16 bytes de entropía.
pub fn phrase_from_entropy(entropy: &[u8; 16]) -> Result<String, WalletError> {
    let words = wordlist();
    if words.len() != 2048 {
        return Err(WalletError);
    }
    let checksum = Sha256::digest(entropy)[0] >> 4;
    let mut indices = Vec::with_capacity(12);
    let mut acc = 0u32;
    let mut acc_bits = 0u32;
    let pieces = entropy
        .iter()
        .copied()
        .zip(std::iter::repeat(8u32))
        .chain(std::iter::once((checksum << 4, 4u32)));
    for (byte, width) in pieces {
        acc = (acc << width) | u32::from(byte >> (8 - width));
        acc_bits += width;
        while acc_bits >= 11 {
            let shift = acc_bits - 11;
            let index = ((acc >> shift) & 0x7ff) as usize;
            indices.push(words.get(index).copied().ok_or(WalletError)?);
            acc &= if shift == 0 { 0 } else { (1u32 << shift) - 1 };
            acc_bits -= 11;
        }
    }
    if indices.len() != 12 || acc_bits != 0 {
        return Err(WalletError);
    }
    Ok(indices.join(" "))
}

/// Recupera los 16 bytes de entropía si la frase y su suma coinciden.
pub fn entropy_from_phrase(phrase: &str) -> Result<[u8; 16], WalletError> {
    let words = wordlist();
    let parts: Vec<&str> = phrase.split_whitespace().collect();
    if parts.len() != 12 {
        return Err(WalletError);
    }
    let mut data = [0u8; 17];
    let mut bit_len = 0usize;
    for word in parts {
        let index = words.binary_search(&word).map_err(|_| WalletError)? as u32;
        for shift in (0..11).rev() {
            let bit = ((index >> shift) & 1) as u8;
            let byte = bit_len / 8;
            let offset = 7 - (bit_len % 8);
            data[byte] |= bit << offset;
            bit_len += 1;
        }
    }
    if bit_len != 132 {
        return Err(WalletError);
    }
    let mut entropy = [0u8; 16];
    entropy.copy_from_slice(&data[..16]);
    let checksum = data[16] >> 4;
    if Sha256::digest(entropy)[0] >> 4 != checksum {
        return Err(WalletError);
    }
    Ok(entropy)
}

/// 64 bytes de semilla BIP-0039. La clave de firma usa los primeros 32.
pub fn seed_from_phrase(phrase: &str) -> Result<[u8; 64], WalletError> {
    let _ = entropy_from_phrase(phrase)?;
    let mut seed = [0u8; 64];
    pbkdf2_hmac_sha512(phrase.as_bytes(), b"mnemonic", ROUNDS, &mut seed);
    Ok(seed)
}

pub fn random_entropy() -> Result<[u8; 16], WalletError> {
    let mut entropy = [0u8; 16];
    getrandom::getrandom(&mut entropy).map_err(|_| WalletError)?;
    Ok(entropy)
}

fn hmac_sha512(key: &[u8], message: &[u8]) -> [u8; 64] {
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..64].copy_from_slice(&Sha512::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0u8; BLOCK];
    let mut outer_pad = [0u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] = block_key[index] ^ 0x36;
        outer_pad[index] = block_key[index] ^ 0x5c;
    }
    let mut inner = Sha512::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha512::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    let digest = outer.finalize();
    let mut out = [0u8; 64];
    out.copy_from_slice(&digest);
    out
}

fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    let mut block = 1u32;
    let mut offset = 0;
    while offset < out.len() {
        let mut message = Vec::with_capacity(salt.len() + 4);
        message.extend_from_slice(salt);
        message.extend_from_slice(&block.to_be_bytes());
        let mut mixed = hmac_sha512(password, &message);
        let mut folded = mixed;
        for _ in 1..rounds {
            mixed = hmac_sha512(password, &mixed);
            for byte in 0..64 {
                folded[byte] ^= mixed[byte];
            }
        }
        let count = (out.len() - offset).min(64);
        out[offset..offset + count].copy_from_slice(&folded[..count]);
        offset += count;
        block += 1;
    }
}
