//! Recorrido de un rango de bloques. No abre storage.

/// Un rango vacío no es un bloque.
pub fn range_len(payloads: &[Vec<u8>]) -> Result<usize, &'static str> {
    if payloads.is_empty() {
        Err("invalid encoding")
    } else {
        Ok(payloads.len())
    }
}

/// Aplica cada payload en orden y se detiene en el primero que falla.
pub fn apply_in_order<T, E>(
    payloads: &[Vec<u8>],
    mut step: impl FnMut(&[u8]) -> Result<T, E>,
) -> Result<Vec<T>, E> {
    let mut applied = Vec::with_capacity(payloads.len());
    for payload in payloads {
        applied.push(step(payload)?);
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_range_is_rejected() {
        let payloads: &[Vec<u8>] = &[];
        assert_eq!(range_len(payloads), Err("invalid encoding"));
    }

    #[test]
    fn range_stops_at_the_first_invalid_payload() {
        let payloads = vec![b"a".to_vec(), b"bad".to_vec(), b"c".to_vec()];
        let mut seen = Vec::new();
        let err = apply_in_order(&payloads, |payload| {
            if payload == b"bad" {
                return Err("invalid height");
            }
            seen.push(payload.to_vec());
            Ok(payload.len())
        })
        .unwrap_err();
        assert_eq!(err, "invalid height");
        assert_eq!(seen, vec![b"a".to_vec()]);
    }

    #[test]
    fn range_applies_every_payload_in_order() {
        let payloads = vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()];
        let applied =
            apply_in_order(&payloads, |payload| Ok::<_, &'static str>(payload.to_vec())).unwrap();
        assert_eq!(applied, payloads);
    }
}
