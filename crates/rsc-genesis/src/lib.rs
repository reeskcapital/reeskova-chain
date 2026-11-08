//! Definición determinista del génesis de RSC Chain v1.
//!
//! Este crate describe con qué arranca la cadena: versión, [`ChainId`],
//! timestamp explícito, cuentas iniciales y el [`GenesisHash`] de ese
//! documento. No pone la cadena en marcha.
//!
//! Los campos opcionales [`Genesis::amm_assets`], [`Genesis::amm_balances`],
//! [`Genesis::amm_governors`] y [`Genesis::amm_permissionless_creation`]
//! declaran el ledger multi-activo (`rsc-assets`) y la política de creación
//! de pools. No se mezclan con `native_balance` ni entran en los presupuestos
//! de [`GenesisAllocationCategory`].
//!
//! No modifica estado, no calcula un state root ni un ledger root, no escribe
//! almacenamiento, no crea bloques, no ejecuta
//! transacciones ni WASM y no abre red. Declara los parámetros de
//! `ConsensusConfig` y la lista de validadores; no arranca el motor.
//! Tampoco lee el reloj, no genera claves y no firma.
//!
//! El mismo documento produce los mismos bytes canónicos y el mismo
//! [`GenesisHash`] en cualquier máquina. El orden del `Vec` de cuentas no
//! entra en ese hash: la vista canónica ordena por [`AccountId`] ascendente
//! sin modificar el valor guardado. Los bloques AMM se ordenan por
//! `asset_id` y por `(account_id, asset_id)`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::collections::BTreeSet;

use rsc_consensus::{
    validate_validator_record, ConsensusConfig, NetworkMode, ParticipationScore, ValidatorRecord,
    ValidatorSet,
};
use rsc_core::{AccountId, ChainId, Hash};
use rsc_crypto::{hash, Hash256, KeyId, PublicKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Única versión de génesis aceptada por [`Genesis::validate`].
pub const GENESIS_VERSION_V1: u32 = 1;

/// Unidades mínimas por 1 RSK según Política Monetaria v1.0 (`10^18`).
///
/// Debe coincidir con `rsc_monetary::AMOUNT_SCALE`. Los fixtures de test
/// pueden usar otra escala; los documentos de red productivos usan esta.
pub const RSK_AMOUNT_SCALE: u64 = 1_000_000_000_000_000_000;

/// Fee mínimo de protocolo por defecto en redes alineadas a la política
/// (0.000001 RSK = `10^12` unidades mínimas). Cero sigue siendo válido.
pub const RSK_DEFAULT_MINIMUM_FEE: u128 = 1_000_000_000_000;

/// Supply máximo RSK v1.0 en unidades mínimas (`35_999_999 * 10^18`).
pub const RSK_MAX_SUPPLY: u128 = 35_999_999 * (RSK_AMOUNT_SCALE as u128);

/// Presupuesto minería (`21_599_999 * 10^18`).
pub const RSK_MINING_BUDGET: u128 = 21_599_999 * (RSK_AMOUNT_SCALE as u128);

/// Presupuesto validadores / staking (`7_200_000 * 10^18`).
pub const RSK_VALIDATOR_BUDGET: u128 = 7_200_000 * (RSK_AMOUNT_SCALE as u128);

/// Presupuesto comunidad (`5_400_000 * 10^18`).
pub const RSK_COMMUNITY_BUDGET: u128 = 5_400_000 * (RSK_AMOUNT_SCALE as u128);

/// Presupuesto tesorería (`1_800_000 * 10^18`).
pub const RSK_TREASURY_BUDGET: u128 = 1_800_000 * (RSK_AMOUNT_SCALE as u128);

/// Sub-presupuesto holders dentro de validadores (`5_400_000 * 10^18`).
pub const RSK_HOLDER_REWARDS_BUDGET: u128 = 5_400_000 * (RSK_AMOUNT_SCALE as u128);

/// Sub-presupuesto operadores dentro de validadores (`1_800_000 * 10^18`).
pub const RSK_OPERATOR_REWARDS_BUDGET: u128 = 1_800_000 * (RSK_AMOUNT_SCALE as u128);

/// Prefijo de dominio del documento. No es una variante de `CryptoDomain`:
/// `rsc-crypto` no define un dominio de génesis. El patrón es el mismo que
/// el header de bloque: ASCII estable || cuerpo canónico, luego SHA-256.
const DOMAIN_GENESIS: &[u8] = b"RSC-GENESIS\0";

/// Mismo dominio que `rsc_assets::native_asset_id` (`RSC-ASSETS-NATIVE/V1\0`).
///
/// Se duplica el preimage aquí para no acoplar `rsc-genesis` al ledger de
/// activos; ambos deben permanecer alineados.
const DOMAIN_NATIVE_ASSET: &[u8] = b"RSC-ASSETS-NATIVE/V1\0";

/// Identidad asignada al documento de mainnet. No es una regla de `rsc-core`.
pub const MAINNET_CHAIN_ID: ChainId = ChainId(1);
/// Identidad asignada al documento de testnet. No es una regla de `rsc-core`.
pub const TESTNET_CHAIN_ID: ChainId = ChainId(2);
/// Identidad asignada al documento de devnet. No es una regla de `rsc-core`.
pub const DEVNET_CHAIN_ID: ChainId = ChainId(3);

/// Límites y escala que pertenecen a una red, no al tipo `Amount`.
///
/// `amount_scale` es cuántas unidades mínimas componen 1 RSK. `minimum_fee`
/// puede ser cero: v1 registra el valor y no cobra dentro de este crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkParameters {
    /// Unidades mínimas por 1 RSK. Tiene que ser mayor que cero.
    pub amount_scale: u64,
    /// Máximo de bytes canónicos de un bloque.
    pub max_block_size: u32,
    /// Máximo de transacciones en un bloque.
    pub max_transactions_per_block: u32,
    /// Máximo de bytes de una transacción.
    pub max_transaction_bytes: u32,
    /// Fee mínimo por transacción, en unidades mínimas. Cero está permitido.
    pub minimum_fee: u128,
    /// Tamaño máximo del comité. Lo fija este documento.
    pub committee_size: usize,
    /// Bloques por epoch. Lo fija este documento.
    pub epoch_length: u64,
    /// Peso mínimo de un validador de este documento.
    pub min_validator_weight: u128,
    /// Peso máximo de un validador de este documento.
    pub max_validator_weight: u128,
    /// Umbral de validadores para salir de bootstrap. Lo fija este documento.
    pub min_validators_for_distributed: usize,
    /// Semilla pública del beacon de consenso. `Hash::ZERO` no es un beacon.
    pub consensus_randomness_seed: Hash,
}

/// `account_id || owner_key_id || native_balance || allocation`.
const ACCOUNT_ENCODED_LEN: usize = 32 + 32 + 16 + 1;

/// `account_id || owner_key_id || owner_public_key || voting_power || eligible`.
const VALIDATOR_ENCODED_LEN: usize = 32 + 32 + 32 + 16 + 1;

/// `asset_id || decimals || symbol[8]`.
const AMM_ASSET_ENCODED_LEN: usize = 32 + 1 + 8;

/// `account_id || asset_id || amount`.
const AMM_BALANCE_ENCODED_LEN: usize = 32 + 32 + 16;

/// Longitud máxima del símbolo AMM (coincide con `rsc_assets::AssetMetadata`).
pub const AMM_SYMBOL_MAX_LEN: usize = 8;

/// Decimales máximos admitidos para un activo de génesis AMM.
pub const AMM_MAX_DECIMALS: u8 = 18;

/// Categoría presupuestaria de una cuenta de génesis (Política Monetaria v1.0).
///
/// Debe coincidir con `rsc_monetary::EmissionCategory`. Los tokens no
/// atribuidos no se emiten: solo lo declarado aquí entra en circulación.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenesisAllocationCategory {
    /// Reserva de minería.
    Mining,
    /// Reserva agregada de validadores / staking.
    Validators,
    /// Reserva comunitaria.
    Community,
    /// Tesorería (desarrollo / infraestructura).
    Treasury,
    /// Sub-presupuesto: recompensas a holders.
    HolderRewards,
    /// Sub-presupuesto: operadores de validadores.
    OperatorRewards,
}

