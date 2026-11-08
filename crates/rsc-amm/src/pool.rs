//! Gestión determinista de pools de liquidez.
//!
//! Este módulo es el subestado lógico del AMM: creación, identidad, lookup,
//! validación, activación y pausa. No abre RocksDB ni escribe en
//! `rsc-storage`; expone encoding canónico para que la capa de estado del
//! host lo persista de forma atómica junto al resto de la transición.
//!
//! No transfiere tokens ni calcula swaps: eso pertenece a accounting /
//! liquidity / swap. Aquí solo se registran metadatos y reservas ya
//! contabilizadas por esas capas.

use crate::constants::{AMM_VERSION, DEFAULT_MIN_LIQUIDITY, DOMAIN_POOL_ID};
use crate::events::AmmEvent;
use crate::types::{AssetId, FeeBps, LpShares, PoolId, PoolKey, Reserves};
use crate::AmmError;
use rsc_core::{AccountId, BlockHeight, Hash};
use rsc_crypto::hash;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Estado operacional de un pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PoolStatus {
    /// Registrado; sin swaps. Permite bootstrap de liquidez.
    Created = 0,
    /// Inicializado y disponible para operaciones normales.
    Active = 1,
    /// Swaps y nuevos aportes deshabilitados; retiros de emergencia permitidos.
    Paused = 2,
}

impl PoolStatus {
    /// ¿Se permite un swap?
    pub const fn allows_swap(self) -> bool {
        matches!(self, Self::Active)
    }

    /// ¿Se permite aportar liquidez (incl. bootstrap en Created)?
    pub const fn allows_mint(self) -> bool {
        matches!(self, Self::Created | Self::Active)
    }

    /// ¿Se permite retirar liquidez?
    ///
    /// Incluye [`Self::Created`] (salida tras bootstrap) y [`Self::Paused`]
    /// (política de emergencia). El sink bloqueado sigue siendo intocable.
    pub const fn allows_burn(self) -> bool {
        matches!(self, Self::Created | Self::Active | Self::Paused)
    }

    fn from_u8(v: u8) -> Result<Self, AmmError> {
        match v {
            0 => Ok(Self::Created),
            1 => Ok(Self::Active),
            2 => Ok(Self::Paused),
            _ => Err(AmmError::InvalidEncoding),
        }
    }
}

/// Campos que definen la identidad única de un pool (fee tier incluido).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PoolIdentity {
    /// Activo A canónico (`asset_a < asset_b`).
    pub asset_a: AssetId,
    /// Activo B canónico.
    pub asset_b: AssetId,
    /// Fee de swap en basis points.
    pub fee_bps: FeeBps,
    /// Versión AMM.
    pub version: u32,
}

impl PoolIdentity {
    /// Construye identidad ordenando el par y validando el fee.
    pub fn new(left: AssetId, right: AssetId, fee_bps: FeeBps) -> Result<Self, AmmError> {
        let key = PoolKey::new(left, right)?;
        // Revalida el fee aunque ya venga envuelto.
        let fee = FeeBps::new(fee_bps.get())?;
        Ok(Self {
            asset_a: key.asset_a,
            asset_b: key.asset_b,
            fee_bps: fee,
            version: AMM_VERSION,
        })
    }

    /// Bytes canónicos que entran en el hash de [`PoolId`].
    pub fn identity_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 32 + 2 + 4);
        out.extend_from_slice(&self.asset_a.0 .0);
        out.extend_from_slice(&self.asset_b.0 .0);
        out.extend_from_slice(&self.fee_bps.get().to_be_bytes());
        out.extend_from_slice(&self.version.to_be_bytes());
        out
    }

    /// [`PoolId`] determinista: `SHA-256(DOMAIN_POOL_ID || identity_bytes)`.
    pub fn pool_id(&self) -> PoolId {
        let mut preimage = Vec::with_capacity(DOMAIN_POOL_ID.len() + 70);
        preimage.extend_from_slice(DOMAIN_POOL_ID);
        preimage.extend_from_slice(&self.identity_bytes());
        let digest = hash(&preimage);
        PoolId(Hash(*digest.as_bytes()))
    }
}

