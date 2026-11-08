//! Constantes matemáticas inmutables y valores por defecto de protocolo.
//!
//! ## Inmutables vs gobernanza
//!
//! - **Inmutables** ([`BPS_DENOMINATOR`], [`PRICE_SCALE`]): denominadores de
//!   la aritmética. Forman parte de la definición matemática y no se cambian
//!   sin un hard fork del AMM.
//! - **Configurables** ([`DEFAULT_SWAP_FEE_BPS`], [`MAX_SWAP_FEE_BPS`],
//!   [`DEFAULT_MIN_LIQUIDITY`], [`DEFAULT_MAX_SLIPPAGE_BPS`]): propuestas o
//!   límites de política. Las funciones de [`crate::math`] reciben el fee
//!   (y políticas aplicables) como argumento; **no** leen estos defaults
//!   dentro de los cálculos financieros.

/// Denominador de basis points. Inmutable.
///
/// `1 bps = 1 / 10_000` de la cantidad de referencia.
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Fee de swap propuesto por defecto: 30 bps = 0.30 %.
///
/// Es un valor de configuración sugerido, no una constante de fórmula.
pub const DEFAULT_SWAP_FEE_BPS: u16 = 30;

/// Tope configurable de fee de swap, en basis points.
///
/// Debe ser estrictamente menor que [`BPS_DENOMINATOR`] para que el factor
/// `(BPS - fee)` sea positivo. Valor de política, no de fórmula.
pub const MAX_SWAP_FEE_BPS: u16 = 1_000; // 10 %

/// Liquidez mínima bloqueada al crear un pool (estilo Uniswap V2).
///
/// Política opcional: si está habilitada, las shares iniciales se reducen
/// en esta cantidad y el remanente queda permanentemente bloqueado (sink /
/// burn). Ver [`crate::math::shares`].
pub const DEFAULT_MIN_LIQUIDITY: u128 = 1_000;

/// Deslizamiento máximo propuesto por defecto, en basis points.
///
/// Lo aplica la capa de ejecución / UX; la matemática pura no lo impone.
pub const DEFAULT_MAX_SLIPPAGE_BPS: u16 = 100; // 1 %

/// Escala entera para precios spot (`reserve_out * PRICE_SCALE / reserve_in`).
///
/// Inmutable. Permite representar ratios sin floats.
pub const PRICE_SCALE: u128 = 1_000_000_000_000_000_000; // 1e18

/// Versión del protocolo AMM incluida en la identidad del pool.
///
/// Inmutable por hard fork. Distintas versiones producen [`crate::PoolId`] distintos.
pub const AMM_VERSION: u32 = 1;

/// Prefijo de dominio para el hash de [`crate::PoolId`].
///
/// No es un `CryptoDomain` de `rsc-crypto`; el patrón es el mismo que génesis:
/// ASCII estable || cuerpo canónico, luego SHA-256.
pub const DOMAIN_POOL_ID: &[u8] = b"RSC-AMM-POOL-ID/V1\0";

/// Cuenta sink de LP bloqueado (liquidez mínima).
///
/// Las shares en este sink no son retirables ni transferibles. Anclan el pool
/// y no pertenecen a ningún proveedor.
pub const LP_SINK_ACCOUNT: rsc_core::AccountId = rsc_core::AccountId([0u8; 32]);