impl GenesisAllocationCategory {
    /// Tag canónico de 1 byte.
    pub const fn tag(self) -> u8 {
        match self {
            Self::Mining => 1,
            Self::Validators => 2,
            Self::Community => 3,
            Self::Treasury => 4,
            Self::HolderRewards => 5,
            Self::OperatorRewards => 6,
        }
    }

    /// Desde el tag canónico.
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Mining),
            2 => Some(Self::Validators),
            3 => Some(Self::Community),
            4 => Some(Self::Treasury),
            5 => Some(Self::HolderRewards),
            6 => Some(Self::OperatorRewards),
            _ => None,
        }
    }

    /// Techo presupuestario de la categoría en unidades mínimas.
    pub const fn budget(self) -> u128 {
        match self {
            Self::Mining => RSK_MINING_BUDGET,
            Self::Validators => RSK_VALIDATOR_BUDGET,
            Self::Community => RSK_COMMUNITY_BUDGET,
            Self::Treasury => RSK_TREASURY_BUDGET,
            Self::HolderRewards => RSK_HOLDER_REWARDS_BUDGET,
            Self::OperatorRewards => RSK_OPERATOR_REWARDS_BUDGET,
        }
    }
}

/// Documento de génesis. Declara el punto cero; no lo aplica.
///
/// Construirlo no valida, no lee la hora y no genera identidades. La
/// aceptación es [`Self::validate`]. El hash de protocolo es
/// [`Self::genesis_hash`], calculado sobre una vista ordenada.
///
/// La serialización serde conserva el orden entregado y no es el encoding
/// canónico. Solo [`Self::canonical_bytes`] entra en el hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    /// Versión del documento. v1 solo acepta [`GENESIS_VERSION_V1`].
    pub version: u32,
    /// Red a la que pertenece este génesis.
    pub chain_id: ChainId,
    /// Segundos fijados por quien construye el documento.
    ///
    /// No proviene de `SystemTime::now()` ni de ningún reloj del proceso.
    pub timestamp: u64,
    /// Escala, límites de bloque y fee mínimo de esta red.
    pub parameters: NetworkParameters,
    /// Cuentas declaradas, en el orden entregado por el llamador.
    ///
    /// Ese orden se conserva aquí. El hash usa otra vista, ordenada por
    /// [`AccountId`].
    pub initial_accounts: Vec<GenesisAccount>,
    /// Validadores declarados, en el orden entregado por el llamador.
    ///
    /// No se deducen de [`Self::initial_accounts`]. El orden se conserva en
    /// [`Self::validator_set`] y entra en [`Self::canonical_bytes`].
    pub validators: Vec<GenesisValidator>,
    /// Activos AMM registrados en génesis (`rsc-assets`). Vacío por defecto.
    ///
    /// No incluye el nativo tipado (ya lo registra el ledger). El orden del
    /// `Vec` se conserva en serde; el hash ordena por [`Hash`] del `asset_id`.
    #[serde(default)]
    pub amm_assets: Vec<GenesisAmmAsset>,
    /// Saldos iniciales del ledger AMM (fondeo LP / testnet). Vacío por defecto.
    ///
    /// No suma a presupuestos de [`GenesisAllocationCategory`]. Cada
    /// `asset_id` debe estar en [`Self::amm_assets`] o ser el nativo tipado.
    #[serde(default)]
    pub amm_balances: Vec<GenesisAmmBalance>,
    /// Gobernadores AMM iniciales. Vacío por defecto.
    #[serde(default)]
    pub amm_governors: Vec<AccountId>,
    /// Si `true` (default), cualquiera con assets puede `CreatePool`.
    ///
    /// `false` = solo cuentas en [`Self::amm_governors`] (mainnet temprano).
    #[serde(default = "default_amm_permissionless")]
    pub amm_permissionless_creation: bool,
}

fn default_amm_permissionless() -> bool {
    true
}

/// Activo registrado en el ledger AMM al aplicar el génesis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisAmmAsset {
    /// Identificador de 32 bytes. Tiene que ser único dentro del documento.
    pub asset_id: Hash,
    /// Decimales de presentación (`0..=`[`AMM_MAX_DECIMALS`]).
    pub decimals: u8,
    /// Etiqueta ASCII corta no vacía (máx. [`AMM_SYMBOL_MAX_LEN`] bytes).
    pub symbol: String,
}

/// Saldo inicial en el ledger AMM (no es `native_balance` de State).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisAmmBalance {
    /// Cuenta que recibe el saldo. Debe existir en [`Genesis::initial_accounts`].
    pub account_id: AccountId,
    /// Activo a abonar (registrado o nativo tipado).
    pub asset_id: Hash,
    /// Cantidad estrictamente positiva.
    pub amount: u128,
}

/// Cuenta presente al inicio. No es una transacción ni una escritura de estado.
///
/// Las identidades y la categoría presupuestaria las aporta el llamador. Este
/// crate no deriva el [`AccountId`] desde la clave ni crea un par de claves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// Identidad de la cuenta. Tiene que ser única dentro del documento.
    pub account_id: AccountId,
    /// Clave propietaria ya derivada. No es una clave privada.
    #[serde(with = "key_id_serde")]
    pub owner_key_id: KeyId,
    /// Balance nativo inicial. En v1 tiene que ser mayor que cero.
    pub native_balance: u128,
    /// Presupuesto monetario del que sale este saldo.
    pub allocation: GenesisAllocationCategory,
}

/// Validador declarado en el génesis. No es una [`GenesisAccount`].
///
/// El peso y la elegibilidad los aporta el documento. No salen del balance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Identidad de protocolo. Tiene que ser `derive_account_id(owner_key_id)`.
    pub account_id: AccountId,
    /// `KeyId` de [`Self::owner_public_key`].
    #[serde(with = "key_id_serde")]
    pub owner_key_id: KeyId,
    /// Clave pública que verifica los votos de este validador.
    #[serde(with = "public_key_serde")]
    pub owner_public_key: PublicKey,
    /// Peso de voto declarado. Tiene que caer en el intervalo del documento.
    pub voting_power: u128,
    /// Elegible para el comité.
    pub eligible: bool,
}

/// Identidad del documento de génesis.
///
/// Es el SHA-256 del encoding canónico. No es un `StateRoot`, no es un
/// ledger root y no es un hash de bloque. El documento no guarda este valor:
/// se deriva.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenesisHash(pub Hash256);

/// Fallo del documento de génesis.
///
/// No es un error de almacenamiento, estado, consenso, ejecución ni red.
/// El hash de `rsc-crypto` no falla, así que esta enumeración no envuelve
/// un error criptográfico.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum GenesisError {
    /// `version` distinta de [`GENESIS_VERSION_V1`].
    #[error("invalid genesis version")]
    InvalidVersion,
    /// Reservado para un timestamp que una versión futura rechace.
    ///
    /// v1 acepta cualquier `u64` explícito. El constructor nunca consulta el reloj.
    #[error("invalid genesis timestamp")]
    InvalidTimestamp,
    /// `initial_accounts` está vacío. v1 no inserta cuentas implícitas.
    #[error("genesis has no initial accounts")]
    EmptyAccounts,
    /// Una identidad no cumple el formato de `rsc-core` / `rsc-crypto`.
    ///
    /// Esos tipos ya rechazan longitudes incorrectas al construirse. v1 no
    /// tiene otra regla estructural que devolver este error.
    #[error("invalid genesis account")]
    InvalidAccount,
    /// `native_balance` es cero. v1 no admite cuentas génesis vacías.
    #[error("genesis account balance must be greater than zero")]
    ZeroBalance,
    /// El mismo [`AccountId`] aparece más de una vez. No se fusionan saldos.
    #[error("duplicate genesis account id")]
    DuplicateAccount,
    /// El número de cuentas no cabe en el `u32` canónico.
    #[error("invalid genesis encoding")]
    InvalidEncoding,
    /// Escala, límite de bloque o `ConsensusConfig` rechazado.
    #[error("invalid genesis parameters")]
    InvalidParameters,
    /// Un validador no cumple el binding o el intervalo de peso.
    #[error("invalid genesis validator")]
    InvalidValidator,
    /// El mismo [`AccountId`] de validador aparece más de una vez.
    #[error("duplicate genesis validator")]
    DuplicateValidator,
    /// La suma de saldos de una categoría supera su presupuesto.
    #[error("genesis allocation exceeds category budget")]
    AllocationBudgetExceeded,
    /// Tag de categoría desconocido en el encoding.
    #[error("invalid genesis allocation category")]
    InvalidAllocation,
    /// `asset_id` repetido en [`Genesis::amm_assets`].
    #[error("duplicate genesis amm asset id")]
    DuplicateAmmAsset,
    /// Metadatos de activo AMM inválidos (decimals, símbolo o id nulo/nativo).
    #[error("invalid genesis amm asset")]
    InvalidAmmAsset,
    /// `amm_balances.asset_id` no está en `amm_assets` ni es el nativo tipado.
    #[error("genesis amm balance references unknown asset")]
    UnknownAmmAsset,
    /// `amm_balances.amount` es cero.
    #[error("genesis amm balance must be greater than zero")]
    ZeroAmmAmount,
    /// Cuenta de `amm_balances` / `amm_governors` ausente en `initial_accounts`.
    #[error("genesis amm account missing from initial_accounts")]
    AmmAccountMissing,
    /// Par `(account_id, asset_id)` repetido en [`Genesis::amm_balances`].
    #[error("duplicate genesis amm balance")]
    DuplicateAmmBalance,
    /// El mismo gobernador AMM aparece más de una vez.
    #[error("duplicate genesis amm governor")]
    DuplicateAmmGovernor,
}

