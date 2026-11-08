//! Instrucciones AMM canónicas (encoding big-endian fijo).

use crate::types::{AssetId, FeeBps, LpShares, PoolId};
use crate::AmmError;
use rsc_core::{Amount, BlockHeight, Hash};

const TAG_CREATE_POOL: u8 = 0x01;
const TAG_ADD_LIQUIDITY: u8 = 0x02;
const TAG_REMOVE_LIQUIDITY: u8 = 0x03;
const TAG_SWAP_EXACT_INPUT: u8 = 0x04;
const TAG_SWAP_EXACT_OUTPUT: u8 = 0x05;
const TAG_PAUSE_POOL: u8 = 0x06;
const TAG_ACTIVATE_POOL: u8 = 0x07;

/// Instrucción AMM embebida en [`rsc_ledger::TransactionPayload::Amm`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AmmInstruction {
    /// Crear pool (sin liquidez).
    CreatePool {
        /// Activo izquierdo (se ordena canónicamente).
        asset_left: AssetId,
        /// Activo derecho.
        asset_right: AssetId,
        /// Fee en basis points.
        fee_bps: FeeBps,
    },
    /// Depositar liquidez.
    AddLiquidity {
        /// Pool.
        pool_id: PoolId,
        /// Máximo A.
        max_amount_a: Amount,
        /// Máximo B.
        max_amount_b: Amount,
        /// Mínimo LP.
        min_lp_shares: LpShares,
        /// Deadline por altura (`u64::MAX` = sin deadline).
        deadline: u64,
    },
    /// Retirar liquidez.
    RemoveLiquidity {
        /// Pool.
        pool_id: PoolId,
        /// Shares a quemar.
        lp_shares: LpShares,
        /// Mínimo A.
        min_amount_a: Amount,
        /// Mínimo B.
        min_amount_b: Amount,
        /// Deadline.
        deadline: u64,
    },
    /// Swap exact-input.
    SwapExactInput {
        /// Pool.
        pool_id: PoolId,
        /// Token in.
        token_in: AssetId,
        /// Token out.
        token_out: AssetId,
        /// Cantidad exacta de entrada.
        amount_in: Amount,
        /// Mínimo de salida.
        min_amount_out: Amount,
        /// Deadline.
        deadline: u64,
    },
    /// Swap exact-output.
    SwapExactOutput {
        /// Pool.
        pool_id: PoolId,
        /// Token in.
        token_in: AssetId,
        /// Token out.
        token_out: AssetId,
        /// Cantidad exacta de salida.
        amount_out: Amount,
        /// Máximo de entrada.
        max_amount_in: Amount,
        /// Deadline.
        deadline: u64,
    },
    /// Pausar pool (requiere autoridad).
    PausePool {
        /// Pool.
        pool_id: PoolId,
    },
    /// Activar pool (requiere autoridad).
    ActivatePool {
        /// Pool.
        pool_id: PoolId,
    },
}

