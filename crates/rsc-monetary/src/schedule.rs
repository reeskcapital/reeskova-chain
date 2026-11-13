//! Subsidio de minería por persona.
//!
//! Los primeros cobran 0,10 RSK al día cada uno. Ese pago se mantiene
//! mientras sean 900 o menos, porque 900 × 0,10 = 90. A partir de ahí la
//! red reparte 90 RSK al día y cada persona cobra menos. No se emite de
//! más que la reserva que quede, ni el residuo que no se puede partir
//! en partes iguales.

use crate::policy::AMOUNT_SCALE;

/// 0,10 RSK por persona y día, en unidades mínimas.
pub const PERSONAL_DAILY_CAP: u128 = AMOUNT_SCALE / 10;

/// 90 RSK al día para toda la red, en unidades mínimas.
pub const NETWORK_DAILY_CAP: u128 = AMOUNT_SCALE * 90;

/// Segundos de un día de cadena. El reloj es el timestamp del bloque.
pub const SECURITY_DAY_SECS: u64 = 86_400;

/// Día de cadena ya abierto por `block_timestamp`, contado desde el génesis.
///
/// El día 0 es el primero. Un bloque en el mismo segundo que el génesis
/// todavía no abre un día liquidable: la liquidación paga los días ya
/// cumplidos, así que el primer pago ocurre al cruzar 86.400 segundos.
pub fn chain_day(genesis_timestamp: u64, block_timestamp: u64) -> u64 {
    block_timestamp.saturating_sub(genesis_timestamp) / SECURITY_DAY_SECS
}

/// Lo que cobra una persona en un día con `payees` personas cobrando.
///
/// Cero si no hay nadie. 0,10 RSK hasta 900 personas. Después, 90 RSK
/// partidos entre todas.
pub fn personal_daily_subsidy(payees: u64) -> u128 {
    if payees == 0 {
        return 0;
    }
    let people = u128::from(payees);
    PERSONAL_DAILY_CAP.min(NETWORK_DAILY_CAP / people)
}

/// Subsidio de un día para `payees` personas, sin pasar de `remaining`.
///
/// El resultado es múltiplo del número de personas. El residuo que no
/// alcanza para una parte igual se queda en la reserva.
pub fn daily_subsidy(remaining: u128, payees: u64) -> u128 {
    subsidy_for_days(remaining, payees, 1)
}

/// Subsidio de `days` días al mismo número de personas.
///
/// Los días atrasados usan el recuento de hoy: la cadena no guarda cuánta
/// gente cobró en un día ya cerrado. Cero días, cero personas o reserva
/// vacía no emiten nada.
pub fn subsidy_for_days(remaining: u128, payees: u64, days: u64) -> u128 {
    if payees == 0 || days == 0 || remaining == 0 {
        return 0;
    }
    let people = u128::from(payees);
    let per_person = personal_daily_subsidy(payees);
    if per_person == 0 {
        return 0;
    }
    let wanted = per_person
        .saturating_mul(people)
        .saturating_mul(u128::from(days));
    let mint = wanted.min(remaining);
    (mint / people) * people
}

/// `u128` en JSON decimal, para no perder precisión por encima de 2^53.
pub mod u128_decimal {
    use core::fmt;
    use serde::de::Visitor;
    use serde::{Deserializer, Serializer};

    /// Escribe el entero como texto.
    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    /// Lee texto decimal o un entero pequeño de un runtime anterior.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        deserializer.deserialize_any(DecimalU128)
    }

    struct DecimalU128;

    impl Visitor<'_> for DecimalU128 {
        type Value = u128;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("entero decimal de la reserva de minería")
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<u128, E> {
            value.parse().map_err(E::custom)
        }

        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<u128, E> {
            Ok(u128::from(value))
        }

        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<u128, E> {
            u128::try_from(value).map_err(E::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::MINING_BUDGET;

    #[test]
    fn the_first_miners_earn_a_tenth_and_then_the_network_cap_is_split() {
        assert_eq!(personal_daily_subsidy(0), 0);
        assert_eq!(personal_daily_subsidy(1), PERSONAL_DAILY_CAP);
        assert_eq!(personal_daily_subsidy(100), PERSONAL_DAILY_CAP);
        assert_eq!(personal_daily_subsidy(900), PERSONAL_DAILY_CAP);
        assert_eq!(personal_daily_subsidy(1_000), NETWORK_DAILY_CAP / 1_000);
        assert_eq!(
            personal_daily_subsidy(10_000_000),
            NETWORK_DAILY_CAP / 10_000_000
        );

        assert_eq!(daily_subsidy(MINING_BUDGET, 0), 0);
        assert_eq!(daily_subsidy(MINING_BUDGET, 1), PERSONAL_DAILY_CAP);
        assert_eq!(daily_subsidy(MINING_BUDGET, 100), PERSONAL_DAILY_CAP * 100);
        assert_eq!(daily_subsidy(MINING_BUDGET, 900), NETWORK_DAILY_CAP);
        assert_eq!(daily_subsidy(MINING_BUDGET, 1_000), NETWORK_DAILY_CAP);
        assert_eq!(daily_subsidy(MINING_BUDGET, 10_000_000), NETWORK_DAILY_CAP);
        assert_eq!(
            subsidy_for_days(MINING_BUDGET, 900, 365),
            NETWORK_DAILY_CAP * 365
        );
        assert_eq!(subsidy_for_days(MINING_BUDGET, 100, 0), 0);
        let exhausted = subsidy_for_days(MINING_BUDGET, 900, u64::MAX);
        assert_eq!(exhausted, (MINING_BUDGET / 900) * 900);
        assert!(exhausted <= MINING_BUDGET);
        assert_eq!(daily_subsidy(1, 1_000), 0);
        assert_eq!(chain_day(1_700_000_000, 1_700_000_000), 0);
        assert_eq!(chain_day(1_700_000_000, 1_700_086_400), 1);
    }
}