impl NetworkParameters {
    /// Parámetros de red alineados a la Política Monetaria RSK v1.0.
    ///
    /// `amount_scale = 10^18`. El resto de límites de consenso los aporta el
    /// operador; este helper fija la escala monetaria y un fee mínimo por defecto.
    pub fn rsk_policy_v1(
        max_block_size: u32,
        max_transactions_per_block: u32,
        max_transaction_bytes: u32,
        committee_size: usize,
        epoch_length: u64,
        min_validator_weight: u128,
        max_validator_weight: u128,
        min_validators_for_distributed: usize,
        consensus_randomness_seed: Hash,
    ) -> Self {
        Self {
            amount_scale: RSK_AMOUNT_SCALE,
            max_block_size,
            max_transactions_per_block,
            max_transaction_bytes,
            minimum_fee: RSK_DEFAULT_MINIMUM_FEE,
            committee_size,
            epoch_length,
            min_validator_weight,
            max_validator_weight,
            min_validators_for_distributed,
            consensus_randomness_seed,
        }
    }

    /// Rechaza escala o límites en cero. El fee mínimo puede ser cero.
    pub fn check(self) -> Result<(), GenesisError> {
        if self.amount_scale == 0
            || self.max_block_size == 0
            || self.max_transactions_per_block == 0
            || self.max_transaction_bytes == 0
        {
            return Err(GenesisError::InvalidParameters);
        }
        Ok(())
    }

    /// Copia estos campos a [`ConsensusConfig`] y aplica `ConsensusConfig::validate`.
    pub fn consensus_config(self) -> Result<ConsensusConfig, GenesisError> {
        let config = ConsensusConfig {
            committee_size: self.committee_size,
            epoch_length: self.epoch_length,
            min_validator_weight: self.min_validator_weight,
            max_validator_weight: self.max_validator_weight,
            min_validators_for_distributed: self.min_validators_for_distributed,
        };
        config
            .validate()
            .map_err(|_| GenesisError::InvalidParameters)?;
        Ok(config)
    }
}

impl Genesis {
    /// Construye el documento con los valores dados.
    ///
    /// No valida, no ordena las cuentas, no asigna timestamp, no genera
    /// identificadores y no toca estado ni disco.
    pub fn new(
        version: u32,
        chain_id: ChainId,
        timestamp: u64,
        parameters: NetworkParameters,
        initial_accounts: Vec<GenesisAccount>,
        validators: Vec<GenesisValidator>,
    ) -> Self {
        Self {
            version,
            chain_id,
            timestamp,
            parameters,
            initial_accounts,
            validators,
            amm_assets: Vec::new(),
            amm_balances: Vec::new(),
            amm_governors: Vec::new(),
            amm_permissionless_creation: true,
        }
    }

    /// Comprueba la versión, cuentas y bloques AMM opcionales. No modifica `self`.
    ///
    /// Acepta solo la versión 1, exige al menos una cuenta, exige balance
    /// estrictamente positivo y rechaza un [`AccountId`] repetido. No rellena
    /// huecos, no borra duplicados y no suma saldos. Los saldos AMM no
    /// consumen presupuestos de [`GenesisAllocationCategory`].
    ///
    /// Un timestamp cualquiera es válido en v1: el valor ya viene del llamador.
    pub fn validate(&self) -> Result<(), GenesisError> {
        if self.version != GENESIS_VERSION_V1 {
            return Err(GenesisError::InvalidVersion);
        }
        self.parameters.check()?;
        if self.parameters.consensus_randomness_seed == Hash::ZERO {
            return Err(GenesisError::InvalidParameters);
        }
        self.validate_accounts()?;
        self.validate_amm()?;
        self.parameters.consensus_config()?;
        self.validator_set()?;
        Ok(())
    }

    /// Lista explícita convertida en el [`ValidatorSet`] público, en modo `Bootstrap`.
    ///
    /// Conserva el orden del documento. `participation_score` queda en
    /// [`ParticipationScore::default`]: el documento no trae ese campo.
    pub fn validator_set(&self) -> Result<ValidatorSet, GenesisError> {
        let config = self.parameters.consensus_config()?;
        let mut seen = BTreeSet::new();
        let mut validators = Vec::with_capacity(self.validators.len());
        for validator in &self.validators {
            if !seen.insert(validator.account_id) {
                return Err(GenesisError::DuplicateValidator);
            }
            let weight = rsc_consensus::committee::bounded_weight(validator.voting_power, &config)
                .map_err(|_| GenesisError::InvalidValidator)?;
            if weight != validator.voting_power {
                return Err(GenesisError::InvalidValidator);
            }
            let record = ValidatorRecord {
                id: validator.account_id,
                owner_key_id: validator.owner_key_id,
                owner_public_key: validator.owner_public_key,
                voting_power: weight,
                participation_score: ParticipationScore::default(),
                eligible: validator.eligible,
            };
            validate_validator_record(&record).map_err(|_| GenesisError::InvalidValidator)?;
            validators.push(record);
        }
        Ok(ValidatorSet {
            validators,
            mode: NetworkMode::Bootstrap,
        })
    }

    /// Bytes canónicos del documento, con las cuentas ordenadas por [`AccountId`].
    ///
    /// No altera `initial_accounts`. Rechaza lista vacía, balance cero,
    /// duplicados y un conteo que no quepa en `u32`. No reescribe una versión
    /// desconocida: el `u32` declarado forma parte de los bytes, y
    /// [`Self::validate`] decide si esa versión puede aceptarse.
    ///
    /// Forma, en big-endian:
    ///
    /// `RSC-GENESIS\0 || version || chain_id || timestamp || amount_scale || max_block_size || max_transactions_per_block || max_transaction_bytes || minimum_fee || committee_size || epoch_length || min_validator_weight || max_validator_weight || min_validators_for_distributed || consensus_randomness_seed || account_count || accounts || validator_count || validators || amm_asset_count || amm_assets || amm_balance_count || amm_balances || amm_governor_count || amm_governors || amm_permissionless_creation`
    ///
    /// `committee_size` y `min_validators_for_distributed` entran como `u64`.
    /// `consensus_randomness_seed` son 32 bytes crudos.
    /// Cada cuenta es `account_id || owner_key_id || native_balance || allocation`.
    /// Cada validador es `account_id || owner_key_id || owner_public_key || voting_power || eligible`.
    /// Activos AMM ordenados por `asset_id`: `asset_id || decimals || symbol[8]`.
    /// Saldos AMM ordenados por `(account_id, asset_id)`:
    /// `account_id || asset_id || amount`.
    /// Gobernadores AMM ordenados por `account_id`.
    /// No incluye [`GenesisHash`].
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, GenesisError> {
        self.validate_accounts()?;
        self.validate_amm()?;
        self.encode_sorted()
    }