impl AmmInstruction {
    /// Encoding canónico.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::CreatePool {
                asset_left,
                asset_right,
                fee_bps,
            } => {
                out.push(TAG_CREATE_POOL);
                out.extend_from_slice(&asset_left.0 .0);
                out.extend_from_slice(&asset_right.0 .0);
                out.extend_from_slice(&fee_bps.get().to_be_bytes());
            }
            Self::AddLiquidity {
                pool_id,
                max_amount_a,
                max_amount_b,
                min_lp_shares,
                deadline,
            } => {
                out.push(TAG_ADD_LIQUIDITY);
                out.extend_from_slice(&pool_id.0 .0);
                out.extend_from_slice(&max_amount_a.0.to_be_bytes());
                out.extend_from_slice(&max_amount_b.0.to_be_bytes());
                out.extend_from_slice(&min_lp_shares.0.to_be_bytes());
                out.extend_from_slice(&deadline.to_be_bytes());
            }
            Self::RemoveLiquidity {
                pool_id,
                lp_shares,
                min_amount_a,
                min_amount_b,
                deadline,
            } => {
                out.push(TAG_REMOVE_LIQUIDITY);
                out.extend_from_slice(&pool_id.0 .0);
                out.extend_from_slice(&lp_shares.0.to_be_bytes());
                out.extend_from_slice(&min_amount_a.0.to_be_bytes());
                out.extend_from_slice(&min_amount_b.0.to_be_bytes());
                out.extend_from_slice(&deadline.to_be_bytes());
            }
            Self::SwapExactInput {
                pool_id,
                token_in,
                token_out,
                amount_in,
                min_amount_out,
                deadline,
            } => {
                out.push(TAG_SWAP_EXACT_INPUT);
                out.extend_from_slice(&pool_id.0 .0);
                out.extend_from_slice(&token_in.0 .0);
                out.extend_from_slice(&token_out.0 .0);
                out.extend_from_slice(&amount_in.0.to_be_bytes());
                out.extend_from_slice(&min_amount_out.0.to_be_bytes());
                out.extend_from_slice(&deadline.to_be_bytes());
            }
            Self::SwapExactOutput {
                pool_id,
                token_in,
                token_out,
                amount_out,
                max_amount_in,
                deadline,
            } => {
                out.push(TAG_SWAP_EXACT_OUTPUT);
                out.extend_from_slice(&pool_id.0 .0);
                out.extend_from_slice(&token_in.0 .0);
                out.extend_from_slice(&token_out.0 .0);
                out.extend_from_slice(&amount_out.0.to_be_bytes());
                out.extend_from_slice(&max_amount_in.0.to_be_bytes());
                out.extend_from_slice(&deadline.to_be_bytes());
            }
            Self::PausePool { pool_id } => {
                out.push(TAG_PAUSE_POOL);
                out.extend_from_slice(&pool_id.0 .0);
            }
            Self::ActivatePool { pool_id } => {
                out.push(TAG_ACTIVATE_POOL);
                out.extend_from_slice(&pool_id.0 .0);
            }
        }
        out
    }

    /// Decodifica encoding canónico.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AmmError> {
        let (tag, rest) = bytes.split_first().ok_or(AmmError::InvalidEncoding)?;
        match *tag {
            TAG_CREATE_POOL => {
                if rest.len() != 32 + 32 + 2 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::CreatePool {
                    asset_left: AssetId(Hash(rest[0..32].try_into().unwrap())),
                    asset_right: AssetId(Hash(rest[32..64].try_into().unwrap())),
                    fee_bps: FeeBps::new(u16::from_be_bytes(rest[64..66].try_into().unwrap()))?,
                })
            }
            TAG_ADD_LIQUIDITY => {
                if rest.len() != 32 + 16 * 3 + 8 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::AddLiquidity {
                    pool_id: PoolId(Hash(rest[0..32].try_into().unwrap())),
                    max_amount_a: Amount(u128::from_be_bytes(rest[32..48].try_into().unwrap())),
                    max_amount_b: Amount(u128::from_be_bytes(rest[48..64].try_into().unwrap())),
                    min_lp_shares: LpShares(u128::from_be_bytes(
                        rest[64..80].try_into().unwrap(),
                    )),
                    deadline: u64::from_be_bytes(rest[80..88].try_into().unwrap()),
                })
            }
            TAG_REMOVE_LIQUIDITY => {
                if rest.len() != 32 + 16 * 3 + 8 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::RemoveLiquidity {
                    pool_id: PoolId(Hash(rest[0..32].try_into().unwrap())),
                    lp_shares: LpShares(u128::from_be_bytes(rest[32..48].try_into().unwrap())),
                    min_amount_a: Amount(u128::from_be_bytes(rest[48..64].try_into().unwrap())),
                    min_amount_b: Amount(u128::from_be_bytes(rest[64..80].try_into().unwrap())),
                    deadline: u64::from_be_bytes(rest[80..88].try_into().unwrap()),
                })
            }
            TAG_SWAP_EXACT_INPUT => {
                if rest.len() != 32 * 3 + 16 * 2 + 8 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::SwapExactInput {
                    pool_id: PoolId(Hash(rest[0..32].try_into().unwrap())),
                    token_in: AssetId(Hash(rest[32..64].try_into().unwrap())),
                    token_out: AssetId(Hash(rest[64..96].try_into().unwrap())),
                    amount_in: Amount(u128::from_be_bytes(rest[96..112].try_into().unwrap())),
                    min_amount_out: Amount(u128::from_be_bytes(
                        rest[112..128].try_into().unwrap(),
                    )),
                    deadline: u64::from_be_bytes(rest[128..136].try_into().unwrap()),
                })
            }
            TAG_SWAP_EXACT_OUTPUT => {
                if rest.len() != 32 * 3 + 16 * 2 + 8 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::SwapExactOutput {
                    pool_id: PoolId(Hash(rest[0..32].try_into().unwrap())),
                    token_in: AssetId(Hash(rest[32..64].try_into().unwrap())),
                    token_out: AssetId(Hash(rest[64..96].try_into().unwrap())),
                    amount_out: Amount(u128::from_be_bytes(rest[96..112].try_into().unwrap())),
                    max_amount_in: Amount(u128::from_be_bytes(
                        rest[112..128].try_into().unwrap(),
                    )),
                    deadline: u64::from_be_bytes(rest[128..136].try_into().unwrap()),
                })
            }
            TAG_PAUSE_POOL => {
                if rest.len() != 32 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::PausePool {
                    pool_id: PoolId(Hash(rest[0..32].try_into().unwrap())),
                })
            }
            TAG_ACTIVATE_POOL => {
                if rest.len() != 32 {
                    return Err(AmmError::InvalidEncoding);
                }
                Ok(Self::ActivatePool {
                    pool_id: PoolId(Hash(rest[0..32].try_into().unwrap())),
                })
            }
            _ => Err(AmmError::InvalidEncoding),
        }
    }

    /// Deadline opcional: `u64::MAX` significa ausente.
    pub fn deadline_height(deadline: u64) -> Option<BlockHeight> {
        if deadline == u64::MAX {
            None
        } else {
            Some(BlockHeight(deadline))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_roundtrip() {
        let cases = [
            AmmInstruction::CreatePool {
                asset_left: AssetId(Hash([1; 32])),
                asset_right: AssetId(Hash([2; 32])),
                fee_bps: FeeBps::new(30).unwrap(),
            },
            AmmInstruction::PausePool {
                pool_id: PoolId(Hash([3; 32])),
            },
            AmmInstruction::SwapExactInput {
                pool_id: PoolId(Hash([4; 32])),
                token_in: AssetId(Hash([1; 32])),
                token_out: AssetId(Hash([2; 32])),
                amount_in: Amount(100),
                min_amount_out: Amount(90),
                deadline: u64::MAX,
            },
        ];
        for instr in cases {
            let bytes = instr.to_bytes();
            assert_eq!(AmmInstruction::from_bytes(&bytes).unwrap(), instr);
        }
    }
}