/// Pool de liquidez de producto constante.
///
/// Las reservas y el supply solo cambian cuando accounting/liquidity lo
/// informan vía [`PoolRegistry::record_reserves`]. Este tipo no mueve saldos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidityPool {
    /// Identificador determinista.
    pub pool_id: PoolId,
    /// Identidad (par + fee + versión).
    pub identity: PoolIdentity,
    /// Reservas actuales (cero al crear).
    pub reserves: Reserves,
    /// Supply total de LP shares (incluye sink bloqueado).
    pub total_supply: LpShares,
    /// Shares bloqueadas en el sink de liquidez mínima.
    pub locked_supply: LpShares,
    /// Estado operacional.
    pub status: PoolStatus,
    /// Altura de creación.
    pub created_at: BlockHeight,
    /// Cuenta creadora (sin privilegios admin automáticos).
    pub creator: AccountId,
}

impl LiquidityPool {
    /// Par canónico.
    pub fn pair_key(&self) -> PoolKey {
        PoolKey {
            asset_a: self.identity.asset_a,
            asset_b: self.identity.asset_b,
        }
    }

    /// ¿Cumple liquidez mínima para activación?
    pub fn meets_activation_requirements(&self, min_liquidity: u128) -> bool {
        self.reserves.reserve_a.0 > 0
            && self.reserves.reserve_b.0 > 0
            && self.total_supply.0 > min_liquidity
            && self.total_supply.0 >= self.locked_supply.0
    }

    fn validate_shape(&self) -> Result<(), AmmError> {
        if self.pool_id != self.identity.pool_id() {
            return Err(AmmError::InvalidEncoding);
        }
        if self.identity.asset_a >= self.identity.asset_b {
            return Err(AmmError::InvalidPair);
        }
        FeeBps::new(self.identity.fee_bps.get())?;
        if self.identity.version != AMM_VERSION {
            return Err(AmmError::InvalidEncoding);
        }
        if self.locked_supply.0 > self.total_supply.0 {
            return Err(AmmError::InvalidReserves);
        }
        Ok(())
    }
}

/// Catálogo de activos registrados. Lo aporta el host / estado.
pub trait AssetCatalog {
    /// `true` si el activo puede usarse en un pool.
    fn is_registered(&self, asset: &AssetId) -> bool;
}

/// Autoridades de gobernanza / emergencia para pause, activate y create cerrado.
pub trait AmmAuthority {
    /// ¿Puede pausar, reactivar o crear pools con policy cerrada?
    fn can_manage_status(&self, actor: &AccountId) -> bool;
}

/// Política de creación y activación del registro.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolPolicy {
    /// Si `true`, cualquiera crea. Si `false`, solo governors (`AmmAuthority`).
    pub permissionless_creation: bool,
    /// Liquidez mínima exigida para `activate_pool`.
    pub min_liquidity: u128,
}

impl Default for PoolPolicy {
    fn default() -> Self {
        Self {
            permissionless_creation: true,
            min_liquidity: DEFAULT_MIN_LIQUIDITY,
        }
    }
}

/// Registro determinista de pools (subestado AMM).
///
/// Índices ordenados por [`BTreeMap`] para reproducibilidad entre validadores.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolRegistry {
    pools: BTreeMap<PoolId, LiquidityPool>,
    by_identity: BTreeMap<PoolIdentity, PoolId>,
    policy: PoolPolicy,
    /// Eventos de la transición en curso (el host los drena).
    pending_events: Vec<AmmEvent>,
}

impl PoolRegistry {
    /// Registro vacío con política dada.
    pub fn new(policy: PoolPolicy) -> Self {
        Self {
            pools: BTreeMap::new(),
            by_identity: BTreeMap::new(),
            policy,
            pending_events: Vec::new(),
        }
    }

    /// Política activa.
    pub fn policy(&self) -> &PoolPolicy {
        &self.policy
    }

    /// Actualiza la política (solo gobernanza del host debería llamarlo).
    pub fn set_policy(&mut self, policy: PoolPolicy) {
        self.policy = policy;
    }