    /// SHA-256 de [`Self::canonical_bytes`] mediante `rsc_crypto::hash`.
    ///
    /// Falla con el mismo error que el encoding si las cuentas no son
    /// canónicas. Dos documentos con las mismas cuentas en distinto orden
    /// producen el mismo hash. Cambiar versión, cadena, timestamp, cuenta,
    /// clave propietaria o balance cambia el hash.
    ///
    /// Tener hash no basta para aceptar el documento: la versión se comprueba
    /// en [`Self::validate`].
    pub fn genesis_hash(&self) -> Result<GenesisHash, GenesisError> {
        Ok(GenesisHash(hash(&self.canonical_bytes()?)))
    }

    fn validate_accounts(&self) -> Result<(), GenesisError> {
        account_count(self.initial_accounts.len())?;
        if self.initial_accounts.is_empty() {
            return Err(GenesisError::EmptyAccounts);
        }
        if self
            .initial_accounts
            .iter()
            .any(|account| account.native_balance == 0)
        {
            return Err(GenesisError::ZeroBalance);
        }
        reject_duplicate_accounts(&self.initial_accounts)?;
        validate_allocation_budgets(&self.initial_accounts)
    }

    fn validate_amm(&self) -> Result<(), GenesisError> {
        account_count(self.amm_assets.len())?;
        account_count(self.amm_balances.len())?;
        account_count(self.amm_governors.len())?;

        let native = typed_native_asset_id();
        let mut registered = BTreeSet::new();
        for asset in &self.amm_assets {
            if asset.asset_id == Hash::ZERO || asset.asset_id == native {
                return Err(GenesisError::InvalidAmmAsset);
            }
            if asset.decimals > AMM_MAX_DECIMALS {
                return Err(GenesisError::InvalidAmmAsset);
            }
            if !valid_amm_symbol(&asset.symbol) {
                return Err(GenesisError::InvalidAmmAsset);
            }
            if !registered.insert(asset.asset_id) {
                return Err(GenesisError::DuplicateAmmAsset);
            }
        }

        let accounts: BTreeSet<AccountId> = self
            .initial_accounts
            .iter()
            .map(|account| account.account_id)
            .collect();

        let mut balance_keys = BTreeSet::new();
        for balance in &self.amm_balances {
            if balance.amount == 0 {
                return Err(GenesisError::ZeroAmmAmount);
            }
            if !accounts.contains(&balance.account_id) {
                return Err(GenesisError::AmmAccountMissing);
            }
            if balance.asset_id != native && !registered.contains(&balance.asset_id) {
                return Err(GenesisError::UnknownAmmAsset);
            }
            if !balance_keys.insert((balance.account_id, balance.asset_id)) {
                return Err(GenesisError::DuplicateAmmBalance);
            }
        }

        let mut governors = BTreeSet::new();
        for governor in &self.amm_governors {
            if !accounts.contains(governor) {
                return Err(GenesisError::AmmAccountMissing);
            }
            if !governors.insert(*governor) {
                return Err(GenesisError::DuplicateAmmGovernor);
            }
        }
        Ok(())
    }

    fn encode_sorted(&self) -> Result<Vec<u8>, GenesisError> {
        let count = account_count(self.initial_accounts.len())?;
        let mut ordered = self.initial_accounts.clone();
        ordered.sort_by_key(|account| account.account_id);

        let mut amm_assets = self.amm_assets.clone();
        amm_assets.sort_by(|a, b| a.asset_id.cmp(&b.asset_id));
        let mut amm_balances = self.amm_balances.clone();
        amm_balances.sort_by(|a, b| {
            (a.account_id, a.asset_id).cmp(&(b.account_id, b.asset_id))
        });
        let mut amm_governors = self.amm_governors.clone();
        amm_governors.sort();

        let mut out = Vec::with_capacity(
            DOMAIN_GENESIS.len()
                + 4
                + 4
                + 8
                + 8
                + 4
                + 4
                + 4
                + 16
                + 8
                + 8
                + 16
                + 16
                + 8
                + 32
                + 4
                + ordered.len() * ACCOUNT_ENCODED_LEN
                + 4
                + self.validators.len() * VALIDATOR_ENCODED_LEN
                + 4
                + amm_assets.len() * AMM_ASSET_ENCODED_LEN
                + 4
                + amm_balances.len() * AMM_BALANCE_ENCODED_LEN
                + 4
                + amm_governors.len() * 32
                + 1,
        );
        out.extend_from_slice(DOMAIN_GENESIS);
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.chain_id.0.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.parameters.amount_scale.to_be_bytes());
        out.extend_from_slice(&self.parameters.max_block_size.to_be_bytes());
        out.extend_from_slice(&self.parameters.max_transactions_per_block.to_be_bytes());
        out.extend_from_slice(&self.parameters.max_transaction_bytes.to_be_bytes());
        out.extend_from_slice(&self.parameters.minimum_fee.to_be_bytes());
        out.extend_from_slice(&u64_from_usize(self.parameters.committee_size)?.to_be_bytes());
        out.extend_from_slice(&self.parameters.epoch_length.to_be_bytes());
        out.extend_from_slice(&self.parameters.min_validator_weight.to_be_bytes());
        out.extend_from_slice(&self.parameters.max_validator_weight.to_be_bytes());
        out.extend_from_slice(
            &u64_from_usize(self.parameters.min_validators_for_distributed)?.to_be_bytes(),
        );
        out.extend_from_slice(&self.parameters.consensus_randomness_seed.0);
        out.extend_from_slice(&count.to_be_bytes());
        for account in &ordered {
            out.extend_from_slice(&account.account_id.0);
            out.extend_from_slice(account.owner_key_id.as_bytes());
            out.extend_from_slice(&account.native_balance.to_be_bytes());
            out.push(account.allocation.tag());
        }
        let validator_count = account_count(self.validators.len())?;
        out.extend_from_slice(&validator_count.to_be_bytes());
        for validator in &self.validators {
            out.extend_from_slice(&validator.account_id.0);
            out.extend_from_slice(validator.owner_key_id.as_bytes());
            out.extend_from_slice(&validator.owner_public_key.to_bytes());
            out.extend_from_slice(&validator.voting_power.to_be_bytes());
            out.push(u8::from(validator.eligible));
        }
        let asset_count = account_count(amm_assets.len())?;
        out.extend_from_slice(&asset_count.to_be_bytes());
        for asset in &amm_assets {
            out.extend_from_slice(&asset.asset_id.0);
            out.push(asset.decimals);
            out.extend_from_slice(&encode_amm_symbol(&asset.symbol));
        }
        let balance_count = account_count(amm_balances.len())?;
        out.extend_from_slice(&balance_count.to_be_bytes());
        for balance in &amm_balances {
            out.extend_from_slice(&balance.account_id.0);
            out.extend_from_slice(&balance.asset_id.0);
            out.extend_from_slice(&balance.amount.to_be_bytes());
        }
        let governor_count = account_count(amm_governors.len())?;
        out.extend_from_slice(&governor_count.to_be_bytes());
        for governor in &amm_governors {
            out.extend_from_slice(&governor.0);
        }
        out.push(u8::from(self.amm_permissionless_creation));
        Ok(out)
    }
}

fn valid_amm_symbol(symbol: &str) -> bool {
    let bytes = symbol.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= AMM_SYMBOL_MAX_LEN
        && bytes.iter().all(|b| b.is_ascii() && *b != 0)
}

fn encode_amm_symbol(symbol: &str) -> [u8; AMM_SYMBOL_MAX_LEN] {
    let mut tag = [0u8; AMM_SYMBOL_MAX_LEN];
    let bytes = symbol.as_bytes();
    let n = bytes.len().min(AMM_SYMBOL_MAX_LEN);
    tag[..n].copy_from_slice(&bytes[..n]);
    tag
}

/// Identificador del activo nativo tipado (ledger AMM), alineado con `rsc-assets`.
pub fn typed_native_asset_id() -> Hash {
    Hash(*hash(DOMAIN_NATIVE_ASSET).as_bytes())
}

impl core::fmt::Debug for GenesisHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GenesisHash({})", encode_hex(self.0.as_bytes()))
    }
}

impl Serialize for GenesisHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_hex(self.0.as_bytes()))
    }
}

impl<'de> Deserialize<'de> for GenesisHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = decode_fixed::<32>(&text).map_err(serde::de::Error::custom)?;
        Hash256::from_bytes(&bytes)
            .map(GenesisHash)
            .map_err(serde::de::Error::custom)
    }
}

