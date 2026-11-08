# RSC Chain — Política Monetaria v1.0

Especificación implementada en el crate `rsc-monetary`.

**Estado:** reglas verificables en código. Aún no cableadas como emisión automática del ledger de red; antes de activar hay que auditar supply ya emitido, minería simulada, consenso, AMM y reservas.

## Parámetros

| Parámetro | Valor |
| --------- | ----- |
| Moneda | RSK |
| Supply máximo | 35.999.999 RSK |
| Escala | `10^18` unidades mínimas |
| Quema de fees de protocolo | 30% |
| Rewards de fees de protocolo | 70% a validadores |
| Fees AMM (LPs) | Sistema separado (no pasan por BurnEngine) |

## Distribución

| Categoría | Asignación |
| --------- | ---------- |
| Minería | 21.599.999 |
| Validadores | 7.200.000 (holders 5.400.000 + operadores 1.800.000) |
| Comunidad | 5.400.000 |
| Tesorería | 1.800.000 |

## Módulos

1. `SupplyTracker`
2. `MonetaryPolicy`
3. `BurnEngine`
4. `StakingEngine`
5. `ValidatorRewards`
6. `TreasuryEngine`
7. `MonetarySystem` / transparencia

## Reglas inviolables

1. Ninguna emisión por encima de 35.999.999 RSK.
2. Emisión solo dentro de presupuestos de categoría.
3. Quemado irreversible.
4. Staking no crea supply ilimitado.
5. Sin APR garantizado.
6. Tesorería no altera saldos de usuarios ajenos.
7. Cambios monetarios vía gobernanza pública (fuera de este crate).

## Cableado de red (activado)

- `NetworkParameters::rsk_policy_v1` fija `amount_scale = 10^18` y un `minimum_fee` por defecto.
- Cada `GenesisAccount` declara `allocation` (`mining` / `validators` / `community` / `treasury` / `holder_rewards` / `operator_rewards`).
- El génesis rechaza sumas por categoría por encima del presupuesto oficial.
- Al aplicar génesis, el nodo crea el fee pool, atribuye supply por categoría y arranca `MonetaryRuntime`.
- Cada `Transfer` con `minimum_fee > 0` usa `StateOperation::TransferWithFee` (montos ya partidos 30/70) y asienta la quema en `MonetaryRuntime`.
- Las fees del AMM para LPs siguen fuera de este motor.
