//! Adaptación de aritmética `Amount` → `MonetaryError`.

use crate::error::MonetaryError;
use rsc_core::{Amount, CoreError};

pub(crate) fn add(a: Amount, b: Amount) -> Result<Amount, MonetaryError> {
    a.checked_add(b).map_err(map_core)
}

pub(crate) fn sub(a: Amount, b: Amount) -> Result<Amount, MonetaryError> {
    a.checked_sub(b).map_err(map_core)
}

fn map_core(err: CoreError) -> MonetaryError {
    match err {
        CoreError::Overflow => MonetaryError::Overflow,
        CoreError::Underflow => MonetaryError::InsufficientSupply,
        _ => MonetaryError::Overflow,
    }
}