fn account_count(len: usize) -> Result<u32, GenesisError> {
    u32::try_from(len).map_err(|_| GenesisError::InvalidEncoding)
}

fn u64_from_usize(value: usize) -> Result<u64, GenesisError> {
    u64::try_from(value).map_err(|_| GenesisError::InvalidEncoding)
}

fn reject_duplicate_accounts(accounts: &[GenesisAccount]) -> Result<(), GenesisError> {
    let mut seen = BTreeSet::new();
    for account in accounts {
        if !seen.insert(account.account_id) {
            return Err(GenesisError::DuplicateAccount);
        }
    }
    Ok(())
}

fn validate_allocation_budgets(accounts: &[GenesisAccount]) -> Result<(), GenesisError> {
    let mut mining = 0u128;
    let mut validators = 0u128;
    let mut community = 0u128;
    let mut treasury = 0u128;
    let mut holders = 0u128;
    let mut operators = 0u128;

    for account in accounts {
        match account.allocation {
            GenesisAllocationCategory::Mining => {
                mining = mining
                    .checked_add(account.native_balance)
                    .ok_or(GenesisError::AllocationBudgetExceeded)?;
            }
            GenesisAllocationCategory::Validators => {
                validators = validators
                    .checked_add(account.native_balance)
                    .ok_or(GenesisError::AllocationBudgetExceeded)?;
            }
            GenesisAllocationCategory::Community => {
                community = community
                    .checked_add(account.native_balance)
                    .ok_or(GenesisError::AllocationBudgetExceeded)?;
            }
            GenesisAllocationCategory::Treasury => {
                treasury = treasury
                    .checked_add(account.native_balance)
                    .ok_or(GenesisError::AllocationBudgetExceeded)?;
            }
            GenesisAllocationCategory::HolderRewards => {
                holders = holders
                    .checked_add(account.native_balance)
                    .ok_or(GenesisError::AllocationBudgetExceeded)?;
            }
            GenesisAllocationCategory::OperatorRewards => {
                operators = operators
                    .checked_add(account.native_balance)
                    .ok_or(GenesisError::AllocationBudgetExceeded)?;
            }
        }
    }

    if mining > RSK_MINING_BUDGET
        || community > RSK_COMMUNITY_BUDGET
        || treasury > RSK_TREASURY_BUDGET
        || holders > RSK_HOLDER_REWARDS_BUDGET
        || operators > RSK_OPERATOR_REWARDS_BUDGET
    {
        return Err(GenesisError::AllocationBudgetExceeded);
    }
    // Holder/Operator consumen el presupuesto agregado de validadores.
    let validator_total = validators
        .checked_add(holders)
        .and_then(|v| v.checked_add(operators))
        .ok_or(GenesisError::AllocationBudgetExceeded)?;
    if validator_total > RSK_VALIDATOR_BUDGET {
        return Err(GenesisError::AllocationBudgetExceeded);
    }
    let issued = mining
        .checked_add(validator_total)
        .and_then(|v| v.checked_add(community))
        .and_then(|v| v.checked_add(treasury))
        .ok_or(GenesisError::AllocationBudgetExceeded)?;
    if issued > RSK_MAX_SUPPLY {
        return Err(GenesisError::AllocationBudgetExceeded);
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::from("0x");
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_fixed<const N: usize>(text: &str) -> Result<[u8; N], &'static str> {
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if text.len() != N * 2 {
        return Err("invalid hex length");
    }
    let raw = text.as_bytes();
    let mut out = [0u8; N];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = (hex_nibble(raw[index * 2])? << 4) | hex_nibble(raw[index * 2 + 1])?;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, &'static str> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex"),
    }
}

