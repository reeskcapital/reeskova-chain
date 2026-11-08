//! Ejecutor nativo AMM: valida tx, aplica instrucción, commit atómico.

use crate::events::AmmEvent;
use crate::liquidity::{AddLiquidityParams, LiquidityEngine, RemoveLiquidityParams};
use crate::native::bridge::{flush_accounting, hydrate_accounting, AssetsCatalog};
use crate::native::instruction::AmmInstruction;
use crate::native::store::AmmStore;
use crate::pool::GovernanceAuthority;
use crate::swap::{ExactInputSwap, ExactOutputSwap, SwapEngine};
use crate::AmmError;
use rsc_assets::AssetLedger;
use rsc_core::{AccountId, Amount, BlockHeight, Hash};
use rsc_crypto::PublicKey;
use rsc_ledger::{Transaction, TransactionError, TransactionPayload};
use rsc_state::{State, StateCommitment, StateError, StateOperation, StateTransition};
use rsc_storage::{RscDatabase, StorageBatch};
use std::sync::Arc;
use thiserror::Error;

/// Contexto de bloque determinista (sin reloj local).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockContext {
    /// Altura del bloque en ejecución.
    pub height: BlockHeight,
    /// Nueva versión de estado tras el commit (`state.version + 1` típico).
    pub new_state_version: u64,
}

/// Resultado de una ejecución AMM confirmada.
#[derive(Clone, Debug)]
pub struct AmmExecutionResult {
    /// Compromiso de estado (nonce consumido).
    pub state_commitment: StateCommitment,
    /// Eventos emitidos (también persistidos en Receipts).
    pub events: Vec<AmmEvent>,
    /// Id de transacción (bytes del ledger).
    pub transaction_id: [u8; 32],
}