    /// Crea un pool sin depositar activos ni activarlo.
    ///
    /// Con `permissionless_creation` cualquiera puede crear. Si la policy está
    /// cerrada, solo un actor autorizado por [`AmmAuthority`] (governor).
    pub fn create_pool<C: AssetCatalog, A: AmmAuthority>(
        &mut self,
        catalog: &C,
        authority: &A,
        creator: AccountId,
        asset_left: AssetId,
        asset_right: AssetId,
        fee_bps: FeeBps,
        block_height: BlockHeight,
    ) -> Result<PoolId, AmmError> {
        if !self.policy.permissionless_creation && !authority.can_manage_status(&creator) {
            return Err(AmmError::CreationDisabled);
        }
        if !catalog.is_registered(&asset_left) || !catalog.is_registered(&asset_right) {
            return Err(AmmError::AssetNotRegistered);
        }

        let identity = PoolIdentity::new(asset_left, asset_right, fee_bps)?;
        let pool_id = identity.pool_id();
        if self.by_identity.contains_key(&identity) || self.pools.contains_key(&pool_id) {
            return Err(AmmError::PoolAlreadyExists);
        }

        let asset_a = identity.asset_a;
        let asset_b = identity.asset_b;
        let fee = identity.fee_bps;
        let pool = LiquidityPool {
            pool_id,
            identity,
            reserves: Reserves::default(),
            total_supply: LpShares::ZERO,
            locked_supply: LpShares::ZERO,
            status: PoolStatus::Created,
            created_at: block_height,
            creator,
        };
        pool.validate_shape()?;

        // Inserción atómica lógica: ambos índices o ninguno.
        self.by_identity.insert(pool.identity, pool_id);
        self.pools.insert(pool_id, pool);
        self.pending_events.push(AmmEvent::PoolCreated {
            pool_id,
            asset_a,
            asset_b,
            fee_bps: fee,
            creator,
            block_height,
        });
        Ok(pool_id)
    }

    /// Obtiene un pool por id.
    pub fn get_pool(&self, pool_id: &PoolId) -> Result<&LiquidityPool, AmmError> {
        self.pools.get(pool_id).ok_or(AmmError::PoolNotFound)
    }

    /// Obtiene un pool mutable (capa liquidity/accounting).
    pub fn get_pool_mut(&mut self, pool_id: &PoolId) -> Result<&mut LiquidityPool, AmmError> {
        self.pools.get_mut(pool_id).ok_or(AmmError::PoolNotFound)
    }

    /// Lookup por par + fee (orden indiferente del par).
    pub fn get_pool_by_pair(
        &self,
        left: AssetId,
        right: AssetId,
        fee_bps: FeeBps,
    ) -> Result<&LiquidityPool, AmmError> {
        let identity = PoolIdentity::new(left, right, fee_bps)?;
        let id = self
            .by_identity
            .get(&identity)
            .ok_or(AmmError::PoolNotFound)?;
        self.get_pool(id)
    }

    /// `true` si existe la identidad.
    pub fn pool_exists(&self, left: AssetId, right: AssetId, fee_bps: FeeBps) -> bool {
        PoolIdentity::new(left, right, fee_bps)
            .ok()
            .and_then(|id| self.by_identity.get(&id).copied())
            .is_some()
    }

    /// Valida forma e índices de un pool registrado.
    pub fn validate_pool(&self, pool_id: &PoolId) -> Result<(), AmmError> {
        let pool = self.get_pool(pool_id)?;
        pool.validate_shape()?;
        let indexed = self
            .by_identity
            .get(&pool.identity)
            .copied()
            .ok_or(AmmError::InvalidEncoding)?;
        if indexed != *pool_id {
            return Err(AmmError::InvalidEncoding);
        }
        Ok(())
    }

    /// Estado operacional.
    pub fn get_pool_status(&self, pool_id: &PoolId) -> Result<PoolStatus, AmmError> {
        Ok(self.get_pool(pool_id)?.status)
    }

    /// Activa un pool que ya cumple liquidez mínima. Solo autoridad.
    pub fn activate_pool<A: AmmAuthority>(
        &mut self,
        authority: &A,
        actor: AccountId,
        pool_id: PoolId,
        block_height: BlockHeight,
    ) -> Result<(), AmmError> {
        if !authority.can_manage_status(&actor) {
            return Err(AmmError::Unauthorized);
        }
        let min_liq = self.policy.min_liquidity;
        let pool = self.get_pool_mut(&pool_id)?;
        match pool.status {
            PoolStatus::Created | PoolStatus::Paused => {}
            PoolStatus::Active => return Err(AmmError::InvalidPoolStatus),
        }
        if !pool.meets_activation_requirements(min_liq) {
            return Err(AmmError::NotReadyToActivate);
        }
        pool.status = PoolStatus::Active;
        self.pending_events.push(AmmEvent::PoolActivated {
            pool_id,
            authority: actor,
            block_height,
        });
        Ok(())
    }