mod key_id_serde {
    use super::{decode_fixed, encode_hex};
    use rsc_crypto::KeyId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key_id: &KeyId, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_hex(key_id.as_bytes()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<KeyId, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = decode_fixed::<32>(&text).map_err(serde::de::Error::custom)?;
        KeyId::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

mod public_key_serde {
    use super::{decode_fixed, encode_hex};
    use rsc_crypto::PublicKey;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &PublicKey, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_hex(&key.to_bytes()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PublicKey, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = decode_fixed::<32>(&text).map_err(serde::de::Error::custom)?;
        PublicKey::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_parameters() -> NetworkParameters {
        NetworkParameters {
            amount_scale: 1_000_000,
            max_block_size: 1_048_576,
            max_transactions_per_block: 1_000,
            max_transaction_bytes: 128 * 1024,
            minimum_fee: 0,
            committee_size: 4,
            epoch_length: 16,
            min_validator_weight: 2,
            max_validator_weight: 9,
            min_validators_for_distributed: 3,
            consensus_randomness_seed: Hash([0x5A; 32]),
        }
    }

    fn document(
        version: u32,
        chain_id: ChainId,
        timestamp: u64,
        initial_accounts: Vec<GenesisAccount>,
    ) -> Genesis {
        Genesis::new(
            version,
            chain_id,
            timestamp,
            test_parameters(),
            initial_accounts,
            Vec::new(),
        )
    }

    fn account(id_byte: u8, key_byte: u8, balance: u128) -> GenesisAccount {
        GenesisAccount {
            account_id: AccountId([id_byte; 32]),
            owner_key_id: KeyId::from_bytes(&[key_byte; 32]).unwrap(),
            native_balance: balance,
            allocation: GenesisAllocationCategory::Community,
        }
    }

    fn sample() -> Genesis {
        document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            1_700_000_000,
            vec![account(0x01, 0x11, 100), account(0x02, 0x22, 200)],
        )
    }

    fn header_len() -> usize {
        DOMAIN_GENESIS.len() + 4 + 4 + 8 + 8 + 4 + 4 + 4 + 16 + 8 + 8 + 16 + 16 + 8 + 32 + 4
    }

    #[test]
    fn valid_genesis_passes_validation() {
        let genesis = sample();
        assert_eq!(genesis.validate(), Ok(()));
        assert!(genesis.genesis_hash().is_ok());
    }

    #[test]
    fn rsk_policy_v1_uses_18_decimal_scale() {
        let parameters = NetworkParameters::rsk_policy_v1(
            1_048_576,
            1_000,
            128 * 1024,
            4,
            16,
            2,
            9,
            3,
            Hash([0x5A; 32]),
        );
        assert_eq!(parameters.amount_scale, RSK_AMOUNT_SCALE);
        assert_eq!(parameters.minimum_fee, RSK_DEFAULT_MINIMUM_FEE);
        assert_eq!(parameters.check(), Ok(()));
        assert_eq!(u128::from(RSK_AMOUNT_SCALE), 1_000_000_000_000_000_000);
    }

    #[test]
    fn allocation_budget_is_enforced_and_enters_hash() {
        let mut over = account(0x01, 0x11, RSK_TREASURY_BUDGET + 1);
        over.allocation = GenesisAllocationCategory::Treasury;
        let genesis = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            vec![over],
        );
        assert_eq!(
            genesis.validate(),
            Err(GenesisError::AllocationBudgetExceeded)
        );

        let mut a = account(0x01, 0x11, 10);
        a.allocation = GenesisAllocationCategory::Mining;
        let mut b = a;
        b.allocation = GenesisAllocationCategory::Community;
        let first = document(GENESIS_VERSION_V1, DEVNET_CHAIN_ID, 10, vec![a]);
        let second = document(GENESIS_VERSION_V1, DEVNET_CHAIN_ID, 10, vec![b]);
        assert_ne!(
            first.genesis_hash().unwrap(),
            second.genesis_hash().unwrap()
        );
        let bytes = first.canonical_bytes().unwrap();
        let tag_at = header_len() + 32 + 32 + 16;
        assert_eq!(bytes[tag_at], GenesisAllocationCategory::Mining.tag());
    }

    #[test]
    fn version_zero_is_rejected() {
        let genesis = document(0, DEVNET_CHAIN_ID, 10, vec![account(0x01, 0x11, 1)]);
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidVersion));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let genesis = document(2, DEVNET_CHAIN_ID, 10, vec![account(0x01, 0x11, 1)]);
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidVersion));
        assert_eq!(
            document(u32::MAX, MAINNET_CHAIN_ID, 10, vec![account(0x01, 0x11, 1)]).validate(),
            Err(GenesisError::InvalidVersion)
        );
    }

    #[test]
    fn empty_accounts_are_rejected() {
        let genesis = document(GENESIS_VERSION_V1, DEVNET_CHAIN_ID, 10, Vec::new());
        assert_eq!(genesis.validate(), Err(GenesisError::EmptyAccounts));
        assert_eq!(genesis.genesis_hash(), Err(GenesisError::EmptyAccounts));
        assert_eq!(genesis.canonical_bytes(), Err(GenesisError::EmptyAccounts));
    }

    #[test]
    fn zero_balance_is_rejected() {
        let genesis = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            vec![account(0x01, 0x11, 5), account(0x02, 0x22, 0)],
        );
        assert_eq!(genesis.validate(), Err(GenesisError::ZeroBalance));
        assert_eq!(genesis.genesis_hash(), Err(GenesisError::ZeroBalance));
        assert_eq!(genesis.canonical_bytes(), Err(GenesisError::ZeroBalance));
    }

    #[test]
    fn duplicate_account_is_rejected() {
        let genesis = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            vec![
                account(0x01, 0x11, 100),
                account(0x02, 0x22, 200),
                account(0x01, 0x33, 500),
            ],
        );
        let before = genesis.initial_accounts.clone();
        assert_eq!(genesis.validate(), Err(GenesisError::DuplicateAccount));
        assert_eq!(genesis.genesis_hash(), Err(GenesisError::DuplicateAccount));
        assert_eq!(
            genesis.canonical_bytes(),
            Err(GenesisError::DuplicateAccount)
        );
        assert_eq!(genesis.initial_accounts, before);
        assert_eq!(genesis.initial_accounts.len(), 3);
    }

    #[test]
    fn deterministic_canonical_bytes() {
        let genesis = sample();
        let first = genesis.canonical_bytes().unwrap();
        let second = genesis.canonical_bytes().unwrap();
        assert_eq!(first, second);
        assert_eq!(first, sample().canonical_bytes().unwrap());
        assert!(first.starts_with(DOMAIN_GENESIS));
    }

    #[test]
    fn canonical_integers_are_big_endian() {
        // Saldo dentro del presupuesto Community; patrón big-endian legible.
        const BALANCE: u128 = 0x0000_0000_0000_0000_0102_0304_0506_0708;
        let genesis = document(
            GENESIS_VERSION_V1,
            ChainId(0x0102_0304),
            0x0102_0304_0506_0708,
            vec![account(0x0a, 0x0b, BALANCE)],
        );
        let bytes = genesis.canonical_bytes().unwrap();
        let at = DOMAIN_GENESIS.len();
        assert_eq!(&bytes[at..at + 4], &1u32.to_be_bytes());
        assert_eq!(&bytes[at + 4..at + 8], &0x0102_0304u32.to_be_bytes());
        assert_eq!(
            &bytes[at + 8..at + 16],
            &0x0102_0304_0506_0708u64.to_be_bytes()
        );
        assert_eq!(
            &bytes[at + 16..at + 24],
            &test_parameters().amount_scale.to_be_bytes()
        );
        assert_eq!(
            &bytes[at + 52..at + 60],
            &u64_from_usize(test_parameters().committee_size)
                .unwrap()
                .to_be_bytes()
        );
        let count_at = header_len() - 4;
        assert_eq!(&bytes[count_at..count_at + 4], &1u32.to_be_bytes());
        let balance_at = header_len() + 64;
        assert_eq!(
            &bytes[balance_at..balance_at + 16],
            &BALANCE.to_be_bytes()
        );
    }

    #[test]
    fn deterministic_genesis_hash() {
        let genesis = sample();
        assert_eq!(
            genesis.genesis_hash().unwrap(),
            genesis.genesis_hash().unwrap()
        );
    }

    #[test]
    fn same_genesis_same_hash() {
        assert_eq!(
            sample().genesis_hash().unwrap(),
            sample().genesis_hash().unwrap()
        );
    }

    #[test]
    fn account_input_order_does_not_change_hash() {
        let alice = account(0x01, 0x11, 100);
        let bob = account(0x02, 0x22, 200);
        let first = document(GENESIS_VERSION_V1, DEVNET_CHAIN_ID, 50, vec![bob, alice]);
        let second = document(GENESIS_VERSION_V1, DEVNET_CHAIN_ID, 50, vec![alice, bob]);
        let before = first.initial_accounts.clone();

        assert_eq!(first.validate(), Ok(()));
        assert_eq!(second.validate(), Ok(()));
        assert_eq!(
            first.genesis_hash().unwrap(),
            second.genesis_hash().unwrap()
        );
        assert_eq!(
            first.canonical_bytes().unwrap(),
            second.canonical_bytes().unwrap()
        );
        assert_eq!(first.initial_accounts, before);
        assert_eq!(first.initial_accounts[0].account_id, bob.account_id);
        assert_eq!(first.initial_accounts[1].account_id, alice.account_id);
    }

    #[test]
    fn genesis_does_not_mutate_input_order() {
        let genesis = document(
            GENESIS_VERSION_V1,
            TESTNET_CHAIN_ID,
            9,
            vec![
                account(0x09, 0x91, 3),
                account(0x02, 0x22, 4),
                account(0x05, 0x55, 5),
            ],
        );
        let before = genesis.initial_accounts.clone();
        genesis.validate().unwrap();
        genesis.canonical_bytes().unwrap();
        genesis.genesis_hash().unwrap();
        assert_eq!(genesis.initial_accounts, before);
    }

    #[test]
    fn canonical_account_sorting_is_deterministic() {
        let genesis = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            50,
            vec![account(0x02, 0x22, 200), account(0x01, 0x11, 100)],
        );
        let bytes = genesis.canonical_bytes().unwrap();
        let again = genesis.canonical_bytes().unwrap();
        assert_eq!(bytes, again);

        let start = header_len();
        assert_eq!(&bytes[start..start + 32], &[0x01; 32]);
        assert_eq!(&bytes[start + 32..start + 64], &[0x11; 32]);
        let second = start + ACCOUNT_ENCODED_LEN;
        assert_eq!(&bytes[second..second + 32], &[0x02; 32]);
        assert_eq!(&bytes[second + 32..second + 64], &[0x22; 32]);
    }

    #[test]
    fn changing_chain_id_changes_hash() {
        let base = sample();
        let mut other = base.clone();
        other.chain_id = MAINNET_CHAIN_ID;
        assert_ne!(base.chain_id, other.chain_id);
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());
    }

    #[test]
    fn changing_version_changes_hash() {
        let base = sample();
        let mut other = base.clone();
        other.version = 2;
        assert_eq!(other.validate(), Err(GenesisError::InvalidVersion));
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());

        let bytes_v1 = base.canonical_bytes().unwrap();
        let mut bytes_v2 = other.canonical_bytes().unwrap();
        let at = DOMAIN_GENESIS.len();
        assert_eq!(&bytes_v1[at..at + 4], &1u32.to_be_bytes());
        assert_eq!(&bytes_v2[at..at + 4], &2u32.to_be_bytes());
        bytes_v2[at..at + 4].copy_from_slice(&1u32.to_be_bytes());
        assert_eq!(bytes_v1, bytes_v2);
    }

    #[test]
    fn changing_timestamp_changes_hash() {
        let base = sample();
        let mut other = base.clone();
        other.timestamp = base.timestamp + 1;
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());
    }

    #[test]
    fn changing_account_id_changes_hash() {
        let base = sample();
        let mut other = base.clone();
        other.initial_accounts[0].account_id = AccountId([0x0f; 32]);
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());
    }

    #[test]
    fn changing_owner_key_id_changes_hash() {
        let base = sample();
        let mut other = base.clone();
        other.initial_accounts[0].owner_key_id = KeyId::from_bytes(&[0x7e; 32]).unwrap();
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());
    }

    #[test]
    fn changing_balance_changes_hash() {
        let base = sample();
        let mut other = base.clone();
        other.initial_accounts[0].native_balance += 1;
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());
    }

    #[test]
    fn no_runtime_clock_dependency() {
        let timestamp = 1_650_000_000u64;
        let first = document(
            GENESIS_VERSION_V1,
            TESTNET_CHAIN_ID,
            timestamp,
            vec![account(0x04, 0x44, 8)],
        );
        let second = document(
            GENESIS_VERSION_V1,
            TESTNET_CHAIN_ID,
            timestamp,
            vec![account(0x04, 0x44, 8)],
        );
        assert_eq!(first.timestamp, timestamp);
        assert_eq!(second.timestamp, timestamp);
        assert_eq!(
            first.genesis_hash().unwrap(),
            second.genesis_hash().unwrap()
        );
        assert_eq!(
            first.canonical_bytes().unwrap(),
            second.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn account_count_above_u32_is_invalid_encoding() {
        assert_eq!(account_count(0), Ok(0));
        assert_eq!(
            account_count(usize::try_from(u32::MAX).expect("u32 fits in usize")),
            Ok(u32::MAX)
        );
        if usize::BITS > 32 {
            assert_eq!(
                account_count(usize::try_from(u32::MAX).unwrap() + 1),
                Err(GenesisError::InvalidEncoding)
            );
        }
    }

    #[test]
    fn zero_scale_is_rejected_and_scale_is_in_the_hash() {
        let mut bad = test_parameters();
        bad.amount_scale = 0;
        let genesis = Genesis::new(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            bad,
            vec![account(0x01, 0x11, 1)],
            Vec::new(),
        );
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidParameters));

        let mut changed = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            vec![account(0x01, 0x11, 1)],
        );
        changed.parameters.amount_scale = 2;
        let base = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            vec![account(0x01, 0x11, 1)],
        );
        assert_ne!(
            base.genesis_hash().unwrap(),
            changed.genesis_hash().unwrap()
        );
    }

    #[test]
    fn zero_limits_are_rejected_and_zero_fee_is_allowed() {
        let accounts = vec![account(0x01, 0x11, 1)];
        for parameters in [
            NetworkParameters {
                max_block_size: 0,
                ..test_parameters()
            },
            NetworkParameters {
                max_transactions_per_block: 0,
                ..test_parameters()
            },
            NetworkParameters {
                max_transaction_bytes: 0,
                ..test_parameters()
            },
        ] {
            let genesis = Genesis::new(
                GENESIS_VERSION_V1,
                DEVNET_CHAIN_ID,
                10,
                parameters,
                accounts.clone(),
                Vec::new(),
            );
            assert_eq!(genesis.validate(), Err(GenesisError::InvalidParameters));
        }

        let free = Genesis::new(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            test_parameters(),
            accounts,
            Vec::new(),
        );
        assert_eq!(free.parameters.minimum_fee, 0);
        assert_eq!(free.validate(), Ok(()));
    }

    #[test]
    fn serde_roundtrip_preserves_order_and_hash() {
        let genesis = document(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            42,
            // Saldos bajo el techo Community; el orden del Vec se conserva en serde.
            vec![account(0x09, 0x91, 1_000_000), account(0x01, 0x11, 7)],
        );
        let json = serde_json::to_string(&genesis).unwrap();
        let restored: Genesis = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, genesis);
        assert_eq!(
            restored.initial_accounts[0].account_id,
            AccountId([0x09; 32])
        );
        assert_eq!(
            restored.genesis_hash().unwrap(),
            genesis.genesis_hash().unwrap()
        );
        assert_ne!(json.as_bytes(), genesis.canonical_bytes().unwrap());

        let hash = genesis.genesis_hash().unwrap();
        let hash_json = serde_json::to_string(&hash).unwrap();
        let hash_back: GenesisHash = serde_json::from_str(&hash_json).unwrap();
        assert_eq!(hash_back, hash);
    }

    fn bound_validator(seed: u8, power: u128, eligible: bool) -> GenesisValidator {
        let secret = rsc_crypto::PrivateKey::from_bytes(&[seed; 32]).unwrap();
        let owner_public_key = secret.public_key();
        let owner_key_id = owner_public_key.key_id();
        GenesisValidator {
            account_id: AccountId::from(rsc_state::derive_account_id(&owner_key_id)),
            owner_key_id,
            owner_public_key,
            voting_power: power,
            eligible,
        }
    }

    fn with_validators(validators: Vec<GenesisValidator>) -> Genesis {
        Genesis::new(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            test_parameters(),
            vec![account(0x01, 0x11, 1)],
            validators,
        )
    }

    #[test]
    fn valid_consensus_document_loads() {
        let validator = bound_validator(0x41, 4, true);
        let genesis = with_validators(vec![validator]);
        assert_eq!(genesis.validate(), Ok(()));
        let config = genesis.parameters.consensus_config().unwrap();
        assert_eq!(config.committee_size, test_parameters().committee_size);
        assert_eq!(config.epoch_length, test_parameters().epoch_length);
        assert_eq!(
            config.min_validator_weight,
            test_parameters().min_validator_weight
        );
        assert_eq!(
            config.max_validator_weight,
            test_parameters().max_validator_weight
        );
        assert_eq!(
            config.min_validators_for_distributed,
            test_parameters().min_validators_for_distributed
        );
        let set = genesis.validator_set().unwrap();
        assert_eq!(set.mode, NetworkMode::Bootstrap);
        assert_eq!(set.validators.len(), 1);
        assert_eq!(set.validators[0].id, validator.account_id);
        assert_eq!(set.validators[0].owner_key_id, validator.owner_key_id);
        assert_eq!(
            set.validators[0].owner_public_key,
            validator.owner_public_key
        );
        assert_eq!(set.validators[0].voting_power, 4);
        assert!(set.validators[0].eligible);
        assert_eq!(
            set.validators[0].participation_score,
            ParticipationScore::default()
        );
        let restored: Genesis = serde_json::from_str(&serde_json::to_string(&genesis).unwrap()).unwrap();
        assert_eq!(restored.validator_set().unwrap(), set);
    }

    #[test]
    fn zero_committee_size_is_rejected() {
        let mut parameters = test_parameters();
        parameters.committee_size = 0;
        let genesis = Genesis::new(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            parameters,
            vec![account(0x01, 0x11, 1)],
            Vec::new(),
        );
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidParameters));
    }

    #[test]
    fn zero_epoch_length_is_rejected() {
        let mut parameters = test_parameters();
        parameters.epoch_length = 0;
        let genesis = Genesis::new(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            parameters,
            vec![account(0x01, 0x11, 1)],
            Vec::new(),
        );
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidParameters));
    }

    #[test]
    fn min_weight_above_max_is_rejected() {
        let mut parameters = test_parameters();
        parameters.min_validator_weight = 9;
        parameters.max_validator_weight = 2;
        let genesis = Genesis::new(
            GENESIS_VERSION_V1,
            DEVNET_CHAIN_ID,
            10,
            parameters,
            vec![account(0x01, 0x11, 1)],
            Vec::new(),
        );
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidParameters));
    }

    #[test]
    fn validator_key_id_mismatch_is_rejected() {
        let mut validator = bound_validator(0x41, 4, true);
        validator.owner_key_id = bound_validator(0x42, 4, true).owner_key_id;
        let genesis = with_validators(vec![validator]);
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidValidator));
    }

    #[test]
    fn validator_account_id_mismatch_is_rejected() {
        let mut validator = bound_validator(0x41, 4, true);
        validator.account_id = AccountId([0xab; 32]);
        let genesis = with_validators(vec![validator]);
        assert_eq!(genesis.validate(), Err(GenesisError::InvalidValidator));
    }

    #[test]
    fn validator_weight_outside_the_interval_is_rejected() {
        let low = with_validators(vec![bound_validator(0x41, 1, true)]);
        let high = with_validators(vec![bound_validator(0x41, 10, true)]);
        assert_eq!(low.validate(), Err(GenesisError::InvalidValidator));
        assert_eq!(high.validate(), Err(GenesisError::InvalidValidator));
    }

    #[test]
    fn an_account_is_not_a_validator_and_a_validator_need_not_be_an_account() {
        let holder = account(0x01, 0x11, 1);
        let validator = bound_validator(0x41, 4, true);
        assert_ne!(holder.account_id, validator.account_id);
        let genesis = with_validators(vec![validator]);
        let set = genesis.validator_set().unwrap();
        assert!(set.validators.iter().all(|record| record.id != holder.account_id));
        assert!(genesis
            .initial_accounts
            .iter()
            .all(|account| account.account_id != validator.account_id));
        assert_eq!(set.validators[0].id, validator.account_id);
        let untouched = GenesisAccount {
            account_id: holder.account_id,
            owner_key_id: holder.owner_key_id,
            native_balance: holder.native_balance,
            allocation: holder.allocation,
        };
        assert_eq!(untouched, holder);
    }

    #[test]
    fn validator_order_fixes_the_set() {
        let first = bound_validator(0x41, 4, true);
        let second = bound_validator(0x42, 5, false);
        let forward = with_validators(vec![first, second]);
        let reverse = with_validators(vec![second, first]);
        let forward_set = forward.validator_set().unwrap();
        assert_eq!(forward.validator_set().unwrap(), forward_set);
        assert_eq!(forward_set.validators[0].id, first.account_id);
        assert_eq!(forward_set.validators[1].id, second.account_id);
        assert!(!forward_set.validators[1].eligible);
        assert_ne!(forward_set, reverse.validator_set().unwrap());
        assert_ne!(
            forward.genesis_hash().unwrap(),
            reverse.genesis_hash().unwrap()
        );
    }

    #[test]
    fn duplicate_validator_is_rejected() {
        let validator = bound_validator(0x41, 4, true);
        let genesis = with_validators(vec![validator, validator]);
        assert_eq!(genesis.validate(), Err(GenesisError::DuplicateValidator));
    }

    #[test]
    fn the_same_genesis_carries_the_same_seed() {
        let first = sample();
        let second = sample();
        assert_ne!(first.parameters.consensus_randomness_seed, Hash::ZERO);
        assert_eq!(
            first.parameters.consensus_randomness_seed,
            second.parameters.consensus_randomness_seed
        );
        assert_eq!(first.canonical_bytes().unwrap(), second.canonical_bytes().unwrap());
        let bytes = first.canonical_bytes().unwrap();
        let seed_at = header_len() - 4 - 32;
        assert_eq!(
            &bytes[seed_at..seed_at + 32],
            &first.parameters.consensus_randomness_seed.0
        );
    }

    #[test]
    fn a_zero_seed_is_rejected_and_json_must_carry_the_field() {
        let mut zero = sample();
        zero.parameters.consensus_randomness_seed = Hash::ZERO;
        assert_eq!(zero.validate(), Err(GenesisError::InvalidParameters));

        let mut value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&sample()).unwrap()).unwrap();
        value["parameters"]
            .as_object_mut()
            .unwrap()
            .remove("consensus_randomness_seed");
        assert!(serde_json::from_value::<Genesis>(value).is_err());
    }

    #[test]
    fn serde_keeps_the_seed_and_the_hash_tracks_it() {
        let genesis = sample();
        let restored: Genesis =
            serde_json::from_str(&serde_json::to_string(&genesis).unwrap()).unwrap();
        assert_eq!(
            restored.parameters.consensus_randomness_seed,
            genesis.parameters.consensus_randomness_seed
        );
        let mut changed = genesis.clone();
        changed.parameters.consensus_randomness_seed = Hash([0x11; 32]);
        assert_ne!(
            genesis.genesis_hash().unwrap(),
            changed.genesis_hash().unwrap()
        );
        assert_ne!(
            genesis.canonical_bytes().unwrap(),
            changed.canonical_bytes().unwrap()
        );
    }

    fn amm_asset(id_byte: u8, decimals: u8, symbol: &str) -> GenesisAmmAsset {
        GenesisAmmAsset {
            asset_id: Hash([id_byte; 32]),
            decimals,
            symbol: symbol.to_string(),
        }
    }

    fn sample_with_amm() -> Genesis {
        let mut genesis = sample();
        genesis.amm_assets = vec![
            amm_asset(0xaa, 18, "tRSK"),
            amm_asset(0xbb, 6, "tUSDC"),
        ];
        genesis.amm_balances = vec![
            GenesisAmmBalance {
                account_id: AccountId([0x01; 32]),
                asset_id: Hash([0xaa; 32]),
                amount: 1_000,
            },
            GenesisAmmBalance {
                account_id: AccountId([0x02; 32]),
                asset_id: Hash([0xbb; 32]),
                amount: 2_000,
            },
        ];
        genesis.amm_governors = vec![AccountId([0x01; 32])];
        genesis
    }

    #[test]
    fn amm_genesis_validates_and_serde_defaults_empty() {
        let genesis = sample_with_amm();
        assert_eq!(genesis.validate(), Ok(()));

        let json = serde_json::to_string(&sample()).unwrap();
        let restored: Genesis = serde_json::from_str(&json).unwrap();
        assert!(restored.amm_assets.is_empty());
        assert!(restored.amm_balances.is_empty());
        assert!(restored.amm_governors.is_empty());
        assert!(restored.amm_permissionless_creation);
        assert_eq!(restored.genesis_hash().unwrap(), sample().genesis_hash().unwrap());
    }

    #[test]
    fn changing_amm_balance_changes_hash() {
        let base = sample_with_amm();
        let mut other = base.clone();
        other.amm_balances[0].amount += 1;
        assert_ne!(base.genesis_hash().unwrap(), other.genesis_hash().unwrap());
    }

    #[test]
    fn changing_amm_permissionless_changes_hash() {
        let base = sample_with_amm();
        let mut closed = base.clone();
        closed.amm_permissionless_creation = false;
        assert_ne!(base.genesis_hash().unwrap(), closed.genesis_hash().unwrap());
    }

    #[test]
    fn amm_asset_order_does_not_change_hash() {
        let forward = sample_with_amm();
        let mut reverse = sample_with_amm();
        reverse.amm_assets.reverse();
        reverse.amm_balances.reverse();
        reverse.amm_governors.reverse();
        assert_eq!(
            forward.genesis_hash().unwrap(),
            reverse.genesis_hash().unwrap()
        );
        assert_eq!(
            forward.canonical_bytes().unwrap(),
            reverse.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn rejects_unknown_amm_asset_zero_amount_and_bad_decimals() {
        let mut unknown = sample_with_amm();
        unknown.amm_balances[0].asset_id = Hash([0xcc; 32]);
        assert_eq!(unknown.validate(), Err(GenesisError::UnknownAmmAsset));

        let mut zero = sample_with_amm();
        zero.amm_balances[0].amount = 0;
        assert_eq!(zero.validate(), Err(GenesisError::ZeroAmmAmount));

        let mut decimals = sample_with_amm();
        decimals.amm_assets[0].decimals = 19;
        assert_eq!(decimals.validate(), Err(GenesisError::InvalidAmmAsset));

        let mut empty_symbol = sample_with_amm();
        empty_symbol.amm_assets[0].symbol.clear();
        assert_eq!(empty_symbol.validate(), Err(GenesisError::InvalidAmmAsset));
    }

    #[test]
    fn rejects_native_asset_in_amm_assets_but_allows_native_balance() {
        let mut listed = sample();
        listed.amm_assets = vec![GenesisAmmAsset {
            asset_id: typed_native_asset_id(),
            decimals: 18,
            symbol: "RSK".into(),
        }];
        assert_eq!(listed.validate(), Err(GenesisError::InvalidAmmAsset));

        let mut funded = sample();
        funded.amm_balances = vec![GenesisAmmBalance {
            account_id: AccountId([0x01; 32]),
            asset_id: typed_native_asset_id(),
            amount: 50,
        }];
        assert_eq!(funded.validate(), Ok(()));
    }

    #[test]
    fn rejects_amm_account_missing_and_duplicate_asset() {
        let mut missing = sample_with_amm();
        missing.amm_balances[0].account_id = AccountId([0xff; 32]);
        assert_eq!(missing.validate(), Err(GenesisError::AmmAccountMissing));

        let mut dup = sample_with_amm();
        dup.amm_assets.push(amm_asset(0xaa, 6, "DUP"));
        assert_eq!(dup.validate(), Err(GenesisError::DuplicateAmmAsset));
    }

    #[test]
    fn amm_balances_do_not_consume_allocation_budgets() {
        let mut genesis = sample();
        genesis.amm_assets = vec![amm_asset(0xaa, 18, "BIG")];
        genesis.amm_balances = vec![GenesisAmmBalance {
            account_id: AccountId([0x01; 32]),
            asset_id: Hash([0xaa; 32]),
            amount: RSK_MAX_SUPPLY.saturating_mul(2),
        }];
        assert_eq!(genesis.validate(), Ok(()));
    }
}
