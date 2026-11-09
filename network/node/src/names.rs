//! Nombre único → wallet. El nombre es público. La frase no se guarda aquí.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameRecord {
    pub name: String,
    pub address: String,
    pub account_id: String,
    pub public_key: String,
    /// Sal y verificador. Nunca la contraseña.
    #[serde(default)]
    pub password_salt: String,
    #[serde(default)]
    pub password_hash: String,
}

pub struct NameBook {
    path: PathBuf,
    inner: Mutex<BTreeMap<String, NameRecord>>,
}

impl NameBook {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let inner = load(&path).unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn lookup(&self, name: &str) -> Result<NameRecord, ()> {
        let (key, _) = normalize(name)?;
        let guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        guard.get(&key).cloned().ok_or(())
    }

    pub fn insert(&self, record: NameRecord) -> Result<(), ()> {
        let (key, _) = normalize(&record.name)?;
        let mut guard = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if guard.contains_key(&key) {
            return Err(());
        }
        if guard.values().any(|known| known.account_id == record.account_id) {
            return Err(());
        }
        guard.insert(key, record);
        save(&self.path, &guard)
    }
}

/// 8 a 128 caracteres. No se recorta: el espacio forma parte de la contraseña.
pub fn accept_password(password: &str) -> Result<(), ()> {
    let len = password.chars().count();
    if !(8..=128).contains(&len) || password.chars().all(char::is_whitespace) {
        return Err(());
    }
    Ok(())
}

/// Sal aleatoria y verificador. La contraseña no sale de esta función.
pub fn seal_password(password: &str) -> Result<(String, String), ()> {
    accept_password(password)?;
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|_| ())?;
    let hash = stretch(password, &salt);
    Ok((hex(&salt), hex(&hash)))
}

pub fn password_matches(record: &NameRecord, password: &str) -> bool {
    if record.password_salt.is_empty() || record.password_hash.is_empty() {
        return false;
    }
    let salt = match unhex(&record.password_salt) {
        Some(salt) if salt.len() == 16 => salt,
        _ => return false,
    };
    let expected = match unhex(&record.password_hash) {
        Some(hash) if hash.len() == 32 => hash,
        _ => return false,
    };
    let actual = stretch(password, &salt);
    same(&actual, &expected)
}

fn stretch(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut material = Vec::with_capacity(salt.len() + password.len());
    material.extend_from_slice(salt);
    material.extend(password.as_bytes());
    let mut digest = rsc_crypto::hash(&material).to_bytes();
    for _ in 1..10_000 {
        let mut next = Vec::with_capacity(32 + password.len());
        next.extend_from_slice(&digest);
        next.extend(password.as_bytes());
        digest = rsc_crypto::hash(&next).to_bytes();
    }
    digest
}

fn same(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let hi = hex_value(bytes[index])?;
        let lo = hex_value(bytes[index + 1])?;
        out.push((hi << 4) | lo);
        index += 2;
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Clave estable y nombre visible. Rechaza vacío, símbolos y más de 80 caracteres.
pub fn normalize(name: &str) -> Result<(String, String), ()> {
    let display = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = display.chars().count();
    if !(3..=80).contains(&len) {
        return Err(());
    }
    if !display
        .chars()
        .all(|c| c.is_alphabetic() || c == ' ' || c == '-' || c == '\'')
    {
        return Err(());
    }
    Ok((display.to_lowercase(), display))
}

fn load(path: &Path) -> Option<BTreeMap<String, NameRecord>> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(path: &Path, book: &BTreeMap<String, NameRecord>) -> Result<(), ()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|_| ())?;
    }
    let text = serde_json::to_string_pretty(book).map_err(|_| ())?;
    fs::write(path, text).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_verifier_matches_only_the_same_secret() {
        assert!(accept_password("corta").is_err());
        let (salt, hash) = seal_password("clave-segura").unwrap();
        assert!(!salt.is_empty() && !hash.is_empty());
        assert!(!hash.contains("clave-segura"));
        let record = NameRecord {
            name: "Ana Costa".to_string(),
            address: "rsc00".to_string(),
            account_id: "aa".to_string(),
            public_key: "bb".to_string(),
            password_salt: salt,
            password_hash: hash,
        };
        assert!(password_matches(&record, "clave-segura"));
        assert!(!password_matches(&record, "otra-clave"));
    }
}