    /// Pausa un pool activo. Solo autoridad. No confisca reservas.
    pub fn pause_pool<A: AmmAuthority>(
        &mut self,
        authority: &A,
        actor: AccountId,
        pool_id: PoolId,
        block_height: BlockHeight,
    ) -> Result<(), AmmError> {
        if !authority.can_manage_status(&actor) {
            return Err(AmmError::Unauthorized);
        }
        let pool = self.get_pool_mut(&pool_id)?;
        if pool.status != PoolStatus::Active {
            return Err(AmmError::InvalidPoolStatus);
        }
        pool.status = PoolStatus::Paused;
        self.pending_events.push(AmmEvent::PoolPaused {
            pool_id,
            authority: actor,
            block_height,
        });
        Ok(())
    }

    /// Lista de ids en orden canónico.
    pub fn list_pools(&self) -> Vec<PoolId> {
        self.pools.keys().copied().collect()
    }

    /// Comprueba política de swap según estado.
    pub fn ensure_swap_allowed(&self, pool_id: &PoolId) -> Result<(), AmmError> {
        let status = self.get_pool_status(pool_id)?;
        if status.allows_swap() {
            Ok(())
        } else {
            Err(AmmError::SwapNotPermitted)
        }
    }

    /// Comprueba política de mint según estado.
    pub fn ensure_mint_allowed(&self, pool_id: &PoolId) -> Result<(), AmmError> {
        let status = self.get_pool_status(pool_id)?;
        if status.allows_mint() {
            Ok(())
        } else {
            Err(AmmError::MintNotPermitted)
        }
    }

    /// Comprueba política de burn / withdraw (incl. emergencia en Paused).
    pub fn ensure_burn_allowed(&self, pool_id: &PoolId) -> Result<(), AmmError> {
        let status = self.get_pool_status(pool_id)?;
        if status.allows_burn() {
            Ok(())
        } else {
            Err(AmmError::BurnNotPermitted)
        }
    }

    /// Actualiza reservas/supply ya movidos por accounting. No transfiere tokens.
    ///
    /// No comprueba mint/burn: la capa de liquidez valida el estado antes.
    /// Usado tras depósitos y retiros (incl. emergencia en [`PoolStatus::Paused`]).
    pub fn record_reserves(
        &mut self,
        pool_id: &PoolId,
        reserves: Reserves,
        total_supply: LpShares,
        locked_supply: LpShares,
    ) -> Result<(), AmmError> {
        let pool = self.get_pool_mut(pool_id)?;
        if locked_supply.0 > total_supply.0 {
            return Err(AmmError::InvalidReserves);
        }
        pool.reserves = reserves;
        pool.total_supply = total_supply;
        pool.locked_supply = locked_supply;
        Ok(())
    }

    /// Encola un evento de la transición en curso.
    pub fn push_event(&mut self, event: AmmEvent) {
        self.pending_events.push(event);
    }

    /// Drena eventos pendientes de la transición.
    pub fn take_events(&mut self) -> Vec<AmmEvent> {
        core::mem::take(&mut self.pending_events)
    }

    /// Vista de eventos pendientes sin drenar.
    pub fn pending_events(&self) -> &[AmmEvent] {
        &self.pending_events
    }

