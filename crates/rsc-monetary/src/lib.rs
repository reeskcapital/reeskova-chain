//! Política monetaria verificable de RSC Chain — RSK v1.0.
//!
//! Orden de módulos (política económica):
//! 1. [`supply`] — Supply Tracker
//! 2. [`policy`] — Monetary Policy
//! 3. [`burn`] — Burn Engine
//! 4. [`staking`] — Staking Engine
//! 5. [`rewards`] — Validator Rewards
//! 6. [`treasury`] — Treasury Engine
//! 7. [`transparency`] — Dashboard de transparencia
//!
//! El nodo liquida el subsidio de minería y el 70% de las comisiones en la
//! bóveda de seguridad, y de ahí lo acredita a las personas que cobraron.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod amount_ext;
mod burn;
mod error;
mod policy;
mod rewards;
mod schedule;
mod security;
mod settlement;
mod staking;
mod supply;
mod transparency;
mod treasury;

pub use burn::{BurnEngine, ProtocolFeeSplit};
pub use error::MonetaryError;
pub use policy::{
    EmissionCategory, EmissionPhase, MonetaryPolicy, AMOUNT_DECIMALS, AMOUNT_SCALE,
    BPS_DENOMINATOR, COMMUNITY_BUDGET, CURRENCY_SYMBOL, HOLDER_REWARDS_BUDGET, MAX_SUPPLY,
    MAX_SUPPLY_WHOLE, MINING_BUDGET, MONETARY_POLICY_VERSION, OPERATOR_REWARDS_BUDGET,
    PROTOCOL_FEE_BURN_BPS, PROTOCOL_FEE_VALIDATOR_BPS, TREASURY_BUDGET, VALIDATOR_BUDGET,
};
pub use rewards::{RewardBalances, ValidatorRewards};
pub use schedule::{
    chain_day, daily_subsidy, personal_daily_subsidy, subsidy_for_days, NETWORK_DAILY_CAP,
    PERSONAL_DAILY_CAP, SECURITY_DAY_SECS,
};
pub use security::{payee_accounts, split_security_pot, SecuritySeat};
pub use settlement::{
    protocol_fee_pool_key_id, protocol_fee_pool_private_key, protocol_fee_pool_seed,
    security_vault_key_id, security_vault_private_key, security_vault_seed, MonetaryRuntime,
    ProtocolFeePlan,
};
pub use staking::{StakePosition, StakingEngine, StakingParams, ValidatorId};
pub use supply::{SupplySnapshot, SupplyTracker};
pub use transparency::{BudgetLine, MonetarySystem, MonetaryTransparencyReport};
pub use treasury::{TreasuryEngine, TreasuryProposal, TreasuryPurpose, VestingCliff};
