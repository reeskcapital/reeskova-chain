# RPC discovery (sin API fija)

RSC Chain no exige un único `https://rpc…` para existir. El consenso y el P2P
son autónomos. El HTTP RPC es una **puerta opcional por nodo**.

## Cómo funciona

1. Cada `rsc-node` puede anunciar su propia URL con `--rpc-public-url`.
2. Puede cargar semillas con `--rpc-seed` (repetible), `RSC_RPC_SEEDS` (CSV) o
   el archivo `<data-dir>/rpc-seeds.json`.
3. Cualquier cliente que llegue a **un** nodo vivo obtiene el directorio con:
   - JSON-RPC `rsc_getRpcEndpoints`
   - HTTP `GET /network` → `{ "endpoints": [ ... ] }`
4. Las apps prueban varios endpoints (failover) y refrescan el directorio.

## Arranque de ejemplo

```powershell
cargo run -p rsc-node -- `
  --data-dir .\data\devnet `
  --network-id devnet `
  --chain-id 3 `
  --genesis .\path\to\genesis.json `
  --rpc-bind 127.0.0.1:8545 `
  --rpc-public-url http://127.0.0.1:8545/rpc `
  --rpc-seed http://127.0.0.1:18547/rpc `
  --rpc-seed http://127.0.0.1:18548/rpc
```

Copiá `apps/rpc-seeds.example.json` a `data/devnet/rpc-seeds.json` si preferís archivo.

## Apps

- No hardcodear un solo host de producción.
- Mantener una lista de semillas locales / community.
- Tras el primer `rsc_getStatus` OK, llamar `rsc_getRpcEndpoints` y mezclar URLs.
- Si un endpoint cae, rotar al siguiente.

El dominio `rpc.reeskova-chain.xyz` puede seguir existiendo como **una** semilla
opcional de bootstrap. No es requisito del protocolo.