    /// Encoding canónico big-endian del registro (sin eventos pendientes).
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&AMM_VERSION.to_be_bytes());
        out.push(u8::from(self.policy.permissionless_creation));
        out.extend_from_slice(&self.policy.min_liquidity.to_be_bytes());
        out.extend_from_slice(&(self.pools.len() as u32).to_be_bytes());
        for pool in self.pools.values() {
            encode_pool(&mut out, pool);
        }
        out
    }

    /// Restaura desde encoding canónico. Eventos pendientes quedan vacíos.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AmmError> {
        let mut cur = bytes;
        let version = read_u32(&mut cur)?;
        if version != AMM_VERSION {
            return Err(AmmError::InvalidEncoding);
        }
        let permissionless = read_u8(&mut cur)? != 0;
        let min_liquidity = read_u128(&mut cur)?;
        let n = read_u32(&mut cur)? as usize;
        let mut registry = Self::new(PoolPolicy {
            permissionless_creation: permissionless,
            min_liquidity,
        });
        for _ in 0..n {
            let pool = decode_pool(&mut cur)?;
            pool.validate_shape()?;
            if registry.pools.contains_key(&pool.pool_id)
                || registry.by_identity.contains_key(&pool.identity)
            {
                return Err(AmmError::PoolAlreadyExists);
            }
            registry.by_identity.insert(pool.identity, pool.pool_id);
            registry.pools.insert(pool.pool_id, pool);
        }
        if !cur.is_empty() {
            return Err(AmmError::InvalidEncoding);
        }
        Ok(registry)
    }
}

fn encode_pool(out: &mut Vec<u8>, pool: &LiquidityPool) {
    out.extend_from_slice(&pool.pool_id.0 .0);
    out.extend_from_slice(&pool.identity.asset_a.0 .0);
    out.extend_from_slice(&pool.identity.asset_b.0 .0);
    out.extend_from_slice(&pool.identity.fee_bps.get().to_be_bytes());
    out.extend_from_slice(&pool.identity.version.to_be_bytes());
    out.extend_from_slice(&pool.reserves.reserve_a.0.to_be_bytes());
    out.extend_from_slice(&pool.reserves.reserve_b.0.to_be_bytes());
    out.extend_from_slice(&pool.total_supply.0.to_be_bytes());
    out.extend_from_slice(&pool.locked_supply.0.to_be_bytes());
    out.push(pool.status as u8);
    out.extend_from_slice(&pool.created_at.0.to_be_bytes());
    out.extend_from_slice(&pool.creator.0);
}

fn decode_pool(cur: &mut &[u8]) -> Result<LiquidityPool, AmmError> {
    let pool_id = PoolId(Hash(read_array::<32>(cur)?));
    let asset_a = AssetId(Hash(read_array::<32>(cur)?));
    let asset_b = AssetId(Hash(read_array::<32>(cur)?));
    let fee_bps = FeeBps::new(read_u16(cur)?)?;
    let version = read_u32(cur)?;
    let reserve_a = rsc_core::Amount(read_u128(cur)?);
    let reserve_b = rsc_core::Amount(read_u128(cur)?);
    let total_supply = LpShares(read_u128(cur)?);
    let locked_supply = LpShares(read_u128(cur)?);
    let status = PoolStatus::from_u8(read_u8(cur)?)?;
    let created_at = BlockHeight(read_u64(cur)?);
    let creator = AccountId(read_array::<32>(cur)?);
    Ok(LiquidityPool {
        pool_id,
        identity: PoolIdentity {
            asset_a,
            asset_b,
            fee_bps,
            version,
        },
        reserves: Reserves::new(reserve_a, reserve_b),
        total_supply,
        locked_supply,
        status,
        created_at,
        creator,
    })
}

fn read_array<const N: usize>(cur: &mut &[u8]) -> Result<[u8; N], AmmError> {
    if cur.len() < N {
        return Err(AmmError::InvalidEncoding);
    }
    let (head, tail) = cur.split_at(N);
    *cur = tail;
    let mut out = [0u8; N];
    out.copy_from_slice(head);
    Ok(out)
}

fn read_u8(cur: &mut &[u8]) -> Result<u8, AmmError> {
    Ok(read_array::<1>(cur)?[0])
}

fn read_u16(cur: &mut &[u8]) -> Result<u16, AmmError> {
    Ok(u16::from_be_bytes(read_array::<2>(cur)?))
}

fn read_u32(cur: &mut &[u8]) -> Result<u32, AmmError> {
    Ok(u32::from_be_bytes(read_array::<4>(cur)?))
}

fn read_u64(cur: &mut &[u8]) -> Result<u64, AmmError> {
    Ok(u64::from_be_bytes(read_array::<8>(cur)?))
}