/// Error del ejecutor nativo (incluye capas host).
#[derive(Debug, Error)]
pub enum NativeAmmError {
    /// Fallo del motor AMM.
    #[error(transparent)]
    Amm(#[from] AmmError),
    /// Fallo de transacción / firma.
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    /// Fallo de estado.
    #[error("state error")]
    State(#[from] StateError),
    /// Fallo de almacenamiento.
    #[error("storage error")]
    Storage,
    /// Payload no es AMM.
    #[error("expected amm transaction payload")]
    NotAmmPayload,
    /// Emisor de la tx no coincide con la cuenta autorizada de la operación.
    #[error("sender unauthorized for amm operation")]
    SenderMismatch,
}

/// Runtime que conecta ledger + assets + AMM + state + storage.
pub struct AmmRuntime {
    db: Arc<RscDatabase>,
    state: State,
}

impl AmmRuntime {
    /// Abre el runtime sobre una base y estado ya inicializados.
    pub fn new(db: Arc<RscDatabase>, state: State) -> Self {
        Self { db, state }
    }

    /// Acceso al estado.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Acceso a la base.
    pub fn db(&self) -> &RscDatabase {
        &self.db
    }

    /// Ejecuta una transacción AMM firmada de forma atómica (valida + write).
    pub fn execute_transaction(
        &self,
        tx: &Transaction,
        signer: &PublicKey,
        ctx: BlockContext,
    ) -> Result<AmmExecutionResult, NativeAmmError> {
        tx.validate(signer)?;
        let mut batch = StorageBatch::new();
        let result = self.stage_transaction(&mut batch, tx, ctx)?;
        self.db.write(batch).map_err(|_| NativeAmmError::Storage)?;
        Ok(result)
    }

    /// Igual que [`Self::execute_transaction`] sin re-verificar la firma.
    ///
    /// Para el path de commit del nodo / validador, alineado con Transfer
    /// (la admisión ya filtró el payload; el nonce lo impone `ConsumeNonce`).
    pub fn execute_committed_transaction(
        &self,
        tx: &Transaction,
        ctx: BlockContext,
    ) -> Result<AmmExecutionResult, NativeAmmError> {
        let mut batch = StorageBatch::new();
        let result = self.stage_transaction(&mut batch, tx, ctx)?;
        self.db.write(batch).map_err(|_| NativeAmmError::Storage)?;
        Ok(result)
    }

    /// Valida y programa AMM + assets + ConsumeNonce + events en `batch`.
    ///
    /// No llama a `write`. El host puede añadir el bloque canónico al mismo lote.
    /// Usa el [`State`] del runtime (misma base que el nodo).
    pub fn stage_transaction(
        &self,
        batch: &mut StorageBatch,
        tx: &Transaction,
        ctx: BlockContext,
    ) -> Result<AmmExecutionResult, NativeAmmError> {
        let instruction_bytes = match &tx.payload {
            TransactionPayload::Amm { instruction } => instruction.as_slice(),
            _ => return Err(NativeAmmError::NotAmmPayload),
        };
        let instruction = AmmInstruction::from_bytes(instruction_bytes)?;
        let tx_id = tx.transaction_id();
        let tx_hash = Hash(*tx_id.0.as_bytes());
        let sender = tx.sender;

        let mut assets = AssetLedger::load(&self.db).map_err(|_| NativeAmmError::Storage)?;
        let mut store = AmmStore::load(&self.db)?;
        hydrate_accounting(&mut store.accounting, &assets);

        let authority = governance_from(&store.governors);
        {
            let catalog = AssetsCatalog::new(&assets);
            match instruction {
                AmmInstruction::CreatePool {
                    asset_left,
                    asset_right,
                    fee_bps,
                } => {
                    store.pools.create_pool(
                        &catalog,
                        &authority,
                        sender,
                        asset_left,
                        asset_right,
                        fee_bps,
                        ctx.height,
                    )?;
                }
                AmmInstruction::AddLiquidity {
                    pool_id,
                    max_amount_a,
                    max_amount_b,
                    min_lp_shares,
                    deadline,
                } => {
                    LiquidityEngine::add_liquidity(
                        &mut store.pools,
                        &mut store.accounting,
                        &catalog,
                        AddLiquidityParams {
                            pool_id,
                            provider: sender,
                            max_amount_a,
                            max_amount_b,
                            min_lp_shares,
                            deadline: AmmInstruction::deadline_height(deadline),
                            current_block: ctx.height,
                            op_id: Some(tx_hash),
                        },
                    )?;
                }
                AmmInstruction::RemoveLiquidity {
                    pool_id,
                    lp_shares,
                    min_amount_a,
                    min_amount_b,
                    deadline,
                } => {
                    LiquidityEngine::remove_liquidity(
                        &mut store.pools,
                        &mut store.accounting,
                        RemoveLiquidityParams {
                            pool_id,
                            provider: sender,
                            lp_shares,
                            min_amount_a,
                            min_amount_b,
                            deadline: AmmInstruction::deadline_height(deadline),
                            current_block: ctx.height,
                            op_id: Some(tx_hash),
                        },
                    )?;
                }
                AmmInstruction::SwapExactInput {
                    pool_id,
                    token_in,
                    token_out,
                    amount_in,
                    min_amount_out,
                    deadline,
                } => {
                    SwapEngine::execute_swap_exact_input(
                        &mut store.pools,
                        &mut store.accounting,
                        &mut store.fees,
                        ExactInputSwap {
                            pool_id,
                            trader: sender,
                            token_in,
                            token_out,
                            amount_in,
                            min_amount_out,
                            deadline: AmmInstruction::deadline_height(deadline),
                            current_block: ctx.height,
                            op_id: Some(tx_hash),
                        },
                    )?;
                }
                AmmInstruction::SwapExactOutput {
                    pool_id,
                    token_in,
                    token_out,
                    amount_out,
                    max_amount_in,
                    deadline,
                } => {
                    SwapEngine::execute_swap_exact_output(
                        &mut store.pools,
                        &mut store.accounting,
                        &mut store.fees,
                        ExactOutputSwap {
                            pool_id,
                            trader: sender,
                            token_in,
                            token_out,
                            amount_out,
                            max_amount_in,
                            deadline: AmmInstruction::deadline_height(deadline),
                            current_block: ctx.height,
                            op_id: Some(tx_hash),
                        },
                    )?;
                }
                AmmInstruction::PausePool { pool_id } => {
                    store
                        .pools
                        .pause_pool(&authority, sender, pool_id, ctx.height)?;
                }
                AmmInstruction::ActivatePool { pool_id } => {
                    store
                        .pools
                        .activate_pool(&authority, sender, pool_id, ctx.height)?;
                }
            }
        }

        flush_accounting(&store.accounting, &mut assets)?;
        let events = store.pools.take_events();

        let root = self.state.state_root()?;
        let version = self.state.version()?;
        let mut transition = StateTransition::new(root, version);
        transition.push(StateOperation::ConsumeNonce {
            account: sender.into(),
            expected_nonce: tx.nonce,
        });

        let commitment =
            self.state
                .stage_transition(batch, &transition, ctx.new_state_version)?;
        assets.stage(batch);
        store.stage(batch);
        AmmStore::stage_events(batch, tx_id.0.as_bytes(), &events)?;

        Ok(AmmExecutionResult {
            state_commitment: commitment,
            events,
            transaction_id: *tx_id.0.as_bytes(),
        })
    }

    /// Inicializa gobernanza AMM (persiste lista de governors). Solo host/génesis.
    pub fn set_governors(&self, governors: Vec<AccountId>) -> Result<(), NativeAmmError> {
        let mut store = AmmStore::load(&self.db)?;
        store.governors = governors;
        let mut batch = StorageBatch::new();
        store.stage(&mut batch);
        self.db.write(batch).map_err(|_| NativeAmmError::Storage)?;
        Ok(())
    }

    /// Fija la política de pools y la stagea en `AmmStore` (sobrevive restart).
    pub fn set_policy(&self, policy: crate::pool::PoolPolicy) -> Result<(), NativeAmmError> {
        let mut store = AmmStore::load(&self.db)?;
        store.pools.set_policy(policy);
        let mut batch = StorageBatch::new();
        store.stage(&mut batch);
        self.db.write(batch).map_err(|_| NativeAmmError::Storage)?;
        Ok(())
    }

    /// Fondea saldos de prueba / génesis en `rsc-assets` (no toca `native_balance`).
    pub fn credit_asset(
        &self,
        account: AccountId,
        asset: crate::types::AssetId,
        amount: Amount,
    ) -> Result<(), NativeAmmError> {
        let mut assets = AssetLedger::load(&self.db).map_err(|_| NativeAmmError::Storage)?;
        assets
            .credit(account, crate::native::bridge::assets_asset_id(asset), amount)
            .map_err(crate::native::bridge::map_assets_err)?;
        let mut batch = StorageBatch::new();
        assets.stage(&mut batch);
        self.db.write(batch).map_err(|_| NativeAmmError::Storage)?;
        Ok(())
    }

    /// Registra un activo en el ledger autoritativo.
    pub fn register_asset(
        &self,
        asset: crate::types::AssetId,
        decimals: u8,
        symbol: &str,
    ) -> Result<(), NativeAmmError> {
        let mut assets = AssetLedger::load(&self.db).map_err(|_| NativeAmmError::Storage)?;
        assets
            .register(
                crate::native::bridge::assets_asset_id(asset),
                rsc_assets::AssetMetadata::new(decimals, symbol),
            )
            .map_err(crate::native::bridge::map_assets_err)?;
        let mut batch = StorageBatch::new();
        assets.stage(&mut batch);
        self.db.write(batch).map_err(|_| NativeAmmError::Storage)?;
        Ok(())
    }

    /// Carga el ledger de activos actual.
    pub fn load_assets(&self) -> Result<AssetLedger, NativeAmmError> {
        AssetLedger::load(&self.db).map_err(|_| NativeAmmError::Storage)
    }

    /// Carga el subestado AMM actual.
    pub fn load_store(&self) -> Result<AmmStore, NativeAmmError> {
        Ok(AmmStore::load(&self.db)?)
    }
}

fn governance_from(governors: &[AccountId]) -> GovernanceAuthority {
    let mut gov = GovernanceAuthority::new();
    for g in governors {
        gov.grant(*g);
    }
    gov
}

/// Firma y empaqueta una instrucción AMM como transacción ledger.
pub fn sign_amm_tx(
    sender: AccountId,
    nonce: u64,
    instruction: &AmmInstruction,
    key: &rsc_crypto::PrivateKey,
) -> Result<Transaction, TransactionError> {
    let mut tx = Transaction {
        version: rsc_ledger::TRANSACTION_VERSION_V1,
        sender,
        nonce,
        payload: TransactionPayload::Amm {
            instruction: instruction.to_bytes(),
        },
        signature: rsc_crypto::Signature::from_bytes(&[0u8; 64]).expect("zero sig bytes"),
    };
    tx.sign(key)?;
    Ok(tx)
}
