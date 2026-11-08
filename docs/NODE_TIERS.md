# RSC Chain — Nodos comunitario vs móvil

## Regla

| Tipo | Quién | Peso | Asiento validador |
| ---- | ----- | ---- | ----------------- |
| Comunitario | 1 usuario (PC/servidor) | **4** | Sí, al ligarse |
| Móvil (cuarto) | 1 usuario móvil | **1** | No |
| Móvil sellado **4/4** | 4 usuarios móviles | **4** | Sí, al completar |

**1 nodo comunitario = 1 nodo móvil 4/4.**

El comunitario es más fuerte *por persona*: un solo usuario aporta el asiento completo. El móvil divide ese mismo asiento en cuatro cuartos; solo al llegar a 4/4 equivale al comunitario.

## Reparto de recompensas (por asiento)

1. Contar asientos = `community_nodes + sealed_mobile_nodes`.
2. Partir el premio a partes iguales por asiento.
3. En un móvil 4/4, ese asiento se vuelve a partir en **4** (uno por minero).

Los cuartos 1/4–3/4 no cobran asiento hasta sellarse.

## Código

- `crates/rsc-node-tiers` — pesos y reparto
- `network/community-node` — 1 user → 1 asiento
- `network/mobile-node` — 4 users → 1 asiento