fn read_u128(cur: &mut &[u8]) -> Result<u128, AmmError> {
    Ok(u128::from_be_bytes(read_array::<16>(cur)?))
}

/// Catálogo en memoria para tests y hosts simples.
#[derive(Clone, Debug, Default)]
pub struct InMemoryAssetCatalog {
    assets: BTreeSet<AssetId>,
}

impl InMemoryAssetCatalog {
    /// Vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un activo.
    pub fn register(&mut self, asset: AssetId) {
        self.assets.insert(asset);
    }
}

impl AssetCatalog for InMemoryAssetCatalog {
    fn is_registered(&self, asset: &AssetId) -> bool {
        self.assets.contains(asset)
    }
}

/// Autoridad fija: un conjunto de cuentas de gobernanza.
#[derive(Clone, Debug, Default)]
pub struct GovernanceAuthority {
    managers: BTreeSet<AccountId>,
}

impl GovernanceAuthority {
    /// Vacía.
    pub fn new() -> Self {
        Self::default()
    }

    /// Añade una autoridad.
    pub fn grant(&mut self, account: AccountId) {
        self.managers.insert(account);
    }
}

impl AmmAuthority for GovernanceAuthority {
    fn can_manage_status(&self, actor: &AccountId) -> bool {
        self.managers.contains(actor)
    }
}

/// Nadie autorizado: útil cuando `permissionless_creation` ya abre la creación.
impl AmmAuthority for () {
    fn can_manage_status(&self, _actor: &AccountId) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rsc_core::Amount;

    fn asset(byte: u8) -> AssetId {
        AssetId(Hash([byte; 32]))
    }

    fn account(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    fn setup() -> (PoolRegistry, InMemoryAssetCatalog, GovernanceAuthority) {
        let mut catalog = InMemoryAssetCatalog::new();
        catalog.register(asset(1));
        catalog.register(asset(2));
        catalog.register(asset(3));
        let mut gov = GovernanceAuthority::new();
        gov.grant(account(9));
        (PoolRegistry::new(PoolPolicy::default()), catalog, gov)
    }

    #[test]
    fn create_pool_valid_and_deterministic_id() {
        let (mut reg, catalog, _) = setup();
        let fee = FeeBps::new(30).unwrap();
        let id1 = reg
            .create_pool(
                &catalog,
                &(),
                account(1),
                asset(2),
                asset(1),
                fee,
                BlockHeight(10),
            )
            .unwrap();
        let id2 = PoolIdentity::new(asset(1), asset(2), fee).unwrap().pool_id();
        assert_eq!(id1, id2);
        assert_eq!(reg.get_pool_status(&id1).unwrap(), PoolStatus::Created);
        assert!(matches!(
            reg.take_events()[0],
            AmmEvent::PoolCreated { .. }
        ));
    }

    #[test]
    fn duplicate_and_reverse_order_rejected() {
        let (mut reg, catalog, _) = setup();
        let fee = FeeBps::new(30).unwrap();
        reg.create_pool(
            &catalog,
            &(),
            account(1),
            asset(1),
            asset(2),
            fee,
            BlockHeight(1),
        )
        .unwrap();
        assert_eq!(
            reg.create_pool(
                &catalog,
                &(),
                account(1),
                asset(2),
                asset(1),
                fee,
                BlockHeight(2),
            )
            .unwrap_err(),
            AmmError::PoolAlreadyExists
        );
    }

    #[test]
    fn distinct_fee_tiers_distinct_ids() {
        let (mut reg, catalog, _) = setup();
        let a = reg
            .create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(2),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap();
        let b = reg
            .create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(2),
                FeeBps::new(100).unwrap(),
                BlockHeight(1),
            )
            .unwrap();
        assert_ne!(a, b);
        assert_eq!(reg.list_pools().len(), 2);
    }

    #[test]
    fn rejects_identical_unregistered_invalid_fee() {
        let (mut reg, catalog, _) = setup();
        assert_eq!(
            reg.create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(1),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap_err(),
            AmmError::InvalidPair
        );
        assert_eq!(
            reg.create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(9),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap_err(),
            AmmError::AssetNotRegistered
        );
        assert!(FeeBps::new(u16::MAX).is_err());
    }

