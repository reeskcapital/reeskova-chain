//! Reparto del premio de seguridad entre las personas que cobraron el día.
//!
//! Un comunitario es una persona. Un móvil 4/4 son cuatro personas y cada
//! una cobra lo mismo que un comunitario. Quien no está en la lista de
//! este día no cobra. El residuo de la división no se asigna aquí: vuelve
//! a la bóveda.

use rsc_core::AccountId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Asiento que finalizó bloques en el día que se liquida.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SecuritySeat {
    /// Un nodo comunitario. Una persona.
    Community {
        /// Cuenta que cobra.
        account: AccountId,
    },
    /// Nodo móvil 4/4. Cuatro personas, cada una con su parte.
    Mobile {
        /// Mineros del nodo, en orden estable.
        miners: [AccountId; 4],
    },
}

impl SecuritySeat {
    /// Asiento de una sola cuenta.
    pub fn community(account: AccountId) -> Self {
        Self::Community { account }
    }

    /// Cuentas que pueden cobrar este asiento.
    pub fn accounts(&self) -> Vec<AccountId> {
        match self {
            Self::Community { account } => vec![*account],
            Self::Mobile { miners } => miners.to_vec(),
        }
    }
}

/// Cuentas que cobran, una vez cada una, en el orden de los asientos.
pub fn payee_accounts(seats: &[SecuritySeat]) -> Vec<AccountId> {
    let mut seen = BTreeSet::new();
    let mut accounts = Vec::new();
    for seat in seats {
        for account in seat.accounts() {
            if seen.insert(account) {
                accounts.push(account);
            }
        }
    }
    accounts
}

/// Parte `pot` en partes iguales entre las personas que cobran.
///
/// Una cuenta repetida cobra una sola vez. El residuo no aparece en el
/// resultado.
pub fn split_security_pot(pot: u128, seats: &[SecuritySeat]) -> Vec<(AccountId, u128)> {
    let accounts = payee_accounts(seats);
    if pot == 0 || accounts.is_empty() {
        return Vec::new();
    }
    let share = pot / u128::try_from(accounts.len()).unwrap_or(u128::MAX);
    if share == 0 {
        return Vec::new();
    }
    accounts
        .into_iter()
        .map(|account| (account, share))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    #[test]
    fn each_person_receives_the_same_share() {
        let seats = vec![
            SecuritySeat::community(id(1)),
            SecuritySeat::Mobile {
                miners: [id(2), id(3), id(4), id(5)],
            },
        ];
        assert_eq!(payee_accounts(&seats).len(), 5);
        let credits = split_security_pot(1_000, &seats);
        for person in [1u8, 2, 3, 4, 5] {
            assert_eq!(credits.iter().find(|(a, _)| *a == id(person)).unwrap().1, 200);
        }
        let paid: u128 = credits.iter().map(|(_, amount)| *amount).sum();
        assert_eq!(paid, 1_000);
    }

    #[test]
    fn empty_seats_pay_nobody() {
        assert!(split_security_pot(500, &[]).is_empty());
        assert!(split_security_pot(0, &[SecuritySeat::community(id(1))]).is_empty());
    }
}
