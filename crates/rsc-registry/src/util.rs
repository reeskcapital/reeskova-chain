//! Utilidades que no imprimen secretos.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() % 2 != 0 || text.len() > 1024 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Comparación independiente del contenido. La longitud distinta falla.
pub fn const_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Quita bearer tokens y queries con credenciales antes de un log.
pub fn sanitize_log(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if starts_with_ignore_ascii(bytes, i, b"bearer ") {
            out.push_str("Bearer redacted");
            i += 7;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }
        if let Some(skip) = redact_query_at(bytes, i) {
            out.push_str("redacted");
            i += skip;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    if out.len() > 300 {
        out.truncate(300);
    }
    out
}

fn starts_with_ignore_ascii(bytes: &[u8], at: usize, prefix: &[u8]) -> bool {
    if at + prefix.len() > bytes.len() {
        return false;
    }
    bytes[at..at + prefix.len()]
        .iter()
        .zip(prefix.iter())
        .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

fn redact_query_at(bytes: &[u8], at: usize) -> Option<usize> {
    const KEYS: [&[u8]; 4] = [b"token=", b"secret=", b"password=", b"key="];
    for key in KEYS {
        if starts_with_ignore_ascii(bytes, at, key) {
            let mut end = at + key.len();
            while end < bytes.len() && bytes[end] != b'&' && bytes[end] != b' ' {
                end += 1;
            }
            return Some(end - at);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let raw = [0x00u8, 0x0f, 0xab, 0xff];
        assert_eq!(hex_decode(&hex_encode(&raw)).unwrap(), raw);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }

    #[test]
    fn sanitize_removes_bearer_and_query_secrets() {
        let text = "Authorization: Bearer super-secret https://rpc.example/rpc?token=abc&x=1 key=zz";
        let clean = sanitize_log(text);
        assert!(!clean.contains("super-secret"));
        assert!(!clean.contains("token=abc"));
        assert!(!clean.contains("key=zz"));
        assert!(clean.contains("Bearer redacted"));
        assert!(clean.contains("redacted"));
        assert!(clean.contains("x=1"));
    }

    #[test]
    fn const_time_eq_checks_bytes() {
        assert!(const_time_eq(b"abc", b"abc"));
        assert!(!const_time_eq(b"abc", b"abd"));
        assert!(!const_time_eq(b"abc", b"ab"));
    }
}