    #[test]
    fn empty_activation_and_auth() {
        let (mut reg, catalog, gov) = setup();
        let id = reg
            .create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(2),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap();
        assert_eq!(
            reg.activate_pool(&gov, account(9), id, BlockHeight(2))
                .unwrap_err(),
            AmmError::NotReadyToActivate
        );
        assert_eq!(
            reg.activate_pool(&gov, account(1), id, BlockHeight(2))
                .unwrap_err(),
            AmmError::Unauthorized
        );
        // Bootstrap liquidez (accounting ya movió fondos).
        reg.record_reserves(
            &id,
            Reserves::new(Amount(50_000), Amount(50_000)),
            LpShares(50_000),
            LpShares(1_000),
        )
        .unwrap();
        reg.activate_pool(&gov, account(9), id, BlockHeight(3))
            .unwrap();
        assert_eq!(reg.get_pool_status(&id).unwrap(), PoolStatus::Active);
    }

    #[test]
    fn pause_blocks_swap_allows_burn() {
        let (mut reg, catalog, gov) = setup();
        let id = reg
            .create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(2),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap();
        reg.record_reserves(
            &id,
            Reserves::new(Amount(50_000), Amount(50_000)),
            LpShares(50_000),
            LpShares(1_000),
        )
        .unwrap();
        reg.activate_pool(&gov, account(9), id, BlockHeight(2))
            .unwrap();
        assert!(reg.ensure_swap_allowed(&id).is_ok());
        assert_eq!(
            reg.pause_pool(&gov, account(1), id, BlockHeight(3))
                .unwrap_err(),
            AmmError::Unauthorized
        );
        reg.pause_pool(&gov, account(9), id, BlockHeight(3))
            .unwrap();
        assert_eq!(
            reg.ensure_swap_allowed(&id).unwrap_err(),
            AmmError::SwapNotPermitted
        );
        assert_eq!(
            reg.ensure_mint_allowed(&id).unwrap_err(),
            AmmError::MintNotPermitted
        );
        assert!(reg.ensure_burn_allowed(&id).is_ok());
    }

    #[test]
    fn persistence_roundtrip() {
        let (mut reg, catalog, gov) = setup();
        let id = reg
            .create_pool(
                &catalog,
                &(),
                account(1),
                asset(1),
                asset(2),
                FeeBps::new(30).unwrap(),
                BlockHeight(7),
            )
            .unwrap();
        reg.record_reserves(
            &id,
            Reserves::new(Amount(10_000), Amount(20_000)),
            LpShares(14_142),
            LpShares(1_000),
        )
        .unwrap();
        reg.activate_pool(&gov, account(9), id, BlockHeight(8))
            .unwrap();
        let _ = reg.take_events();
        let bytes = reg.to_canonical_bytes();
        let restored = PoolRegistry::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(restored.get_pool(&id).unwrap(), reg.get_pool(&id).unwrap());
        assert_eq!(restored.list_pools(), reg.list_pools());
    }

    #[test]
    fn creator_has_no_admin_rights() {
        let (mut reg, catalog, gov) = setup();
        let creator = account(1);
        let id = reg
            .create_pool(
                &catalog,
                &(),
                creator,
                asset(1),
                asset(2),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap();
        reg.record_reserves(
            &id,
            Reserves::new(Amount(50_000), Amount(50_000)),
            LpShares(50_000),
            LpShares(1_000),
        )
        .unwrap();
        reg.activate_pool(&gov, account(9), id, BlockHeight(2))
            .unwrap();
        assert_eq!(
            reg.pause_pool(&gov, creator, id, BlockHeight(3))
                .unwrap_err(),
            AmmError::Unauthorized
        );
    }

    proptest! {
        #[test]
        fn prop_pool_id_independent_of_call_order(
            b1 in 1u8..40,
            b2 in 41u8..80,
            fee in 0u16..500,
        ) {
            let fee = FeeBps::new(fee).unwrap();
            let id_ab = PoolIdentity::new(asset(b1), asset(b2), fee).unwrap().pool_id();
            let id_ba = PoolIdentity::new(asset(b2), asset(b1), fee).unwrap().pool_id();
            assert_eq!(id_ab, id_ba);
        }
    }
}
