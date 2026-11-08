# RSC Chain

Registro único. Cada tarea cerrada se agrega aquí. No se borra un comando ya ejecutado ni se marca PASS un comando que no terminó.

Leyenda: ✅ PASS, ❌ FAIL, ⏸ ABORTED, ⚠ BLOCKED. Un build cancelado no es un fallo de código.

## Estado consolidado

| Fase | Resultado |
| --- | --- |
| Fase 1 — `rsc-storage` | ✅ CLOSED |
| Fase 2 — `ChainStore` ↔ storage | ✅ CLOSED |
| Fase 3 — `rsc-core` | ✅ CLOSED |
| Fase 4 — `rsc-genesis` | ✅ CLOSED |
| Fase 5 — primitiva criptográfica de consensus | ✅ CLOSED |
| Fase 6 — integración de 12 crates | ✅ CLOSED |
| Fase 7 — `network/node` | ⚠ OPEN. TASK 7.1 ✅ CLOSED. TASK 7.2 ✅ CLOSED. TASK 7.3 ✅ CLOSED. TASK 7.4 ✅ CLOSED. TASK 7.5 ✅ CLOSED. TASK 7.6 ✅ CLOSED. TASK 7.7 ✅ CLOSED. TASK 7.8 ✅ CLOSED. TASK 7.9 ✅ CLOSED. TASK 7.10 ✅ CLOSED. TASK 7.11 ✅ CLOSED. TASK 7.12 ✅ CLOSED. TASK 7.13 ✅ CLOSED. TASK 7.14 ✅ CLOSED. TASK 7.15 ✅ CLOSED. TASK 7.16 ✅ CLOSED. TASK 7.17 ✅ CLOSED. TASK 7.18 ✅ CLOSED. TASK 7.19 ✅ CLOSED. TASK 7.20 ✅ CLOSED. |
| Fase 8 — `network/validator` | CONGELADA después de TASK 8.14. TASK 8.1 ✅ CLOSED. TASK 8.2 ✅ CLOSED. TASK 8.3 ✅ CLOSED. TASK 8.4 ✅ CLOSED. TASK 8.4A ⚠ BLOCKED. TASK 8.4B parcialmente decidida. TASK 8.4C ✅ CLOSED. TASK 8.4D ✅ CLOSED. TASK 8.4E ✅ CLOSED. TASK 8.4F ✅ CLOSED. TASK 8.5 ✅ CLOSED. TASK 8.6 ✅ CLOSED. TASK 8.7 ✅ CLOSED. TASK 8.8 ✅ CLOSED. TASK 8.9 ✅ CLOSED. TASK 8.10 ✅ CLOSED. TASK 8.11 ✅ CLOSED. TASK 8.12 ✅ CLOSED. TASK 8.13 ✅ CLOSED. TASK 8.14 ✅ CLOSED. No hay TASK 8.15. |
| Fase 9 — red distribuida | ⚠ OPEN. TASK 9.1 ✅ CLOSED. TASK 9.2 ✅ CLOSED. TASK 9.3 ✅ CLOSED. TASK 9.4 ✅ CLOSED. TASK 9.5 ✅ CLOSED. TASK 9.6 ✅ CLOSED. TASK 9.7 ✅ CLOSED. TASK 9.8 ✅ CLOSED. TASK 9.9 ✅ CLOSED. TASK 9.10 ✅ CLOSED. TASK 9.11 ✅ CLOSED. TASK 9.12 ✅ CLOSED. TASK 9.13 ✅ CLOSED. TASK 9.14 ✅ CLOSED. TASK 9.15 ✅ CLOSED. TASK 9.16 ✅ CLOSED. TASK 9.17 ⚠ BLOCKED. |
| Dominio `reeskova-chain.xyz` | `--persist-peer` ✅ PASS. PUBLIC RPC ⚠ BLOCKED. `GET /network` ⚠ BLOCKED (TASK 9.17). IPv6 ICMP al PC ✅. TCP IPv6 externo a `:48745` ✅. HTTP externo sin `GET /network`. AAAA sin configurar. DNS sigue en Porkbun. |

## Decisiones congeladas

- `rocksdb` = **0.25.0**, feature `zstd`. `librocksdb-sys` **0.19.0+11.8.1**. No cambiar el pin. No `cargo clean`.
- `CARGO_TARGET_DIR` = `/tmp/cursor-sandbox-cache/81a73ff8f235ebbb739173385328a283/cargo-target`. El caché `706041e90c6c23769741d08f3554f32d` ya no existe. No usar `$PWD/target`.
- Reutilizar el `.a` de 457632132 bytes: `ROCKSDB_LIB_DIR=$CARGO_TARGET_DIR/debug/build/librocksdb-sys-38b69e303fd1b6b2/out` y `ROCKSDB_STATIC=1`. La unidad `1cb665332aeaabc0` ya no está.
- Compilador nativo: `CC=clang-18`, `CXX=clang++-18`, `LIBCLANG_PATH=/usr/lib/llvm-18/lib`, `LLVM_CONFIG_PATH=/usr/bin/llvm-config-18`.
- Si `clang++` compila `*.cc` de RocksDB en un out-dir distinto de `38b69e303fd1b6b2`, esa ejecución se detiene.
- `QuorumCertificate` no agrega firmas. Transporta cada `(PublicKey, Signature)`. `verify_qc` reconstruye `phase || epoch || height || view || hash` y verifica bajo `ConsensusVote`.
- Quorum estricto: `3 * poder > 2 * total`.
- `rsc-core` no nombra redes. `MAINNET=1`, `TESTNET=2`, `DEVNET=3` solo en `rsc-genesis`.
- `minimum_fee` entra en el hash de génesis. `rsc-ledger` v1 no lo cobra.
- El `AccountId` de `rsc-state` se deriva del `KeyId`. No es el seed Ed25519.
- No nuevas column families. No subir `STORAGE_SCHEMA_VERSION`.
- `network/node` ensambla crates existentes. No define protocolo.
- `rsc-validator` queda congelado después de TASK 8.14. No hay TASK 8.15 hasta una decisión arquitectónica nueva.
- Fase 9 conecta procesos con `rsc-p2p`. No reabre `rsc-validator` ni `rsc-node`.

## TASK 1 — `rsc-storage`

### Objetivo

Persistencia real con RocksDB 0.25.0 y zstd.

### Archivos modificados

`crates/rsc-storage`. Pin de `rocksdb` en el workspace.

### Implementación

`RscDatabase` abre la base, comprueba `network/id` y `chain/id`, y escribe en las column families ya existentes.

### Código

`DatabaseConfig`, `RscDatabase::open`, `get` / `put` / `delete` / `write` / `scan` con `ColumnFamily`.

### Dependencias

`rocksdb` 0.25.0, `librocksdb-sys` 0.19.0+11.8.1, zstd.

### No hace

No define bloques, estado ni consenso.

### Tests ejecutados

`cargo test -p rsc-storage`

### Resultado

✅ PASS. **24/24**.

### Clippy

Reportado limpio al cerrar la fase. El comando exacto no quedó guardado: el formato de este registro aún no existía.

### Errores encontrados

`libclang` 10 no servía para bindgen 0.72. Se usó clang 18. `g++` 9 rechaza `-std=c++20`.

### Abortos

Ninguno de esta fase quedó como resultado final. Compilaciones nativas anteriores se rehicieron hasta obtener el `.a` de `1cb665332aeaabc0`.

### Decisiones congeladas

RocksDB 0.25.0 + zstd. No bajar a 0.22.

### Estado

✅ CLOSED

## TASK 2 — `ChainStore` ↔ storage

### Objetivo

`RocksDbChainStore` persiste y reabre la cadena sobre `RscDatabase`.

### Archivos modificados

`crates/rsc-chain/src/rocks_store.rs`.

### Implementación

El store guarda `Arc<RscDatabase>`. `new` envuelve la base. `from_shared` y `database` permiten que `rsc-state` use la misma base.

### Código

`RocksDbChainStore::new`, `from_shared`, `database`, `put_block`, `get_block`, `get_block_by_height`, `get_canonical_tip`, `set_canonical_tip`.

### Dependencias

`rsc-storage`, `rsc-core`.

### No hace

No añade column families. La altura se recorre desde el tip. La clave de bloque es el hash de 32 bytes.

### Tests ejecutados

Tests de `RocksDbChainStore` dentro de `rsc-chain`.

### Resultado

✅ PASS. **9/9**. Persistencia y reapertura, altura, corrupción, fallo atómico, clave por hash, identidad de cadena.

### Clippy

No hay un comando de clippy exclusivo de esta tarea registrado.

### Errores encontrados

Ninguno en el resultado final.

### Abortos

Ninguno registrado como resultado de esta tarea.

### Decisiones congeladas

Una sola `RscDatabase` compartida por `Arc`. El trait `ChainStore` no cambia.

### Estado

✅ CLOSED

## TASK 3 — `rsc-core`

### Objetivo

Congelar tipos y constantes estructurales, sin parámetros de red.

### Archivos modificados

`crates/rsc-core`.

### Implementación

Tamaños fijos y tipos nuevos. Sin nombres de red.

### Código

`HASH_SIZE=32`, `ACCOUNT_ID_SIZE=32`, `PUBLIC_KEY_SIZE=32`, `SIGNATURE_SIZE=64`, `GENESIS_HEIGHT=0`. `Amount=u128`, `Nonce=u64`, `ChainId(u32)`.

### Dependencias

Ningún crate de red, storage ni consenso.

### No hace

No define `MAINNET`, `TESTNET` ni `DEVNET`. No define fees ni tamaños de bloque.

### Tests ejecutados

`cargo test -p rsc-core`

### Resultado

✅ PASS. **3/3**.

### Clippy

No hay un comando de clippy exclusivo de esta tarea registrado.

### Errores encontrados

Ninguno en el resultado final.

### Abortos

Ninguno.

### Decisiones congeladas

Los identificadores de red con nombre viven solo en `rsc-genesis`.

### Estado

✅ CLOSED

## TASK 4 — `rsc-genesis`

### Objetivo

Meter `NetworkParameters` en el documento canónico de génesis.

### Archivos modificados

`crates/rsc-genesis`.

### Implementación

El hash de génesis cubre versión, `chain_id`, timestamp, parámetros y cuentas.

### Código

`NetworkParameters`, `Genesis::canonical_bytes`, `Genesis::genesis_hash`. `MAINNET=1`, `TESTNET=2`, `DEVNET=3`.

### Dependencias

`rsc-core`, `rsc-crypto`.

### No hace

Esos números de fixture no son ley de mainnet. `minimum_fee` no se cobra en el ledger.

### Tests ejecutados

`cargo test -p rsc-genesis`

### Resultado

✅ PASS. **24/24**.

### Clippy

Reportado limpio al cerrar la fase. El comando exacto no quedó guardado.

### Errores encontrados

El test de endianness esperaba el conteo de cuentas en un offset viejo. El offset real del conteo es dominio+52. Corregido. `amount_scale` ocupa dominio+16.

### Abortos

Ninguno.

### Decisiones congeladas

Fixture de prueba: `amount_scale=1_000_000`, `max_block_size=1_048_576`, `max_transactions_per_block=1000`, `max_transaction_bytes=131072`, `minimum_fee=0`. Fee cero es válido. Límites cero se rechazan.

### Estado

✅ CLOSED

## TASK 5 — primitiva criptográfica de consensus

### Objetivo

Cerrar el QC sin agregación criptográfica.

### Archivos modificados

`crates/rsc-crypto/src/aggregate.rs`, `crates/rsc-crypto/src/lib.rs`, `crates/rsc-consensus/src/finality.rs`, `crates/rsc-consensus/src/lib.rs`.

### Implementación

`SignatureBundle` guarda firmas Ed25519 individuales. `aggregate_qc` ordena por id de validador y arma el haz. `verify_qc` verifica poder y cada firma. `verify_commit_chain` exige el mismo epoch en los tres QC.

### Código

`SignatureBundle::from_signatures`, `to_bytes`, `from_bytes`, `verify_signature_bundle`. `QuorumCertificate` gana `epoch` y `signatures`.

### Dependencias

`rsc-crypto`, `rsc-core`. No BLS.

### No hace

No comprime varias firmas en una. Un haz vacío se rechaza.

### Tests ejecutados

- `cargo test -p rsc-crypto` → ✅ PASS **43/43** + clippy de ese crate.
- `cargo test -p rsc-chain -p rsc-consensus --offline` → ✅ PASS. `rsc-chain` **19/19** + 1 doctest. `rsc-consensus` **10/10** + 1 doctest.
- `cargo test -p rsc-p2p` → ✅ PASS **43/43**.

### Resultado

✅ PASS en los tres comandos de arriba.

### Clippy

`cargo clippy -p rsc-chain -p rsc-consensus --all-targets` → ⏸ ABORTED. Ver abortos.

### Errores encontrados

Ninguno en los tests que terminaron.

### Abortos

- `cargo test -p rsc-chain -p rsc-consensus -p rsc-p2p` → ⏸ ABORTED (SIGTERM). Empezó `librocksdb-sys` en el out-dir `a04a7633c35f5670`. No es fallo de código.
- `cargo clippy -p rsc-chain -p rsc-consensus --all-targets` → ⏸ ABORTED. Empezó `librocksdb-sys` en `047772b4d8385bbc`. No es fallo de código.
- `cargo check -p rsc-chain -p rsc-consensus -p rsc-p2p` → ⏸ ABORTED por el mismo motivo.

### Decisiones congeladas

El QC no usa agregación criptográfica.

### Estado

✅ CLOSED

## TASK 6 — integración de los 12 crates

### Objetivo

Un proceso de test usa los 12 crates, RocksDB real, bloque, estado, ledger, QC, RPC, reinicio, negativos y determinismo.

### Archivos modificados

- `Cargo.toml` (miembro `crates/rsc-integration-tests`)
- `crates/rsc-integration-tests/Cargo.toml`
- `crates/rsc-integration-tests/src/lib.rs`
- `crates/rsc-integration-tests/tests/integration_12_crates.rs`
- `crates/rsc-chain/src/rocks_store.rs` (`Arc`, `from_shared`, `database`, y `is_multiple_of` para clippy)
- `crates/rsc-state/src/state.rs` (adaptación a `ColumnFamily` y alias de tipos para clippy)

### Implementación

El test orquesta los crates. No añade reglas. `commit` escribe y cierra. `recover` reabre otra instancia lógica sobre la misma ruta. RPC usa `rsc_rpc::methods::dispatch` sin puerto. Cuenta y balance se consultan con el hex del `AccountId`.

### Código

Tests: `integration_12_crates_end_to_end`, `integration_restart_recovery`, `integration_determinism`, `negative_invalid_signature`, `negative_invalid_qc`, `negative_invalid_nonce`, `negative_invalid_balance`, `negative_invalid_block`, `negative_invalid_merkle_root`, `negative_corrupt_storage`, `negative_invalid_wasm`, `negative_malformed_p2p`.

### Dependencias

`rsc-core`, `rsc-crypto`, `rsc-genesis`, `rsc-state`, `rsc-ledger`, `rsc-mempool`, `rsc-execution`, `rsc-chain`, `rsc-consensus`, `rsc-storage`, `rsc-p2p`, `rsc-rpc`.

### No hace

No abre un nodo. No inventa protocolo. No usa `InMemoryChainStore`.

### Tests ejecutados

Con `ROCKSDB_LIB_DIR` apuntando a `librocksdb-sys-1cb665332aeaabc0/out` y `ROCKSDB_STATIC=1`:

```text
cargo test --locked -p rsc-integration-tests --test integration_12_crates --offline --no-run
cargo test --locked -p rsc-integration-tests --test integration_12_crates --offline -- --nocapture --test-threads=1
```

La segunda corrida del mismo test, y una tercera después del clippy final.

### Resultado

✅ PASS. Primera corrida **12/12** en 2.60 s. Segunda corrida **12/12** en 3.34 s, `Finished` en 0.70 s, sin `Compiling librocksdb`. Tercera corrida **12/12** en 2.17 s.

### Clippy

```text
cargo clippy --locked -p rsc-integration-tests --all-targets --all-features --offline -- -D warnings
```

Primera pasada ❌ FAIL: `manual_is_multiple_of` en `rocks_store.rs`, `type_complexity` en `state.rs`, `field_reassign_with_default` en el test. Corregidos. Pasada final ✅ PASS en 1.85 s.

### Errores encontrados

`TRANSACTION_VERSION_V1` no está reexportado en la raíz de `rsc-ledger`; el test usa `rsc_ledger::transaction::TRANSACTION_VERSION_V1`. `dispatch` está en `rsc_rpc::methods`. RPC rechaza el account `"alice"`: exige hex.

### Abortos

- `cargo check -p rsc-state --offline --lib` → ⏸ ABORTED. Retomó la unidad nativa incompleta `047772b4d8385bbc`.
- `cargo test --locked -p rsc-integration-tests --test integration_12_crates --offline --no-run` sin `ROCKSDB_LIB_DIR` → ⏸ ABORTED al aparecer `Compiling librocksdb-sys` en `9794ae80208bc537`.

### Decisiones congeladas

El harness enlaza el `librocksdb.a` ya construido. No se lanza `cargo test` separado de los 12 crates para esta fase.

### Estado

✅ CLOSED

## Fase 7 — contrato de `network/node`

Binario: `network/node/src/main.rs`, paquete `rsc-node`. Este contrato está escrito. Al redactarlo, el archivo no existía. TASK 7.1 creó el proceso y solo lee la configuración.

### Qué hace

Arranca un proceso. Abre una `RscDatabase`. Si no hay tip, aplica el documento de génesis y escribe el bloque 0. Si hay tip, lo recupera. Sirve RPC sobre ese estado. Escucha P2P. Al apagarse suelta el lock de RocksDB.

### Qué no hace

No define consenso, mempool, transición de estado, reglas de ledger, formato de transacción, fees, criptografía, esquema de storage ni protocolo P2P. No agrega firmas. No crea column families. No sube `STORAGE_SCHEMA_VERSION`. No responde RPC antes de tener tip. No interpreta el gossip: `rsc-p2p` entrega bytes opacos. No recompila `librocksdb.a`.

### Inputs

Configuración del operador, fuera de `rsc-core`:

- `data_dir`
- `network_id` (string que `RscDatabase` persiste)
- `chain_id` de storage (string) y `ChainId` u32 para cadena y P2P
- ruta del documento de génesis, obligatoria solo cuando no hay tip
- `listen`: `Multiaddr`
- `rpc`: `SocketAddr`
- `AccountId` local del validador
- `ConsensusConfig` ya definido por `rsc-consensus`

### Outputs

- proceso en ejecución con tip canónico
- `POST /rpc` y `GET /health` del `RpcServer` existente
- direcciones de escucha P2P
- al salir, la misma ruta de datos reabre el mismo tip
- código de salida distinto de cero si la config, la identidad de la base o el génesis obligatorio fallan

### Dependencias permitidas

Los 12 crates: `rsc-core`, `rsc-crypto`, `rsc-genesis`, `rsc-state`, `rsc-ledger`, `rsc-mempool`, `rsc-execution`, `rsc-chain`, `rsc-consensus`, `rsc-storage`, `rsc-p2p`, `rsc-rpc`.

### Dependencias prohibidas

Otro motor de consenso, otro mempool, otro formato de transacción, otra base distinta de `RscDatabase`, un pin nuevo de RocksDB, un crate de protocolo que no exista ya.

### Orden de inicialización

1. Leer la configuración. Si es inválida, salir.
2. `RscDatabase::open`. `NetworkMismatch` o `ChainMismatch`: salir.
3. `State::open(Arc)` y `RocksDbChainStore::from_shared` sobre esa base.
4. Sin tip: `Genesis::validate`, `State::initialize_genesis`, bloque 0, `set_canonical_tip`. Con tip: no reescribir el génesis.
5. Reconstruir `Ledger` desde los `TransactionId` canónicos. El crate no persiste.
6. `Mempool` vacío. `ExecutionEngine::new`. `ConsensusEngine::new` a la altura del tip.
7. `RpcContext` sobre store, state y consensus ya abiertos. Bind de `RpcServer`.
8. `RscNetwork` escucha al final.

### Orden de shutdown

1. `NetworkCommand::Shutdown`.
2. Parar `RpcServer`.
3. Soltar store, state y el `Arc<RscDatabase>`.
4. El proceso siguiente abre `data_dir` y ve el mismo tip.

### Manejo de errores

Un fallo de config, de identidad de la base o de génesis ausente cuando no hay tip termina el proceso. No se abre RPC ni P2P después de ese fallo. No se traga `NetworkMismatch` ni `ChainMismatch`.

### Recovery

Cerrar el proceso suelta el lock. Abrirlo otra vez usa `RscDatabase::open` sobre la misma ruta. El tip y el estado salen de la base, no de memoria del proceso anterior. `Ledger` se recalcula desde la cadena.

### Configuración

La lista de inputs de arriba. Nada de eso se convierte en constante de `rsc-core`.

### Tests

Al escribir este contrato no se había ejecutado ningún test de la fase. Los de TASK 7.1 están registrados en esa tarea. El gate de la fase sigue sin ejecutarse.

### Invariantes

Una base. RPC solo con tip. P2P no descifra bloques. El QC sigue siendo el de la Fase 5. El esquema de storage no cambia.

### Criterios de cierre de la fase

Un `rsc-node` abre RocksDB real, recupera tip y estado tras reinicio, RPC ve ese estado, P2P queda en listen y el apagado suelta el lock, sin recompilar `librocksdb.a`. La fase no se cierra con el contrato: se cierra tarea por tarea.

### Estado

⚠ OPEN

## TASK 7.1 — leer la configuración

### Objetivo

Crear el paquete `rsc-node` con un `main.rs` que lee tres argumentos y termina si son inválidos. Sin abrir RocksDB, sin RPC y sin P2P.

### Archivos modificados

- `Cargo.toml` (miembro `network/node`)
- `Cargo.lock` (entrada `rsc-node`; `rocksdb` sigue en 0.25.0)
- `network/node/Cargo.toml`
- `network/node/src/main.rs`

### Implementación

`parse_args` exige `--data-dir`, `--network-id` y `--chain-id`. Un flag ausente, vacío, repetido, desconocido o un `chain-id` que no es `u32` devuelve error. `main` sale con código 0 si la config es válida y con código 1 si no lo es.

### Código

`Config`, `parse_args`, `set`, `main`.

### Dependencias

Ninguna. No enlaza los 12 crates.

### No hace

No abre la base. No valida el formato de `network_id` de `rsc-p2p`. No acepta génesis, listen, RPC, validador ni `ConsensusConfig`. Esos argumentos son desconocidos y se rechazan.

### Tests ejecutados

```text
cargo test --offline -p rsc-node -- --test-threads=1
```

### Resultado

✅ PASS. **5/5** en 0.00 s. `Finished` en 3.27 s. Sin `Compiling librocksdb`.

### Clippy

```text
cargo clippy --offline -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 1.49 s.

### Errores encontrados

`unwrap` en los tests exigía `Debug` en `Config`. Se añadió el derive y el test se volvió a ejecutar.

### Abortos

Ninguno.

### Decisiones congeladas

La primera entrada del proceso son esos tres flags. El resto de la configuración entra en la tarea que la use.

### Estado

✅ CLOSED

## TASK 7.2 — abrir `RscDatabase` y validar identidad

### Objetivo

Con `--data-dir`, `--network-id` y `--chain-id`, construir `DatabaseConfig` y llamar `RscDatabase::open`. Si `rsc-storage` rechaza la identidad, el proceso sale con código 1.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `Cargo.lock` ( `rsc-node` depende de `rsc-storage` y `tempfile`; `rocksdb` sigue en 0.25.0 )

### Implementación

`open_database` pasa `network_id` tal cual y `chain_id` como su decimal (`3` → `"3"`). `max_value_size` es 8 MiB porque `DatabaseConfig` lo exige para abrir. No se compara dentro de `rsc-node`. `main` imprime el `Display` de `StorageError` y sale con código 1.

### Código

`MAX_VALUE_SIZE`, `open_database`. `main` llama a `RscDatabase::open`.

### Dependencias

`rsc-storage`. En tests, `tempfile`.

### No hace

No implementa otra comparación de identidad. No abre RPC, P2P, `State`, `Ledger`, `Mempool`, `Consensus`, `Execution` ni génesis. No produce bloques.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Ese comando, con el código ya corregido y con `ROCKSDB_LIB_DIR` en el `.a` de `1cb665332aeaabc0` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **8/8** en 10.36 s. `Finished` en 9.16 s. Esa corrida no imprimió `Compiling librocksdb`.

Identidad: reabrir la misma ruta con el mismo `network_id` y el mismo `chain_id` devuelve `Ok`. Otra red devuelve `StorageError::NetworkMismatch`. Otra cadena devuelve `StorageError::ChainMismatch`. El handle se suelta antes de reabrir.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 1 min 33 s. El log imprimió `Compiling librocksdb-sys`. No apareció `cache.cc` ni otro `.cc` de RocksDB. Siguen los tres `librocksdb.a` que ya existían.

### Errores encontrados

`Result::unwrap_err` exigía `Debug` en `RscDatabase`. Los dos tests de mismatch pasaron a `match`. Después se ejecutó el comando que figura en el resultado.

### Abortos

Ninguno. El primer `cargo test --offline --locked` falló antes de compilar porque `Cargo.lock` aún no tenía la dependencia `rsc-storage`. Se regeneró el lock con `--offline`, sin cambiar el pin de RocksDB, y el test de cierre usó `--locked`.

### Decisiones congeladas

La identidad de la base la decide `RscDatabase::open`. `rsc-node` no la reimplementa. El `chain_id` del proceso entra en storage como texto decimal del `u32`.

### Estado

✅ CLOSED

## TASK 7.3 — compartir la base con State y ChainStore

### Objetivo

Una sola `RscDatabase` abierta por `rsc-node` se entrega a `State::open` y a `RocksDbChainStore::from_shared`. No se abre una segunda base.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `Cargo.lock` (`rsc-node` depende de `rsc-chain` y `rsc-state`; `rocksdb` sigue en 0.25.0)

### Implementación

`open_database` sigue siendo la única llamada a `RscDatabase::open`. El `Arc` resultante entra en `share_database`, que clona ese puntero para el estado y mueve el mismo `Arc` al store.

### Código

`share_database`. `main` sale con código 1 si `State::open` falla.

### Dependencias

`rsc-storage`, `rsc-state`, `rsc-chain`.

### No hace

No aplica génesis. No reconstruye el ledger. No abre RPC, P2P, consensus, execution ni mempool. No produce bloques. No vuelve a comprobar la identidad: eso ya lo hizo `RscDatabase::open`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `1cb665332aeaabc0` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **9/9** en 2.03 s. `Finished` en 3.79 s. Esa corrida no imprimió `Compiling librocksdb`.

`state_and_chain_store_share_one_database`: tras `State::open` el `strong_count` del `Arc` pasa de 1 a 2. Tras `from_shared` pasa a 3. `store.database()` es el mismo puntero (`Arc::ptr_eq`). Al soltar estado y store vuelve a 1.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 1 min 18 s. El log imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. Siguen los tres `librocksdb.a` anteriores.

### Errores encontrados

Ninguno en el código que pasó el test.

### Abortos

Ninguno. El primer `cargo test --offline --locked` no compiló: `Cargo.lock` aún no tenía `rsc-chain` ni `rsc-state`. Se actualizó el lock con `--offline`, sin cambiar el pin. Esa corrida imprimió `Compiling librocksdb-sys`, tardó 1 min 59 s, no compiló `.cc` y también dio **9/9**. El resultado de cierre es la corrida `--locked` de arriba.

### Decisiones congeladas

`rsc-node` abre la base una vez. Estado y cadena reciben clones del mismo `Arc<RscDatabase>`.

### Estado

✅ CLOSED

## TASK 7.4 — génesis y bloque 0

### Objetivo

Abrir la base ya compartida, consultar el tip y, solo si no existe, aplicar el documento de `rsc-genesis` y persistir el bloque 0. Si el tip existe, dejar la cadena como está.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `Cargo.lock` (`rsc-node` depende de `rsc-core`, `rsc-genesis`, `serde_json` y, en tests, `rsc-crypto`; `rocksdb` sigue en 0.25.0)

### Implementación

`ensure_tip` consulta `get_canonical_tip`. Con tip, retorna sin leer el archivo. Sin tip, exige `--genesis`, carga el JSON con el `serde` público de `Genesis`, llama `validate` y exige que `chain_id` coincida con el de la config. Cada cuenta se construye con `StateGenesisAccount::new(owner_key_id, native_balance)` y el `account_id` del documento tiene que ser el derivado. Luego `initialize_genesis`, un `Block` de altura 0 con transacciones vacías, `validate_block` con padre `None` y los límites del documento, `put_block` y `set_canonical_tip`.

### Código

`ensure_tip`. `Config` gana `genesis: Option<String>`. `parse_args` acepta `--genesis`. `main` sale con código 1 si `ensure_tip` falla.

### Dependencias

`rsc-storage`, `rsc-state`, `rsc-chain`, `rsc-core`, `rsc-genesis`, `serde_json`.

### No hace

No abre RPC, P2P, mempool, consensus ni execution. No reconstruye el ledger. No produce bloques después del 0. No sincroniza ni arranca la red. No inventa un constructor de bloque distinto de `Block` y `BlockHeader`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `1cb665332aeaabc0` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **13/13** en 9.65 s. `Finished` en 23.25 s. Esa corrida no imprimió `Compiling librocksdb`.

Sin tip, el documento válido deja un tip cuyo bloque es génesis (`is_genesis`), con `ChainId(3)`, y la cuenta derivada queda con balance 500. Una segunda llamada, con la ruta de génesis apuntando a un archivo que no existe, conserva ese mismo tip. Sin `--genesis` el error es `falta genesis` y el tip sigue ausente. Un `chain_id` distinto y un `account_id` que no es el derivado fallan antes de escribir tip.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 4.45 s. El log no imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. Siguen los tres `librocksdb.a` de 457632132 bytes.

### Errores encontrados

`rsc-node` usa tipos de `rsc-core` y el crate no estaba declarado: `unresolved import rsc_core`. Se añadió la dependencia directa. El primer `Cargo.toml` de la tarea también tenía dos tablas `[dev-dependencies]`; quedaron unidas, con `tempfile` y `rsc-crypto`.

### Abortos

Ninguno. El primer `cargo test --offline --locked` falló antes de compilar porque `Cargo.lock` aún no tenía `rsc-core`, `rsc-genesis` ni `serde_json`. Se actualizó el lock con `--offline`, sin cambiar el pin de RocksDB. Esa corrida dio **12/12** en 1.55 s (`Finished` en 11.46 s) y no compiló `librocksdb`. El resultado de cierre es la corrida `--locked` de 13/13, después del test del `account_id` no derivado.

### Decisiones congeladas

El documento entra como JSON del `Serialize` de `rsc-genesis`. No hay decoder público de los bytes canónicos. El bloque 0 usa `Block` y `validate_block` de `rsc-chain`. El proposer es `AccountId::ZERO`. Si ya hay tip, `ensure_tip` no abre el archivo.

### Estado

✅ CLOSED

## TASK 7.5 — reconstruir el Ledger

### Objetivo

Leer la cadena canónica ya persistida y construir un `Ledger` nuevo con los `TransactionId` de esas transacciones, en ese orden. El ledger no se guarda.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `crates/rsc-ledger/src/transaction.rs`
- `Cargo.lock` (`rsc-node` depende de `rsc-ledger`; `rocksdb` sigue en 0.25.0)

### Implementación

`canonical_blocks` parte de `get_canonical_tip` y sigue `previous_block_hash` con `get_block` hasta el génesis. Después invierte esa lista y exige alturas `0, 1, 2, …` y que cada padre sea el hash del bloque anterior. `rebuild_ledger` recorre esos bloques y, dentro de cada uno, `transactions` en el orden guardado. Cada payload entra por `Transaction::from_canonical_bytes` y el id por `Transaction::id` hacia `Ledger::append`. `Ledger::root` es la raíz de esa secuencia.

### Código

`canonical_blocks`, `rebuild_ledger`. `main` sale con código 1 si la reconstrucción falla. `Transaction::canonical_bytes` y `Transaction::from_canonical_bytes`.

### Dependencias

`rsc-chain`, `rsc-ledger`. El store ya abierto. Storage no se consulta aparte.

### No hace

No modifica bloques ni el tip. No escribe otra cadena. No ejecuta transacciones. No modifica `State`. No abre RPC, P2P, mempool ni consensus. No produce bloques. No persiste el `Ledger` ni abre una column family nueva.

### Recorrido canónico

```text
get_canonical_tip
      ↓
get_block, previous_block_hash, hasta altura 0
      ↓
invertir: altura 0, luego 1, luego el tip
      ↓
transactions de cada bloque, en el orden del Vec
      ↓
Transaction::id
      ↓
Ledger::append
      ↓
Ledger::root
```

Sin tip el error es `falta tip`. Un padre ausente es `bloque ausente`. Un `TransactionId` repetido es `duplicate transaction` y el `append` no deja esa entrada.

### Tests ejecutados

```text
cargo test --offline -p rsc-ledger -- --test-threads=1
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `1cb665332aeaabc0` y `ROCKSDB_STATIC=1` en la corrida de `rsc-node`.

### Resultado

✅ PASS. `rsc-ledger` **36/36** en 0.18 s. `Finished` en 15.32 s.

✅ PASS. `rsc-node` **18/18** en 10.94 s. `Finished` en 27.55 s. Esa corrida no imprimió `Compiling librocksdb`.

El bloque 0, sin transacciones, reconstruye un ledger vacío cuya raíz es `Ledger::new().root()`. Una segunda llamada da la misma raíz. El tip y `state_root` no cambian. Dos transferencias en el bloque 1 y una en el bloque 2 quedan en los índices 0, 1 y 2, y la raíz coincide con un `Ledger` armado con esos tres `Transaction::id`. El orden inverso cambia la raíz. La misma transacción en dos bloques devuelve `duplicate transaction` y deja el tip igual. Un tip cuyo padre no está en el store devuelve `bloque ausente` y no reemplaza ese tip.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 13.63 s. El log no imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. Siguen los tres `librocksdb.a` de 457632132 bytes.

### Errores encontrados

El primer `cargo test --offline --locked -p rsc-node` compiló y falló: `TRANSACTION_VERSION_V1` no está en la raíz de `rsc-ledger`. El test pasó a `rsc_ledger::transaction::TRANSACTION_VERSION_V1`. Después se ejecutó la corrida de 18/18.

### Abortos

Ninguno. `cargo test --offline -p rsc-ledger` resolvió el workspace y dejó `rsc-ledger` en las dependencias de `rsc-node` dentro de `Cargo.lock`. El pin de `rocksdb` no cambió.

### Decisiones congeladas

El recorrido canónico es tip → padre → génesis, y el ledger lee esa lista al revés. El identificador es `Transaction::id`. Los bytes del bloque son el cuerpo que ya entra en ese id, seguido de la firma de 64 bytes. El nodo no redefine ese layout. El `Ledger` vive solo en memoria.

### Estado

✅ CLOSED

## TASK 7.6 — mempool, execution y consensus

Integrar en `rsc-node` los tres componentes que ya existen. El paso ocurre después de reconstruir el `Ledger`. No abre una ronda y no produce un bloque posterior al génesis.

### APIs que se usan

- `Mempool::new(MempoolConfig::default(), validator, nonces)`. Los topes son los `DEFAULT_*` del crate. El pool nace vacío. `rsc-node` no llama `insert`.
- `ExecutionEngine::new(ExecutionConfig::default())`. El entorno es `EXECUTION_ENVIRONMENT_ID` (`RSC-WASM-V1`). No se compila ni se ejecuta un módulo.
- `ConsensusEngine::new(config, validators, randomness, local, height)`. `height` es `header.height` del bloque del tip canónico. Tras `new`, la fase es `NewView`, la vista es 0 y no hay propuesta. No se llama `receive_proposal` ni `create_vote`.

`Mempool::new` exige `MempoolValidator` y `PendingNonceProvider`. En esta tarea ambos rechazan: admisión `InvalidTransaction`, nonce `Unavailable`. No decodifican una transacción y no leen `State`.

`ConsensusEngine::new` exige un `ConsensusConfig` que pase `validate` y un `ValidatorSet` no vacío. Esta tarea usa el mínimo que `validate` acepta: comité 1, epoch 1, pesos 1. El único validador es `AccountId::ZERO`, el mismo proposer estructural del bloque 0. El modo es `NetworkMode::Bootstrap`. La aleatoriedad es `Hash::ZERO`. Esos valores arrancan el motor. No son ley de génesis y no eligen un bloque.

### Debe hacer

Crear el mempool vacío, el execution engine y el consensus engine. La altura del consensus es la del tip ya persistido. Si uno de los constructores falla, el proceso sale con código 1. Tip, estado y ledger quedan como estaban.

### No hace

No admite transacciones. No produce bloques. No ejecuta una ronda. No publica gossip. No abre RPC ni P2P. No modifica `State` ni `Ledger`. No persiste el mempool. No añade flags.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `Cargo.lock` (`rsc-node` depende de `rsc-mempool`, `rsc-execution` y `rsc-consensus`; `rocksdb` sigue en 0.25.0)

### Código

`open_services`. `main` lo llama después de `rebuild_ledger` y sale con código 1 si falla. `RefuseAdmission` y `NoCanonicalNonce`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

`LIBCLANG_PATH`, `LLVM_CONFIG_PATH`, `CC` y `CXX` apuntan a clang 18. En esta corrida `ROCKSDB_LIB_DIR` no estaba definido: el `.a` anterior ya no estaba en el caché.

### Resultado

✅ PASS. **20/20** en 1.81 s. `Finished` en 109 min 03 s.

El tip de génesis deja el mempool vacío (`MempoolStatus::Empty`), el execution engine en `RSC-WASM-V1` y el consensus en altura 0, vista 0, fase `NewView`, sin propuesta. `state_root` y la raíz del ledger no cambian. Con un bloque hijo vacío, la altura del consensus es 1 y el tip sigue igual.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 15 min 42 s. `ROCKSDB_LIB_DIR` apuntaba al `.a` de `librocksdb-sys-38b69e303fd1b6b2`. El log imprimió `Compiling librocksdb-sys`. No apareció `cache.cc` ni otro `.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno en el código que pasó el test.

### Abortos

El primer `cargo test --offline` se detuvo a propósito. El caché de compilación había cambiado de directorio y `ROCKSDB_LIB_DIR` apuntaba a un `librocksdb.a` que ya no existía. Había empezado una unidad nueva de `librocksdb-sys` y `clang-18` compilaba `zstd`. No es un fallo de `rsc-node`. El `librocksdb.a` de `target/` del proyecto es de `librocksdb-sys` 0.11, compilado en Windows, y no se usó.

La corrida de cierre compiló RocksDB 11.8.1 con clang 18 en `librocksdb-sys-38b69e303fd1b6b2`. El pin sigue en 0.25.0. El primer `--locked`, antes de esa corrida, falló al instante porque `Cargo.lock` aún no tenía `rsc-mempool`, `rsc-execution` ni `rsc-consensus`. El lock se actualizó con `--offline`.

### Decisiones congeladas

El mempool arranca vacío y no admite transacciones. El execution engine se construye y no ejecuta un módulo. El consensus queda en la altura del tip, en `NewView`, sin propuesta. Comité 1, epoch 1, peso 1, `AccountId::ZERO`, `NetworkMode::Bootstrap` y `Hash::ZERO` solo permiten construir el motor. No son ley de génesis.

### Estado

✅ CLOSED

## TASK 7.7 — RPC de lectura

### Objetivo

Servir el `RpcServer` existente sobre el tip, el estado y la cadena ya abiertos. Salud, estado y lecturas. Sin P2P y sin producir bloques.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `Cargo.lock` (`rsc-node` depende de `rsc-rpc` y `tokio`; `rocksdb` sigue en 0.25.0)

### Implementación

`rpc_context` arma un `RpcContext` con el store, el `State` y el `ConsensusEngine` ya creados. `serve_rpc` usa `RpcServer::new` y `RpcConfig::default()`, que escucha en `127.0.0.1:8545`. `GET /health` es el del crate. Las lecturas salen de `get_canonical_tip`, `get_block`, `get_block_by_height`, `State::get_account` y el comité del engine. `rsc_sendTransaction` responde rechazo y no llama `insert`.

### Código

`NodeRpc`, `rpc_context`, `serve_rpc`. `main` llama a `serve_rpc` después del ledger y de los tres motores. Si el servidor falla, sale con código 1.

### Dependencias

`rsc-rpc`, `rsc-chain`, `rsc-state`, `rsc-consensus`. `tokio` solo para ejecutar `RpcServer::run`.

### No hace

No abre P2P. No produce bloques. No ejecuta WASM. No admite transacciones. No modifica `State` ni el tip. No inventa un `FinalityProof`. No añade flags. `node_id` es `rsc-node` porque todavía no hay identidad de peer. `public_key` del validador queda vacío: el engine no guarda una clave.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **21/21** en 24.28 s. `Finished` en 2.74 s. Esa corrida no imprimió `Compiling librocksdb`.

`rsc_chainId` devuelve `3`. `rsc_getStatus` trae altura 0, `syncing` falso y el hash del tip. `rsc_getBlockByHeight` del 0 devuelve ese bloque, sin transacciones. `rsc_getAccount` de la cuenta de génesis tiene balance `500` y nonce 0. Una transacción ausente es `not found`. `rsc_getFinality` es `service unavailable`. `rsc_sendTransaction` es `transaction rejected`. `GET /health` responde `{"status":"ok"}`.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS en 6 min 17 s. El log no imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno en el código que pasó el test.

### Abortos

Ninguno. El primer `cargo test --offline --locked` falló antes de compilar porque `Cargo.lock` aún no tenía `rsc-rpc` ni `tokio`. Se actualizó el lock con `--offline`, sin cambiar el pin. Esa corrida imprimió `Compiling librocksdb-sys`, no compiló `.cc`, y dio **21/21** en 6.32 s (`Finished` en 19 min 38 s). El resultado de cierre es la corrida `--locked` de arriba.

### Decisiones congeladas

El bind es el default de `RpcConfig`: `127.0.0.1:8545`. No hay flag nuevo. `finalized_height` queda en 0 y `rsc_getFinality` no está disponible: no existe una prueba de finality. Una transacción encontrada en la cadena canónica se informa como `included`. El envío se rechaza.

### Estado

✅ CLOSED

## TASK 7.8 — P2P en el proceso

### Objetivo

Ensamblar `RscNetwork` al final del arranque: `network-id` del operador, transportes y protocolos del crate, escucha y apagado. Sin producir bloques.

### Archivos modificados

- `network/node/Cargo.toml`
- `network/node/src/main.rs`
- `Cargo.lock` (`rsc-node` depende de `rsc-p2p`; `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`)

### Implementación

`p2p_config` parte de `NetworkConfig::default()` y sustituye `network_id`, `chain_id` (`ChainId` del `u32` de la CLI) y `DiscoveryConfig::for_network` con la lista de bootstrap vacía. `validate` aplica `validate_network_id`. `start_p2p` genera la identidad con `LocalPeerIdentity::generate` y llama `RscNetwork::start`. Espera `ListeningStarted` y `ListeningAddressAdded`. `stop_p2p` envía `NetworkCommand::Shutdown` y drena eventos hasta que el canal cierra.

`serve_rpc` espera a que `127.0.0.1:8545` acepte una conexión y después arranca la red. Ante Ctrl-C o SIGTERM apaga la red y luego espera a que termine `RpcServer::run`. Si el RPC termina antes, apaga la red al salir.

### Código

`p2p_config`, `start_p2p`, `stop_p2p`, `wait_rpc`, `process_shutdown`. `main` sigue llamando a `serve_rpc` después del ledger y de los tres motores.

### Dependencias

`rsc-p2p`. Dentro de `start` el crate ya suscribe los topics existentes, abre TCP/QUIC y el resto de protocolos. El nodo no publica.

### No hace

No produce bloques. No llama `publish`, `dial` ni `request`. No ejecuta una ronda de consenso por red. No sincroniza estado. No añade peers de bootstrap ni un flag `--listen`: sigue siendo un argumento desconocido. La escucha es `/ip4/127.0.0.1/tcp/0`, el default del crate. `node_id` del RPC sigue siendo `rsc-node`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **22/22** en 14.84 s. `Finished` en 1 min 48 s. Esa corrida no imprimió `Compiling librocksdb`.

`p2p_listens_and_shuts_down` arma la config con `devnet` y `ChainId(3)`, observa el `PeerId` local y cierra el canal. `DEVNET` no pasa `validate`. `--listen` sigue rechazado.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 12.17 s, en la misma sesión que el test de cierre. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

El primer clippy, sobre el código anterior al ajuste, falló en 13 min 40 s con `clippy::field_reassign_with_default` y `clippy::question_mark`. Esa corrida imprimió `Compiling librocksdb-sys` y no compiló `.cc`. Se corrigió el inicializador de `NetworkConfig` y el `?` de `wait_rpc`. El clippy de cierre es el de arriba.

### Abortos

Ninguno. El primer `cargo test --offline --locked` falló antes de compilar porque `Cargo.lock` aún no tenía `rsc-p2p`. Se actualizó el lock con `--offline`, sin cambiar el pin. Esa corrida imprimió `Compiling librocksdb-sys`, no compiló `.cc`, y dio **22/22** en 4.19 s (`Finished` en 21 min 13 s). Después del ajuste de apagado, un `--locked` intermedio dio **22/22** en 15.07 s (`Finished` en 52.78 s). El resultado de cierre es el `--locked` de 14.84 s.

### Decisiones congeladas

La red usa el `network-id` y el `chain-id` ya leídos. Identidad efímera por proceso. Bootstrap vacío. La suscripción a topics es la de `RscNetwork::start`; el nodo no publica. El apagado por señal envía `Shutdown` y después espera al RPC.

### Estado

✅ CLOSED

## TASK 7.9 — primer ciclo de propuesta

### Objetivo

Un paso local después del arranque: el tip ya persistido entra en `ConsensusEngine::receive_proposal`. El motor pasa de `NewView` a `Propose`. No se escribe un bloque nuevo.

### Contrato

El engine de la 7.6 nace a la altura del tip, en vista 0 y fase `NewView`. `receive_proposal` exige `header.height == state.height` y `header.proposer == proposer()`. Un bloque de la altura siguiente devuelve `WrongView`. Llegar a `Decide` y llamar `new_height` exige la ronda Prepare / PreCommit / Commit. Esa ronda no entra en esta tarea.

El candidato es el bloque del tip canónico, leído con `get_canonical_tip` y `get_block`. `receive_proposal(&tip, None)` usa el camino del crate que no repite `validate_block`: el bloque ya se validó al persistirse, y un reinicio no relee los límites del documento de génesis. No se inventan `max_block_size` ni `max_transactions_per_block`.

Con el comité congelado de un validador `AccountId::ZERO`, `proposer()` es ese validador. El bloque 0 lo propone `AccountId::ZERO`, así que el paso acepta. Si el proposer del tip no coincide, `receive_proposal` devuelve `invalid proposer` y no cambia la fase.

### Debe hacer

Después de `open_services` y antes de RPC, cargar el tip y llamar `receive_proposal`. El evento tiene que ser `ProposalAccepted`. La fase queda en `Propose`, `proposal` es el hash del tip, la vista sigue en 0, `locked` sigue vacío y la altura no cambia. Si falla, el proceso sale con código 1. Tip, estado y ledger quedan como estaban. El mempool sigue vacío y el execution engine no se usa.

### No hace

No llama `create_vote`, `receive_vote`, `timeout` ni `new_height`. No llama `insert` ni `select_for_block`. No llama `compile` ni `execute`. No llama `put_block` ni `set_canonical_tip`. No llama `publish`. No añade flags. No cambia el protocolo P2P.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`propose_tip`. `main` lo llama después de `open_services` y antes de `serve_rpc`. Si falla, sale con código 1.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **24/24** en 29.51 s. `Finished` en 2 min 00 s. Esa corrida no imprimió `Compiling librocksdb`.

El tip de génesis queda como propuesta: fase `Propose`, `proposal` igual al hash del tip, altura 0, vista 0, `locked` vacío. El mempool sigue vacío, el execution engine sigue en `RSC-WASM-V1`, y el tip, el `state_root` y la raíz del ledger no cambian. Un tip cuyo proposer no es `AccountId::ZERO` devuelve `invalid proposer` y la fase sigue en `NewView`.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 16.42 s, en la misma sesión que el test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

El primer ciclo propone el tip ya persistido. No construye el bloque siguiente. `receive_proposal` recibe `None` como contexto de cadena. No hay votos, ni bloque nuevo, ni gossip.

### Estado

✅ CLOSED

## TASK 7.10 — primer voto local

### Objetivo

Después de `ProposalAccepted`, crear el voto Prepare de ese bloque, firmarlo con el camino que ya usa `rsc-consensus` y comprobar la firma. Un voto con la firma alterada se rechaza. No hay QC ni bloque nuevo.

### Contrato

`create_vote(VotePhase::Prepare)` copia epoch, altura, vista y hash de la propuesta ya aceptada. El autor es el validador local, `AccountId::ZERO`. La fase del voto es `Prepare`. `create_vote` no cambia la fase del motor: sigue en `Propose`, vista 0, `locked` vacío.

La firma la pone `sign_vote`, que llama `rsc_crypto::sign` con dominio `CryptoDomain::ConsensusVote`. El seed es el `AccountId` de 32 bytes, como documenta el crate. El nodo no genera otra clave ni otro mensaje. El mensaje público es `vote_bytes`.

`vote_signature_valid` no es pública. El nodo comprueba el voto bueno con `rsc_crypto::verify` sobre `vote_bytes`, la pública derivada de ese mismo `AccountId` y el dominio `ConsensusVote`.

`receive_vote` del voto bueno, con el comité de un miembro y peso 1, cumple `has_quorum` y devuelve `QuorumReached(Prepare)`. Eso guarda un QC. Esta tarea no llama `receive_vote` con el voto bueno.

El voto alterado cambia un byte de la firma. `receive_vote` devuelve `invalid signature` antes de insertarlo. La fase sigue en `Propose` y `proposal` sigue siendo el hash del tip.

### Debe hacer

Después de `propose_tip` y antes de RPC, llamar `create_vote(VotePhase::Prepare)` y verificar la firma. El hash del voto es el del tip. Epoch, altura y vista son los del engine. Si la firma no verifica, el proceso sale con código 1. Tip, estado y ledger quedan como estaban.

### No hace

No llama `receive_vote` con el voto válido. No llama `create_vote` de `PreCommit` ni de `Commit`. No llama `timeout` ni `new_height`. No arma un `QuorumCertificate` ni un `FinalityProof`. No llama `insert`, `compile`, `execute`, `put_block`, `set_canonical_tip` ni `publish`. No añade flags.

### Archivos modificados

- `network/node/Cargo.toml` (`rsc-crypto` pasa de dev-dependency a dependencia del binario)
- `network/node/src/main.rs`

`Cargo.lock` ya nombraba `rsc-crypto` en `rsc-node`. `rocksdb` sigue en 0.25.0.

### Código

`cast_local_vote`. `main` lo llama después de `propose_tip` y antes de `serve_rpc`. El `Vote` se descarta: no se publica y no entra en `receive_vote`. Si falla, sale con código 1.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **25/25** en 19.90 s. `Finished` en 1 min 26 s. Esa corrida no imprimió `Compiling librocksdb`.

El voto Prepare lleva el hash del tip, el epoch del engine, altura 0, vista 0 y autor `AccountId::ZERO`. La fase del motor sigue en `Propose` y `locked` sigue vacío. `rsc_crypto::verify` acepta la firma sobre `vote_bytes` y `ConsensusVote`. Un byte cambiado en la firma hace que `receive_vote` devuelva `invalid signature` y la fase no se mueva. El tip, el `state_root`, la raíz del ledger y el mempool vacío siguen igual. El execution engine no ejecuta.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 10.60 s. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló: el lock ya tenía `rsc-crypto`.

### Decisiones congeladas

El voto es `Prepare` del tip ya propuesto. La firma es la de `sign_vote`. El voto válido no se ingresa con `receive_vote`, porque ese comité de un miembro produciría `QuorumReached(Prepare)`. El voto alterado sí entra en `receive_vote` y se rechaza.

### Estado

✅ CLOSED

## TASK 7.11 — quorum y QC de Prepare

### Objetivo

Ingresar el voto Prepare ya firmado. El motor llega a `QuorumReached(Prepare)` y el QC de esa fase se comprueba. No hay commit ni altura nueva.

### Contrato

`receive_vote` del voto válido, con el comité de un miembro y peso 1, llama `has_quorum` (`3 * poder > 2 * total`) y devuelve `QuorumReached(Prepare)`. La fase pasa a `Prepare`. `locked` sigue vacío: el lock se toma en PreCommit. Altura, vista y `proposal` no cambian.

El QC queda en un campo privado del engine. El nodo lo reconstruye con `aggregate_qc` sobre ese mismo voto y el comité público, y lo comprueba con `verify_qc`. No define otro formato de certificado.

`verify_qc` tiene que aceptar el certificado. Su hash, fase, altura, vista y epoch son los del voto y del engine.

### Debe hacer

Después de `cast_local_vote` y antes de RPC, llamar `receive_vote` con el voto bueno. El evento tiene que ser `QuorumReached(Prepare)`. Luego `aggregate_qc` y `verify_qc`. Si falla, el proceso sale con código 1. Tip, estado y ledger quedan como estaban.

### No hace

No llama `create_vote` de `PreCommit` ni de `Commit`. No llama `timeout` ni `new_height`. No arma un `FinalityProof`. No llama `verify_commit_chain`. No llama `insert`, `compile`, `execute`, `put_block`, `set_canonical_tip` ni `publish`. No añade flags.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`accept_prepare_quorum`. `main` lo llama con el voto de `cast_local_vote` y descarta el `QuorumCertificate`: no lo publica y no lo persiste. Si falla, sale con código 1.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **26/26** en 27.23 s. `Finished` en 1 min 36 s. Esa corrida no imprimió `Compiling librocksdb`.

`receive_vote` del voto Prepare devuelve `QuorumReached(Prepare)`. La fase queda en `Prepare`, `locked` vacío, altura 0, vista 0 y `proposal` igual al hash del tip. `aggregate_qc` arma el certificado y `verify_qc` lo acepta: mismo hash, fase, altura, vista y epoch. El tip, el `state_root`, la raíz del ledger y el mempool vacío siguen igual. El execution engine no ejecuta.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 16.97 s, en la misma sesión que el test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

El QC es solo el de `Prepare`. El engine lo guarda en privado; el nodo lo reconstruye con `aggregate_qc` y lo comprueba con `verify_qc`. No hay PreCommit, Commit, `FinalityProof` ni `new_height`.

### Estado

✅ CLOSED

## TASK 7.12 — PreCommit local

### Objetivo

Con el QC de Prepare ya aceptado, crear el voto PreCommit del mismo hash, firmarlo, ingresarlo y comprobar su QC. No hay Commit ni altura nueva.

### Contrato

El engine está en fase `Prepare`, vista 0, `locked` vacío y `proposal` igual al tip. `create_vote(VotePhase::PreCommit)` copia epoch, altura, vista y ese hash. `sign_vote` firma con `rsc_crypto::sign` en `ConsensusVote`. El nodo comprueba la firma con `rsc_crypto::verify` y `vote_bytes`, igual que el voto Prepare. `create_vote` no cambia la fase.

`receive_vote` de ese voto, con el comité de un miembro, devuelve `QuorumReached(PreCommit)`. La fase pasa a `PreCommit`. El crate asigna `locked` al hash de la propuesta. Altura, vista y `proposal` no cambian.

El QC se reconstruye con `aggregate_qc` y se comprueba con `verify_qc`. Fase del certificado: `PreCommit`. Hash, altura, vista y epoch son los del voto.

### Debe hacer

Después de `accept_prepare_quorum` y antes de RPC, emitir el voto PreCommit e ingresarlo. Si el evento no es `QuorumReached(PreCommit)` o `verify_qc` falla, el proceso sale con código 1. Tip, estado y ledger quedan como estaban.

### No hace

No llama `create_vote` de `Commit`. No llama `timeout` ni `new_height`. No arma un `FinalityProof`. No llama `verify_commit_chain`. No llama `insert`, `compile`, `execute`, `put_block`, `set_canonical_tip` ni `publish`. No añade flags.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`cast_local_vote` acepta la fase. `accept_precommit_quorum` y `checked_certificate`. `main` emite el PreCommit después del QC de Prepare y descarta el certificado. Si falla, sale con código 1. `create_vote(Commit)` no se llama: pedirlo a `cast_local_vote` devuelve `fase distinta`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **27/27** en 27.66 s. `Finished` en 1 min 38 s. Esa corrida no imprimió `Compiling librocksdb`.

El voto PreCommit lleva el hash del tip, altura 0 y vista 0. `receive_vote` devuelve `QuorumReached(PreCommit)`. La fase queda en `PreCommit` y `locked` es el hash del tip. `verify_qc` acepta el certificado de esa fase. El tip, el `state_root`, la raíz del ledger y el mempool vacío siguen igual. El execution engine no ejecuta. La altura sigue en 0.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 13.54 s, en la misma sesión que el test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

PreCommit usa el mismo hash que Prepare. El lock del engine apunta a ese hash. No hay voto Commit, `FinalityProof` ni `new_height`.

### Estado

✅ CLOSED

## TASK 7.13 — Commit y FinalityProof local

### Objetivo

Con el QC de PreCommit ya aceptado, crear el voto Commit del mismo hash, firmarlo, ingresarlo y obtener el `FinalityProof`. La altura no cambia y no se escribe un bloque.

### Contrato

El engine está en fase `PreCommit`, `locked` es el hash del tip y `proposal` es ese hash. `create_vote(VotePhase::Commit)` copia epoch, altura, vista y ese hash. `sign_vote` firma con `rsc_crypto::sign` en `ConsensusVote`. El nodo comprueba la firma con `rsc_crypto::verify` y `vote_bytes`. `create_vote` no cambia la fase.

`receive_vote` de Commit, con el comité de un miembro, no devuelve `QuorumReached`. El crate arma los tres QC, llama `verify_commit_chain` y devuelve `BlockFinalized(FinalityProof)`. La fase pasa a `Decide`. `locked` sigue en el hash del tip. Altura, vista y `proposal` no cambian. No se llama `new_height`.

El nodo reconstruye el QC de Commit con `aggregate_qc`, lo comprueba con `verify_qc` y vuelve a comprobar la prueba con `verify_commit_chain`. El hash de la prueba es el del tip.

### Debe hacer

Después de `accept_precommit_quorum` y antes de RPC, emitir el voto Commit e ingresarlo. Si el evento no es `BlockFinalized` o la prueba no verifica, el proceso sale con código 1. Tip, estado y ledger quedan como estaban. El `FinalityProof` no se publica ni se persiste.

### No hace

No llama `new_height`. No llama `put_block` ni `set_canonical_tip`. No llama `insert`, `compile`, `execute` ni `publish`. No añade flags. `rsc_getFinality` sigue en `service unavailable`: la prueba es local y no es un commit de cadena.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`cast_local_vote` acepta `Commit` cuando la fase es `PreCommit` y `locked` ya es el tip. `accept_commit`. `main` lo llama después del PreCommit y descarta el `FinalityProof`. Si falla, sale con código 1.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **28/28** en 10.04 s. `Finished` en 45.59 s. Esa corrida no imprimió `Compiling librocksdb`.

`receive_vote` del voto Commit devuelve `BlockFinalized`. La fase queda en `Decide`, la altura sigue en 0 y `locked` sigue en el hash del tip. `verify_qc` acepta el QC de Commit y `verify_commit_chain` acepta la prueba sobre ese hash. El tip, el `state_root`, la raíz del ledger y el mempool vacío siguen igual. El execution engine no ejecuta.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 12.39 s, en la misma sesión que el test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

El voto Commit no produce `QuorumReached`: el crate devuelve `BlockFinalized` y deja la fase en `Decide`. No se llama `new_height`. La prueba no se persiste y `rsc_getFinality` sigue sin estar disponible.

### Estado

✅ CLOSED

## TASK 7.14 — aplicar el bloque finalizado

### Objetivo

Después de `BlockFinalized`, avanzar el motor con `new_height` y comprobar que el bloque nombrado por la prueba ya es el snapshot persistido de State, Ledger y tip. No se escribe un bloque nuevo.

### Contrato

La prueba nombra el tip canónico de altura 0. Ese bloque ya se escribió en la 7.4, el `State` ya tiene su `state_root` y el ledger ya contiene sus transacciones. Un canonical tip distinto sería otro hash, y la prueba no lo cubre.

`apply_transition` rechaza una transición sin operaciones. El bloque finalizado no trae transacciones. No se inventa una operación ni se vuelve a aplicar el génesis.

`new_height(None, Hash::ZERO)` exige fase `Decide`. La altura del motor pasa a `proof.height + 1`, la vista vuelve a 0, la fase queda en `NewView`, y `proposal` y `locked` quedan vacíos. Con `epoch_length` 1 el evento es `EpochChanged`. Con un validador y `min_validators_for_distributed` 1, el crate cambia el modo a `Distributed`. La aleatoriedad sigue siendo `Hash::ZERO`. Nada de eso se persiste.

El orden es: comprobar tip, altura, `state_root` y raíz del ledger; después `new_height`. Si la comprobación falla, el motor sigue en `Decide` y no hay escritura. `new_height` no abre un lote.

### Debe hacer

Después de `accept_commit` y antes de RPC, llamar `apply_finalized`. Si el tip, la raíz o la altura no coinciden con la prueba, o si `new_height` no deja el motor en la altura siguiente, el proceso sale con código 1.

### No hace

No llama `apply_transition`. No llama `Ledger::append`. No llama `put_block` ni `set_canonical_tip`. No admite transacciones. No ejecuta WASM. No publica. No llama `create_vote`. No añade flags. `rsc_getFinality` sigue sin estar disponible.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`apply_finalized`. `main` lo llama con la prueba de `accept_commit`, antes de RPC. Si falla, sale con código 1.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **29/29** en 33.18 s. `Finished` en 2 min 27 s. Esa corrida no imprimió `Compiling librocksdb`.

El motor queda en altura 1, epoch 1, vista 0, fase `NewView`, sin propuesta y sin lock. El modo pasa a `Distributed`. El tip, el `state_root`, la versión del estado y la raíz del ledger siguen iguales. El mempool sigue vacío y el execution engine no ejecuta.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 13.23 s, en la misma sesión que el test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

El bloque finalizado ya es el tip. No se escribe otro. `new_height` solo mueve el motor. `apply_transition` no se llama porque el bloque no trae operaciones. La prueba no se persiste.

### Estado

✅ CLOSED

## TASK 7.15 — admisión de una transacción

### Objetivo

Una transacción canónica firmada entra en el mempool y queda `Ready`. Una transacción mal formada o de una cuenta ausente se rechaza. No se ejecuta ni se escribe en la cadena.

### Contrato

`ChainAdmission` sustituye el rechazo fijo. `validate` usa `Transaction::from_canonical_bytes`. La versión tiene que ser `TRANSACTION_VERSION_V1`. Una transferencia de importe 0 se rechaza. El `sender` tiene que existir en `State`. El hash del pool es `Transaction::id`. Los bytes guardados son los recibidos.

`from_canonical_bytes` no verifica la firma: la `PublicKey` no viaja en esos bytes y `Transaction::validate` la exige. El nodo no añade un sobre nuevo. La firma tiene que decodificar como 64 bytes. Una firma alterada que sigue decodificando no se rechaza en esta tarea.

`current_nonce` lee `Account::nonce` y no escribe. Si el nonce de la transacción es el de la cuenta, `queue_state` es `Ready`. El motor puede estar en altura 1; el pool no consulta la altura.

### Debe hacer

`open_services` construye el pool con `ChainAdmission`. `submit_signed` llama `Mempool::insert`. Si `insert` falla, RPC responde `InvalidTransaction`. Un arranque sin transacción deja el pool vacío.

### No hace

La admisión no llama `receive_proposal`, `create_vote`, `apply_transition`, `Ledger::append`, `compile`, `execute`, `put_block`, `set_canonical_tip` ni `publish`. No añade flags. El ciclo local ya cerrado en 7.9–7.14 sigue en `main` y no abre otra ronda.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`ChainAdmission` abre un `State` sobre `store.database()`. `validate` decodifica con `Transaction::from_canonical_bytes`, exige `TRANSACTION_VERSION_V1`, rechaza una transferencia de importe 0 y exige que el `sender` exista. El hash guardado es `Transaction::id` convertido a `rsc_core::TransactionId`. `current_nonce` devuelve `Account::nonce`. `open_services` entrega `Arc<Mempool>`. `submit_signed` llama `insert`; cualquier error queda como `InvalidTransaction`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **30/30** en 7.67 s. `Finished` en 4.85 s. Esa corrida no imprimió `Compiling librocksdb`.

Una transferencia firmada con la clave del génesis, nonce 0 e importe 7, entra y `queue_state` es `Ready`. `ready_count` queda en 1. El byte `00`, el importe 0 y un sender ausente se rechazan. El motor sigue en altura 1, fase `NewView`. El tip, el `state_root`, la versión del estado y la raíz del ledger siguen iguales. El execution engine no ejecuta. `rsc_sendTransaction` con `"00"` sigue en `TransactionRejected`.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 4.01 s, en la misma sesión que el test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

La primera compilación falló: `database()` ya devuelve `Arc`, y `Arc::clone` recibía ese valor. Quedó `State::open(store.database())`. El `--locked` de cierre compiló y pasó.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

`from_canonical_bytes` no verifica la firma. La `PublicKey` no viaja en esos bytes y `State` guarda `KeyId`. El nodo no añade un sobre. La prueba llama `Transaction::validate` con la clave pública del génesis antes de `insert`. Una firma que sigue decodificando no se rechaza aquí. El arranque deja el pool vacío. La admisión no escribe cadena.

### Estado

✅ CLOSED

## TASK 7.16 — transición de una transacción Ready

### Objetivo

La transacción `Ready` se convierte en una `StateTransition` y `State::apply_transition` la valida y la escribe. No hay bloque nuevo, ni append de ledger, ni otra ronda de consenso.

### Contrato

`prepare_ready` toma la primera transacción de `Mempool::ready_transactions(1)`. Si no hay ninguna, no lee ni escribe estado. Decodifica los bytes con `Transaction::from_canonical_bytes` y exige que el hash sea `Transaction::id`.

Un `Transfer` se traduce al `StateOperation::Transfer` que ya existe: `from`, `to`, `amount` y `expected_nonce` salen de la transacción. `StateTransition::new` queda anclada a `state_root` y `version` actuales. `apply_transition` recibe `version + 1`. Esa función es la validación: importe, cuentas distintas, existencia, nonce y balance. Si falla, el lote no se publica.

`rsc-execution::execute` pide un `CompiledModule` y un `ExecutionRequest`. Un `Transfer` no trae bytes de módulo. El crate declara que el ABI de contratos es posterior. Esta tarea no compila un módulo inventado ni llama `compile` ni `execute`. El motor sigue en `RSC-WASM-V1`.

El tip canónico conserva el `state_root` anterior. El estado vivo queda con la raíz y la versión nuevas. La transacción sigue en el mempool y pasa a `Pending`, porque el nonce de la cuenta ya avanzó. No se llama `remove_committed`.

### Debe hacer

Después de `apply_finalized` y antes de RPC, `main` llama `prepare_ready`. Con el pool vacío no hay escritura. Un receptor ausente o un balance insuficiente devuelven el `Display` de `StateError` y dejan el estado como estaba.

### No hace

No llama `put_block`, `set_canonical_tip`, `Ledger::append`, `receive_proposal`, `create_vote`, `new_height`, `compile`, `execute` ni `publish`. No añade flags. No abre otra ronda.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`prepare_ready`. `main` lo llama después de `apply_finalized` y antes de RPC. Con el pool vacío regresa sin tocar el estado.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **32/32** en 14.26 s. `Finished` en 3 min 23 s. Esa corrida no imprimió `Compiling librocksdb`.

Una transferencia de 7, nonce 0, entre las dos cuentas del génesis deja al emisor en balance 493 y nonce 1, y al receptor en balance 107 y nonce 0. La versión del estado sube una unidad y `state_root` cambia. El tip sigue apuntando al bloque 0, cuyo `state_root` es el anterior y que no contiene la transacción. La raíz del ledger no cambia. El motor sigue en altura 1, fase `NewView`. La transacción queda en el mempool como `Pending`. Una segunda llamada no vuelve a escribir.

Un receptor ausente devuelve `account not found`. Un importe mayor que el balance devuelve `insufficient balance`. En ambos casos la raíz, la versión, el tip y el ledger quedan iguales, y la transacción sigue `Ready`.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 1 min 06 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

`apply_transition` es la validación pública de la transferencia y escribe el lote. No hay un ensayo que no persista. El bloque canónico no se actualiza: su `state_root` sigue siendo el anterior. `compile` y `execute` no se llaman, porque un `Transfer` no trae un `WasmModule` y el ABI de contratos no existe en `rsc-execution`. La transacción no se saca del mempool.

### Estado

✅ CLOSED

## TASK 7.17 — commit persistente del bloque

### Objetivo

La transferencia `Ready`, el bloque que la contiene, el índice de hijos, el tip canónico y el estado nuevo se publican en un solo `RscDatabase::write`. Al reabrir la base aparecen juntos.

### Contrato

`commit_ready` toma la primera transacción de `ready_transactions(1)`. Si no hay ninguna, no escribe. La transición se arma igual que en la 7.16. `State::stage_transition` la valida y llena un `StorageBatch` sin escribir. `apply_transition` sigue escribiendo su propio lote: la 7.16 no cambia de comportamiento.

Con la raíz nueva se construye el bloque hijo del tip. La altura es la del padre más uno. `chain_id`, versión y timestamp se copian del padre. El proposer es `AccountId::ZERO`. `state_root` es la raíz de la transición. Las transacciones son los bytes admitidos. `validate_block` comprueba padre, altura, cadena, versión y raíz de transacciones. Los topes de tamaño del contexto son los de este bloque: el nodo no guarda los límites del génesis junto al tip y no inventa otro máximo.

`Ledger::append` corre sobre un ledger reconstruido, en memoria. La raíz no se guarda en una clave nueva. Al reabrir, `rebuild_ledger` la vuelve a obtener de los bloques.

`RocksDbChainStore::stage_canonical` añade al mismo lote el bloque, el índice `ch/` del padre y `canonical-tip`. No escribe `block_by_height`, `transactions`, `transaction_locations` ni `receipts`: el store canónico no usa esas familias para el recorrido. Un solo `write` publica el lote. Si la transición o el bloque fallan antes, ese `write` no se llama y la base queda como estaba.

Después del `write`, `remove_committed` saca la transacción del mempool. El motor de consenso no llama `new_height`.

`main` llama `commit_ready` en el lugar donde llamaba `prepare_ready`. Con el pool vacío no hay escritura. `prepare_ready` queda `cfg(test)`: las pruebas de la 7.16 siguen viendo la transición aislada, y el binario no conserva un segundo escritor.

### Debe hacer

Reabrir la misma base y ver el bloque de altura 1, la transacción dentro, el `state_root` vivo igual al del header, la raíz de ledger de esa transacción y el tip canónico en ese bloque.

### No hace

No publica por P2P. No añade un segundo validador. No sincroniza estado. No produce otro bloque. No reinicia el motor de consenso. No llama `compile` ni `execute`. No añade flags.

### Archivos modificados

- `crates/rsc-state/src/state.rs`
- `crates/rsc-chain/src/rocks_store.rs`
- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`State::stage_transition` llena el lote y no escribe. `apply_transition` sigue escribiendo el suyo. `RocksDbChainStore::stage_canonical` añade bloque, hijos y tip. `commit_ready` hace el único `write`. `main` lo llama después de `apply_finalized`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **34/34** en 25.88 s. `Finished` en 3 min 29 s. Esa corrida no imprimió `Compiling librocksdb`.

Una transferencia de 7 queda en el bloque de altura 1. Al reabrir, el tip es ese bloque, la altura 0 sigue alcanzable, la transacción está dentro, el `state_root` vivo coincide con el header, la versión subió una unidad y la raíz del ledger reconstruido es la de esa transacción. Balances: emisor 493 / nonce 1, receptor 107 / nonce 0. El motor sigue en altura 1, fase `NewView`. El pool queda vacío. Una segunda llamada no escribe otra vez.

Un receptor ausente devuelve `account not found`. El tip, los hijos, la raíz, la versión y el ledger no cambian. La transacción sigue `Ready`.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 20.14 s, en la misma sesión que el test de cierre. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

El primer clippy rechazó `prepare_ready` como código muerto en el binario, porque `main` ya no la llama. Quedó `#[cfg(test)]`. El test y el clippy de cierre pasaron.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

El lote único lleva el estado, el bloque, el índice de hijos y el tip. No se escriben `block_by_height`, `transactions`, `transaction_locations` ni `receipts`. La raíz del ledger no tiene clave propia: sale de `rebuild_ledger`. Los topes de `validate_block` son el tamaño de este bloque. El proposer es `AccountId::ZERO` y el timestamp es el del padre. El motor no avanza. `compile` y `execute` no se llaman.

### Estado

✅ CLOSED

## TASK 7.18 — alinear el motor con el tip

### Objetivo

Después del commit, el motor queda en la altura siguiente al bloque persistido, en `NewView`, sin propuesta y sin lock. No se construye el bloque siguiente.

### Contrato

`new_height` solo acepta la fase `Decide`. En `NewView` devuelve `invalid phase` y no cambia la altura. No hay otra función que mueva la altura.

`align_engine` compara la altura del tip con la del motor. Si el tip va una altura por detrás, no hace nada: es el estado de la 7.14, con el motor ya en la altura siguiente. Si coinciden, la fase tiene que ser `NewView`, la vista 0, y la propuesta y el lock vacíos. Entonces reutiliza el ciclo local ya cerrado: `propose_tip`, Prepare, PreCommit, Commit y `apply_finalized`. Esa última función llama `new_height(None, Hash::ZERO)` sobre el tip persistido. Con `epoch_length` 1 el evento es `EpochChanged`. El modo ya es `Distributed` y no vuelve a `Bootstrap`.

El tip, el `state_root`, la versión y la raíz del ledger no cambian. No aparece un bloque en la altura nueva.

### Debe hacer

`main` llama `align_engine` después de `commit_ready`. Con el pool vacío el tip sigue en 0 y el motor en 1, así que la llamada no vota.

### No hace

No llama `commit_ready` otra vez. No llama `put_block` ni `set_canonical_tip`. No publica. No llama `compile` ni `execute`. No reinicia el proceso. No añade flags.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`align_engine`. `main` lo llama después de `commit_ready`. Si la altura del tip no es la del motor, regresa sin votar.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **35/35** en 36.17 s. `Finished` en 3 min 08 s. Esa corrida no imprimió `Compiling librocksdb`.

Con el tip en altura 0 y el motor en 1, `align_engine` no se mueve. Después del commit, el tip y el motor están en 1 y `new_height` desde `NewView` devuelve `invalid phase`. El ciclo local sobre ese tip deja el motor en altura 2, epoch 2, vista 0, fase `NewView`, sin propuesta y sin lock. El modo sigue `Distributed`. El tip sigue siendo el bloque 1. No hay bloque de altura 2. El `state_root`, la versión y la raíz del ledger quedan como los dejó el commit. Una segunda llamada no avanza más.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 1 min 05 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

`new_height` no se llama en `NewView`. El avance usa el ciclo local ya existente y `apply_finalized`. No se escribe el bloque de la altura nueva. El motor no se persiste.

### Estado

✅ CLOSED

## TASK 7.19 — restart y recovery de un nodo

### Objetivo

Cerrar el proceso después del bloque 1 y volver a abrir la misma base. El segundo proceso reconstruye el mismo estado lógico sin aplicar otra vez la transición y sin escribir el bloque.

### Contrato

La recuperación es el arranque que ya existe cuando hay tip. No se añade un escritor nuevo.

1. `RscDatabase::open` valida la identidad. Otra red es `NetworkMismatch`. Otra cadena es `ChainMismatch`.
2. `share_database` abre `State` y `RocksDbChainStore` sobre el mismo `Arc`.
3. `ensure_tip` ve el tip y regresa. No lee el génesis y no reescribe el bloque 0.
4. `rebuild_ledger` recorre los bloques canónicos.
5. `open_services` crea un mempool vacío, el execution engine y un `ConsensusEngine` nuevo en la altura del tip, modo `Bootstrap`. El consenso no está persistido.
6. `resume` propone ese tip, cierra Prepare, PreCommit y Commit, llama `apply_finalized` y luego `commit_ready` y `align_engine`. El pool vacío no admite ni aplica nada. `align_engine` ve el tip una altura por detrás y no vota otra vez.

Antes de `resume`, el tip, el hash del bloque, la transacción, el `state_root`, la versión, la raíz del ledger, los balances y los nonces ya son los del proceso anterior. Después de `resume` siguen iguales. El motor queda en altura 2, epoch 2, vista 0, fase `NewView`, modo `Distributed`, sin propuesta y sin lock. No hay bloque de altura 2.

### Debe hacer

`main` usa `resume` para el tramo que va de `propose_tip` a `align_engine`. El test cierra todos los dueños de la base y repite la secuencia.

### No hace

No llama `stage_transition` ni `stage_canonical` en la reapertura. No publica. No añade un segundo nodo. No añade flags.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

`resume` agrupa el tramo que ya hacía `main`: proponer el tip, cerrar el ciclo local, `apply_finalized`, `commit_ready` y `align_engine`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **36/36** en 52.46 s. `Finished` en 3 min 15 s. Esa corrida no imprimió `Compiling librocksdb`.

El proceso se cierra con el bloque 1 persistido y el motor en altura 2. Al reabrir, otra red y otra cadena se rechazan. Con la identidad original, y sin génesis, el tip, los bytes del bloque, el `TransactionId`, el `state_root`, la versión, la raíz del ledger, los balances y los nonces ya coinciden antes de `resume`. El mempool nace vacío y el motor nace en altura 1, `NewView`, modo `Bootstrap`. Después de `resume` el motor queda en altura 2, epoch 2, vista 0, `NewView`, modo `Distributed`, sin propuesta y sin lock. El bloque y el estado no cambian. No hay bloque de altura 2.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 38.30 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

La recuperación no tiene un camino distinto del arranque con tip. No se vuelve a llamar `stage_transition` ni `stage_canonical`. El consenso se recrea en memoria. `commit_ready` no escribe porque el pool está vacío.

### Estado

✅ CLOSED

## TASK 7.20 — smoke del proceso

### Objetivo

Recorrer, en un solo test, el proceso que ya existe: CLI, base, génesis, estado, ledger, mempool, consenso, RPC, P2P, una transacción válida, una inválida, el bloque 1, el apagado y la recuperación. No se añade una función de producción.

### Contrato

El arranque usa `parse_args`, `open_database`, `share_database`, `ensure_tip`, `rebuild_ledger`, `open_services` y `resume`. Con el pool vacío ese `resume` deja el tip en el bloque 0 y el motor en altura 1.

La transacción entra por `rsc_sendTransaction`, el mismo `insert` del nodo. `00` responde `TransactionRejected`. La transferencia firmada de 7 queda `Ready`. Después el test llama `commit_ready` y `align_engine`, que son las funciones de `resume`. El servidor no las vuelve a llamar al recibir la transacción: el `resume` de arranque ya corrió con el pool vacío.

El bloque 1 queda persistido. Su `state_root` es el estado vivo. `rebuild_ledger` da la raíz de esa transacción. El motor queda en altura 2, epoch 2, vista 0, `NewView`, modo `Distributed`. `rsc_getFinality` sigue en `ServiceUnavailable`: la prueba de finalidad no se guarda. `finalized_height` sigue en 0.

RPC escucha en `127.0.0.1:8545` y `/health` responde `{"status":"ok"}`. P2P llega a escuchar y `stop_p2p` lo apaga. Al soltar la base y repetir el arranque sin génesis, el tip, el bloque, las raíces, los balances, los nonces y el motor coinciden. No hay bloque 2.

### No hace

No añade flags, un bucle de producción, ni persistencia de la prueba de finalidad. No publica. No abre un segundo nodo.

### Debe hacer

El test `the_operator_process_commits_and_recovers` recorre ese contrato sobre las funciones que ya usa `main`. `write_pair_genesis` solo extrae el documento de dos cuentas que ya usaba `opened_pair`.

### Archivos modificados

- `network/node/src/main.rs`

Sin dependencia nueva. `rocksdb` sigue en 0.25.0.

### Código

El test llama `parse_args`, `open_database`, `share_database`, `ensure_tip`, `rebuild_ledger`, `open_services` y `resume`. Admite `00` y la transferencia de 7 por `dispatch` de `rsc_sendTransaction` sobre el mismo `Arc<Mempool>`. Luego llama `commit_ready` y `align_engine`. `RpcServer::run` escucha en `127.0.0.1:8545`; `GET /health` responde `{"status":"ok"}`. `start_p2p` y `stop_p2p` abren y cierran la red. Al soltar la base, el mismo arranque sin `--genesis` recupera el bloque 1.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-node -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **37/37** en 13.20 s. `Finished` en 6.84 s. Esa corrida no imprimió `Compiling librocksdb`.

El arranque con génesis deja el tip en el bloque 0, el ledger vacío y el motor en altura 1. `00` responde `TransactionRejected`. La transferencia firmada de 7 queda `Ready` y `commit_ready` persiste el bloque 1: `state_root` igual al estado vivo, un `TransactionId`, emisor 493 / nonce 1, receptor 107 / nonce 0. `rebuild_ledger` reconstruye esa raíz. El motor queda en altura 2, epoch 2, vista 0, `NewView`, modo `Distributed`, sin propuesta y sin lock. `rsc_getFinality` responde `ServiceUnavailable` y `finalized_height` sigue en 0. `GET /health` sobre `127.0.0.1:8545` devuelve `{"status":"ok"}`. P2P escucha y `stop_p2p` lo apaga. Tras soltar la base, el segundo arranque sin génesis conserva el tip, los bytes del bloque, el `state_root`, la versión, la raíz del ledger, los balances y los nonces. El motor vuelve a altura 2, epoch 2, vista 0, `NewView`, modo `Distributed`. El mempool queda vacío. No hay bloque de altura 2.

### Clippy

```text
cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings
```

✅ PASS. `Finished` en 13.14 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

La primera lectura de `/health` corrió dentro de un runtime `current_thread`. El hilo bloqueado en el socket no dejaba atender al servidor y `read_to_end` devolvió `WouldBlock`. El tramo de RPC y P2P pasó a un runtime `multi_thread` y la petición se hace `flush` antes de leer. La corrida de cierre ya incluye ese arreglo.

### Abortos

Ninguno. El `--locked` compiló porque no hubo dependencia nueva.

### Decisiones congeladas

El smoke no añade un bucle de producción. `resume` de arranque corre con el pool vacío; el bloque 1 lo escriben `commit_ready` y `align_engine`, las mismas funciones que `resume` ya llama. La finalidad correcta de esta tarea es el motor alineado y el `state_root` del bloque igual al estado vivo. `rsc_getFinality` sigue indisponible y `finalized_height` sigue en 0.

### Estado

✅ CLOSED

## Fase 8 — contrato de `network/validator`

Binario: `network/validator/src/main.rs`, paquete `rsc-validator`. Este contrato está escrito antes del código. Al redactarlo, el directorio no existe. El proceso es distinto de `rsc-node`. `rsc-node` sigue ensamblando el nodo. Este proceso carga la clave del validador.

### Qué hace

Lee `--key-file` y deriva el `KeyId` con `PrivateKey::from_bytes` y `PrivateKey::key_id`. Lee `--data-dir`, `--network-id` y `--chain-id`, arma `DatabaseConfig` y llama `RscDatabase::open` una vez. Con esa `Arc` abre `State` y `RocksDbChainStore`. Recorre el tip canónico hasta el bloque 0 y reconstruye el `Ledger` con `Transaction::id` y `Ledger::append`. Si la clave, la base y ese recorrido son válidos, escribe el `KeyId` en stdout y termina con código 0.

### Qué no hace

No guarda la `PrivateKey` en la base ni en otro archivo. No imprime la `PrivateKey`. No vuelve a comprobar `network_id` ni `chain_id`: eso lo hace `RscDatabase::open`. No aplica génesis. No escribe bloques. No modifica `State` ni el `Ledger` persistido. No crea `Mempool`. No crea `ExecutionEngine`. No crea `ConsensusEngine`. No inicia RPC. No inicia P2P. No crea votos, propuestas ni QC. No ejecuta transacciones. No llama `sign_vote`. No cambia el seed que `sign_vote` usa hoy (`AccountId`). No asigna un rol de red al `KeyId`.

### Inputs

- `--key-file`: archivo de exactamente 32 bytes para `PrivateKey::from_bytes`
- `--data-dir`: ruta que recibe `RscDatabase::open`
- `--network-id`: string que entra en `DatabaseConfig.network_id`
- `--chain-id`: `u32` cuya forma decimal entra en `DatabaseConfig.chain_id` (`3` → `"3"`)

`DatabaseConfig.max_value_size` es 8 MiB porque el struct lo exige para abrir. No forma parte de la identidad y el proceso no lo compara.

Cualquier otro flag es desconocido y se rechaza. En esta tarea eso incluye `--genesis`, `--listen` y `--rpc`.

### Outputs

- clave de 32 bytes, base abierta y ledger reconstruido: código 0, stdout con el `KeyId` en hex minúsculas de 64 caracteres, sin `0x`
- configuración inválida, archivo ilegible, clave rechazada por `PrivateKey::from_bytes`, error de `RscDatabase::open`, o recorrido canónico rechazado: código 1, el error en stderr, stdout vacío

### Dependencias permitidas

`rsc-crypto`, `rsc-core`, `rsc-storage`, `rsc-state`, `rsc-chain` y `rsc-ledger`. En tests, `tempfile` y `rsc-genesis`.

### Dependencias prohibidas

`rsc-consensus`, `rsc-mempool`, `rsc-execution`, `rsc-rpc`, `rsc-p2p` y el paquete `rsc-node`. Otro formato de clave. Un pin nuevo de RocksDB. Una reconstrucción distinta de las APIs públicas ya usadas por `rsc-node`.

`ValidatorId` en `rsc-consensus` es `AccountId`. `ConsensusConfig::validate` existe y esta tarea no lo llama. `LocalPeerIdentity` existe en `rsc-p2p` y esta tarea no lo llama.

### Errores

- `falta key-file`, `falta data-dir`, `falta network-id`, `falta chain-id`
- `falta el valor de …`
- `key-file vacío`, `data-dir vacío`, `network-id vacío`, `chain-id vacío`
- `key-file repetido`, `data-dir repetido`, `network-id repetido`, `chain-id repetido`
- `chain-id no es u32`
- `argumento desconocido …`
- el `Display` del error de lectura
- `invalid private key` cuando el archivo no tiene 32 bytes
- el `Display` de `StorageError`: `storage network mismatch`, `storage chain mismatch`, `storage path rejected`
- `falta tip`, `bloque ausente`, `duplicate transaction`

### Invariantes

La `PrivateKey` vive solo mientras `key_id` la usa y no se imprime. El archivo de clave no es hex ni JSON. La identidad de la base la decide `RscDatabase::open`. Una sola apertura. `State` y `RocksDbChainStore` comparten esa `Arc`. El `Ledger` sale del recorrido canónico y no se persiste. La reconstrucción no llama `initialize_genesis`, `apply_transition`, `put_block` ni `set_canonical_tip`. `sign_vote` permanece igual. El esquema de storage no cambia.

### Tests

Al escribir este contrato no se había ejecutado ningún test del paquete. Los de TASK 8.1 se registran al cerrarla.

### Criterio de cierre de TASK 8.1

```text
cargo test --offline --locked -p rsc-validator -- --test-threads=1
cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings
```

Los dos comandos tienen que terminar. La fase sigue abierta después de 8.1.

### Estado

⚠ OPEN

## TASK 8.1 — bootstrap del validador

### Objetivo

Crear el paquete `rsc-validator`. El proceso lee `--key-file`, carga la clave con `PrivateKey::from_bytes`, deriva el `KeyId` y termina.

### Archivos modificados

- `Cargo.toml` (miembro `network/validator`)
- `Cargo.lock` (entrada `rsc-validator`; `rocksdb` sigue en 0.25.0)
- `network/validator/Cargo.toml`
- `network/validator/src/main.rs`

### Implementación

`parse_args` acepta solo `--key-file`. Un flag ausente, vacío, repetido o desconocido devuelve error. `read_identity` lee el archivo y llama `PrivateKey::from_bytes` y `key_id`. `main` imprime el hex del `KeyId` y sale con código 0. Cualquier error de esa cadena sale con código 1.

### Código

`Config`, `parse_args`, `set`, `read_identity`, `format_key_id`, `bootstrap`, `main`.

### Dependencias

`rsc-crypto`. En tests, `tempfile`.

### No hace

No abre la base ni ningún servicio del nodo. No construye `ValidatorRecord`, `ValidatorSet` ni `ConsensusEngine`. No firma.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-validator -- --test-threads=1
```

### Resultado

✅ PASS. **8/8** en 0.08 s. `Finished` en 5.35 s. Esa corrida no imprimió `Compiling librocksdb`.

Un archivo de 32 bytes produce el mismo `KeyId` que `PrivateKey::from_bytes`. Stdout de un arranque válido es ese id en 64 hex minúsculas. Falta el flag, falta el valor, valor vacío, flag repetido y `--data-dir` se rechazan. Un archivo ausente devuelve el error de lectura. Un archivo de 16 bytes devuelve `invalid private key`.

### Clippy

```text
cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings
```

✅ PASS. `Finished` en 52.74 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Errores encontrados

La primera compilación, con `cargo test --offline -p rsc-validator`, falló: `unwrap_err` exigía `Debug` en `Config`. Se añadió el derive. El comando de cierre es el `--locked` de arriba, ya con ese arreglo. Esa primera corrida actualizó `Cargo.lock` con `rsc-validator` y dejó `rocksdb` en 0.25.0.

### Abortos

Ninguno.

### Decisiones congeladas

La entrada del proceso es `--key-file`, 32 bytes crudos. El `KeyId` sale de `PrivateKey::key_id`. El secreto no se conserva. `sign_vote` no se toca.

### Estado

✅ CLOSED

## TASK 8.2 — base e identidad del validador

### Objetivo

Abrir `RscDatabase` una vez, con `--data-dir`, `--network-id` y `--chain-id`, después de derivar el `KeyId`. La identidad persistida la valida `RscDatabase::open`.

### Alcance

El proceso sigue leyendo `--key-file`. Añade los tres flags que `DatabaseConfig` necesita. `chain_id` entra como el decimal del `u32`. `max_value_size` es 8 MiB. El handle se suelta al terminar la función.

### Qué hace

`parse_args` exige los cuatro flags. `read_identity` obtiene el `KeyId` y suelta la `PrivateKey`. `open_database` llama `RscDatabase::open`. `main` imprime el `KeyId` solo si esa llamada devuelve `Ok`.

### Qué no hace

No escribe la clave. No compara la red ni la cadena por su cuenta. No abre `State`, `Ledger`, `Mempool`, `ConsensusEngine`, RPC ni P2P. No produce votos, propuestas, QC ni bloques. No aplica génesis.

### Inputs

`--key-file`, `--data-dir`, `--network-id`, `--chain-id`.

### Outputs

Código 0 y el hex del `KeyId` si la clave y la base abren. Código 1 y el error en stderr en cualquier otro caso.

### Dependencias

`rsc-storage`, además de `rsc-crypto`. En tests, `tempfile`.

### Errores

Los de configuración de 8.1, más los de los tres flags nuevos, `chain-id no es u32`, y el `Display` de `StorageError` para ruta rechazada, `NetworkMismatch` y `ChainMismatch`.

### Invariantes

Una sola apertura por arranque. La clave no entra en la base. Reabrir la misma ruta con la misma red y la misma cadena devuelve `Ok`. Otra red y otra cadena salen de `RscDatabase::open`.

### Criterio de cierre

```text
cargo test --offline --locked -p rsc-validator -- --test-threads=1
cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Archivos modificados

- `network/validator/Cargo.toml`
- `network/validator/src/main.rs`
- `Cargo.lock` (`rsc-validator` depende de `rsc-storage`; `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`)

### Implementación

`parse_args` exige `--key-file`, `--data-dir`, `--network-id` y `--chain-id`. El `chain_id` es `u32` y entra en `DatabaseConfig` como `to_string()`. `read_identity` llama `PrivateKey::from_bytes` y `key_id`, y suelta la clave. `open_database` llama `RscDatabase::open` una vez. `main` imprime el hex del `KeyId` solo después de ese `Ok`.

### Código

`MAX_VALUE_SIZE`, `open_database`. `bootstrap` llama a `read_identity` y después a `open_database`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-validator -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **12/12** en 1.05 s. `Finished` en 1 min 18 s. El log imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

Una clave de 32 bytes abre una base nueva y el `KeyId` coincide con `PrivateKey::key_id`. La misma ruta, la misma red y la misma cadena reabren. Otra red devuelve `storage network mismatch`. Otra cadena devuelve `storage chain mismatch`. Una ruta con `..` devuelve `storage path rejected`. Un archivo ausente devuelve el error de lectura. Un archivo de 16 bytes devuelve `invalid private key`. Falta un flag, falta el valor, valor vacío, flag repetido, `chain-id` que no es `u32` y `--listen` se rechazan. El hex del `KeyId` y esos errores no contienen el seed `11` repetido.

### Clippy

```text
cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings
```

✅ PASS. `Finished` en 28.94 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`.

### Errores encontrados

Ninguno en la corrida de cierre.

### Abortos

Ninguno. `Cargo.lock` se actualizó antes, con `cargo metadata --offline`, para registrar la dependencia `rsc-storage`. El pin de `rocksdb` no cambió. El `--locked` compiló con ese lock.

### Decisiones congeladas

La identidad de la base la decide `RscDatabase::open`. El proceso no la reimplementa. El `chain_id` entra como texto decimal del `u32`. La `PrivateKey` no se escribe ni se imprime. `max_value_size` es 8 MiB y no es identidad.

### Estado

✅ CLOSED

## TASK 8.3 — State y Ledger del validador

### Objetivo

Después de `RscDatabase::open`, abrir `State` y `RocksDbChainStore` sobre la misma `Arc` y reconstruir el `Ledger` desde la cadena canónica.

### Qué hace

`share_database` llama `State::open` y `RocksDbChainStore::from_shared`. `rebuild_ledger` recorre `get_canonical_tip`, `get_block` y `previous_block_hash` hasta el bloque 0, en el orden de las transacciones, con `Transaction::from_canonical_bytes`, `Transaction::id` y `Ledger::append`. `main` conserva `State`, `Ledger` y `KeyId` en memoria e imprime el `KeyId` si el recorrido termina.

### Qué no hace

No aplica génesis. No escribe bloques. No llama `initialize_genesis`, `apply_transition`, `put_block` ni `set_canonical_tip`. No crea `Mempool`, `ExecutionEngine` ni `ConsensusEngine`. No inicia RPC ni P2P. No produce votos, propuestas ni QC. No abre la base por segunda vez.

### Inputs

Los cuatro flags de 8.2. La cadena ya persistida. Esta tarea no añade flags.

### Outputs

Código 0 y el hex del `KeyId` cuando el tip existe y el ledger reconstruye. Código 1 en caso contrario. Sin tip: `falta tip`. Bloque ausente: `bloque ausente`. `TransactionId` repetido: `duplicate transaction`.

### Dependencias

`rsc-core`, `rsc-state`, `rsc-chain` y `rsc-ledger`, además de `rsc-crypto` y `rsc-storage`. En tests, `rsc-genesis` y `tempfile`.

### Recorrido

`RscDatabase::open`, `State::open`, `RocksDbChainStore::from_shared`, `get_canonical_tip`, `get_block`, `previous_block_hash` hasta altura 0, transacciones en el orden guardado, `Transaction::id`, `Ledger::append`, `Ledger::root`.

`rsc-node` no exporta ese recorrido. El validador llama las mismas APIs públicas, en el mismo orden, con los mismos textos de error. No se añade un método nuevo en `rsc-chain` ni en `rsc-ledger`.

### Invariantes

Una apertura. `Arc::ptr_eq` entre la base y `store.database()`. El tip, los bytes del bloque, el `state_root` y la versión no cambian. El `LedgerRoot` coincide con un `Ledger` armado con los mismos `TransactionId`. No aparece un bloque nuevo.

### Errores

`falta tip`, `bloque ausente`, `duplicate transaction`, más `ciclo en la cadena`, `hash de bloque distinto`, `genesis inconsistente`, `altura inconsistente` y `padre distinto`, que ya devuelve el mismo recorrido en `rsc-node`. `NetworkMismatch` y `ChainMismatch` siguen saliendo de `RscDatabase::open`.

### Tests ejecutados

```text
cargo test --offline --locked -p rsc-validator -- --test-threads=1
```

Con `ROCKSDB_LIB_DIR` en el `.a` de `librocksdb-sys-38b69e303fd1b6b2` y `ROCKSDB_STATIC=1`.

### Resultado

✅ PASS. **17/17** en 14.40 s. `Finished` en 18.63 s. Esa corrida no imprimió `Compiling librocksdb`.

Con el bloque 0 ya escrito, `State` abre, el tip es génesis y el ledger queda vacío, con la misma raíz que `Ledger::new`. Con un bloque hijo y una transacción, el `LedgerRoot` coincide con `Ledger::append` de ese `TransactionId`. `State` y `RocksDbChainStore` comparten la `Arc`: el conteo pasa de 1 a 3 y `Arc::ptr_eq` se cumple. Otra red devuelve `storage network mismatch`. Otra cadena devuelve `storage chain mismatch`. Un padre ausente devuelve `bloque ausente`. Una transacción repetida devuelve `duplicate transaction`. Después de soltar el proceso, el tip, los bytes del bloque, el `state_root` y la versión siguen iguales. El hex del `KeyId` no contiene el seed.

### Clippy

```text
cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings
```

✅ PASS. `Finished` en 27.70 s, después del test. No imprimió `Compiling librocksdb-sys`. No apareció `cache.cc`. El `librocksdb.a` sigue en 457632132 bytes.

### Archivos modificados

- `network/validator/Cargo.toml`
- `network/validator/src/main.rs`
- `Cargo.lock` (`rsc-validator` depende de `rsc-core`, `rsc-state`, `rsc-chain`, `rsc-ledger` y, en tests, `rsc-genesis`; `rocksdb` sigue en 0.25.0)

### Implementación

`share_database` abre `State` y `RocksDbChainStore` sobre la misma `Arc`. `canonical_blocks` y `rebuild_ledger` recorren el tip con `get_canonical_tip`, `get_block`, `previous_block_hash`, `Transaction::from_canonical_bytes`, `Transaction::id` y `Ledger::append`. `bootstrap` llama esas funciones una vez después de `read_identity`. `main` lee la versión del estado y la raíz del ledger, e imprime el `KeyId`.

### Código

`Recovered`, `share_database`, `rebuild_ledger`, `canonical_blocks`. `bootstrap` los llama después de `open_database`.

### Errores encontrados

La primera corrida de tests falló en dos casos: `snapshot` volvía a abrir la base mientras `Recovered` todavía tenía el lock. El segundo `snapshot` quedó después de `drop(ready)`. La corrida de cierre es la de arriba.

### Abortos

Ninguno. `Cargo.lock` se actualizó antes, con `cargo metadata --offline`, para las dependencias nuevas. El pin de `rocksdb` no cambió.

### Decisiones congeladas

El recorrido no se exporta desde `rsc-node` y no se añade a `rsc-chain` ni a `rsc-ledger`. El validador llama las mismas APIs, en el mismo orden, con los mismos textos. No aplica génesis ni escribe la cadena. El fixture de test prepara el bloque 0 y el bloque hijo; el proceso solo lee.

### Estado

✅ CLOSED

## TASK 8.4 — ConsensusEngine del validador

### Objetivo

Construir `ConsensusEngine` sobre el tip ya recuperado, con la configuración y la identidad que las APIs públicas ya definen.

### Qué se inspeccionó

`ConsensusEngine::new(config, validators, randomness, local, height)` es el constructor público. Exige un `ConsensusConfig` que pase `validate`, un `ValidatorSet`, un `Hash` de aleatoriedad, un `AccountId` local y la altura. Dentro de `new` el crate fija la vista en 0, la fase en `NewView`, la propuesta vacía y el lock vacío. El epoch sale de `epoch_of(height, config.epoch_length)`, esto es `height / epoch_length`. `select_committee` arma el comité desde el `ValidatorSet`. El constructor no lee el tip, no abre la base y no persiste el motor. `PrivateKey` no es un argumento.

`ValidatorId` ya existe: es un reexport de `AccountId`. `ValidatorRecord.id` es ese `AccountId`. `voting_power`, `participation_score` y `eligible` son campos del registro. `NetworkMode` viaja dentro del `ValidatorSet`. `new` no lo calcula.

`ConsensusConfig::validate` rechaza `committee_size == 0`, `epoch_length == 0`, `max_validator_weight == 0` y `min_validator_weight > max_validator_weight`. El error es `invalid config`. Un conjunto sin elegibles es `empty committee`.

La altura inicial sí está en el tip recuperado: `block.header.height.0`.

### Qué no está en ninguna API pública ni en la cadena

El génesis no guarda comité, epoch, pesos ni modo. El bloque y el estado tampoco. No hay `ConsensusConfig` persistido ni un constructor que lo lea.

`rsc-node` rellena comité 1, epoch 1, pesos 1, `AccountId::ZERO`, `NetworkMode::Bootstrap` y `Hash::ZERO`. El registro de la 7.6 dice que esos números no son ley de génesis. Los tests de `rsc-consensus` usan otro mínimo, comité 3 y epoch 10. Copiar cualquiera de los dos sería elegir una configuración que la cadena no tiene.

La identidad del proceso es el `KeyId`. El motor guarda un `AccountId`. `derive_account_id` produce la cuenta de `rsc-state`. `sign_vote` sigue usando los 32 bytes del `AccountId` como seed. No hay una función pública que ate la `PrivateKey` del validador al `AccountId` que el motor almacena.

### Qué no hace

No se añade código. No se construye el motor con números elegidos aquí. No se modifica `rsc-consensus`. No se crean propuestas, votos ni QC.

### Estado

⚠ BLOCKED. Falta el origen público de `ConsensusConfig`, del `ValidatorSet`, del `Hash` de aleatoriedad y del `AccountId` local del validador. La altura del tip ya está. El resto no se inventa. TASK 8.4A no lo desbloquea.

## TASK 8.4A — Fuente de configuración del consenso

### Problema

`ConsensusEngine::new` pide cuatro datos que la cadena recuperada no trae: `ConsensusConfig`, `ValidatorSet`, el `Hash` de aleatoriedad y el `AccountId` local. Esta tarea busca la fuente. No construye el motor.

### APIs inspeccionadas

- `rsc-consensus`: `ConsensusEngine::new`, `ConsensusConfig`, `ConsensusConfig::validate`, `ValidatorSet`, `ValidatorRecord`, `NetworkMode`, `CommitteeSelectionInput`, `select_committee`, `epoch_of`, `new_height`. `ValidatorId` es `AccountId`.
- `rsc-core`: `AccountId`, `Hash`. No nombra comité, epoch ni beacon.
- `rsc-crypto`: `PrivateKey`, `KeyId`, `CryptoDomain`. Los dominios llegan a `ValidatorIdentity`, `NodeIdentity` y `AccountIdentity`. No hay dominio de beacon.
- `rsc-genesis`, porque el comentario de `ConsensusConfig` dice que el comité, el epoch y el umbral los fija el génesis: `Genesis`, `NetworkParameters`, `GenesisAccount`.
- `rsc-state`: `derive_account_id`. Produce la cuenta. No declara el `AccountId` del motor.
- `network/node`: `Config` tiene `data_dir`, `network_id`, `chain_id` y génesis. El motor se arma en `open_services` con números que la 7.6 dejó fuera de la ley de génesis.
- `network/validator`: `Config` tiene `key_file`, `data_dir`, `network_id` y `chain_id`. Guarda el `KeyId` en memoria. No tiene consenso.

### Datos disponibles

La altura del tip, `block.header.height.0`. El `KeyId` del proceso. `view = 0`, fase `NewView`, propuesta vacía y lock vacío los escribe `new`. El epoch, cuando exista `epoch_length`, lo calcula `epoch_of`.

### Datos ausentes

`ConsensusConfig`, el comité, los pesos, el `ValidatorSet`, el `Hash` de aleatoriedad y el `AccountId` local del motor.

### Fuentes existentes

`ConsensusConfig` documenta que esos campos los fija el génesis. `Genesis` y `NetworkParameters` guardan versión, `chain_id`, timestamp, escala, límites de bloque, fee y cuentas con balance. No guardan comité, epoch, pesos ni modo. `GenesisAccount` trae `account_id`, `owner_key_id` y balance. El crate dice que no deriva el `account_id` desde la clave. No es un `ValidatorRecord`.

`CommitteeSelectionInput.randomness` está documentado como beacon verificable, no como `rand`. `CommitteeMember.vrf_proof` remite a un VRF futuro. `new_height` recibe el `Hash` del llamador. No hay función que lo calcule desde el tip, el génesis o el estado.

`derive_account_id` existe para la cuenta de estado. `sign_vote` sigue tomando el seed desde los bytes del `AccountId`. Ninguna de las dos APIs dice que el `AccountId` local del motor sea el `KeyId` de esta `PrivateKey`.

### Decisión

Los cuatro orígenes siguen sin fuente. No se copia la configuración de `rsc-node`. No se copia la de los tests de `rsc-consensus`. No se crea un default. No se añade un flag ni un archivo. No se extiende el génesis en esta tarea. No se modifica `rsc-consensus`. No se construye `ConsensusEngine`.

### Fuente de ConsensusConfig

No hay. El comentario apunta al génesis y el documento no contiene los campos.

### Fuente de ValidatorSet

No hay. Las cuentas del génesis no son registros de validador: no traen peso, elegibilidad ni `NetworkMode`.

### Fuente de randomness

No hay fuente protocolaria. El crate pide un beacon verificable. `Hash::ZERO` queda fuera: es el valor de prueba de `rsc-node`, no un beacon.

### Fuente de local AccountId

No hay. `PrivateKey` produce `KeyId`. El motor guarda `AccountId`. Esa relación no se congela aquí.

### Reglas de validación

Cuando exista fuente, `ConsensusConfig::validate` sigue siendo el rechazo del crate: `invalid config`. Un conjunto sin elegibles sigue siendo `empty committee`. Esta tarea no añade otra regla.

### Qué no se permite inferir

No se infiere el comité desde la memoria del proceso, desde el orden de archivos, desde `AccountId::ZERO`, desde el `KeyId`, ni desde `derive_account_id`. No se infiere el `Hash` de aleatoriedad desde `Hash::ZERO` ni desde el hash del bloque.

### Dependencias

Ninguna nueva. Sin código.

### Criterio para desbloquear TASK 8.4

Los cuatro orígenes tienen que ser públicos y los mismos para los validadores de una red: la configuración de comité y epoch, el `ValidatorSet` con pesos, el beacon del `Hash`, y el `AccountId` local de cada proceso. Con uno solo ausente, 8.4 sigue bloqueada.

### Estado

⚠ BLOCKED. TASK 8.4 permanece BLOCKED.

## TASK 8.4B — Fuentes protocolarias de ConsensusEngine

### Estado actual

8.4 y 8.4A siguen BLOCKED. Esta tarea no escribe código, no modifica crates y no construye el motor. La altura del tip ya entra en `ConsensusEngine::new`. `new` fija vista 0, fase `NewView`, propuesta vacía y lock vacío. `ValidatorId` es `AccountId`.

### Cuatro datos faltantes

`ConsensusConfig`, `ValidatorSet`, el `Hash` de aleatoriedad y el `AccountId` local.

### Opciones encontradas

| Dato | Lo que ya existe | Clase |
| --- | --- | --- |
| Altura | `block.header.height.0` del tip | PROTOCOL DATA, ya disponible |
| Vista, fase, propuesta, lock | los escribe `ConsensusEngine::new` | DERIVED DATA del constructor |
| Epoch | `epoch_of(height, epoch_length)` | DERIVED DATA, espera `epoch_length` |
| `KeyId` | `PrivateKey::key_id` | DERIVED DATA del LOCAL SECRET |
| `PrivateKey` | archivo `--key-file`, solo en memoria | LOCAL SECRET |
| `ConsensusConfig` | struct público. El comentario dice que lo fija el génesis. `Genesis` y `NetworkParameters` no tienen esos campos | sin fuente |
| `ValidatorSet` | struct público. `GenesisAccount` es cuenta con balance, no registro de validador | sin fuente |
| `randomness` | campo `Hash` de `CommitteeSelectionInput`, documentado como beacon verificable. `new_height` lo recibe del llamador | sin fuente |
| `AccountId` local | tipo del motor. `derive_account_id` es la cuenta de estado. `sign_vote` usa los bytes del `AccountId` como seed | sin vínculo |

No hay una opción ya respaldada por una API que lea estos cuatro datos de la cadena. Los números de `rsc-node` y de los tests de `rsc-consensus` quedan fuera.

### Decisiones congeladas

Estos orígenes quedan fijados para v1. No se eligen números. No se copia la configuración de `rsc-node` ni la de los tests. No hay defaults ocultos. No se escribe código en esta tarea.

#### ConsensusConfig → génesis

PROTOCOL DATA. El documento de génesis es la única fuente. Los campos del struct entran ahí: `committee_size`, `epoch_length`, `min_validator_weight`, `max_validator_weight` y `min_validators_for_distributed`. `validate` exige comité distinto de cero, epoch distinto de cero, peso máximo distinto de cero y peso mínimo menor o igual que el máximo. `min_validators_for_distributed` es del mismo struct y entra en el documento aunque `validate` no rechace el cero.

Quién lo crea: el documento. Quién lo valida: `ConsensusConfig::validate`. Todos los validadores leen el mismo documento, el que ya entra en el hash de génesis. No cambia por altura. El epoch es DERIVED DATA: `epoch_of(height, epoch_length)`. Dos documentos distintos producen epochs distintos. La API que ya lo consume es `ConsensusEngine::new`. Leer estos campos del documento es una API que el génesis todavía no tiene. No se añade ahora.

#### ValidatorSet → lista explícita

PROTOCOL DATA. Una lista de validadores del génesis, distinta de `GenesisAccount`. Una cuenta con balance no es un validador. El peso no sale del balance. La elegibilidad no sale del balance.

Cada registro aporta, con tipos que ya existen:

- `AccountId`
- `owner_key_id`, que es `KeyId`
- peso, que es el `voting_power` de `ValidatorRecord`
- `eligible`

`select_committee` sigue armando el comité de una vista. Eso es DERIVED DATA. `participation_score` no entra en esta lista y no se fija en 0 como ley.

#### NetworkMode → propiedad global

Está en `ValidatorSet`, no en cada `ValidatorRecord`. El arranque del validador no elige el modo. La variante que el crate documenta como set inicial de génesis es `Bootstrap`. La única transición que ya existe está en `new_height`: si el modo es `Bootstrap` y el número de validadores alcanza `min_validators_for_distributed`, pasa a `Distributed`. Esa regla permanece en el crate.

#### randomness → semilla pública y beacon determinista

PROTOCOL DATA. El génesis lleva una semilla pública, `consensus_randomness_seed`. No es `Hash::ZERO`. No es un secreto.

El beacon de una altura es determinista y verificable. Todos los validadores de la misma red calculan el mismo valor para la misma altura. Conceptualmente:

```text
beacon(height) = Hash(domain, consensus_randomness_seed, epoch, height, parent_block_hash)
```

La primitiva que ya existe es `hash_domain` / `encode_domain_payload`. Esto es un deterministic verifiable beacon. No es un VRF y no se promete impredecibilidad. `new_height` sigue recibiendo el `Hash` del llamador. El de la altura de arranque es este beacon.

`CryptoDomain` no tiene una variante de beacon. El byte de dominio de esa fórmula no está en el enum. Elegir una variante existente o añadir una queda pendiente de protocolo. No se elige aquí. `Hash::ZERO` sigue fuera.

#### local AccountId → `derive_account_id`

La firma real es `derive_account_id(owner_key_id: &KeyId) -> AccountId`. No acepta la `PrivateKey` ni la `PublicKey`. El camino, sin otra derivación, es:

```text
PrivateKey
    ↓
public_key()
    ↓
key_id()
    ↓
KeyId
    ↓
derive_account_id
    ↓
AccountId de rsc-state
    ↓
From hacia rsc_core::AccountId
```

Ese `AccountId` es el local del motor y tiene que ser el `AccountId` del registro de génesis de ese validador. `PrivateKey` sigue siendo LOCAL SECRET. `KeyId` y `AccountId` son DERIVED DATA.

#### Identidad → `owner_key_id`

```text
PrivateKey::key_id == GenesisValidator.owner_key_id
```

Si no coincide, el arranque rechaza la identidad. Un `AccountId` escrito a mano, sin este binding, no se acepta. El `AccountId` del registro tiene que ser `derive_account_id` de ese mismo `owner_key_id`.

#### `sign_vote`

USER/PROTOCOL DECISION REQUIRED.

`sign_vote` construye la clave con `PrivateKey::from_bytes` sobre los 32 bytes del `AccountId`. El comentario del crate dice que, mientras no haya capa de identidad, el seed es el `AccountId`. `vote_signature_valid` usa la misma clave.

El `AccountId` que sale de `derive_account_id` es un hash del `KeyId`. No es el seed de la `PrivateKey` del proceso. Una firma hecha con esa `PrivateKey` no verifica bajo `sign_vote`. Una firma hecha por `sign_vote` verifica contra el seed sintético del `AccountId`, no contra la clave pública del validador.

No se afirma que el `AccountId` como seed sea la `PrivateKey`. No se inventa esa equivalencia. La relación `PrivateKey` ↔ `owner_key_id` ↔ `AccountId` ↔ verificación de la firma no queda cerrada con las APIs públicas actuales.

### Invariantes

Un mismo génesis da el mismo `ConsensusConfig`, la misma lista de validadores, los mismos pesos, la misma elegibilidad y la misma semilla. El beacon de una altura es el mismo en todos los procesos. El `KeyId` local tiene que estar en esa lista. El secreto no se escribe. El motor no se persiste. `validate` y `select_committee` siguen siendo las funciones que ya existen.

### Implicaciones

Génesis: el documento todavía no guarda estos campos. Esta tarea no lo modifica.

Validador: sigue con `--key-file`, la base, `State` y `Ledger`. No hay flags nuevos. El arranque, cuando exista, rechaza un `KeyId` que no esté en la lista.

Consenso: `new` ya recibe config, set, `Hash` y `AccountId`. `sign_vote` no demuestra que ese `AccountId` sea la clave del proceso.

### APIs que serían necesarias

No se añaden ahora. Faltarían la lectura de estos campos en el génesis, el dominio de beacon que `CryptoDomain` no tiene, y una firma de voto que verifique la `PrivateKey` del `owner_key_id`. Esta última es la que impide desbloquear 8.4.

### Estado

PARCIALMENTE DECIDIDA. El `ConsensusConfig`, la lista de validadores, el peso, la elegibilidad, el `NetworkMode` global, la semilla, la forma del beacon y el `AccountId` vía `derive_account_id` quedan congelados como origen. El vínculo de `sign_vote` con la `PrivateKey` queda cerrado en TASK 8.4C. 8.4 sigue BLOCKED. 8.4A sigue BLOCKED. No se construye el motor.

## TASK 8.4C — Binding criptográfico del validador

### Problema original

Un voto de consenso tenía que quedar firmado por la `PrivateKey` del proceso y verificarse contra la identidad pública de ese validador. `sign_vote` no usaba esa clave.

### Causa

`PrivateKey::public_key` produce `PublicKey`. `PublicKey::key_id` produce `KeyId`. `derive_account_id(&KeyId)` produce el `AccountId` de protocolo. Ese `AccountId` es un hash. No es el seed Ed25519.

La API anterior era `sign_vote(vote: &mut Vote)`. Por dentro, `vote_key` hacía `PrivateKey::from_bytes` sobre los 32 bytes de `vote.validator`. `vote_signature_valid` y `verify_qc` repetían esa reconstrucción para obtener la `PublicKey`. La clave del proceso y la clave que firmaba el voto eran distintas.

### Regla

`AccountId` es identidad de protocolo. `PrivateKey` es secreto. `sign_vote` recibe `&PrivateKey` y no reconstruye una clave desde un `AccountId`.

La cadena de la identidad es:

```text
PrivateKey
    ↓
PublicKey
    ↓
KeyId
    ↓
derive_account_id
    ↓
AccountId
```

### ValidatorRecord

El registro queda con los tipos públicos que ya existen:

- `id: AccountId`
- `owner_key_id: KeyId`
- `owner_public_key: PublicKey` de `rsc-crypto`
- `voting_power`
- `participation_score`, que ya estaba y no entra en el binding
- `eligible`

`validate_validator_record` exige `owner_public_key.key_id() == owner_key_id` y `AccountId::from(derive_account_id(&owner_key_id)) == id`. `find_validator_by_key_id` localiza el registro de ese `KeyId` y aplica la misma validación. Un `AccountId` que no salga de ese `KeyId` queda rechazado.

`CommitteeMember` copia `owner_public_key` para que `aggregate_qc` y `verify_qc` verifiquen contra esa clave. El formato de `QuorumCertificate` sigue siendo el haz de firmas. No se añade la clave al certificado.

### APIs

- `sign_vote(vote, key: &PrivateKey) -> Result<(), ConsensusError>` firma `vote_bytes` bajo `CryptoDomain::ConsensusVote`.
- `vote_signature_valid(vote, public_key: &PublicKey)` llama a `rsc_crypto::verify`.
- `verify_validator_vote(record, vote)` comprueba autor, `validate_validator_record`, y la firma con `owner_public_key`.
- `create_vote(phase, key)` firma con esa clave si su `PublicKey` y su `KeyId` son los del registro cuyo `id` es el validador local.
- `receive_vote` busca el registro del `vote.validator` y verifica con `owner_public_key`.

`vote_bytes` conserva el layout: fase en un byte, epoch, altura y vista en `u64` little-endian, y el hash de 32 bytes. `has_quorum` sigue siendo `3 * poder > 2 * total`.

### Génesis

`rsc-genesis` no cambia en esta tarea. Cuando la lista de validadores entre en el documento, cada entrada tendrá que traer `owner_key_id`, `owner_public_key` y `AccountId` coherentes, y la carga tendrá que pasar por `validate_validator_record`. El balance no decide la identidad ni la elegibilidad. `AccountId::ZERO` no sustituye esa identidad.

### rsc-node

El ensamblador conserva `AccountId::ZERO` como validador local, porque el bloque 0 ya lo trae como proposer. Pasa a `create_vote` la clave explícita `PrivateKey::from_bytes(&[0u8; 32])` y guarda la `PublicKey` y el `KeyId` de esa clave en el registro. `sign_vote` no deriva esa clave. Ese registro no cumple `derive_account_id` y el nodo no lo presenta como identidad de protocolo.

El test de integración hace lo mismo con sus `validator_id` de prueba: `signer_for` entrega la `PrivateKey` a `sign_vote`. La función no la reconstruye. Esos registros tampoco cumplen `derive_account_id`. La prueba de protocolo es `real_key_signs_and_registered_public_key_verifies`.

### Invariantes

La `PrivateKey` no se imprime: `Debug` y `Display` son `PrivateKey(****)`. El dominio del voto es `ConsensusVote`. El mensaje firmado no cambia. El quórum no cambia. No hay `ConsensusEngine` en `network/validator`.

### Archivos

`crates/rsc-consensus`, `network/node/src/main.rs`, `crates/rsc-integration-tests/tests/integration_12_crates.rs`, `Cargo.lock`. `rsc-genesis` y `network/validator` no cambian.

### Dependencias

`rsc-consensus` depende de `rsc-state` para llamar a `derive_account_id`. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`. `librocksdb.a` sigue en 457632132 bytes.

### No hace

No construye `ConsensusEngine` en el validador. No carga un `ValidatorSet` de producción. No elige el byte de dominio del beacon. No crea bloques, P2P, persistencia de consenso ni finality nueva.

### Tests ejecutados

`cargo test --offline --locked -p rsc-consensus -- --test-threads=1`

`Compiling rsc-consensus`. Finished `test` en 20.27 s. **12/12**, finished in 7.48 s. Doc-tests **1/1**, finished in 1.87 s.

`cargo clippy --offline --locked -p rsc-consensus --all-targets -- -D warnings`

Finished `dev` en 6.90 s. Exit 0.

`cargo test --offline --locked -p rsc-node -- --test-threads=1`

`Compiling rsc-consensus`, `Compiling rsc-node`. Finished `test` en 3 m 53 s. **37/37**, finished in 23.90 s. No aparece `Compiling librocksdb-sys`.

`cargo clippy --offline --locked -p rsc-node --all-targets -- -D warnings`

Finished `dev` en 1 m 24 s. Exit 0.

`cargo test --offline --locked -p rsc-integration-tests -- --test-threads=1`

Finished `test` en 3 m 02 s. El binario de la librería tiene 0 tests. `integration_12_crates` **12/12**, finished in 13.01 s. Doc-tests 0. No aparece `Compiling librocksdb-sys`.

`cargo clippy --offline --locked -p rsc-integration-tests --all-targets -- -D warnings`

Finished `dev` en 3 m 20 s. Exit 0.

### Resultado

✅ PASS en los tres crates.

### Clippy

✅ PASS en `rsc-consensus`, `rsc-node` y `rsc-integration-tests`, con `-D warnings`.

### Errores encontrados

El primer `cargo clippy -p rsc-consensus` terminó con exit 101 a los 64.41 s: `redundant_closure` en `finality.rs` sobre `.map(|id| vote_of(id))`. Quedó `.map(vote_of)`. El comando de cierre, ya con ese cambio, es el que figura arriba.

### Abortos

Ninguno. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. No hubo compilación de `*.cc` de RocksDB.

### Decisiones congeladas

`sign_vote(&mut Vote, &PrivateKey)` firma `vote_bytes` bajo `ConsensusVote`. La verificación de protocolo es `verify_validator_vote`: autor, `key_id(owner_public_key) == owner_key_id`, `derive_account_id(owner_key_id) == AccountId`, y `verify` con `owner_public_key`. `CommitteeMember.public_key` es la copia que usan `aggregate_qc` y `verify_qc`. El QC y `has_quorum` quedan como estaban.

### Estado

✅ CLOSED. El vínculo criptográfico del voto queda verificable. 8.4 sigue BLOCKED: el génesis todavía no entrega `ConsensusConfig`, la lista de validadores ni `consensus_randomness_seed`, y `CryptoDomain` sigue sin el byte del beacon. 8.4A sigue BLOCKED. No se construye el motor del validador.

## TASK 8.4D — Genesis ConsensusConfig + ValidatorSet

### Decisión

El documento de génesis es la fuente de `ConsensusConfig` y de `ValidatorSet`. El binding de 8.4C permanece: `validate_validator_record` sigue exigiendo `owner_public_key.key_id() == owner_key_id` y `derive_account_id(owner_key_id) == id`. Esta tarea no lo reescribe.

`NetworkParameters` incorpora los cinco campos que `ConsensusConfig` ya tiene. `Genesis::consensus_config` copia esos campos al tipo público `rsc_consensus::ConsensusConfig` y llama a `ConsensusConfig::validate`. No hay una segunda configuración.

`Genesis` incorpora `validators: Vec<GenesisValidator>`, una lista distinta de `initial_accounts`. `Genesis::validator_set` construye `ValidatorRecord` y `ValidatorSet` públicos. El modo del set inicial es `NetworkMode::Bootstrap`. La transición a `Distributed` sigue en `new_height`.

### Campos

`NetworkParameters` suma:

- `committee_size: usize`
- `epoch_length: u64`
- `min_validator_weight: u128`
- `max_validator_weight: u128`
- `min_validators_for_distributed: usize`

`GenesisValidator`:

- `account_id: AccountId`
- `owner_key_id: KeyId`
- `owner_public_key: PublicKey`
- `voting_power: u128`
- `eligible: bool`

`GenesisAccount` conserva `account_id`, `owner_key_id` y `native_balance`. El peso no sale del balance. La elegibilidad no sale del balance. Una cuenta que no esté en `validators` no entra en el set. Un validador puede no estar en `initial_accounts`: ninguna API pública exige esa pertenencia.

`participation_score` no entra en el documento. `ValidatorRecord` exige el campo. La conversión escribe `ParticipationScore::default()`, el valor que el tipo ya define.

### Serialización

Serde sigue sin ser el encoding canónico. `owner_public_key` viaja en hex, igual que `owner_key_id`.

`canonical_bytes`, después de `minimum_fee` y antes del conteo de cuentas, añade en big-endian: `committee_size` como `u64`, `epoch_length`, `min_validator_weight`, `max_validator_weight` y `min_validators_for_distributed` como `u64`. Después de las cuentas añade el conteo de validadores y cada registro en el orden del documento: `account_id || owner_key_id || owner_public_key || voting_power || eligible`. Las cuentas siguen ordenándose por `AccountId` solo en esa vista. Los validadores conservan el orden entregado, y ese orden entra en el hash.

### Validaciones

`Genesis::validate` llama a `NetworkParameters::check`, a la validación de cuentas que ya existía, a `ConsensusConfig::validate` y a `validator_set`.

`validator_set` llama a `bounded_weight`. Un peso menor que el mínimo devuelve el error de esa función. Un peso mayor que el máximo sale recortado; si el valor devuelto no es el declarado, el documento se rechaza. Después llama a `validate_validator_record`. Un `AccountId` repetido se rechaza. La lista puede estar vacía: el documento no inventa validadores.

`AccountId::ZERO` con la clave de 32 bytes en cero no es un validador de este documento.

### Conversiones

```text
Genesis
    ↓
NetworkParameters
    ↓
ConsensusConfig::validate
    ↓
ConsensusConfig
```

```text
Genesis.validators
    ↓
ValidatorRecord
    ↓
validate_validator_record
    ↓
ValidatorSet { mode: Bootstrap }
```

### Fuera de esta tarea

`consensus_randomness_seed`, el beacon y una variante de `CryptoDomain` para ese beacon. El mismo struct `Genesis` es el sitio donde la tarea siguiente puede añadir la semilla. No se crea un segundo documento.

No se construye `ConsensusEngine`. No hay votos, bloques, P2P ni persistencia de consenso. `network/validator` no cambia.

### Archivos

`crates/rsc-genesis`, `Cargo.lock`. Los literales de `NetworkParameters` y `Genesis::new` en `network/node/src/main.rs` y en `crates/rsc-integration-tests/tests/integration_12_crates.rs` reciben los campos nuevos y una lista de validadores vacía. `rsc-consensus` y `network/validator` no cambian de lógica.

### APIs utilizadas

`ConsensusConfig::validate`, `rsc_consensus::committee::bounded_weight`, `validate_validator_record`, `ParticipationScore::default`, `NetworkMode::Bootstrap`.

### Tests ejecutados

`cargo test --offline --locked -p rsc-genesis -- --test-threads=1`

`Compiling rsc-consensus`, `Compiling rsc-genesis`. Finished `test` en 41.50 s. **34/34**, finished in 0.12 s. Doc-tests 0. No aparece `Compiling librocksdb-sys`.

`cargo test --offline --locked -p rsc-consensus -- --test-threads=1`

`Compiling rsc-consensus`. Finished `test` en 15.26 s. **12/12**, finished in 9.28 s. Doc-tests **1/1**, finished in 1.82 s.

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

`Compiling rsc-validator`. Finished `test` en 44.73 s. **17/17**, finished in 20.73 s. No aparece `Compiling librocksdb-sys`.

`cargo check --offline --locked --tests -p rsc-node -p rsc-integration-tests`

Finished `dev` en 56.35 s. Compila los llamadores. No reejecuta sus suites.

### Resultado

✅ PASS.

### Clippy

`cargo clippy --offline --locked -p rsc-genesis --all-targets -- -D warnings`  
Finished `dev` en 32.69 s. Exit 0.

`cargo clippy --offline --locked -p rsc-consensus --all-targets -- -D warnings`  
Finished `dev` en 25.92 s. Exit 0.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`  
Finished `dev` en 28.71 s. Exit 0.

### Cargo.lock

`rsc-genesis` depende de `rsc-consensus`. `rsc-state` entra como dev-dependency. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### Errores encontrados

Ninguno en los comandos de cierre.

### Abortos

Ninguno. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. No hubo compilación de `*.cc` de RocksDB.

### Estado

✅ CLOSED. `ConsensusConfig` y `ValidatorSet` salen del génesis. 8.4 sigue BLOCKED: el documento no tiene `consensus_randomness_seed` y `CryptoDomain` no tiene el dominio del beacon. 8.4A sigue BLOCKED. No se construye el motor del validador.

## TASK 8.4E — Consensus Randomness Beacon

### Problema

`ConsensusEngine` recibe un `Hash` de aleatoriedad. Esa entrada no tenía fuente. `Hash::ZERO` no es un beacon. `CryptoDomain` no tenía una variante propia: reutilizar `ConsensusVote` mezclaría el voto con el beacon.

### Fuente del seed

`NetworkParameters.consensus_randomness_seed` es un `rsc_core::Hash` de 32 bytes. Es dato público del documento de génesis. Todos los validadores de la red leen el mismo documento y obtienen el mismo seed. No sale de un archivo de operador ni de la `PrivateKey`.

`Genesis::validate` rechaza `Hash::ZERO`. El struct no tiene `Default` que rellene ese valor. Un JSON sin el campo no deserializa.

### Canonical encoding del seed

Después de `min_validators_for_distributed` y antes de `account_count`, 32 bytes crudos del seed. Entran en `canonical_bytes` y por tanto en `GenesisHash`. Cambiar un byte del seed cambia el hash. Serde lo transporta como hex de `Hash`; ese JSON no es el encoding canónico.

### Dominio

`CryptoDomain::ConsensusBeacon`. `CryptoDomain::id` devuelve **10**. `from_id(10)` lo recupera. Los bytes 0 a 9 siguen ocupados por las variantes que ya existían. `ConsensusVote` permanece en 4.

La separación la hace `hash_domain` / `encode_domain_payload`: `RSC || version_u16_le || domain_id || data`, con `CryptoVersion::V1`.

### Beacon

Función pública `consensus_beacon(seed, epoch, height, parent_block_hash) -> Hash`.

`consensus_beacon_bytes` es la única representación de los inputs, 80 bytes, en este orden:

1. `seed` — 32 bytes crudos
2. `epoch` — `u64` little-endian
3. `height` — `u64` little-endian
4. `parent_block_hash` — 32 bytes crudos

Little-endian porque `encode_domain_payload` ya escribe la versión así y el voto de consenso escribe sus enteros así. No hay strings, `Debug`, `Display`, serde ni el layout de memoria de un struct de Rust.

```text
consensus_beacon = Hash(hash_domain(ConsensusBeacon, V1, consensus_beacon_bytes(...)))
```

La preimagen completa es `RSC || 0x01 0x00 || 0x0A ||` esos 80 bytes.

### Propiedades

El mismo seed, epoch, altura y hash padre producen el mismo `Hash` en cualquier proceso. Un byte distinto en cualquiera de los cuatro inputs produce otro digest. El dominio `ConsensusVote` sobre los mismos 80 bytes produce otro digest. No hace falta un secreto para recomputarlo.

Es un deterministic verifiable beacon. No es aleatoriedad impredecible, no es un VRF y no se llama secure randomness. `select_committee` sigue recibiendo el `Hash` ya calculado. Esta tarea no llama a `ConsensusEngine::new`.

### Archivos

`crates/rsc-crypto/src/hash.rs`, `crates/rsc-consensus/src/committee.rs`, `crates/rsc-consensus/src/lib.rs`, `crates/rsc-genesis/src/lib.rs`. Los literales de prueba de `network/node` y de `rsc-integration-tests` rellenan `consensus_randomness_seed` con un `Hash` distinto de cero. `Cargo.lock` no cambia.

### APIs

`hash_domain`, `encode_domain_payload`, `CryptoDomain::id`, `CryptoVersion::V1`, `consensus_beacon`, `consensus_beacon_bytes`.

### Tests ejecutados

`cargo test --offline --locked -p rsc-genesis -- --test-threads=1`

`Compiling rsc-crypto`, `rsc-chain`, `rsc-state`, `rsc-consensus`, `rsc-genesis`. Finished `test` en 1 m 31 s. **37/37**, finished in 1.36 s. Doc-tests 0.

`cargo test --offline --locked -p rsc-crypto -- --test-threads=1`

Finished `test` en 27.66 s. **43/43**, finished in 2.74 s. Doc-tests 0.

`cargo test --offline --locked -p rsc-consensus -- --test-threads=1`

Finished `test` en 32.16 s. **13/13**, finished in 19.79 s. Doc-tests **1/1**, finished in 1.80 s.

No aparece `Compiling librocksdb-sys`.

`cargo check --offline --locked --tests -p rsc-node -p rsc-integration-tests`

Finished `dev` en 1 m 22 s. Compila los llamadores. No reejecuta sus suites.

### Resultado

✅ PASS.

### Clippy

`cargo clippy --offline --locked -p rsc-genesis --all-targets -- -D warnings`  
Finished `dev` en 42.41 s. Exit 0.

`cargo clippy --offline --locked -p rsc-crypto --all-targets -- -D warnings`  
Finished `dev` en 24.87 s. Exit 0.

`cargo clippy --offline --locked -p rsc-consensus --all-targets -- -D warnings`  
Finished `dev` en 27.63 s. Exit 0.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0.

### Errores encontrados

Ninguno.

### Abortos

Ninguno. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. No hubo compilación de `*.cc` de RocksDB.

### Estado

✅ CLOSED. El seed, el dominio 10 y el beacon quedan definidos y probados. 8.4 queda READY: las cuatro entradas del motor tienen fuente pública. No se construye `ConsensusEngine`. 8.4A sigue BLOCKED como registro de cuando esas fuentes no existían.

## TASK 8.4F — Construir ConsensusEngine del validador

### Decisión

`rsc-validator` construye el `ConsensusEngine` público después de recuperar el tip y el ledger. No hay una segunda configuración, un segundo set ni otro beacon.

La base no guarda `ConsensusConfig`, la lista de validadores ni `consensus_randomness_seed`. El proceso lee el documento de génesis desde `--genesis`. La ruta es configuración del operador. Los números salen del documento. `--genesis` es obligatorio. Un documento cuyo `chain_id` no coincide con `--chain-id` se rechaza con `chain-id del genesis distinto`, el mismo texto que ya usa `rsc-node`.

### Orden

```text
Genesis::validate
    ↓
NetworkParameters::consensus_config
    ↓
Genesis::validator_set
    ↓
find_validator_by_key_id(KeyId local)
    ↓
AccountId local
    ↓
altura del tip y previous_block_hash
    ↓
epoch_of(altura, epoch_length)
    ↓
consensus_beacon(seed, epoch, altura, padre)
    ↓
ConsensusEngine::new
```

`ConsensusEngine::new` fija vista 0 y fase `NewView`. La `PrivateKey` se descarta al obtener el `KeyId`, como ya hacía el proceso. El motor no la recibe. Esta tarea no emite votos, no avanza la altura y no escribe la base.

### Objetivo

Arrancar el `ConsensusEngine` real de `rsc-validator` en el tip ya recuperado, con las APIs públicas de 8.4C, 8.4D y 8.4E.

### Archivos

`network/validator/src/main.rs`, `network/validator/Cargo.toml`, `Cargo.lock`. `rsc-consensus` y `rsc-genesis` no cambian.

### Implementación

`Config` exige `--genesis`. `open_engine` lee el JSON, llama `Genesis::validate`, compara `chain_id` con `--chain-id`, toma `consensus_config()` y `validator_set()`, resuelve el `AccountId` local con `find_validator_by_key_id`, lee altura y `previous_block_hash` del tip, calcula `epoch_of` y `consensus_beacon`, y llama `ConsensusEngine::new`. `Recovered` guarda ese motor. `main` lee `engine.state().height` y sigue imprimiendo solo el `KeyId` en 64 hex minúsculas.

Los números del documento de prueba viven en el helper de test. El proceso no tiene defaults de comité, epoch, pesos ni seed.

### Código

`Genesis::validate`, `NetworkParameters::consensus_config`, `Genesis::validator_set`, `find_validator_by_key_id`, `committee::epoch_of`, `consensus_beacon`, `ConsensusEngine::new`.

### Dependencias

`rsc-validator` depende de `rsc-consensus`, `rsc-genesis` y `serde_json`. `tempfile` sigue en dev-dependencies.

### No hace

No emite votos, no avanza la altura, no escribe la base, no hace P2P, no crea bloques ni QC. No copia los números de `rsc-node` ni los de los tests de `rsc-consensus`. No crea otra `ConsensusConfig`, otro `ValidatorSet` ni otra función de beacon. No pasa `Hash::ZERO` como seed. En altura 0 el padre del bloque génesis es el hash cero del header; ese valor entra como padre, y el seed del documento es otro `Hash`. La `PrivateKey` se descarta al obtener el `KeyId`. `ConsensusEngine::new` no la recibe.

### Tests

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

El primer `--locked` sale 101 en 4.60 s: `cannot update the lock file ... because --locked was passed`. El lock no tenía `rsc-consensus` ni `serde_json` en `rsc-validator`.

`cargo metadata --offline` actualiza esas dependencias en 5.94 s. `rocksdb` permanece 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

La compilación siguiente sale 101 en 8.66 s: `unwrap_err` exige `Debug` en `Recovered`, y `ConsensusEngine` no lo implementa. El helper de test `failure` lee el `Err` sin ese `Debug`.

El comando oficial, ya con el lock al día, compila `rsc-validator` y termina `Finished test` en 46.24 s. **19/19**, finished in 25.78 s. No aparece `Compiling librocksdb-sys`. `librocksdb.a` sigue en 457632132 bytes.

Cubren el motor en altura 0 (vista 0, `NewView`, epoch 0, modo `Bootstrap`, miembro local = `derive_account_id`, comité igual a `select_committee` del beacon) y en altura 1 (epoch 0, comité con el padre real del bloque). Rechazan clave fuera del set (`ineligible validator`), seed `Hash::ZERO` (`invalid genesis parameters`) y `chain_id` distinto (`chain-id del genesis distinto`). Siguen los rechazos de recuperación, de identidad y de flags, incluidos `falta genesis`, `genesis vacío` y `genesis repetido`. `--listen` sigue siendo argumento desconocido.

### Resultado

✅ PASS.

### Clippy

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

`Checking rsc-ledger`, `Checking rsc-validator`. Finished `dev` en 9.42 s. Exit 0.

### Errores reales

El lock estaba atrasado respecto de `Cargo.toml`. `Recovered` no puede derivar `Debug` porque el motor no lo implementa. Ambos quedan resueltos en esta tarea.

### Abortos

Ninguno. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. No hubo compilación de `*.cc` de RocksDB.

### Decisiones congeladas

`--genesis` es obligatorio porque la base no guarda la configuración, el set ni el seed. La ruta es del operador; los valores son del documento. El texto de `chain_id` distinto es `chain-id del genesis distinto`. El beacon de arranque es `consensus_beacon` con el seed del documento, el epoch de `epoch_of`, la altura del tip y el `previous_block_hash` de ese bloque. El modo inicial es el `Bootstrap` que ya escribe `Genesis::validator_set`. Esta tarea no ejecuta la transición a `Distributed`.

### Estado

✅ CLOSED. `ConsensusEngine` arranca en el tip recuperado. TASK 8.4 queda CLOSED: el motor del validador existe. TASK 8.4A sigue BLOCKED como registro de cuando esas fuentes no existían. Fase 8 sigue OPEN.

## TASK 8.5 — Primer Prepare vote del validator

### Objetivo

Sobre el `ConsensusEngine` ya construido, aceptar el tip persistido como propuesta y dejar en memoria un voto `Prepare` firmado con la `PrivateKey` real y verificado con la `PublicKey` registrada. El ciclo termina ahí.

### APIs públicas utilizadas

Las mismas que `network/node` ya llama, sin copiar su quorum:

- `ChainStore::get_canonical_tip` y `get_block`
- `ConsensusEngine::receive_proposal(block, None)`
- `ConsensusEngine::create_vote(VotePhase::Prepare, &PrivateKey)`
- `create_vote` llama `sign_vote(vote, &PrivateKey)` bajo `CryptoDomain::ConsensusVote`
- `verify_validator_vote(record, vote)` comprueba autor, `validate_validator_record` y la firma contra `owner_public_key`

`vote_bytes` sigue siendo un byte de fase, `epoch` `u64` LE, `height` `u64` LE, `view` `u64` LE y 32 bytes del hash. El dominio es `ConsensusVote`.

### Identidad local

```text
PrivateKey del --key-file
    ↓
key_id()
    ↓
find_validator_by_key_id
    ↓
ValidatorRecord
    ↓
AccountId del voto
```

`create_vote` rechaza la clave si `public_key()` o `key_id()` no coinciden con ese registro. No se reconstruye una `PrivateKey` desde el `AccountId`. `AccountId::ZERO` no se usa como validador: el registro del documento no es esa cuenta.

### Propuesta utilizada

El bloque del tip persistido. `receive_proposal` exige la misma altura que el motor y `header.proposer == select_proposer` de la vista. El contexto de validación es `None`, igual que `propose_tip` del nodo. El proceso no fabrica un bloque hijo y no reescribe el header. Un tip cuyo proposer no es el de la vista se rechaza con `invalid proposer`.

El fixture de prueba escribe ese proposer en el bloque que ya persiste antes del arranque: el `AccountId` del único `ValidatorRecord`. Con un miembro, `select_proposer` devuelve ese id. El proceso solo lo lee.

### Altura, epoch, view y hash

La altura y el epoch son los que `ConsensusEngine::new` fijó desde el tip. La vista queda en 0. El hash del voto es `block_hash` del tip, el mismo que `receive_proposal` guarda en `proposal`.

### Recorrido de firma

```text
receive_proposal(tip, None) → ProposalAccepted
    ↓
create_vote(Prepare, &PrivateKey real)
    ↓
sign_vote
    ↓
verify_validator_vote(registro, voto)
```

La `PrivateKey` se descarta después de firmar. No entra en `Recovered`, ni en stdout, ni en el texto de error.

### Validación

`verify_validator_vote` acepta el voto. Una firma alterada devuelve `invalid signature`. El motor queda en fase `Propose`, con `proposal` igual al tip, `locked` vacío y la misma altura. No se llama `receive_vote`, así que no hay quorum ni QC.

### Qué NO hace

No hay quorum, QC, PreCommit, Commit, `FinalityProof`, `new_height`, bloque nuevo, transición de estado, append de ledger, escritura de tip, gossip ni bucle. Este proceso no tiene mempool. No se persiste el estado de consenso.

### Invariantes

`engine.height` sigue igual a la altura del tip canónico. `vote.validator` es el `AccountId` del registro local. La firma verifica contra `owner_public_key`. El layout de `vote_bytes` y el dominio `ConsensusVote` no cambian.

### Tests

El arranque produce el voto. El registro local coincide con el `KeyId` del archivo. El `AccountId`, la altura, el epoch, la vista y el hash del voto coinciden con el motor y el tip. La firma es la de `sign_vote` con la `PrivateKey` del archivo y verifica con la clave pública del registro. Una firma alterada es `invalid signature`. El tip, el estado, el ledger y el almacén quedan iguales. La altura no avanza y no aparece un bloque nuevo. Un proposer distinto se rechaza. La salida sigue siendo solo el `KeyId`.

### Criterio de cierre

`cargo test --offline --locked -p rsc-validator -- --test-threads=1` y `cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`. Sin `cargo clean` y sin cambiar `CARGO_TARGET_DIR`. `rsc-consensus` y `rsc-crypto` no se modifican.

### Archivos

`network/validator/src/main.rs`. `Cargo.lock` no cambia. `rsc-consensus`, `rsc-crypto` y `rsc-chain` no cambian.

### Implementación

`bootstrap` conserva la `PrivateKey` del archivo hasta firmar. `open_engine` devuelve el motor y el `ValidatorRecord` local. `prepare_vote` lee el tip, llama `receive_proposal(&block, None)`, exige `ProposalAccepted`, llama `create_vote(Prepare, &key)` y `verify_validator_vote`. Después descarta la clave. `Recovered` guarda el voto en memoria. `main` lee `vote.height` y sigue imprimiendo solo el `KeyId`.

El fixture `seed_chain` escribe el proposer del bloque. El arranque de éxito usa el `AccountId` del validador de la clave `0x11`. Un tip con `AccountId::ZERO` como proposer se rechaza.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

`Compiling rsc-validator`. Finished `test` en 42.75 s. **21/21**, finished in 31.93 s.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

`Checking rsc-validator`. Finished `dev` en 18.37 s. Exit 0.

No aparece `Compiling librocksdb-sys`. `librocksdb.a` sigue en 457632132 bytes.

### Resultado

✅ PASS.

### Errores

Ninguno en la ejecución de cierre.

### Correcciones

Ninguna después de la primera compilación de esta tarea.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### Compilación de RocksDB

No hubo. No se compiló ningún `*.cc`.

### Ejecuciones abortadas

Ninguna. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Estado

✅ CLOSED. El proceso deja un Prepare verificado en memoria, en la altura del tip. No hay QC ni bloque nuevo. Fase 8 sigue OPEN.

## TASK 8.6 — Quorum + Prepare QC del validator

### Objetivo

Pasar el Prepare de 8.5 por el quorum de `rsc-consensus` y dejar en memoria el `QuorumCertificate` de esa fase, verificado. El ciclo termina en fase `Prepare`.

### APIs reales utilizadas

- `ConsensusEngine::receive_vote`
- `has_quorum`: `3 * poder > 2 * total`, dentro de `aggregate_qc` y de `verify_qc`
- `finality::aggregate_qc`
- `verify_qc`

No hay otra fórmula ni otro certificado. `SignatureBundle` no cambia.

### ValidatorSet

El set es el de `Genesis::validator_set`, el mismo que ya tiene el motor. El comité es el de `engine.state().committee`. No se copia el comité de `rsc-node` y no se cambia `ConsensusConfig`. Con un validador, el poder del comité es el de ese registro. La fórmula existente da quorum cuando ese poder es positivo: `3 * P > 2 * P`.

### Validator local y voto

El voto es el que `create_vote(Prepare, &PrivateKey)` ya firmó. `receive_vote` lo busca por `vote.validator` en el set, exige `owner_public_key.key_id() == owner_key_id` y verifica la firma. No se reconstruye la clave desde el `AccountId`.

### Reglas de verificación que ya aplica el crate

`receive_vote` rechaza epoch, altura o vista distintos con `wrong view`. Un autor ausente del set, o una firma que no verifica, es `invalid signature`. El hash forma parte de `vote_bytes`: cambiarlo sin volver a firmar cae en `invalid signature`. El mismo validador, la misma fase y otro hash, firmado con la misma `PrivateKey`, es `Equivocation` y no sustituye el voto guardado. Recibir otra vez el mismo voto devuelve `ProposalAccepted` y no lo reemplaza.

`aggregate_qc` ignora un voto cuya fase no es la pedida. Un PreCommit no entra en el QC de Prepare. El proceso no llama `receive_vote` con otra fase: esa API, si el quorum se cumple, abriría PreCommit. Esta tarea no la abre.

### aggregate_qc y verify_qc

Sobre el Prepare ya aceptado, `aggregate_qc` arma el certificado y `verify_qc` lo comprueba contra el comité. El QC conserva epoch, fase `Prepare`, altura, vista, hash del bloque y las firmas de los votantes. Una segunda llamada a `aggregate_qc` con el mismo voto produce el mismo certificado.

### Estado del engine

Después del quorum la fase es `Prepare`, `locked` sigue vacío, la altura no cambia y `proposal` sigue siendo el hash del tip.

### Invariantes

`engine.height` sigue igual a la altura del tip. El QC no se escribe. El tip, State y Ledger quedan iguales. Este proceso no tiene mempool.

### Qué NO hace

No hay PreCommit, Commit, `FinalityProof`, `new_height`, bloque nuevo, transición de estado, append de ledger, persistencia ni P2P.

### Tests

Quorum `QuorumReached(Prepare)`, `aggregate_qc` y `verify_qc` aceptan, la segunda agregación coincide, una firma alterada es `invalid signature`, un autor fuera del set es `invalid signature`, epoch, altura o vista distintos son `wrong view`, un hash distinto firmado con la misma clave es equivocación y no reemplaza el voto, un PreCommit no produce QC de Prepare, y el almacén no cambia.

### Criterio de cierre

`cargo test --offline --locked -p rsc-validator -- --test-threads=1` y `cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`. Sin `cargo clean` y sin cambiar `CARGO_TARGET_DIR`. `rsc-consensus` no se modifica.

### Archivos

`network/validator/src/main.rs`. `Cargo.lock` no cambia. `rsc-consensus` no cambia.

### Implementación

`prepare_quorum` llama `receive_vote` con el Prepare de 8.5 y exige `QuorumReached(Prepare)`. Después llama `aggregate_qc` y `verify_qc` sobre ese voto y el comité del motor. `Recovered` guarda el QC en memoria. `main` lee `qc.height` y sigue imprimiendo solo el `KeyId`. La fase queda en `Prepare` y `locked` vacío.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

`Compiling rsc-validator`. Finished `test` en 39.25 s. **22/22**, finished in 47.66 s.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

`Checking rsc-validator`. Finished `dev` en 8.05 s. Exit 0.

No aparece `Compiling librocksdb-sys`. `librocksdb.a` sigue en 457632132 bytes.

### Resultado

✅ PASS.

### Errores

Ninguno en la ejecución de cierre.

### Correcciones

Ninguna después de la primera compilación de esta tarea.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

No hubo compilación. No se compiló ningún `*.cc`.

### Abortos

Ninguno. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Estado

✅ CLOSED. El Prepare QC queda en memoria y `verify_qc` lo acepta. La fase del motor es `Prepare`. No hay PreCommit ni bloque nuevo. Fase 8 sigue OPEN.

## TASK 8.7 — PreCommit del validator

### Objetivo

Firmar el PreCommit de la misma propuesta que ya tiene Prepare QC, pasar ese voto por el quorum existente y dejar el PreCommit QC en memoria. El motor termina en fase `PreCommit`, con el lock en el hash del tip.

### Prepare QC usado

El certificado que 8.6 ya verificó. El PreCommit exige que ese QC tenga fase `Prepare` y el mismo epoch, altura, vista y hash que el voto Prepare. No se fabrica otra propuesta.

### Recorrido

```text
Prepare QC
    ↓
create_vote(PreCommit, &PrivateKey)
    ↓
sign_vote bajo ConsensusVote
    ↓
verify_validator_vote
    ↓
receive_vote
    ↓
QuorumReached(PreCommit)
    ↓
aggregate_qc
    ↓
verify_qc
```

`create_vote` no cambia la fase. El lock lo escribe `receive_vote` al alcanzar el quorum de PreCommit: `locked` pasa a ser el hash de la propuesta.

### Identidad y firma

La misma `PrivateKey` del archivo, el mismo `ValidatorRecord` y el mismo `AccountId`. No se usa `AccountId::ZERO`. El layout de `vote_bytes` y el dominio `ConsensusVote` no cambian. Epoch, altura, vista y hash son los del Prepare.

### Recepción, quorum y QC

`receive_vote` aplica las comprobaciones del crate. `has_quorum` sigue siendo `3 * poder > 2 * total`, dentro de `aggregate_qc` y `verify_qc`. El PreCommit QC conserva epoch, fase `PreCommit`, altura, vista, hash y `SignatureBundle`. Una segunda agregación del mismo voto produce el mismo certificado.

Repetir el mismo PreCommit devuelve `ProposalAccepted` y no lo reemplaza. Una firma alterada o un autor fuera del set es `invalid signature`. Epoch, altura o vista distintos son `wrong view`. Otro hash, firmado con la misma clave, es equivocación y no sustituye el voto. `aggregate_qc` de fase PreCommit ignora un voto Commit. El proceso no llama `receive_vote` con Commit.

### Estado final del engine

Fase `PreCommit`. `locked` es el hash del tip. La altura no cambia. El Prepare QC y el PreCommit QC permanecen en memoria.

### Invariantes

El tip, State y Ledger quedan iguales. Este proceso no tiene mempool. El PreCommit QC no se persiste.

### Qué NO hace

No hay voto Commit, QC de Commit, `FinalityProof`, `new_height`, transición de estado, append de ledger, bloque nuevo, persistencia ni P2P.

### Tests

El PreCommit sale del Prepare QC, verifica con la clave registrada, alcanza `QuorumReached(PreCommit)`, `aggregate_qc` y `verify_qc` aceptan, la segunda agregación coincide, y los rechazos de firma, autor, vista y hash respetan el crate. El almacén y la altura no cambian. El lock es el hash del tip.

### Criterio de cierre

`cargo test --offline --locked -p rsc-validator -- --test-threads=1` y `cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`. Sin `cargo clean` y sin cambiar `CARGO_TARGET_DIR`. `rsc-consensus` no se modifica.

### Archivos

`network/validator/src/main.rs`. `Cargo.lock` no cambia. `rsc-consensus` no cambia.

### Implementación

`precommit_vote` exige que el Prepare QC coincida con el voto Prepare y llama `create_vote(PreCommit, &PrivateKey)`. `verify_validator_vote` comprueba la firma. `precommit_quorum` llama `receive_vote`, exige `QuorumReached(PreCommit)`, y luego `aggregate_qc` y `verify_qc`. `Recovered` guarda el voto y el QC de PreCommit en memoria. `main` lee sus alturas y sigue imprimiendo solo el `KeyId`.

### APIs

`create_vote`, `sign_vote`, `verify_validator_vote`, `receive_vote`, `has_quorum`, `aggregate_qc`, `verify_qc`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

La primera corrida salió 101 en 3 m 49 s: el `c++` del sistema rechazó `-std=c++20` al compilar `librocksdb-sys`. No había `librocksdb.a` en el caché nuevo.

La corrida de cierre usa `clang++-18`. Compila `librocksdb-sys` porque ese archivo no estaba. Finished `test` en 72 m 14 s. **23/23**, finished in 7.11 s. `librocksdb.a` queda en 457632132 bytes.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

Una corrida intermedia empezó a compilar `*.cc` en otro directorio de salida mientras el `.a` ya existía. Se detuvo. La corrida de cierre fija `ROCKSDB_LIB_DIR` en el directorio de ese `.a`. `Compiling librocksdb-sys` enlaza el archivo existente. Finished `dev` en 4 m 59 s. Exit 0.

### Resultado

✅ PASS.

### Errores

El compilador `c++` del sistema no acepta `-std=c++20`. Esa corrida no es el cierre.

### Correcciones

Ninguna en el código del validador. La corrida de cierre usa `clang-18` y, en Clippy, el `librocksdb.a` ya construido.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite de cierre compiló los `*.cc` porque el archivo estático no estaba en el caché. Clippy no los volvió a compilar.

### Abortos

La corrida con `c++` del sistema, exit 101. La corrida de Clippy que recompilaba RocksDB en otro directorio, detenida antes de terminar. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Estado

✅ CLOSED. El motor queda en fase `PreCommit`, con `locked` en el hash del tip. El Prepare QC y el PreCommit QC están en memoria. No hay Commit ni bloque nuevo. Fase 8 sigue OPEN.

## TASK 8.8 — Commit + FinalityProof del validator

### Objetivo

Firmar el Commit de la misma propuesta que ya tiene PreCommit QC, construir el Commit QC y dejar en memoria la `FinalityProof` que devuelve `BlockFinalized`. La altura no cambia.

### PreCommit QC

El certificado que 8.7 ya verificó. El Commit exige fase `PreCommit` y el mismo epoch, altura, vista y hash. No se acepta otra propuesta.

### Commit vote y firma

`create_vote(Commit, &PrivateKey)` firma bajo `ConsensusVote`. `vote_bytes` no cambia. `verify_validator_vote` usa el `ValidatorRecord` local. El `AccountId` no es `AccountId::ZERO`.

### Quorum y Commit QC

`receive_vote` del Commit, con quorum, no devuelve `QuorumReached`: el crate arma los tres QC, llama `verify_commit_chain` y devuelve `BlockFinalized`. El proceso vuelve a llamar `aggregate_qc` y `verify_qc` sobre ese voto. El QC tiene fase `Commit`, el mismo epoch, altura, vista y hash, y el `SignatureBundle` que produce `aggregate_qc`. `has_quorum` sigue siendo `3 * poder > 2 * total`.

### verify_commit_chain y FinalityProof

`verify_commit_chain` comprueba Prepare, PreCommit y Commit contra el comité. La prueba queda en `Recovered`. No se escribe y no se publica por RPC.

### Estado del engine

Fase `Decide`. `locked` sigue en el hash del tip. La altura no cambia. A partir de `Decide`, `receive_vote` rechaza cualquier voto nuevo con `invalid phase`, incluido el mismo Commit. No sustituye el voto guardado. Una firma alterada sigue siendo `invalid signature` en `verify_validator_vote`. Un certificado que no coincide con la prueba falla `verify_commit_chain`. `aggregate_qc` de fase Commit ignora un voto de otra fase.

### Invariantes

Tip, State, Ledger y ChainStore quedan iguales. Este proceso no tiene mempool. La prueba no se persiste.

### Qué NO hace

No llama `new_height` ni `apply_finalized`. No hay transición de estado, append de ledger, bloque nuevo, persistencia ni P2P.

### Tests

El Commit sale del PreCommit QC, la firma verifica, `receive_vote` devuelve `BlockFinalized`, `aggregate_qc` y `verify_qc` aceptan el Commit QC, `verify_commit_chain` acepta la prueba y rechaza una alterada. Un voto posterior es `invalid phase`. El almacén y la altura no cambian.

### Criterio de cierre

`cargo test --offline --locked -p rsc-validator -- --test-threads=1` y `cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`. Sin `cargo clean` y sin cambiar `CARGO_TARGET_DIR`. Se reutiliza el `librocksdb.a` ya construido. `rsc-consensus` no se modifica.

### Archivos

`network/validator/src/main.rs`. `Cargo.lock` no cambia. `rsc-consensus` no cambia.

### Implementación

`commit_vote` exige que el PreCommit QC coincida con el voto PreCommit y llama `create_vote(Commit, &PrivateKey)`. `verify_validator_vote` comprueba la firma. `commit_finality` llama `receive_vote`, exige `BlockFinalized`, y luego `aggregate_qc`, `verify_qc` y `verify_commit_chain`. `Recovered` guarda el voto Commit, el QC y la prueba en memoria. `main` lee sus alturas y sigue imprimiendo solo el `KeyId`.

### APIs

`create_vote`, `sign_vote`, `verify_validator_vote`, `receive_vote`, `has_quorum`, `aggregate_qc`, `verify_qc`, `verify_commit_chain`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

La primera corrida salió 101 en 44.53 s: `cannot find value real` en un test de Prepare. `Compiling librocksdb-sys` enlazó el `.a` existente; no hubo `*.cc`.

La corrida de cierre compila solo `rsc-validator`. Finished `test` en 12.38 s. **24/24**, finished in 33.67 s.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

`Checking rsc-validator`. Finished `dev` en 14.30 s. Exit 0. No recompila RocksDB.

### Resultado

✅ PASS.

### Errores

El test de Prepare quedó sin la variable `real` al ajustar los votos posteriores a `Decide`.

### Correcciones

Se quitó la firma de un voto que `aggregate_qc` ignora por la fase. No cambia el protocolo.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

Se reutilizó `librocksdb.a` de 457632132 bytes con `ROCKSDB_LIB_DIR`. No hubo compilación de `*.cc`.

### Abortos

La corrida de compilación con `real` fuera de alcance, exit 101. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Estado

✅ CLOSED. El motor queda en fase `Decide`, con `locked` en el hash del tip. `verify_commit_chain` acepta la `FinalityProof` en memoria. `receive_vote` devolvió `BlockFinalized`. No hay `new_height` ni bloque nuevo. Fase 8 sigue OPEN.

## TASK 8.9 — Persistencia y recuperación de FinalityProof

### Objetivo

La `FinalityProof` que el validador ya verifica sobrevive a cerrar y volver a abrir la misma base. El segundo arranque la lee, vuelve a llamar `verify_commit_chain` y la compara con la prueba del ciclo.

### APIs reales

`RocksDbChainStore::database` devuelve el `Arc<RscDatabase>` ya abierto. `RscDatabase::put` y `get` escriben en `ColumnFamily::Default`. No se usa el crate `rocksdb` desde `rsc-validator`. No se añade una column family.

`SignatureBundle::to_bytes` y `from_bytes` son el encuadre público del haz: conteo `u32` little-endian y cada firma de 64 bytes. `FinalityProof` y `QuorumCertificate` no tienen un codec en `rsc-consensus`. El encuadre de los tres QC es el que la suite de integración ya escribe con `RscDatabase::put`: longitud del cuerpo, hash, altura, vista, epoch, fase, poder, votantes y el haz. La clave del proceso es `finality`. La clave `integ/finality` sigue siendo solo de esa suite.

Al decodificar, el hash, la altura y la vista de la prueba salen del QC de Prepare, que `verify_commit_chain` exige iguales en los tres certificados.

### Recorrido

El primer arranque termina el ciclo y, si la clave no existe, guarda la prueba. El siguiente arranque lee esa clave, verifica la prueba contra el comité y exige que sea igual a la que el ciclo acaba de producir. La altura no avanza. El tip no se reescribe.

### Invariantes

Tras reabrir: el hash y la altura del tip, la raíz de estado y la raíz del ledger siguen iguales. El motor queda en `Decide`, en la misma altura, con el mismo lock. No aparece un bloque nuevo.

### Qué NO hace

No cambia `has_quorum`, el consenso ni `SignatureBundle`. No añade criptografía, P2P, `rsc_getFinality` ni producción de bloques. No llama `new_height`.

### Tests

Un segundo `bootstrap` recupera la misma prueba y `verify_commit_chain` la acepta. Una prueba con otro hash de bloque es `invalid finality`. Una firma alterada dentro del QC es `invalid signature`. El tip, el estado y el ledger no cambian.

### Criterio de cierre

`cargo test --offline --locked -p rsc-validator -- --test-threads=1` y `cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`. Sin `cargo clean` y sin cambiar `CARGO_TARGET_DIR`. Se reutiliza el `librocksdb.a` ya construido.

### Archivos

`network/validator/src/main.rs`. `Cargo.lock` no cambia. `rsc-consensus` y `rsc-storage` no cambian.

### Implementación

`recover_proof` lee `finality` en `ColumnFamily::Default`. Si no está, escribe la prueba del ciclo con `RscDatabase::put`. Si está, la decodifica, llama `verify_commit_chain` y exige que sea igual a la prueba recién producida. El proceso usa `RocksDbChainStore::database`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

La primera corrida compiló `rsc-validator` y terminó `Finished test` en 18.43 s. **24 pasaron y 1 falló**, finished in 21.69 s. Exit 101. `finality_survives_reopen` reabría la base mientras `Recovered` aún la tenía: `lock hold by current process`.

La corrida de cierre, ya soltando el proceso antes de leer la clave, compila `rsc-validator`. Finished `test` en 22.69 s. **25/25**, finished in 29.61 s.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

`Checking rsc-validator`. Finished `dev` en 11.25 s. Exit 0.

### Resultado

✅ PASS.

### Errores

El test de reapertura abría la base con el motor anterior todavía vivo.

### Correcciones

La lectura de `finality` queda después de `drop`. El protocolo no cambia.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

Se reutilizó `librocksdb.a` de 457632132 bytes con `ROCKSDB_LIB_DIR`. No hubo compilación de `*.cc`.

### Abortos

Ninguno. La corrida de 24/25 es un fallo de test, no un aborto de compilación. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Estado

✅ CLOSED. La `FinalityProof` se recupera de `ColumnFamily::Default` y `verify_commit_chain` la acepta. El tip, el estado y el ledger no cambian. El motor sigue en `Decide` a la misma altura. Fase 8 sigue OPEN.

## TASK 8.10 — Exponer FinalityProof por RPC

### Objetivo

`rsc_getFinality` devuelve la `FinalityProof` que el validador ya guardó en la clave `finality`, después de decodificarla y de `verify_commit_chain`. Si la clave no está, la respuesta sigue siendo el error que la API ya define.

### APIs reales

`methods::dispatch` enruta `rsc_getFinality` a `ConsensusService::finality`. El resultado público es `RpcFinality`. `map_service_error` convierte `ServiceError::ConsensusUnavailable` en `RpcError::ServiceUnavailable` (`service unavailable`, código `-32005`). `ServiceError::Internal` sigue en `RpcError::InternalError`. Los mensajes públicos de `RpcError` no incorporan el `Display` de consenso.

`RocksDbChainStore::database` y `RscDatabase::get` leen `ColumnFamily::Default`. La clave sigue siendo `finality`. `verify_commit_chain` no cambia. `SignatureBundle::to_bytes` sigue dentro de `encode_qc`. `encode_hex` publica los bytes en hexadecimal minúsculo.

`FinalityProof` no tiene campo `epoch`. El epoch de la respuesta es `prepare.epoch`, el que `verify_commit_chain` exige en los tres QC.

### Formato de respuesta

`RpcFinality`:

- `finalized_hash`: hexadecimal del `block_hash`
- `finalized_height`: `height`
- `epoch`: `prepare.epoch`
- `view`: `view`
- `prepare`, `precommit`, `commit`: hexadecimal de cada QC con el encuadre ya persistido (longitud, cuerpo y `SignatureBundle::to_bytes`)

### Origen

El arranque llama `read_finality` sobre la clave que `recover_proof` dejó escrita. Esa función hace `get`, `decode_proof` y `verify_commit_chain`, y arma el `RpcFinality`. El test construye el `RpcContext` y llama `dispatch("rsc_getFinality")`. El binario no abre un listener.

### Cuando no existe

`get` devuelve `None`. `read_finality` responde el mensaje público `service unavailable`. El servicio lo mapea a `ServiceError::ConsensusUnavailable` y `dispatch` devuelve `RpcError::ServiceUnavailable`.

### Pruebas negativas

Bytes que no decodifican: `invalid finality`, y `dispatch` no es éxito. Una firma alterada dentro del QC de Commit: `invalid signature`, y `dispatch` no es éxito. Un Commit QC de otro hash de bloque, con firma válida de ese otro hash: `invalid finality`, y `dispatch` no es éxito. Esos tres rechazos llegan a RPC como `ServiceError::Internal`.

### Invariantes

La consulta no escribe la clave, no cambia el tip, la altura, la raíz de estado ni la raíz del ledger, y deja el motor en la misma altura y en `Decide`. No produce un bloque ni avanza la altura.

`rsc-node` sigue respondiendo `ServiceUnavailable`: no lee la clave `finality`.

### Qué NO hace

No cambia `verify_commit_chain`, `QuorumCertificate`, `SignatureBundle` ni `has_quorum`. No añade criptografía ni P2P. No abre una segunda clave. No produce bloques. No enlaza el crate `rocksdb` desde `rsc-validator`.

### Archivos

`network/validator/src/main.rs`. `network/validator/Cargo.toml`. `crates/rsc-rpc/src/lib.rs`. `crates/rsc-rpc/src/methods.rs`. `crates/rsc-integration-tests/tests/integration_12_crates.rs`. `Cargo.lock`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

Finished `test` en 3m 44s. **26/26**, finished in 12.57s. Exit 0.

`cargo test --offline --locked -p rsc-rpc -- --test-threads=1`

Finished `test` en 0.52s. **81/81**, finished in 0.07s. Doc-tests: 0, finished in 0.00s. Exit 0.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

Finished `dev` en 2m 56s. Exit 0.

`cargo clippy --offline --locked -p rsc-rpc --all-targets -- -D warnings`

Finished `dev` en 2m 20s. Exit 0.

`cargo test --offline --locked -p rsc-integration-tests -- --test-threads=1`

El literal de `RpcFinality` de esa suite tuvo que ganar los tres campos. Finished `test` en 23m 32s. La librería tiene 0 tests, finished in 0.00s. `integration_12_crates`: **12/12**, finished in 5.47s. Doc-tests: 0, finished in 0.00s. Exit 0.

`cargo check --offline --locked -p rsc-node`

Finished `dev` en 11m 25s. Exit 0. La suite de `rsc-node` no se ejecutó: `NodeRpc::finality` sigue devolviendo `ServiceError::ConsensusUnavailable`.

### Resultado

✅ PASS.

### Cargo.lock

`rsc-validator` declara `rsc-rpc` y `tokio`. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

`Compiling librocksdb-sys` enlazó el `librocksdb.a` de 457632132 bytes. No hubo compilación de `*.cc` de RocksDB. En la suite de integración, `aws-lc-sys` compiló fuentes C porque ese objeto no estaba en este caché. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. `rsc_getFinality` devuelve la prueba persistida y `verify_commit_chain` la acepta. Sin la clave, la respuesta es `ServiceUnavailable`. Una prueba corrupta, una firma alterada o un hash distinto no producen éxito. El motor sigue en `Decide`. Fase 8 sigue OPEN.

## TASK 8.11 — Validator real-process RPC smoke + restart

### Objetivo

El binario `rsc-validator` sirve la `FinalityProof` persistida por HTTP/JSON-RPC. Al cerrarlo y volver a abrirlo sobre la misma base, `rsc_getFinality` devuelve la misma prueba.

### Proceso

El arranque sigue el ciclo ya existente hasta `Decide` y la clave `finality`. Después imprime el `KeyId` y llama `RpcServer::run` con `RpcConfig::default`. El apagado es la señal que ese servidor ya espera (`SIGTERM`).

El comando del smoke es:

`rsc-validator --key-file <clave> --data-dir <base> --network-id devnet --chain-id 3 --genesis <genesis.json>`

### Endpoint

`POST http://127.0.0.1:8545/rpc`

Cuerpo: `{"jsonrpc":"2.0","method":"rsc_getFinality","id":1}`

El bind es `RpcConfig::default`. La ruta es la de `RpcServer`: `POST /rpc`.

### Respuesta

Antes del restart y después, `result` trae el mismo `finalized_hash`, `finalized_height`, `epoch`, `view`, `prepare`, `precommit` y `commit`. `verify_commit_chain` acepta la prueba decodificada de esa respuesta. El motor del arranque queda en `Decide`, en la altura de la prueba.

Entre los dos procesos, el hash y la altura del tip, los bytes del bloque, la raíz de estado, la raíz del ledger, los bytes de `finality` y el `commit_count` quedan iguales. No hay bloque en la altura siguiente.

### Pruebas negativas

Con el proceso escuchando, borrar la clave `finality` hace que el POST devuelva el error `-32005` `service unavailable`, sin `result`.

Una prueba corrupta, un Commit QC de otro hash y una firma alterada hacen que el POST no traiga `result`. `read_finality` responde `invalid finality`, `invalid finality` e `invalid signature`. Un arranque posterior del mismo binario sobre esa base sale con código 1 y escribe ese texto en stderr. El tip y la altura no cambian.

### APIs

`RpcServer::new`, `RpcServer::run`, `RpcConfig::default`, `RpcContext::new`, `ConsensusService::finality`, `read_finality`, `RscDatabase::get` / `put` / `delete` en `ColumnFamily::Default`. El mensaje público de ausencia es `RpcError::ServiceUnavailable`.

### Qué NO hace

No cambia el consenso, `has_quorum`, `SignatureBundle`, `QuorumCertificate` ni el encuadre persistido. No añade criptografía ni P2P. No toca `rsc-node`: su `rsc_getFinality` sigue en `ServiceUnavailable`. No produce un bloque ni avanza la altura.

### Archivos

`network/validator/src/main.rs`. `network/validator/Cargo.toml` (`tokio` en las dependencias del binario, con `rt-multi-thread`, `net` y `time`). `network/validator/tests/process_bin.rs`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

Finished `test` en 34.75 s. **27/27**, finished in 35.43 s. `process_bin`: **1/1**, finished in 0.24 s. Exit 0.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

Finished `dev` en 5.60 s. Exit 0.

`cargo test --offline --locked -p rsc-rpc -- --test-threads=1`

Finished `test` en 0.83 s. **81/81**, finished in 0.15 s. Doc-tests: 0, finished in 0.00 s. Exit 0.

`cargo clippy --offline --locked -p rsc-rpc --all-targets -- -D warnings`

Finished `dev` en 1.14 s. Exit 0.

`rsc-integration-tests` no se modificó. No se ejecutó.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

No hubo compilación de `*.cc`. Se reutilizó `librocksdb.a` de 457632132 bytes. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. El binario sirve `rsc_getFinality` en `POST /rpc` y, tras `SIGTERM` y un segundo arranque, devuelve la misma prueba. Fase 8 sigue OPEN.

## TASK 8.12 — Multi-validator quorum y FinalityProof

### Objetivo

Demostrar, dentro de `rsc-validator`, el ciclo Prepare → PreCommit → Commit con cuatro validadores del mismo Genesis. Tres votos alcanzan quorum. Dos no. La `FinalityProof` sale de esos votos.

### Comité

Cuatro `ValidatorRecord` del mismo documento. Cada uno tiene `voting_power` 1 y `eligible` verdadero. `committee_size` 4. `min_validator_weight` 1. `max_validator_weight` 30. `epoch_length` 20. `min_validators_for_distributed` 5, así el modo sigue en Bootstrap. El poder total del comité es 4.

`has_quorum` queda igual: `3 * poder > 2 * total`. Con total 4, tres votos dan `9 > 8`. Dos votos dan `6 > 8`.

La semilla pública del beacon es `Hash([0x3C; 32])`. En altura 0 el padre es `Hash::ZERO`. Los cuatro motores ven el mismo epoch, altura 0, vista 0 y hash de bloque.

### Identidad

Cada registro sale de `PrivateKey` → `owner_public_key` → `owner_key_id` → `derive_account_id(owner_key_id)` → `ValidatorRecord.id`. La clave privada no se registra.

| Validator | `owner_key_id` | `AccountId` | Poder |
| --- | --- | --- | --- |
| 0 | `94c3dff7c6159776e7b0bc696fd7c99a07043c618d21c0f6c4d4758c74e74d07` | `0x5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f` | 1 |
| 1 | `f45120ede57a8d43e98bfe3f40e5c2ee4dbeeb33de63fdf91ea6f268ebf39ec7` | `0x3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c` | 1 |
| 2 | `6a79590af0f277a4de2e21955c74ce5c8396f6cf2549f15c6235f50d75c2b13d` | `0xec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31` | 1 |
| 3 | `9c18063c8392ba8551348f59cf0acdccdefbb33cf4464ed8b5697325691aff5c` | `0xa50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a` | 1 |

`owner_public_key.key_id()` coincide con `owner_key_id`. El `AccountId` es el derivado. Cada uno está en el `ValidatorSet` y en el comité, es elegible y tiene peso 1.

### 3/4

Los cuatro firman Prepare, PreCommit y Commit con `create_vote` y su `PrivateKey`. `verify_validator_vote` acepta cada voto. El motor colector recibe tres votos por fase. Los validadores 0, 1 y 2 entran. El validador 3 no entra en los QC.

Prepare: `QuorumReached(Prepare)`, QC con `voting_power` 3 y tres votantes de peso 1. `verify_qc` lo acepta. La fase pasa a Prepare. El lock sigue vacío. La altura sigue en 0.

PreCommit: `QuorumReached(PreCommit)`. `verify_qc` lo acepta. La fase pasa a PreCommit. El lock queda en el hash del tip.

Commit: el tercer voto devuelve `BlockFinalized`. El QC de Commit tiene poder 3. `verify_qc` lo acepta. `verify_commit_chain` acepta la prueba. La prueba trae esos tres QC, el hash del tip, altura 0 y vista 0. La fase queda en `Decide`. La altura del motor sigue en 0. El lock sigue en el tip.

### 2/4

Otro motor, todavía en Propose, recibe solo los Prepare de los validadores 0 y 1. Los dos eventos son `VoteCreated`. La fase sigue en Propose. El lock sigue vacío. La altura sigue en 0. `aggregate_qc` devuelve `None`. `has_quorum(2, 4)` es falso. No hay `BlockFinalized` ni `FinalityProof`.

### Identidad incorrecta

Sobre un motor con dos Prepare válidos:

- Un voto con `AccountId::ZERO` devuelve `invalid signature`.
- El voto del validador 0 firmado con la clave del validador 3: `verify_validator_vote` y `receive_vote` devuelven `invalid signature`.
- Un registro cuyo `owner_key_id` es el del validador 1 y cuya pública sigue siendo la del validador 0: `validate_validator_record` devuelve `invalid signature`.
- Un registro cuya `owner_public_key` es la del validador 1: `validate_validator_record` y `verify_validator_vote` devuelven `invalid signature`.
- `create_vote` del validador 2 con la clave del validador 1 devuelve `invalid signature`.

La fase sigue en Propose y la altura en 0. Esos votos no completan el quorum.

### Contexto y duplicados

En otro motor: el Prepare del validador 0 entra una vez (`VoteCreated`). La segunda copia, con el mismo epoch, altura, vista, fase y hash, devuelve `ProposalAccepted`. El Prepare del validador 1 entra después (`VoteCreated`). La fase sigue en Propose: el duplicado no suma poder.

Un Prepare del validador 2 con epoch, altura o vista cambiados, cada uno por separado y vuelto a firmar, devuelve `wrong view`. La fase sigue en Propose.

Un segundo Prepare del validador 0 con otro `block_hash`, firmado con su clave, devuelve `Equivocation`. El voto original permanece. La fase sigue en Propose.

El Prepare válido del validador 2 produce `QuorumReached(Prepare)` sobre el hash original. La altura sigue en 0. El QC agregado de esos tres votos tiene ese hash.

### Invariantes

Hasta el `BlockFinalized` y después, el tip, la raíz de estado, la raíz del ledger, la altura, el `commit_count` y la ausencia de bloque en la altura 1 se mantienen. La clave `finality` sigue ausente. La prueba queda en el valor de `BlockFinalized`. No se escribe otro formato.

### Qué NO hace

No cambia `has_quorum`, `SignatureBundle`, `QuorumCertificate`, `verify_qc`, `verify_commit_chain`, `FinalityProof`, `CryptoDomain` ni el algoritmo criptográfico. No añade P2P. No persiste una estructura nueva. No llama `new_height`. No toca el arranque de un solo validador con peso 3. No modifica `rsc-consensus`, `rsc-node` ni `rsc-integration-tests`.

### Archivos

`network/validator/src/main.rs` (prueba). `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

Finished `test` en 28.52 s. **28/28**, finished in 41.36 s. `process_bin`: **1/1**, finished in 0.24 s. Exit 0.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

Finished `dev` en 22.16 s. Exit 0.

`rsc-consensus` no se modificó. No se ejecutó. `rsc-integration-tests` no se modificó. No se ejecutó.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

No hubo compilación de `*.cc`. Se reutilizó `librocksdb.a` de 457632132 bytes. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. Tres de cuatro validadores producen una `FinalityProof` que `verify_commit_chain` acepta. Dos de cuatro no alcanzan quorum. Fase 8 sigue OPEN.

## TASK 8.13 — Voting Power ponderado en el ciclo real del validator

### Objetivo

Demostrar que el quorum de `rsc-validator` usa el `voting_power` de cada `ValidatorRecord` del `ValidatorSet`. La fórmula `has_quorum` permanece `3 * poder > 2 * total`.

### Comité

Cuatro validadores del mismo Genesis. `committee_size` 4. `min_validator_weight` 1. `max_validator_weight` 30. `epoch_length` 20. `min_validators_for_distributed` 5. El modo sigue en Bootstrap. El poder total es 6.

| Validator | `owner_key_id` | `AccountId` | Poder |
| --- | --- | --- | --- |
| 0 | `94c3dff7c6159776e7b0bc696fd7c99a07043c618d21c0f6c4d4758c74e74d07` | `0x5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f` | 2 |
| 1 | `f45120ede57a8d43e98bfe3f40e5c2ee4dbeeb33de63fdf91ea6f268ebf39ec7` | `0x3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c` | 2 |
| 2 | `6a79590af0f277a4de2e21955c74ce5c8396f6cf2549f15c6235f50d75c2b13d` | `0xec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31` | 1 |
| 3 | `9c18063c8392ba8551348f59cf0acdccdefbb33cf4464ed8b5697325691aff5c` | `0xa50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a` | 1 |

Cada registro sale de `PrivateKey` → `owner_public_key` → `owner_key_id` → `derive_account_id` → `AccountId`. La clave privada no se registra. `owner_public_key.key_id()` coincide con `owner_key_id`. El peso del miembro del comité es el del registro.

### Cálculo

`3 * poder > 2 * 6`, o sea `3 * poder > 12`.

- Poder 4: `12 > 12` es falso.
- Poder 5: `15 > 12` es verdadero.
- Poder 2: `6 > 12` es falso.
- Tres firmas contadas como 1 cada una serían poder 3: `9 > 12` es falso. El caso que finaliza lo hace porque el QC trae poder 5, no 3.

Los votos de un caso comparten epoch, altura 0, vista 0 y el hash del tip.

### Poder 4

Los validadores 0 y 1 firman Prepare, PreCommit y Commit. Otro motor recibe esas dos firmas en las tres fases. Cada evento es `VoteCreated`. La fase sigue en Propose. El lock sigue vacío. La altura sigue en 0. `aggregate_qc` no devuelve QC. No hay `BlockFinalized` ni `FinalityProof`. Dos firmas válidas no equivalen a quorum.

En el motor que sí finaliza, esas mismas dos firmas dejan la fase quieta: Propose antes del tercer Prepare, Prepare antes del tercer PreCommit, y PreCommit antes del tercer Commit.

### Poder 5

El colector recibe también al validador 2.

Prepare: `QuorumReached(Prepare)`. El QC tiene `voting_power` 5, tres votantes y pesos 2, 2 y 1. `verify_qc` lo acepta. La fase pasa a Prepare. El lock sigue vacío.

PreCommit: `QuorumReached(PreCommit)`. El QC tiene poder 5. `verify_qc` lo acepta. La fase pasa a PreCommit. El lock queda en el hash del tip.

Commit: el voto del validador 2 devuelve `BlockFinalized`. El QC de Commit tiene poder 5. `verify_qc` y `verify_commit_chain` lo aceptan. La prueba contiene esos tres QC. El validador 3 no entra. La fase queda en `Decide`. La altura sigue en 0.

### Poder 2

Los validadores 2 y 3 firman las tres fases. Otro motor las recibe. La fase sigue en Propose. `aggregate_qc` no devuelve QC. No hay finality.

### Duplicados y contexto

Repetir el Prepare del validador 0 devuelve `ProposalAccepted`. Después entra el validador 1 y la fase sigue en Propose: el duplicado no suma otro peso 2. Si lo sumara, el poder sería 6 y habría quorum.

Un Prepare del validador 2 con epoch, altura o vista cambiados devuelve `wrong view`. Otro hash del validador 0, firmado con su clave, devuelve `Equivocation`. El voto original permanece. El Prepare válido del validador 2 produce entonces `QuorumReached(Prepare)` con poder 5 y el hash original.

### Invariantes

El tip, la raíz de estado, la raíz del ledger, la altura, el `commit_count` y la ausencia de bloque en la altura 1 se mantienen. La clave `finality` sigue ausente. La prueba queda en el valor de `BlockFinalized`.

### Qué NO hace

No cambia `has_quorum`, `SignatureBundle`, `QuorumCertificate`, `verify_qc`, `verify_commit_chain`, `FinalityProof` ni la criptografía. No añade P2P, red, elección de líder, producción de bloques ni persistencia nueva. No modifica `rsc-consensus`. El comité de peso 1 de TASK 8.12 sigue igual.

### Archivos

`network/validator/src/main.rs` (prueba). `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

Finished `test` en 1.45 s. **29/29**, finished in 49.34 s. `process_bin`: **1/1**, finished in 0.05 s. Exit 0.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

Finished `dev` en 6.06 s. Exit 0.

`rsc-consensus` no se modificó. No se ejecutó. `rsc-integration-tests` no se modificó. No se ejecutó.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

No hubo compilación de `*.cc`. Se reutilizó `librocksdb.a` de 457632132 bytes. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. El QC que finaliza lleva poder 5 del `ValidatorSet`. Poder 4 y poder 2 no arman quorum ni `FinalityProof`. Fase 8 sigue OPEN.

## TASK 8.14 — Weighted FinalityProof: persistencia, recovery y RPC

### Objetivo

La `FinalityProof` ponderada de TASK 8.13 se guarda, se recupera y se publica por `rsc_getFinality` con la ruta ya existente.

### Comité

El mismo de TASK 8.13. Poder total 6.

| Validator | `owner_key_id` | `AccountId` | Poder |
| --- | --- | --- | --- |
| 0 | `94c3dff7c6159776e7b0bc696fd7c99a07043c618d21c0f6c4d4758c74e74d07` | `0x5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f` | 2 |
| 1 | `f45120ede57a8d43e98bfe3f40e5c2ee4dbeeb33de63fdf91ea6f268ebf39ec7` | `0x3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c` | 2 |
| 2 | `6a79590af0f277a4de2e21955c74ce5c8396f6cf2549f15c6235f50d75c2b13d` | `0xec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31` | 1 |
| 3 | `9c18063c8392ba8551348f59cf0acdccdefbb33cf4464ed8b5697325691aff5c` | `0xa50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a` | 1 |

Los validadores 0, 1 y 2 forman el quorum. Poder 5. `3 * 5 > 2 * 6` es `15 > 12`.

### QCs y prueba

Prepare, PreCommit y Commit. Cada QC tiene tres votantes, `voting_power` 5 y pesos 2, 2 y 1. `verify_qc` acepta los tres. `verify_commit_chain` acepta la prueba. El validador 3 no entra en esos QC.

### Persistencia y recovery

`recover_proof` escribe la prueba en `ColumnFamily::Default`, clave `finality`, con el encuadre ya existente. Se cierra la base y se vuelve a abrir la misma. Los bytes recuperados son los escritos. `decode_proof` y `verify_commit_chain` aceptan esa prueba contra el comité del mismo Genesis. El tip, su altura, la raíz de estado y la raíz del ledger siguen iguales. No hay bloque en la altura siguiente.

### RPC

`read_finality` y `rsc_getFinality` devuelven la misma prueba:

- `finalized_hash`: hex del hash del bloque
- `finalized_height`: altura de la prueba
- `epoch`: `prepare.epoch`
- `view`: vista de la prueba
- `prepare`, `precommit`, `commit`: hex del encuadre de cada QC

Esos tres campos decodifican los QC de poder 5.

### Pruebas negativas

Borrar la clave `finality` hace que `read_finality` responda `service unavailable` y que `rsc_getFinality` sea `ServiceUnavailable`.

Cambiar el `voting_power` del validador 2 en el comité, de 1 a 2, después de generado el QC: `verify_commit_chain` y `read_finality` responden `invalid qc`. El comité original sigue aceptando los bytes guardados.

Un Commit QC de poder 5 sobre otro hash, con firmas válidas de los validadores 0, 1 y 2, hace que `verify_commit_chain` y `read_finality` respondan `invalid finality`.

Una firma alterada del Commit QC hace que `read_finality` responda `invalid signature`.

Quitar al validador 2 y dejar el poder en 4: `has_quorum(4, 6)` es falso y `verify_qc` responde `invalid qc`. `rsc_getFinality` no tiene éxito.

Sustituir el id del validador 2 por el del validador 3, conservando la firma anterior, hace que `verify_qc` responda `invalid signature`. Los votos reales de 0, 1 y 3 arman QC de poder 5 y `verify_commit_chain` los acepta. Los votos de 1, 2 y 3 suman poder 4 y `aggregate_qc` no devuelve QC. Reemplazar al validador 0 por el validador 3, con poder declarado 1 y firma de ese validador, deja el QC en poder 4 y `verify_qc` responde `invalid qc`.

Después de esas escrituras, el tip, la altura, la raíz de estado y la raíz del ledger siguen iguales.

### Qué NO hace

No cambia `has_quorum`, el `ValidatorRecord`, el `ValidatorSet`, `QuorumCertificate`, `SignatureBundle`, `verify_qc`, `verify_commit_chain`, `FinalityProof`, el formato persistido, `CryptoDomain` ni la criptografía. No añade P2P, elección de líder, producción de bloques, otra clave ni otro endpoint. No modifica `rsc-consensus` ni `rsc-rpc`.

### Archivos

`network/validator/src/main.rs` (prueba). `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-validator -- --test-threads=1`

Finished `test` en 4.00 s. **30/30**, finished in 53.19 s. `process_bin`: **1/1**, finished in 0.02 s. Exit 0.

`cargo clippy --offline --locked -p rsc-validator --all-targets -- -D warnings`

Finished `dev` en 3.84 s. Exit 0.

`cargo test --offline --locked -p rsc-rpc -- --test-threads=1`

Finished `test` en 2.34 s. **81/81**, finished in 0.20 s. Doc-tests: 0, finished in 0.00 s. Exit 0.

`cargo clippy --offline --locked -p rsc-rpc --all-targets -- -D warnings`

Finished `dev` en 1.36 s. Exit 0.

`rsc-consensus` no se modificó. No se ejecutó. `rsc-integration-tests` no se modificó. No se ejecutó.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

No hubo compilación de `*.cc`. Se reutilizó `librocksdb.a` de 457632132 bytes. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. La prueba de poder 5 queda en la clave `finality`, sobrevive al reopen y `rsc_getFinality` devuelve esos QC. Fase 8 sigue OPEN.

## Cierre de fase — `rsc-validator` congelado

El desarrollo de `rsc-validator` queda congelado después de TASK 8.14. No se abre TASK 8.15.

Quedan CLOSED las tareas 8.1, 8.2, 8.3, 8.4, 8.4C, 8.4D, 8.4E, 8.4F y 8.5 a 8.14. TASK 8.4A sigue BLOCKED. TASK 8.4B sigue parcialmente decidida. Esos dos registros no se reabren con este cierre.

El componente ya demuestra el arranque, la identidad criptográfica, el `ValidatorSet` tomado del Genesis, el beacon, Prepare / PreCommit / Commit, los QC, `verify_qc`, `verify_commit_chain`, la `FinalityProof`, su persistencia y su recuperación, `rsc_getFinality`, el proceso HTTP y el restart, el comité de varios validadores, el quorum por `voting_power` y la prueba ponderada guardada.

Hasta una decisión arquitectónica nueva no se agregan tareas 8.x, cambios de consenso o de quorum, RPC, P2P, producción de bloques, elección de líder ni red distribuida. `Cargo.lock` y las dependencias permanecen como quedaron en TASK 8.14. No se declaran tareas de implementación sin un requisito funcional concreto.

## Fase 9 — red distribuida

`rsc-validator` sigue congelado. Esta fase usa el stack ya existente de `rsc-p2p` entre procesos. No abre consenso, propagación ni sincronización.

## TASK 9.1 — P2P real entre procesos

### Objetivo

Dos procesos independientes de `rsc-p2p` se escuchan, uno marca al otro con `NetworkHandle::dial` y cada uno reconoce al PeerId del otro. El apagado es `SIGTERM`.

### Procesos

Binario `rsc-p2p`. Cada uno recibe `--data-dir`, `--network-id` y `--chain-id`. El que marca añade `--dial` con la multiaddr que el otro imprimió. `data-dir` se crea vacío: el proceso no abre RocksDB, cadena, estado, ledger, mempool ni consenso.

La identidad es `LocalPeerIdentity::generate`. La escucha es `NetworkConfig` con `"/ip4/127.0.0.1/tcp/0"` y `DiscoveryConfig::for_network` sin bootstrap. Tras `ListeningAddressAdded`, la línea `listen` es esa dirección más `Protocol::P2p` del PeerId local. El dial es `NetworkHandle::dial` de esa multiaddr.

El swarm es el de `build_swarm`: TCP, Noise, Yamux, QUIC, Identify, Kademlia, ping y request-response. Esta tarea marca por el listen TCP. QUIC queda en el swarm y no se añade otro transporte.

### Corrida registrada

```
rsc-p2p --data-dir /tmp/rsc-p2p-a.3Lt0zI --network-id devnet --chain-id 3
rsc-p2p --data-dir /tmp/rsc-p2p-b.OTLEtN --network-id devnet --chain-id 3 --dial /ip4/127.0.0.1/tcp/40579/p2p/12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
```

Proceso A, pid 109739:

```
peer 12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
listen /ip4/127.0.0.1/tcp/40579/p2p/12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
connected 12D3KooWSNZhuNQtoMDmL4YZRwqPhRGjgKUCAogwqJyVtsqDt3vS
identified 12D3KooWSNZhuNQtoMDmL4YZRwqPhRGjgKUCAogwqJyVtsqDt3vS
handshake 12D3KooWSNZhuNQtoMDmL4YZRwqPhRGjgKUCAogwqJyVtsqDt3vS
handshake 12D3KooWSNZhuNQtoMDmL4YZRwqPhRGjgKUCAogwqJyVtsqDt3vS
```

Proceso B, pid 109753:

```
peer 12D3KooWSNZhuNQtoMDmL4YZRwqPhRGjgKUCAogwqJyVtsqDt3vS
listen /ip4/127.0.0.1/tcp/40507/p2p/12D3KooWSNZhuNQtoMDmL4YZRwqPhRGjgKUCAogwqJyVtsqDt3vS
connected 12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
identified 12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
handshake 12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
handshake 12D3KooWB7DghLjdbofQ3QHbEFjXpSWy2f7QtXRJJCLVvZc2AAqH
```

A reconoce a B. B reconoce a A. `handshake` sale dos veces en cada proceso: `HandshakeAccepted` al recibir la oferta y otra vez al recibir la respuesta, el mismo evento que ya emite `RscNetwork`. stderr vacío. Los PeerId son identidad libp2p; no hay clave privada en la salida.

### Apagado

`SIGTERM` a 109739 y 109753. `/proc/<pid>/task/*/children` vacío antes de la señal. Exit A=0. Exit B=0. Ninguno quedó en ejecución. Cada `data-dir` quedó con 0 entradas y sin `LOCK`.

### Pruebas negativas

El test de integración `tests/two_processes.rs` levanta el mismo binario.

1. `network-id` `mainnet` contra `devnet`, mismo dial: aparece `rejected` con `network mismatch`. No hay línea `handshake`.
2. `chain-id` 3 contra 1, mismo `network-id`: aparece `rejected` con `chain mismatch`. No hay línea `handshake`.
3. Dial a `/ip4/127.0.0.1/tcp/1/p2p/<PeerId aleatorio>`: `dial-failed` contiene `dial failed`. No hay `connected` ni `handshake`.
4. Mismo host y puerto de A con un PeerId distinto: `dial-failed` contiene `dial failed`. No hay `connected` ni `handshake`.

El rechazo de red o de cadena lo emite quien evalúa la oferta (`HandshakeRejected`), igual que `test_network_mismatch` y `test_chain_mismatch`.

### Invariante

El proceso no abre tip, `StateRoot`, `LedgerRoot`, altura, mempool ni fase de consenso. `data-dir` no recibe archivos.

### Qué NO hace

No hay Prepare, PreCommit, Commit, QC, `FinalityProof`, producción de bloques, propagación, sync ni elección de líder. No cambia `has_quorum`, `SignatureBundle`, `QuorumCertificate`, `verify_qc`, `verify_commit_chain` ni el formato de bloque. No modifica `rsc-validator` ni `rsc-node`.

### Archivos

`crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `crates/rsc-p2p/Cargo.toml`: feature `signal` de `tokio` para `SIGTERM`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1 && cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `test` en 4m 31s.

`src/lib.rs`: **43/43**, finished in 10.59s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00s.

`tests/two_processes.rs`: **5/5**, finished in 1.22s. Incluye `two_processes_dial_and_recognize_each_other`, los dos rechazos de handshake, el puerto cerrado y el PeerId inalcanzable.

Doc-tests: **0**, finished in 0.00s.

Clippy: Finished `dev` en 2m 20s. Exit 0 del comando encadenado. `elapsed_ms` 425038.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. `tokio` sigue en 1.53.1, checksum `202caea871b69668250d242070849eb495be178ed697a3e98aebce5bc81a0bed`. `rocksdb` no entra en este paquete.

### RocksDB

No hubo compilación de `*.cc` ni de `librocksdb-sys`. El log compila `rsc-p2p` y, en clippy, el árbol de libp2p. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. Dos procesos con `data-dir`, identidad y puerto distintos, el mismo `network-id` y el mismo `chain-id`, se reconocen por el stack P2P existente y salen con `SIGTERM`. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.2 — Peer discovery / bootstrap real

### Objetivo

Tres procesos usan el bootstrap de Kademlia que ya arma `RscNetwork::start`. B y C reciben solo la multiaddr de A. No hay `NetworkHandle::dial` entre ellos.

### Topología

A es la semilla. B y C son nodos. Los tres usan `network-id` `devnet` y `chain-id` 3. Cada uno tiene PeerId, puerto y `data-dir` propios. `data-dir` queda vacío: el proceso no abre cadena, ledger, consenso ni mempool.

### Mecanismo

`--bootstrap` es una multiaddr con `/p2p/<PeerId>`. `peer_id_from_multiaddr` y `BootstrapPeer::new` arman la semilla. Esa lista entra en `NetworkConfig.bootstrap_peers` y en `DiscoveryConfig::for_network`.

`RscNetwork::start` llama `kad.add_address` y `swarm.add_peer_address`. Kademlia inserta la semilla y, con el throttle automático de 500 ms que ya trae libp2p, `bootstrap()` marca ese PeerId (`ToSwarm::Dial`). Ese dial es del behaviour de Kademlia. No es `NetworkHandle::dial`.

Identify devuelve las direcciones de escucha y el loop las vuelve a cargar en Kademlia (`address`, `identified`). Ese mismo camino llama `kad.bootstrap()` otra vez (`bootstrap-started`). `RoutingUpdated` sale como `discovered`. Cada paso de `QueryResult::Bootstrap(Ok)` sale como `bootstrap-ok`.

La consulta a A devuelve al otro nodo. Kademlia lo marca. B y C no reciben la multiaddr del otro en la línea de comandos.

### Corrida registrada

```
rsc-p2p --data-dir /tmp/rsc-p2p-a.KHJB25 --network-id devnet --chain-id 3
rsc-p2p --data-dir /tmp/rsc-p2p-b.q7LI1U --network-id devnet --chain-id 3 --bootstrap /ip4/127.0.0.1/tcp/45345/p2p/12D3KooWN8LojnhLcTdPAaGCnTGkLnraDmqxyUTsEbevPxLWMoyg
rsc-p2p --data-dir /tmp/rsc-p2p-c.WAyPaJ --network-id devnet --chain-id 3 --bootstrap /ip4/127.0.0.1/tcp/45345/p2p/12D3KooWN8LojnhLcTdPAaGCnTGkLnraDmqxyUTsEbevPxLWMoyg
```

A, pid 111553, PeerId `12D3KooWN8LojnhLcTdPAaGCnTGkLnraDmqxyUTsEbevPxLWMoyg`, listen `/ip4/127.0.0.1/tcp/45345/p2p/12D3KooWN8LojnhLcTdPAaGCnTGkLnraDmqxyUTsEbevPxLWMoyg`.

B, pid 111568, PeerId `12D3KooWCXswGiL6M28DeQ2WgnqsY6voGFJem4uoEvofy1XpE7Hf`, listen `/ip4/127.0.0.1/tcp/36223/p2p/12D3KooWCXswGiL6M28DeQ2WgnqsY6voGFJem4uoEvofy1XpE7Hf`.

C, pid 111569, PeerId `12D3KooWNiXymxhfVQx2YiE79M7s7ChPBD7hxCgGE4JWfgCigEgY`, listen `/ip4/127.0.0.1/tcp/45795/p2p/12D3KooWNiXymxhfVQx2YiE79M7s7ChPBD7hxCgGE4JWfgCigEgY`.

B aprendió a A: `discovered`, `connected`, `handshake`, `address /ip4/127.0.0.1/tcp/45345`, `identified`, `bootstrap-started`, `bootstrap-ok`.

C aprendió a A con los mismos eventos y la misma dirección.

A reconoció a B y a C: `connected`, `address`, `identified`, `discovered`, `handshake`, `bootstrap-started`, `bootstrap-ok`.

B aprendió a C sin su multiaddr: `connected`, `discovered`, `handshake`, `address /ip4/127.0.0.1/tcp/45795`, `identified`.

C aprendió a B sin su multiaddr: `connected`, `handshake`, `address /ip4/127.0.0.1/tcp/36223`, `identified`, `discovered`.

stderr vacío en los tres. Los PeerId son identidad libp2p. No hay clave privada en la salida.

### Apagado

`SIGTERM` a 111553, 111568 y 111569. `/proc/<pid>/task/*/children` vacío. Exit A=0, B=0, C=0. Ninguno quedó en ejecución. Cada `data-dir` quedó con 0 entradas y sin `LOCK`. Los tres directorios se borraron.

### Pruebas negativas

El mismo binario, en `tests/two_processes.rs`. Las cinco pruebas de TASK 9.1 siguen en ese archivo y pasaron en la misma corrida.

1. Bootstrap `/ip4/127.0.0.1/tcp/1/p2p/<PeerId aleatorio>`: `dial-failed` contiene `dial failed`. No hay `connected`, `identified` ni `handshake`.
2. Semilla `mainnet` / chain 1 y nodo `devnet` / chain 3: `rejected` con `network mismatch`. No hay `handshake`.
3. Misma red, chain 3 contra chain 1: `rejected` con `chain mismatch`. No hay `handshake`.
4. El puerto real de A con otro PeerId: `dial-failed` con `dial failed`, sin sesión en ninguno de los dos procesos. Una multiaddr sin `/p2p/` sale con código 1 y stderr `invalid address`.
5. `/ip4/127.0.0.1/tcp/9/p2p/<PeerId aleatorio>`: `dial-failed` con `dial failed`. No hay `connected`, `identified` ni `handshake`.

`bootstrap-ok` es el `QueryResult::Bootstrap(Ok)` de cada paso de la consulta. No se usa como prueba de sesión. La sesión es `connected`, `identified` y `handshake`.

### Invariante

El proceso no abre tip, `StateRoot`, `LedgerRoot`, altura, mempool, consenso ni bloques. `data-dir` no recibe archivos.

### Qué NO hace

No hay transacciones, propagación, votos, QC, finalidad, producción de bloques ni sync. No cambia `rsc-validator` ni el algoritmo de consenso. No añade un discovery distinto de Kademlia, Identify y `BootstrapPeer`.

### Archivos

`crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `docs/rsc-chain.md`. `network.rs` no se modificó.

### Comandos

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1 && cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `test` en 2.94 s.

`src/lib.rs`: **43/43**, finished in 10.56 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **12/12**, finished in 7.13 s. Incluye las 5 pruebas de TASK 9.1, `three_processes_bootstrap_from_the_seed` y 6 negativos de bootstrap.

Doc-tests: **0**, finished in 0.00 s.

Clippy: Finished `dev` en 2.94 s. Exit 0. `elapsed_ms` 25684.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. `tokio` sigue en 1.53.1, checksum `202caea871b69668250d242070849eb495be178ed697a3e98aebce5bc81a0bed`. `rocksdb` no entra en este paquete.

### RocksDB

No hubo compilación de `*.cc` ni de `librocksdb-sys`. El log compila `rsc-p2p`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. A, B y C se encuentran con el bootstrap de Kademlia ya existente. B y C aprenden a A y, por la consulta, uno al otro, sin `dial` manual entre ellos. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.3 — Propagación real de una transacción

### Objetivo

Una transacción v1 firmada entra por `rsc_sendTransaction` en A, A la admite en el mempool y la publica en `/rsc/devnet/transactions/1`. B y C la decodifican con `Transaction::from_canonical_bytes`, la validan con `Transaction::validate` y la insertan con `Mempool::insert`. Los tres quedan con el mismo `TransactionId`. No se crea un bloque.

### Topología

El mismo bootstrap de TASK 9.2. A es la semilla. B y C solo reciben `--bootstrap` de A. Los tres usan `network-id` `devnet`, `chain-id` 3, `--signer-key` con la clave pública del firmante y `--rpc` en un puerto distinto. No hay `NetworkHandle::dial` entre B y C.

A, PeerId `12D3KooWKRuGNF54TEe3sE7wyHReY8eqfp8QxMMP8UuzPzZpjB4m`, listen `/ip4/127.0.0.1/tcp/34693/p2p/12D3KooWKRuGNF54TEe3sE7wyHReY8eqfp8QxMMP8UuzPzZpjB4m`, RPC `127.0.0.1:44339`.

B, PeerId `12D3KooWBDvoyUzDq2WVt5aAPBWzRgBETbgKgjtd2LhLPVz2JXFk`, listen `/ip4/127.0.0.1/tcp/33761/p2p/12D3KooWBDvoyUzDq2WVt5aAPBWzRgBETbgKgjtd2LhLPVz2JXFk`, RPC `127.0.0.1:36931`.

C, PeerId `12D3KooWERvWzutdtry2qgRak9SBWQsygXp6DJpkdcm9BH9KmWb7`, listen `/ip4/127.0.0.1/tcp/34209/p2p/12D3KooWERvWzutdtry2qgRak9SBWQsygXp6DJpkdcm9BH9KmWb7`, RPC `127.0.0.1:44957`.

Antes de la transacción, A, B y C se reconocen entre sí: `connected`, `identified` y `handshake`. El descubrimiento sigue siendo el de Kademlia.

### Flujo

La transacción es v1, nonce 0, importe 7. El sender es la cuenta derivada del `KeyId` del firmante (`RSC/STATE/ACCOUNT-ID/V1`). El recipient es la cuenta derivada de otra clave. `Transaction::sign` firma con `CryptoDomain::Transaction`. `Transaction::validate` acepta esa firma. Los bytes publicados son `canonical_bytes`. La clave pública no viaja en esos bytes: cada proceso la recibe en `--signer-key` para poder llamar a `validate`. La clave privada no se imprime.

`POST /rpc` con `rsc_sendTransaction` y `signed_transaction` en hex. `TransactionService::submit_signed` llama `Mempool::insert`. Si entra, `NetworkHandle::publish(GossipTopic::Transactions, bytes)` usa el topic que ya arma `topic_name`: `/rsc/<network-id>/transactions/1`.

B y C reciben `GossipMessageReceived`. `SignedAdmission` decodifica, llama `Transaction::validate` y `Mempool::insert` guarda el hash `Transaction::id`. No reenvían por otro sobre.

`TransactionId` en A, B y C:

`3d2d54ef983f1e7bf4526e9c3a619e1c9fe8238fd41aebd7a1a2baf431d3babf`

A imprimió `admitted` y `published` de ese id. B y C imprimieron `admitted` del mismo id. La respuesta RPC de A trae `result.transaction_hash` con ese id.

### Deduplicación

El segundo `rsc_sendTransaction` del mismo hex responde `transaction rejected`. A imprime `rejected transaction is already in the mempool`. Hay una sola línea `admitted` y una sola `published` en A, y una sola `admitted` en B y en C.

### Negativos

1. Un byte alterado de la firma: `Transaction::validate` falla. RPC `transaction rejected`. A imprime `rejected transaction validation failed`. No hay `published`. B no imprime `admitted`.
2. `signed_transaction` `00`: `from_canonical_bytes` falla. `rejected transaction is invalid`. No hay `published` ni `admitted` en el otro proceso.
3. El cuerpo canónico y `rsc_sendTransaction` no traen un `TransactionId` aparte. El id es `Transaction::id` del cuerpo. La API no permite enviar un id distinto de esos bytes.
4. `Transaction` no tiene campo de `network-id` ni de `chain-id`. `Transaction::validate` no rechaza por red. El aislamiento sigue siendo el topic `/rsc/<network-id>/transactions/1` y el handshake de TASK 9.1 y 9.2.
5. Importe 0: `InvalidAmount` dentro de `validate`. `rejected transaction validation failed`. No se publica.

### Invariantes

Este proceso no abre base, estado, ledger ni consenso. `rsc_getStatus` responde `latest_height` 0 y `latest_block_hash` de 64 ceros, antes y después, en A, B y C. `rsc_getFinality` responde `service unavailable`. `rsc_getBlockByHeight` de altura 0 responde `not found`. El `data-dir` queda vacío. El nonce canónico que ve el mempool es 0 porque no hay estado ejecutado; la transacción declara nonce 0.

### Apagado

El test envía `SIGTERM`. Cada proceso sale con código 0, sin procesos hijos, sin `LOCK`, y el directorio temporal se borra.

### Qué NO hace

No hay Prepare, PreCommit, Commit, QC, `FinalityProof`, producción de bloques, propagación de bloques, sync, transición de estado ni append de ledger. No cambia `rsc-validator`, `rsc-mempool` ni `rsc-rpc`. No añade un sobre distinto de los bytes canónicos.

### Archivos

`crates/rsc-p2p/Cargo.toml`. `crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `Cargo.lock`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1 && cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `test` en 3m 27s.

`src/lib.rs`: **43/43**, finished in 11.09 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **16/16**, finished in 15.99 s. Incluye las pruebas de TASK 9.1 y 9.2, `three_processes_propagate_a_transaction`, la firma alterada, el payload `00` y el importe 0.

Doc-tests: **0**, finished in 0.00 s.

Clippy: Finished `dev` en 2m 20s. Exit 0. `elapsed_ms` 378346.

`rsc-mempool` y `rsc-rpc` no se modificaron. No se ejecutaron sus suites.

### Resultado

✅ PASS.

### Cargo.lock

`rsc-p2p` declara dependencia de `rsc-ledger`, `rsc-mempool` y `rsc-rpc`. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

No hubo compilación de `*.cc` ni de `librocksdb-sys`. El log compila `rsc-p2p` y, en clippy, libp2p, `rsc-ledger`, `rsc-mempool` y `rsc-rpc`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. La transacción `3d2d54ef983f1e7bf4526e9c3a619e1c9fe8238fd41aebd7a1a2baf431d3babf` queda en el mempool de A, B y C. La altura, el hash de bloque y la finalidad no se mueven. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.4 — Propagación real de una propuesta

### Objetivo

Una propuesta válida sale de un proceso, viaja por el gossip de bloques que ya existe y otros dos procesos la decodifican y la aceptan con `ConsensusEngine::receive_proposal`.

### Tipo, encoding y topic

No hay un tipo `Proposal`. La propuesta es un `Block`. `receive_proposal(&Block, Option<&ChainValidationContext>)` es el método de recepción. El encoding es `encode_block` / `decode_block` (Borsh del header y de las transacciones). El topic es `GossipTopic::Blocks`, el que arma `topic_name`: `/rsc/devnet/blocks/1`. El hash anunciado es `block_hash`, que es `header_hash` del header canónico.

### Topología

El mismo bootstrap de TASK 9.2. A es el proposer y la semilla. B y C arrancan con `--bootstrap` de A y `--consensus observe`. A arranca con `--consensus propose`. Los tres usan `network-id` `devnet`, `chain-id` 3, la clave pública del firmante de TASK 9.3 en `--signer-key` y un `--rpc` distinto. B y C no reciben la multiaddr del otro. No hay `NetworkHandle::dial` entre B y C.

Antes de publicar, A, B y C se reconocen: `connected`, `identified` y `handshake`, incluido el enlace B↔C que arma Kademlia.

En la corrida con salida de la prueba, A fue `12D3KooWLmCKEmM5U89GvR78a3AT23okmEGiQFTHMTHPqbtwHP1N`, listen `/ip4/127.0.0.1/tcp/40423/p2p/12D3KooWLmCKEmM5U89GvR78a3AT23okmEGiQFTHMTHPqbtwHP1N`, RPC `127.0.0.1:45027`.

B fue `12D3KooWFm4PLWZaKvshNff9ttpFKREH4RKwQcHUNiqzSBimn9qB`, listen `/ip4/127.0.0.1/tcp/41635/p2p/12D3KooWFm4PLWZaKvshNff9ttpFKREH4RKwQcHUNiqzSBimn9qB`, RPC `127.0.0.1:45259`.

C fue `12D3KooW9xfXmAzWN9koxiFBMaXMttpKLinNHDTCfjriw9QYrKwL`, listen `/ip4/127.0.0.1/tcp/44733/p2p/12D3KooW9xfXmAzWN9koxiFBMaXMttpKLinNHDTCfjriw9QYrKwL`, RPC `127.0.0.1:43855`.

Esos `PeerId` y puertos son de esa ejecución. La suite `--locked` volvió a levantar los tres procesos y la prueba pasó con identidades nuevas.

### Proposer y bloque

Los tres procesos construyen el mismo `ConsensusConfig` (comité 1, `epoch_length` 10, peso mínimo 1, peso máximo 20), el mismo `ValidatorSet` de un validador elegible y el mismo beacon `consensus_beacon` de semilla cero a altura 0. `validate_validator_record` acepta el registro: el `id` es `derive_account_id` del `KeyId` de esa clave.

`select_proposer` devuelve `5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f`. A imprime `proposer` con ese valor. El bloque es de altura 0, `chain_id` 3, versión 1, padre `BlockHash::ZERO`, transacciones vacías y `transactions_root` igual a `compute_transaction_root`. El `state_root` del header es cero. A publica `encode_block` de ese bloque.

Hash del bloque en esa corrida:

`094a7ebd44e59062a640a5d35cdc8e8551eb9ea36f893e20258b526e5441a452`

B y C hacen `decode_block` y `receive_proposal` con un `ChainValidationContext` de chain 3, versión 1, máximo 1_048_576 bytes y 1_000 transacciones, sin padre. La API devuelve `ProposalAccepted`. El engine queda en fase `Propose`, `proposal` con ese hash, altura 0, epoch 0, view 0. El identificador local de B y C es otra cuenta; `receive_proposal` acepta igual porque el proposer del header coincide con `select_proposer`.

### Duplicado

A llama `publish` dos veces con los mismos bytes. Hay dos líneas `published` de ese hash. En B y C toda línea `accepted` es ese mismo hash, y la última línea `state` guarda una sola propuesta. `proposal` es un `Option<BlockHash>`: una segunda `receive_proposal` del mismo bloque, si el gossip la entrega, vuelve a devolver `ProposalAccepted` y deja el mismo hash. La fase sigue en `Propose`.

### Negativos

A publica después, por el mismo topic, cuatro payloads. B y C los rechazan con la API existente:

1. Header con otro `proposer`: `proposal-rejected invalid proposer`. El `proposal` guardado sigue siendo el hash válido.
2. Header de altura 1: `receive_proposal` compara la altura con `state.height` y devuelve `proposal-rejected wrong view`. El bloque no tiene campo epoch ni view. El epoch del engine es `epoch_of(0, 10) = 0` y la view sigue en 0. La altura distinta es el `WrongView` de esta API.
3. `transactions_root` distinto de `compute_transaction_root`: `validate_block` falla y `receive_proposal` devuelve `proposal-rejected invalid block`. El bloque no lleva firma. `validate_block` no verifica firmas, así que `receive_proposal` no devuelve `invalid signature`.
4. Un byte `0xff`: `decode_block` devuelve `invalid encoding`. Esa propuesta no entra a `receive_proposal`.

### Invariantes

El proceso no abre base de cadena, estado ni ledger. `rsc_getStatus` sigue en `latest_height` 0 y `latest_block_hash` de 64 ceros, en A, B y C, antes y después. `rsc_getFinality` responde `service unavailable`. `rsc_getBlockByHeight` de altura 0 responde `not found`. El `data-dir` queda vacío. La fase no pasa a `Prepare`, `PreCommit`, `Commit` ni `Decide`. No se llama `create_vote`. No aparece un QC.

### Apagado

El test envía `SIGTERM`. Cada proceso sale con código 0, sin procesos hijos, sin `LOCK`, y el directorio temporal se borra.

### Qué queda fuera

Prepare, PreCommit, Commit, QC, `FinalityProof`, producción de un bloque persistido, sync y catch-up. `has_quorum` y `ValidatorSet` del crate no se modifican. `rsc-validator` y `rsc-node` no se tocan. `rsc-consensus` no se modifica.

### Archivos

`crates/rsc-p2p/Cargo.toml`. `crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `Cargo.lock`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline -p rsc-p2p --test two_processes -- --test-threads=1 three_processes_propagate_a_proposal --nocapture`

Finished `test` en 7m 41s. La prueba pasó en 12.30 s. Exit 0. De ahí salen los `PeerId`, el proposer y el hash de arriba.

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1`

Finished `test` en 5m 12s.

`src/lib.rs`: **43/43**, finished in 15.13 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **17/17**, finished in 20.26 s. Siguen las pruebas de TASK 9.1, 9.2 y 9.3, y entra `three_processes_propagate_a_proposal`.

Doc-tests: **0**, finished in 0.00 s. Exit 0. `elapsed_ms` 350176.

`cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `dev` en 5.98 s. Exit 0.

`rsc-consensus`, `rsc-node` y `rsc-validator` no se modificaron. No se ejecutaron sus suites.

### Resultado

✅ PASS.

### Cargo.lock

`rsc-p2p` declara dependencia de `rsc-chain`, `rsc-consensus` y `rsc-state`. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La primera compilación listó `librocksdb-sys` porque `rsc-chain` arrastra storage. El enlace usó el `librocksdb.a` ya existente de 457632132 bytes en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. Ese archivo no cambió de tamaño ni de fecha. No se compiló `*.cc`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. B y C aceptan el bloque `094a7ebd44e59062a640a5d35cdc8e8551eb9ea36f893e20258b526e5441a452` del proposer `5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f`. La altura, el hash de estado RPC y la finalidad no se mueven. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.5 — Propagación real de votos y QC

### Objetivo

Cuatro procesos con el mismo génesis intercambian votos Prepare, PreCommit y Commit, y los QC que arma `aggregate_qc`, por el topic de consenso que ya existe. `verify_validator_vote`, `receive_vote`, `has_quorum` y `verify_qc` conservan su resultado.

### Topic y encoding

El topic es `GossipTopic::Consensus`: `/rsc/devnet/consensus/1`.

El voto firmado sigue siendo `vote_bytes`: un byte de fase, `epoch`, `height` y `view` en `u64` little-endian, y 32 bytes de `block_hash`. En el gossip esos bytes van después del `AccountId` del autor (32) y de la `Signature` (64). `sign_vote` firma `vote_bytes` con `CryptoDomain::ConsensusVote`. El receptor reconstruye el `Vote` y llama `verify_validator_vote` contra el `ValidatorRecord` del génesis, y después `receive_vote`.

El QC usa el layout que ya persiste el validador: longitud `u32` little-endian y un cuerpo con hash, altura, vista, epoch, fase, poder, votantes y `SignatureBundle::to_bytes`. `verify_qc` comprueba ese certificado contra el comité. El proceso no abre la base del validador.

### Génesis y poder

Los cuatro procesos cargan el mismo `Genesis` de chain-id 3, comité 4, `epoch_length` 10 y semilla de beacon `0x5A` repetida. `Genesis::validate` y `validator_set` aceptan cuatro registros. El poder es 2, 2, 1 y 1. El total del comité es 6.

V0, poder 2, cuenta `5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f`.

V1, poder 2, cuenta `3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c`. Es el proposer de `select_proposer`.

V2, poder 1, cuenta `ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31`.

V3, poder 1, cuenta `a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a`. No emite voto.

### Topología

El bootstrap es el de TASK 9.2. V0 es la semilla. V1, V2 y V3 solo reciben `--bootstrap` de V0. Los cuatro usan `devnet` y chain-id 3. Antes de los votos, cada par tiene `connected`, `identified` y `handshake`.

En la corrida con salida de la prueba:

V0, PeerId `12D3KooWSYsUbvCJhz9wDsoZxkRVErta4FrZVBj8FNnQ1vgLBnMw`, listen `/ip4/127.0.0.1/tcp/44071/p2p/12D3KooWSYsUbvCJhz9wDsoZxkRVErta4FrZVBj8FNnQ1vgLBnMw`, RPC `127.0.0.1:39379`.

V1, PeerId `12D3KooWAakK33Fyb6jMFshjdANEBnNq9e9Z7KWWnbQFMbWe3mCk`, listen `/ip4/127.0.0.1/tcp/41947/p2p/12D3KooWAakK33Fyb6jMFshjdANEBnNq9e9Z7KWWnbQFMbWe3mCk`, RPC `127.0.0.1:38989`.

V2, PeerId `12D3KooWAqn98KsXj5CUsYypiLTANFiE1qaS2KLnPb8KUhaY1bcW`, listen `/ip4/127.0.0.1/tcp/42801/p2p/12D3KooWAqn98KsXj5CUsYypiLTANFiE1qaS2KLnPb8KUhaY1bcW`, RPC `127.0.0.1:34095`.

V3, PeerId `12D3KooWNz2NwHNRe4Qj6sbEj3DbLcTxaB2yHq45VT2VgwDuoczu`, listen `/ip4/127.0.0.1/tcp/34717/p2p/12D3KooWNz2NwHNRe4Qj6sbEj3DbLcTxaB2yHq45VT2VgwDuoczu`, RPC `127.0.0.1:41335`.

Esos `PeerId` y puertos son de esa ejecución. La suite `--locked` volvió a levantar los cuatro procesos y la prueba pasó con identidades de red nuevas.

### Propuesta

El bloque es el de TASK 9.4: altura 0, chain-id 3, padre cero, transacciones vacías, proposer igual a `select_proposer`. El hash en esa corrida es `4a049f4a12646601562d42d4a9c94d8994b5dd2d28e39a33fb3e32cf0bcbfe6d`. Los cuatro lo aceptan con `receive_proposal`. Epoch 0, view 0, fase `Propose`.

### Prepare, PreCommit y Commit

V0, V1 y V2 llaman `create_vote` y `verify_validator_vote` con su `PrivateKey`, publican el voto y los demás lo incorporan con `receive_vote`.

Con V0 + V1 + V2 el QC tiene `voting_power` 5 y el comité `total_voting_power` 6. `has_quorum(5, 6)` es verdadero: `3 * 5 = 15` y `2 * 6 = 12`. Cada proceso imprime `quorum Prepare 5 6 15 12`, arma el QC con `aggregate_qc` y `verify_qc` lo acepta (`qc-ok Prepare 5`). `receive_vote` devuelve `QuorumReached(Prepare)` y la fase pasa a `Prepare`.

El mismo recorrido, con votos de la fase siguiente, produce `quorum PreCommit 5 6 15 12` y `qc-ok PreCommit 5`. La fase pasa a `PreCommit` y `locked` queda en el hash de la propuesta. La altura sigue en 0.

Los votos Commit cierran el mismo quorum. `receive_vote` del voto que completa el poder devuelve `BlockFinalized`. `verify_commit_chain` acepta la prueba. La fase queda en `Decide`. La altura, la view y el hash propuesto siguen iguales. No se llama `new_height`. El bloque no se escribe.

### Duplicado

V3 vuelve a entregar el voto Prepare de V0. `receive_vote` del mismo autor, la misma fase y el mismo hash devuelve `ProposalAccepted`. El proceso imprime `vote-duplicate`. El voto guardado no se reemplaza y el poder del QC sigue en 5: hay una sola línea `quorum` por fase. El id de mensaje de gossipsub es el hash del contenido, así que los procesos que ya recibieron esos bytes exactos no ven una segunda entrega.

### Negativos

1. Un byte de la firma cambiado: `vote-rejected invalid signature`. No entra al mapa de votos.
2. Un autor ausente del `ValidatorSet`, aunque su firma sea válida para su clave: `receive_vote` devuelve `invalid signature`. No suma poder.
3. `epoch`, `height` o `view` distintos: `receive_vote` los compara antes de la firma y devuelve `wrong view`. Hay tres rechazos, uno por campo.
4. Un byte del `block_hash` cambiado sin volver a firmar: el hash está dentro de `vote_bytes`, y `receive_vote` devuelve `invalid signature`.
5. Siete bytes `0xab`: no decodifican como voto ni como QC. `consensus-rejected invalid encoding`. No se llama `receive_vote`.
6. Un QC con el último byte de `SignatureBundle` alterado, y otro con el primer byte del `block_hash` alterado: `decode` arma el certificado y `verify_qc` devuelve `invalid signature` (`qc-rejected`). No sustituye al QC válido.

### Invariantes

El proceso no abre base de cadena, estado ni ledger. `rsc_getStatus` sigue en altura 0 y hash de 64 ceros. `rsc_getFinality` responde `service unavailable`. `rsc_getBlockByHeight` de altura 0 responde `not found`. El `data-dir` queda vacío. La altura de consenso permanece en 0.

### Apagado

El test envía `SIGTERM`. Cada proceso sale con código 0, sin procesos hijos, sin `LOCK`, y el directorio temporal se borra.

### Qué queda fuera

Producción automática de bloque, persistencia de finality, cambio de `rsc_getFinality`, sync y catch-up. `has_quorum`, `QuorumCertificate`, `SignatureBundle`, `verify_qc` y `verify_commit_chain` no se modifican. `rsc-consensus`, `rsc-node` y `rsc-validator` no se tocan.

### Archivos

`crates/rsc-p2p/Cargo.toml`. `crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `Cargo.lock`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline -p rsc-p2p --test two_processes -- --test-threads=1 four_validators_propagate_votes_and_qcs --nocapture`

Finished `test` en 23.80 s en la corrida previa, y la corrida que registró los `PeerId` pasó en 10.81 s. Exit 0.

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1`

Finished `test` en 3m 21s.

`src/lib.rs`: **43/43**, finished in 10.55 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **18/18**, finished in 26.71 s. Siguen las pruebas de TASK 9.1, 9.2, 9.3 y 9.4, y entra `four_validators_propagate_votes_and_qcs`.

Doc-tests: **0**, finished in 0.00 s. Exit 0. `elapsed_ms` 239939.

`cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `dev` en 28.30 s. Exit 0.

`rsc-consensus`, `rsc-node` y `rsc-validator` no se modificaron. No se ejecutaron sus suites.

### Resultado

✅ PASS.

### Cargo.lock

`rsc-p2p` declara dependencia de `rsc-genesis`. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite `--locked` compiló `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes sigue en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. Los votos Prepare, PreCommit y Commit de V0, V1 y V2 viajan por `/rsc/devnet/consensus/1`. El QC de cada fase verifica con poder 5 de 6. La altura no avanza y el bloque no se persiste. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.6 — Consenso distribuido real

### Objetivo

Cuatro procesos ejecutan Prepare, PreCommit y Commit con su propio `ConsensusEngine`. Cada uno avanza con la propuesta que recibió, los votos que recibió y las APIs ya existentes. El harness lee la salida y comprueba el apagado. No elige la fase ni arma el QC por los demás.

### Topología

El mismo génesis de TASK 9.5: chain-id 3, `devnet`, comité 4, poderes 2, 2, 1 y 1, total 6. V0 es la semilla. V1, V2 y V3 reciben `--bootstrap` de V0. Cada proceso tiene su `PeerId`. Antes de la propuesta, cada par tiene `connected`, `identified` y `handshake`.

`select_proposer` elige a V1, cuenta `3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c`.

En la corrida con salida de `four_validators_decide_from_local_engines`:

V0, PeerId `12D3KooWKYfWHbEfQQNRncYzWBMVwGubfb6w5DCjXp15fkK7r9yn`, listen `/ip4/127.0.0.1/tcp/41011/p2p/12D3KooWKYfWHbEfQQNRncYzWBMVwGubfb6w5DCjXp15fkK7r9yn`.

V1, PeerId `12D3KooWLwhjhW6p2nYP5YLfmf6sZQoYq6bTBwpgqKMxgVH68q1P`, listen `/ip4/127.0.0.1/tcp/39911/p2p/12D3KooWLwhjhW6p2nYP5YLfmf6sZQoYq6bTBwpgqKMxgVH68q1P`.

V2, PeerId `12D3KooWCv1QXnkBvNVYVpnWgFcTEcZta24jrAH56iBNrJvtsBSr`, listen `/ip4/127.0.0.1/tcp/41031/p2p/12D3KooWCv1QXnkBvNVYVpnWgFcTEcZta24jrAH56iBNrJvtsBSr`.

V3, PeerId `12D3KooW9wfwGBqc97xaKAXvPBYBTqXwFf8mRmsizVVtV3UDAP84`, listen `/ip4/127.0.0.1/tcp/38707/p2p/12D3KooW9wfwGBqc97xaKAXvPBYBTqXwFf8mRmsizVVtV3UDAP84`.

Esos `PeerId` y puertos son de esa ejecución. La suite `--locked` volvió a levantar los procesos y pasó con identidades de red nuevas.

### Propuesta y votos

V1 publica el bloque en `GossipTopic::Blocks`. El hash es `4a049f4a12646601562d42d4a9c94d8994b5dd2d28e39a33fb3e32cf0bcbfe6d`. Cada proceso lo acepta con `receive_proposal`. Epoch 0, altura 0, view 0.

V0, V1 y V2 llaman `create_vote` con su `PrivateKey` cuando su engine local ya aceptó la propuesta. El voto sale por `/rsc/devnet/consensus/1`. Cada receptor lo pasa a su `receive_vote`.

V3 está en el `ValidatorSet` con poder 1. Recibe la propuesta y los votos. No llama `create_vote`. Su cuenta no entra en los QC.

### Quorum local

Cuando el `receive_vote` de un proceso devuelve `QuorumReached`, ese proceso llama `aggregate_qc`, `verify_qc` y `has_quorum` sobre los votos que él guardó. Imprime `quorum`, la lista `voters` y `qc-ok` antes de publicar nada. El paso a PreCommit y a Commit lo dispara ese mismo `QuorumReached`, en ese proceso.

Prepare, PreCommit y Commit imprimen, en los cuatro procesos:

`quorum <fase> 5 6 15 12`

`voters <fase> 3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c 5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31`

`qc-ok <fase> 5`

El orden de `voters` es el de `aggregate_qc`: V1, V0, V2. Poder 2 + 2 + 1 = 5. `3 * 5 = 15` y `2 * 6 = 12`. La cuenta de V3, `a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a`, no aparece. Hay una sola línea `quorum` por fase.

### FinalityProof

El voto Commit que completa el poder hace que `receive_vote` devuelva `BlockFinalized`. El proceso llama `verify_commit_chain` sobre esa prueba e imprime:

`proof 4a049f4a12646601562d42d4a9c94d8994b5dd2d28e39a33fb3e32cf0bcbfe6d 0 0 0 5 5 5`

`proof-voters` repite, para Prepare, PreCommit y Commit, las tres cuentas y el poder 5.

El estado final de V0, V1, V2 y V3 es `phase Decide`, altura 0, epoch 0, view 0, propuesta `4a049f4a12646601562d42d4a9c94d8994b5dd2d28e39a33fb3e32cf0bcbfe6d`. V3 llega a `Decide` porque aplicó los votos recibidos. Su firma no está en la prueba. La altura no cambia. No se llama `new_height`.

### Dos validadores

`two_validators_do_not_reach_quorum` levanta solo V0 y V1, con el mismo génesis y el mismo comité de poder total 6. Los dos firman Prepare. El poder acumulado es 4. `has_quorum(4, 6)` es falso: `3 * 4 = 12` y `2 * 6 = 12`. Cada proceso imprime `no-quorum Prepare 4 6 12 12`.

En esa corrida, V0 es `12D3KooWCzcGTE3xLo44pyE1m4YENDk9e5afV6ZZfSEBeYUMZ8fL` y V1 es `12D3KooWDRBMhMnbrhYcAb8HfwEBVSoas7LJvwehpLgCaCESwBwR`. El proposer sigue siendo V1 y el hash del bloque es el mismo. Después de esa línea, ninguno imprime `quorum`, `qc-ok`, `finalized` ni `proof`. No hay voto PreCommit ni Commit. La fase permanece en `Propose`.

### Consistencia, duplicado y equivocación

Sobre un voto ya propagado, un solo campo alterado sigue el rechazo de `receive_vote`:

1. Un byte de la firma: `invalid signature`.
2. El autor cambiado a otra cuenta del comité, con la firma original: `invalid signature`. No suma el poder de esa cuenta.
3. Un autor fuera del `ValidatorSet`: `invalid signature`.
4. `epoch`, `height` o `view` distintos: `wrong view`.
5. Un byte del `block_hash` sin volver a firmar: `invalid signature`.

Esos votos no aumentan el poder. El QC permanece en 5.

V3 entrega otra vez el mismo voto Prepare. `receive_vote` devuelve `ProposalAccepted` e imprime `vote-duplicate`. Hay una sola línea `quorum` por fase.

Después del quorum Prepare de V0, V0 firma con `sign_vote` un segundo Prepare del mismo autor y otro `block_hash`. `receive_vote` devuelve `Equivocation` y deja el voto original. Los cuatro procesos imprimen `vote-rejected equivocation`. El QC de poder 5 se conserva.

### Invariantes

La finalidad queda en memoria. El proceso no abre base de cadena, estado ni ledger. `rsc_getStatus` sigue en altura 0 y hash de 64 ceros. `rsc_getFinality` responde `service unavailable`. `rsc_getBlockByHeight` de altura 0 responde `not found`. El `data-dir` queda vacío. No hay sync ni altura siguiente.

### Apagado

El test envía `SIGTERM`. Cada proceso sale con código 0, sin procesos hijos, sin `LOCK`, y el directorio temporal se borra.

### Qué queda fuera

Producción automática del bloque siguiente, persistencia de `FinalityProof`, cambio de `rsc_getFinality` y sync. `has_quorum`, `QuorumCertificate`, `SignatureBundle`, `verify_qc` y `verify_commit_chain` no se modifican. `rsc-consensus`, `rsc-node` y `rsc-validator` no se tocan.

### Archivos

`crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 four_validators_decide_from_local_engines two_validators_do_not_reach_quorum --nocapture`

Finished `test` en 1m 53s. Dos pruebas, finished in 24.15 s. Exit 0.

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1`

Finished `test` en 5.29 s.

`src/lib.rs`: **43/43**, finished in 10.56 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **20/20**, finished in 56.33 s. Siguen las pruebas de TASK 9.1, 9.2, 9.3, 9.4 y 9.5. Entran `four_validators_decide_from_local_engines` y `two_validators_do_not_reach_quorum`.

Doc-tests: **0**, finished in 0.00 s. Exit 0.

`cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `dev` en 9.31 s. Exit 0.

`rsc-consensus`, `rsc-node` y `rsc-validator` no se modificaron. No se ejecutaron sus suites.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambio. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite `--locked` compiló `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. V0, V1 y V2 terminan en `Decide` con el mismo hash, la misma altura, el mismo epoch y la misma view. Cada proceso verifica su `FinalityProof` en memoria, con QC de poder 5 de V0, V1 y V2. V0 y V1 solos quedan en `Propose`. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.7 — Producción automática de un bloque

### Objetivo

Una transacción Ready en el mempool hace que el proposer, elegido por `select_proposer`, arme un bloque con las APIs que ya usa `commit_ready` del nodo. El bloque viaja por `GossipTopic::Blocks`. Cada proceso cierra Prepare, PreCommit y Commit en su engine y, después de `BlockFinalized`, persiste ese bloque con `stage_transition`, `stage_canonical`, `Ledger::append` y `remove_committed`. Sale un solo bloque, de altura 0 a altura 1.

### Topología

Cuatro procesos, el génesis de TASK 9.5 y el bootstrap de TASK 9.2. V0 es la semilla. Poder 2, 2, 1 y 1. Total 6. `devnet`, chain-id 3.

`select_proposer` a altura 1 elige a V1, cuenta `3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c`.

En la corrida con salida de `four_processes_commit_one_ready_transaction`:

V0, PeerId `12D3KooWGmvAMd4UZCSAXuLJEP53TCy6MsHn96F4LpwNWZbXh2FK`, listen `/ip4/127.0.0.1/tcp/46595/p2p/12D3KooWGmvAMd4UZCSAXuLJEP53TCy6MsHn96F4LpwNWZbXh2FK`.

V1, PeerId `12D3KooWS8ayfUxvDaVvHQMcfdgF9tfbWL6zBcDxvvqdhFTaovqz`, listen `/ip4/127.0.0.1/tcp/34819/p2p/12D3KooWS8ayfUxvDaVvHQMcfdgF9tfbWL6zBcDxvvqdhFTaovqz`.

V2, PeerId `12D3KooWEu7xh6SBLSLjUGJ31P3gpK3P5HSVEH81wsctPj9JLocb`, listen `/ip4/127.0.0.1/tcp/44753/p2p/12D3KooWEu7xh6SBLSLjUGJ31P3gpK3P5HSVEH81wsctPj9JLocb`.

V3, PeerId `12D3KooWJoLPq7kZTg7UAqUveJD9H9gFCU1HdV2o75jiKErJjrYa`, listen `/ip4/127.0.0.1/tcp/44769/p2p/12D3KooWJoLPq7kZTg7UAqUveJD9H9gFCU1HdV2o75jiKErJjrYa`.

Esos `PeerId` y puertos son de esa ejecución. La suite `--locked` volvió a levantar los procesos y pasó con identidades de red nuevas.

### Génesis y transacción

Cada proceso abre la base en su `data-dir` e instala el bloque de génesis con `initialize_genesis`, `validate_block`, `put_block` y `set_canonical_tip`. Ese bloque es el padre, altura 0, hash `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`. El mempool vacío no publica un bloque de altura 1.

`rsc_sendTransaction` entrega la transferencia v1 `7d79fb0fa368f2f5e2f6400937628c522b3faa1251e4967a37bb1fc658f5525c`, de V0 hacia V1, importe 1, nonce 0. Los cuatro mempools la marcan `ready`. Una transacción de importe 0 se rechaza y no entra al mempool.

### Bloque

V1 toma esa transacción con `ready_transactions(1)`. Calcula la transición con `StateTransition` y `stage_transition` sin escribir el lote, arma el bloque con `compute_transaction_root`, la raíz nueva de estado, el hash del padre y la altura siguiente, y lo publica en `GossipTopic::Blocks`.

Hash `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`.

Padre `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`.

Altura 1.

`transactions_root` `150b9c41833304bfdfb453ae5900480e06dafc80f3a265f0f9d77a6d89706ca7`.

`state_root` `fb4a3be7462a48a82fa17b7b3b091dfbf5c90ef59038b72059b3098d3d3ef70c`.

Cada proceso lo acepta con `receive_proposal`.

### Consenso y prueba

V0, V1 y V2 firman Prepare, PreCommit y Commit. V3 no firma. Cada proceso imprime `quorum <fase> 5 6 15 12`, `qc-ok <fase> 5` y los votantes V1, V0 y V2. `receive_vote` del Commit que completa el poder devuelve `BlockFinalized`. `verify_commit_chain` acepta la prueba:

`proof e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da 0 1 0 5 5 5`

La fase queda en `Decide`. Epoch 0, view 0, altura de consenso 1. No se llama `new_height`.

### Persistencia y mempool

Después de la prueba, cada proceso escribe el mismo bloque: `stage_transition`, comprobación de la raíz de estado, `Ledger::append` de la transacción, `stage_canonical` y `write`. La raíz de ledger queda `86910eef67e68df000951eb6ebb4acb7fce418943016602f8e663386e65cc9b2`. `remove_committed` deja el mempool en `pool 0`. El hijo del génesis es uno: `children 1`.

V0 queda con balance 0 y nonce 1. V1 queda con balance 2 y nonce 0. `rsc_getStatus` muestra altura 1 y el hash del bloque. `rsc_getBlockByHeight` devuelve la altura 0 y la altura 1, y `not found` para la altura 2. `rsc_getAccount` repite esos balances. La transacción aparece una vez.

### Negativos

1. Con el mempool vacío no aparece la línea `block`. La altura sigue en 0.
2. La transferencia de importe 0 responde `transaction rejected` y no se admite.
3. Un bloque con otro proposer: `proposal-rejected invalid proposer`.
4. Un bloque con `transactions_root` alterado: `proposal-rejected invalid block`.
5. V0 y V1 firman y V2 y V3 no firman. El poder es 4. `no-quorum Prepare 4 6 12 12`. No hay `finalized`, `proof` ni `committed`. La altura sigue en 0. En esa corrida el bloque propuesto tiene el mismo hash `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`.
6. V3 vuelve a entregar el mismo bloque. Hay una sola línea `committed` y un solo hijo.

### Apagado

El test envía `SIGTERM`. Cada proceso sale con código 0, sin procesos hijos. El directorio temporal, incluida la base, se borra después de esa salida.

### Qué queda fuera

Sync, reinicio, particiones, recuperación de un validador, producción continua de más alturas, persistencia del mempool y contratos. `has_quorum`, el `ValidatorSet`, los pesos, `SignatureBundle`, el QC, `verify_qc`, `verify_commit_chain` y `FinalityProof` no se modifican. `rsc-consensus`, `rsc-node`, `rsc-mempool`, `rsc-chain`, `rsc-state`, `rsc-ledger` y `rsc-validator` no se tocan.

### Archivos

`crates/rsc-p2p/Cargo.toml`. `crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `Cargo.lock`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 four_processes_commit_one_ready_transaction power_four_does_not_finalize_a_block --nocapture`

Finished `test` en 0.59 s. Dos pruebas, finished in 17.98 s. Exit 0.

`cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `dev` en 6.25 s. Exit 0.

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1`

Finished `test` en 3m 19s.

`src/lib.rs`: **43/43**, finished in 9.33 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **22/22**, finished in 91.60 s. Siguen las pruebas de TASK 9.1 a 9.6. Entran `four_processes_commit_one_ready_transaction` y `power_four_does_not_finalize_a_block`.

Doc-tests: **0**, finished in 0.00 s. Exit 0.

Las suites de `rsc-node`, `rsc-mempool`, `rsc-chain`, `rsc-state` y `rsc-ledger` no se ejecutaron: esos crates no cambiaron.

### Resultado

✅ PASS.

### Cargo.lock

`rsc-p2p` declara dependencia de `rsc-storage`. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite `--locked` compiló `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. Una transacción Ready produce un bloque de altura 1. Los cuatro procesos quedan con el mismo hash, la misma altura, la misma raíz de estado, la misma raíz de ledger y el mismo tip. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.8 — Sincronización de bloques de extremo a extremo

### Objetivo

Un nodo que sigue en el génesis descarga los bloques que los demás ya finalizaron y llega al mismo tip canónico. El camino es el protocolo `/rsc/block-sync/1.0.0` que ya existe: `RscRequest::GetBlockByHeight` y `RscResponse::Blocks` con el `encode_block` de cada altura. Cada bloque se valida y se persiste antes de pedir el siguiente.

### Topología

El génesis, el `network-id` `devnet` y el chain-id 3 son los de TASK 9.5. El bootstrap es el de TASK 9.2. Poder 2, 2, 1 y 1. Total 6.

Fase 1: V0, V1 y V2, roles `sync-0`, `sync-1` y `sync-2`. V3 apagado. Los tres producen y finalizan el bloque 1 y después el bloque 2.

Fase 2: V3 arranca con rol `behind`, su propio `data-dir` y solo el bloque de génesis. Descubre a los otros por el bootstrap y pide los bloques. No recibe el bloque 1 ni el bloque 2 por `GossipTopic::Blocks`.

En la corrida con salida de `behind_node_syncs_two_committed_blocks`:

V0, PeerId `12D3KooWFM4babafRioHgg51YQbqd6MASGPVXYUsgFA4264drDuH`, listen `/ip4/127.0.0.1/tcp/42769/p2p/12D3KooWFM4babafRioHgg51YQbqd6MASGPVXYUsgFA4264drDuH`.

V1, PeerId `12D3KooWG3f3Sd6xg2KFMGTsBMKctqkGoHbHsAYg9gr8u7HhuLZ6`, listen `/ip4/127.0.0.1/tcp/46699/p2p/12D3KooWG3f3Sd6xg2KFMGTsBMKctqkGoHbHsAYg9gr8u7HhuLZ6`.

V2, PeerId `12D3KooWAJC6ahernfsJPvvFHc81CneEP6VgskUzTL774Ed4zEUh`, listen `/ip4/127.0.0.1/tcp/41309/p2p/12D3KooWAJC6ahernfsJPvvFHc81CneEP6VgskUzTL774Ed4zEUh`.

V3, PeerId `12D3KooWJWXEoTVhNRKu47hfC8drjivde4QhMZsMrbFELEvY8A5N`, listen `/ip4/127.0.0.1/tcp/37563/p2p/12D3KooWJWXEoTVhNRKu47hfC8drjivde4QhMZsMrbFELEvY8A5N`.

Esos `PeerId` y puertos son de esa ejecución. La suite `--locked` volvió a levantar los procesos y pasó con identidades de red nuevas.

### Alturas iniciales

V0, V1 y V2 instalan el génesis y quedan en altura 0, hash `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`. V3 todavía no existe. Cuando arranca, su base solo tiene ese mismo génesis y su primera línea `tip` termina en altura 0 con ese hash.

### Bloque 1

La transferencia v1 `7d79fb0fa368f2f5e2f6400937628c522b3faa1251e4967a37bb1fc658f5525c` va de V0 a V1, importe 1, nonce 0. `select_proposer` a altura 1 elige a V1, cuenta `3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c`.

Hash `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`.

`previous_block_hash` `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`, el hash del bloque 0.

Altura 1.

`transactions_root` `150b9c41833304bfdfb453ae5900480e06dafc80f3a265f0f9d77a6d89706ca7`.

`state_root` `fb4a3be7462a48a82fa17b7b3b091dfbf5c90ef59038b72059b3098d3d3ef70c`.

Raíz de ledger `86910eef67e68df000951eb6ebb4acb7fce418943016602f8e663386e65cc9b2`.

V0, V1 y V2 quedan con ese tip. V0 queda con balance 0 y nonce 1. V1 queda con balance 2 y nonce 0.

### Bloque 2

Después del commit, cada proceso llama `new_height(None, Hash::ZERO)`, la misma llamada que `apply_finalized` del nodo. Con esa semilla, `select_proposer` a altura 2 elige a V2, cuenta `ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31`. V2 está entre los tres procesos activos. V3 sigue apagado.

La segunda transferencia es `e491c0f05dfd9778c32132aba5bc80633856b304f3bc3dd93bd763e85a80dba9`, de V1 a V2, importe 1, nonce 0. Entra al mempool después de que el bloque 1 ya está escrito.

Hash `cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464`.

`previous_block_hash` `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`, el hash del bloque 1.

Altura 2.

`transactions_root` `735931e258ef0ee9e902c698a73c80b23d0e00503ccf58da4a9e7c9ddc7445ab`.

`state_root` `93695ca3e3d15dd4a3e71024d20b5268e315e30f54165d2e4038e6d4abed7226`.

Raíz de ledger `d331cdadfbad9e734c4e59950a6316112253b86367a97bf32b646c5b09b25219`.

V0, V1 y V2 terminan la fase 1 con tip de altura 2, esa raíz de estado y esa raíz de ledger. V1 queda con balance 1 y nonce 1. V2 queda con balance 2 y nonce 0. Hay dos líneas `committed` en cada uno. El rol `produce-N` sigue en un solo bloque: su meta es la altura 1 y no llama `new_height`. La prueba `four_processes_commit_one_ready_transaction` sigue en verde.

### Protocolo

V3 imprime `sync-protocol /rsc/block-sync/1.0.0`. Al primer `HandshakeAccepted` pide la altura 1 con `GetBlockByHeight`. Un peer que ya tiene el bloque responde `Blocks` con un solo payload, los bytes de `encode_block`, y deja `sync-serve 1` más el hash. V3 valida, persiste y pide la altura 2. El mismo intercambio trae el bloque 2. Después pide la altura 3, recibe `Empty`, imprime `sync-empty 3` y `sync-complete`.

La línea `bytes <altura> <hex>` del bloque 1 coincide en V0 y en V3. La del bloque 2 también. V3 no imprime `block`, `admitted`, `quorum` ni `finalized`. El pull no crea transacciones, no vuelve a correr el consenso de esos bloques y no llama `new_height`.

### Validación y persistencia

Antes de escribir, V3 exige, en este orden: `decode_block`, altura igual al tip más uno, `previous_block_hash` igual al hash del tip, y `validate_block`. Después `write_committed` recalcula la transición y compara `state_root` antes de `database.write`. Si la raíz no coincide, el lote no se escribe. La secuencia queda 0 → 1 → 2. El tip de V3 pasa de altura 0 a altura 2. `rsc_getStatus` de los cuatro muestra altura 2 y el hash del bloque 2. La altura 3 responde `not found`.

### Negativos

1. Un peer en altura 0 responde `Empty` a la altura 1. El nodo atrasado imprime `sync-empty 1` y `sync-failed`. No hay `committed` ni `synced`. El tip sigue en 0. Prueba `a_missing_block_fails_sync_without_writing`.
2. Siete bytes `0xab`: `sync-rejected invalid encoding`.
3. Un bloque de altura 1 con `previous_block_hash` cero: `sync-rejected invalid parent`.
4. Un bloque de altura 9: `sync-rejected invalid height`.
5. Un bloque con `transactions_root` alterado: `sync-rejected invalid block`.
6. Un bloque con `state_root` distinto del que produce la transición: `sync-rejected state root distinto`. La base no avanza.
7. La altura 9 ausente: `sync-empty 9` y `sync-failed`. El tip del auditor sigue en 0. Prueba `block_sync_rejects_invalid_payloads`.
8. Un nodo `behind` con `network-id` `mainnet` contra `devnet`: `rejected` con `network mismatch`. No hay `synced` ni `sync-complete`.
9. Un nodo `behind` con chain-id 1 contra chain-id 3: `rejected` con `chain mismatch`. No hay `synced` ni `sync-complete`. Prueba `block_sync_rejects_a_different_network_or_chain`.

### Apagado

Cada prueba envía `SIGTERM`. El proceso sale con código 0, sin procesos hijos. El directorio temporal, incluida la base, se borra después de esa salida.

### Qué queda fuera

Recuperación completa tras reinicio, catch-up automático después de un crash, particiones, stress, contratos y producción continua de más alturas. El génesis y el `ValidatorSet` no cambian. `has_quorum`, `SignatureBundle`, el QC, `verify_qc` y `verify_commit_chain` no se modifican. `rsc-node`, `rsc-validator`, `rsc-consensus` y `rsc-chain` no se tocan.

### Archivos

`crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 behind_node_syncs_two_committed_blocks --nocapture`

Finished `test` en 25.00 s. Una prueba, finished in 36.93 s. Exit 0.

`cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `dev` en 7.72 s. Exit 0.

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1`

Finished `test` en 7.44 s.

`src/lib.rs`: **43/43**, finished in 9.50 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **26/26**, finished in 103.77 s. Siguen las pruebas de TASK 9.1 a 9.7. Entran `behind_node_syncs_two_committed_blocks`, `block_sync_rejects_invalid_payloads`, `a_missing_block_fails_sync_without_writing` y `block_sync_rejects_a_different_network_or_chain`.

Doc-tests: **0**, finished in 0.00 s. Exit 0. El comando tardó 122325 ms.

`cargo test --offline --locked -p rsc-chain -- --test-threads=1`

Finished `test` en 1m 24s. `src/lib.rs`: **19/19**, finished in 4.29 s. Doc-tests: **1/1**, finished in 0.97 s. Exit 0. El crate no cambió.

`cargo clippy --offline --locked -p rsc-chain --all-targets -- -D warnings`

Finished `dev` en 58.51 s. Exit 0.

`rsc-node` y las integration tests no se ejecutaron: esos crates no cambiaron.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. Fecha 2026-09-27 20:45:11, 124448 bytes. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

Las suites compilaron `rsc-p2p` y `rsc-chain`. No listaron `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. V3 arranca en el génesis, pide el bloque 1 y el bloque 2 por `/rsc/block-sync/1.0.0`, valida y persiste cada uno, y los cuatro procesos quedan en el bloque `cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464`, con la misma raíz de estado y la misma raíz de ledger. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.9 — Reinicio y catch-up

### Objetivo

V3 se apaga con el bloque 1 ya escrito. V0, V1 y V2 siguen y finalizan el bloque 2 y el bloque 3. V3 vuelve a abrir la misma base, recupera el bloque 1 y pide por `/rsc/block-sync/1.0.0` solo las alturas que le faltan.

### Topología

El génesis, `devnet` y el chain-id 3 son los de TASK 9.5. El bootstrap es el de TASK 9.2. Los cuatro procesos arrancan con rol `catch-N`. La meta de ese rol es la altura 3. `produce-N` sigue en un solo bloque y `sync-N` sigue en dos.

En la corrida con salida de `restart_catches_up_blocks_two_and_three`:

V0, PeerId `12D3KooWMY7geeNWLY9sZgqCG2Twe3cRhwh7CPkerkkXH3fAL2Y4`, listen `/ip4/127.0.0.1/tcp/45489/p2p/12D3KooWMY7geeNWLY9sZgqCG2Twe3cRhwh7CPkerkkXH3fAL2Y4`.

V1, PeerId `12D3KooWBfqe9PnJqYPeEdpspe9sGPvfpKKuMo8HECuoWaTvYmwp`, listen `/ip4/127.0.0.1/tcp/40891/p2p/12D3KooWBfqe9PnJqYPeEdpspe9sGPvfpKKuMo8HECuoWaTvYmwp`.

V2, PeerId `12D3KooWHzZ5SwPaSXiwrdMhUau8E2ky6m96fyUuuGTG8gCwsv2S`, listen `/ip4/127.0.0.1/tcp/44705/p2p/12D3KooWHzZ5SwPaSXiwrdMhUau8E2ky6m96fyUuuGTG8gCwsv2S`.

V3, después del reinicio, PeerId `12D3KooWRJXpDBoGtL72FsrUvUgJoYkoJgCBDNssP1wDcV43Xxki`, listen `/ip4/127.0.0.1/tcp/37757/p2p/12D3KooWRJXpDBoGtL72FsrUvUgJoYkoJgCBDNssP1wDcV43Xxki`.

`LocalPeerIdentity::generate` no guarda la clave de red. El PeerId de escucha del proceso reiniciado es otro. La cuenta del validador 3 sigue siendo `a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a`, poder 1, porque el rol vuelve a usar la misma clave del índice 3. Esos PeerId son de esa ejecución. La suite `--locked` pasó con identidades de red nuevas.

### Antes del apagado

Los cuatro instalan el génesis `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36` e imprimen `installed-genesis`.

La transferencia `7d79fb0fa368f2f5e2f6400937628c522b3faa1251e4967a37bb1fc658f5525c` produce el bloque 1, el mismo de TASK 9.7. Hash `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`. Padre, el génesis. Altura 1. `state_root` `fb4a3be7462a48a82fa17b7b3b091dfbf5c90ef59038b72059b3098d3d3ef70c`. Raíz de ledger `86910eef67e68df000951eb6ebb4acb7fce418943016602f8e663386e65cc9b2`. Los cuatro tienen ese tip.

### Apagado de V3

`SIGTERM`. El proceso sale con código 0 y sin hijos. El directorio queda con `CURRENT`. La misma base se vuelve a abrir después.

### Bloques 2 y 3 sin V3

`new_height(None, Hash::ZERO)` elige a V2 en la altura 2 y otra vez en la altura 3. V2 está entre los tres procesos que siguen.

Bloque 2, el mismo de TASK 9.8. Hash `cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464`. Padre, el bloque 1. Transacción `e491c0f05dfd9778c32132aba5bc80633856b304f3bc3dd93bd763e85a80dba9`, de V1 a V2. `state_root` `93695ca3e3d15dd4a3e71024d20b5268e315e30f54165d2e4038e6d4abed7226`. Raíz de ledger `d331cdadfbad9e734c4e59950a6316112253b86367a97bf32b646c5b09b25219`.

Bloque 3. Hash `87e3196b89e2c2213bf053dd7e52f273b0879478849182a6d16aba8cc53c9359`. Padre, el bloque 2. Transacción `41d36fefb1da178f8454cf0f5ae878552bf001f564087cbba9b89ef5247b9d45`, de V2 a V0. `transactions_root` `93e892b68a8aae2550b6c9d92c15b0195dc25058c1235747cdfaf6eb865140b5`. `state_root` `6f63c7abb8b47c70d97b4c9554763f0c86f7eeda32280d4612b90873d773efaa`. Raíz de ledger `0256835d20d122d56db4a73ec674b6ea67ebdaa4c3543b19e6367c839a16de07`.

V0, V1 y V2 quedan con tres líneas `committed` y tip de altura 3. No aparece un bloque 4.

### Estado persistido y reinicio

V3 abre el mismo `data-dir`, el mismo `network-id`, el mismo chain-id y el rol `behind`. Imprime `recovered 1` con el hash del bloque 1 y `stored 1` con los mismos bytes que tenía antes del apagado. No imprime `installed-genesis`. La primera línea `tip` es la altura 1. La cuenta impresa es otra vez el validador 3.

El pull pide `GetBlockByHeight` desde la altura guardada más uno. La secuencia es `sync-request 2`, `synced 2`, `sync-request 3`, `synced 3`, `sync-request 4`, `sync-empty 4`, `sync-complete`. No hay `sync-request 1`. Cada bloque pasa por `decode_block`, la altura contigua, el padre y `validate_block` antes de `write_committed`. Los bytes 2 y 3 coinciden con los de V0. V3 no imprime `block`, `admitted`, `quorum` ni `finalized`. Escribe dos commits nuevos, los de las alturas 2 y 3, y no reescribe el bloque 1.

Al terminar, los cuatro `rsc_getStatus` muestran altura 3 y el hash del bloque 3. La altura 4 responde `not found`. En V3, V0 queda con balance 1 y nonce 1, V1 con balance 1 y nonce 1, y V2 con balance 1 y nonce 1. La raíz de estado y la raíz de ledger del commit 3 coinciden con las de V0.

### Negativos

1. Reabrir la base de V3 con `network-id` `mainnet`: el proceso sale con código distinto de 0 y `storage network mismatch`.
2. Reabrirla con chain-id 1: `storage chain mismatch`.
3. Un peer que solo tiene el bloque 1 responde `Empty` a la altura 2. El reinicio imprime `sync-empty 2` y `sync-failed`. El tip sigue en el bloque 1. Prueba `restart_rejects_bad_payloads_and_a_short_peer`.
4. Con el bloque 1 ya recuperado, un bloque de altura 2 con `transactions_root` alterado: `sync-rejected invalid block`.
5. Un bloque de altura 2 con padre cero: `sync-rejected invalid parent`.
6. Un bloque de altura 3 pedido mientras el tip sigue en 1: `sync-rejected invalid height`. No hay `synced` ni `committed`. `rsc_getStatus` sigue en altura 1 y la altura 2 responde `not found`.

### Apagado final

Cada prueba cierra los procesos con `SIGTERM`, código 0 y sin hijos. El directorio temporal se borra después de esa salida.

### Qué queda fuera

Particiones, stress, contratos y un bloque 4. `has_quorum`, el QC, `verify_qc`, `verify_commit_chain` y `FinalityProof` no cambian. `rsc-validator`, `rsc-node`, `rsc-chain`, `rsc-storage` y `rsc-consensus` no se tocan.

### Archivos

`crates/rsc-p2p/src/bin/rsc-p2p.rs`. `crates/rsc-p2p/tests/two_processes.rs`. `docs/rsc-chain.md`.

### Comandos

`cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 restart_catches_up_blocks_two_and_three --nocapture`

Finished `test` en 8.41 s. Una prueba, finished in 36.57 s. Exit 0.

`cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings`

Finished `dev` en 4.55 s. Exit 0.

`cargo test --offline --locked -p rsc-p2p -- --test-threads=1`

Finished `test` en 2.05 s.

`src/lib.rs`: **43/43**, finished in 9.08 s.

`src/bin/rsc-p2p.rs`: **0**, finished in 0.00 s.

`tests/two_processes.rs`: **28/28**, finished in 184.56 s. Siguen las pruebas de TASK 9.1 a 9.8. Entran `restart_catches_up_blocks_two_and_three` y `restart_rejects_bad_payloads_and_a_short_peer`.

Doc-tests: **0**, finished in 0.00 s. Exit 0. El comando tardó 204192 ms.

`rsc-node`, `rsc-chain` y `rsc-storage` no se ejecutaron: esos crates no cambiaron.

### Resultado

✅ PASS.

### Cargo.lock

Sin cambios. Fecha 2026-09-27 20:45:11, 124448 bytes. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite compiló `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Abortos

Ninguno.

### Estado

✅ CLOSED. V3 vuelve a abrir la base del bloque 1, sincroniza el bloque 2 y el bloque 3, y los cuatro procesos quedan en `87e3196b89e2c2213bf053dd7e52f273b0879478849182a6d16aba8cc53c9359`, con la misma raíz de estado y la misma raíz de ledger. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.10 — Devnet operacional

### Objetivo

Un harness arranca cuatro procesos `rsc-p2p` ya probados, observa la red por RPC, produce los bloques 1, 2 y 3, los detiene, los vuelve a abrir sobre las mismas bases y produce el bloque 4.

### Arquitectura

El harness está en `devnet_produces_blocks_restarts_and_continues`. Orquesta el binario existente. El rol `devnet-N` es el rol de producción con meta de altura 4. El comité, los poderes 2, 2, 1 y 1, el quorum de poder 5 y `has_quorum` siguen iguales. El bootstrap es el de TASK 9.2: validator-1, validator-2 y validator-3 reciben solo la dirección de validator-0.

El génesis lo construye cada proceso con `vote_genesis` y el chain-id 3. Es el mismo génesis de TASK 9.5: `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`. La clave de cada validador sigue saliendo del índice del rol. `LocalPeerIdentity::generate` sigue creando el PeerId de escucha en cada arranque.

Si la base ya tiene tip, el engine de un rol con meta mayor que 0 arranca en la altura siguiente con `Hash::ZERO`, el mismo argumento que `new_height` usa desde TASK 9.8. Una base nueva, con tip 0, sigue creando el engine en la altura 1 con la semilla de génesis.

La espera de readiness usa `GET /health`, que ya responde `{"status":"ok"}`, y las líneas `connected` e `identified` del descubrimiento. La mempool se confirma con la línea `ready`. Los bloques los construye el proposer del proceso, a partir de la transacción que entra por `rsc_sendTransaction`.

`rsc_getTransaction` recorre los bloques canónicos ya persistidos, igual que `rsc-node`. `rsc_getBalance` lee la cuenta persistida. `rsc_getFinality` sigue respondiendo que el servicio de consenso no está disponible, porque este proceso guarda el bloque finalizado y la altura en `rsc_getStatus.finalized_height`. Los encuadres de QC siguen sin persistirse, igual que en `rsc-node`.

### Corrida observada

En `devnet_produces_blocks_restarts_and_continues --nocapture`:

Antes del apagado:

- validator-0, PeerId `12D3KooWBhKwJBKHvJMXx64FHQBgJgLH3c2gJrrtzdCTViVZqjCr`, P2P `/ip4/127.0.0.1/tcp/44923`, RPC `127.0.0.1:45175`, data-dir `/tmp/rsc-p2p-devnet-0-151722-1790562003497745759`
- validator-1, PeerId `12D3KooWKyenpaVXnGija9HHTtQBd28c9t1DtsBBtZWUUPvikkE5`, P2P `/ip4/127.0.0.1/tcp/34067`, RPC `127.0.0.1:34543`, data-dir `/tmp/rsc-p2p-devnet-1-151722-1790562003498833777`
- validator-2, PeerId `12D3KooWB8bsVTafyDRcv1M3UsYqeuhPk3ksUzkH9sG158yohuZe`, P2P `/ip4/127.0.0.1/tcp/46541`, RPC `127.0.0.1:37413`, data-dir `/tmp/rsc-p2p-devnet-2-151722-1790562003499314122`
- validator-3, PeerId `12D3KooWDnMzuaFnwa9pQZ1Rk3zMFueEbmL3TLFT181upS45uci2`, P2P `/ip4/127.0.0.1/tcp/33623`, RPC `127.0.0.1:34689`, data-dir `/tmp/rsc-p2p-devnet-3-151722-1790562003499749740`

Las cuentas siguen siendo V0 `5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f` poder 2, V1 `3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c` poder 2, V2 `ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31` poder 1 y V3 `a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a` poder 1. Después del reinicio cada proceso imprimió la misma línea `validator`.

Con la mempool vacía, `rsc_getStatus` quedó en altura 0 y `finalized_height` 0. Una transferencia de monto 0 fue rechazada y no apareció ningún bloque.

La transferencia `7d79fb0fa368f2f5e2f6400937628c522b3faa1251e4967a37bb1fc658f5525c` produjo el bloque 1 `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`. Padre, el génesis. `state_root` `fb4a3be7462a48a82fa17b7b3b091dfbf5c90ef59038b72059b3098d3d3ef70c`. Raíz de ledger `86910eef67e68df000951eb6ebb4acb7fce418943016602f8e663386e65cc9b2`.

La transferencia `e491c0f05dfd9778c32132aba5bc80633856b304f3bc3dd93bd763e85a80dba9` produjo el bloque 2 `cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464`. Padre, el bloque 1. `state_root` `93695ca3e3d15dd4a3e71024d20b5268e315e30f54165d2e4038e6d4abed7226`. Raíz de ledger `d331cdadfbad9e734c4e59950a6316112253b86367a97bf32b646c5b09b25219`.

La transferencia `41d36fefb1da178f8454cf0f5ae878552bf001f564087cbba9b89ef5247b9d45` produjo el bloque 3 `87e3196b89e2c2213bf053dd7e52f273b0879478849182a6d16aba8cc53c9359`. Padre, el bloque 2. `state_root` `6f63c7abb8b47c70d97b4c9554763f0c86f7eeda32280d4612b90873d773efaa`. Raíz de ledger `0256835d20d122d56db4a73ec674b6ea67ebdaa4c3543b19e6367c839a16de07`.

Los cuatro procesos imprimieron la misma línea `committed` en cada altura. `rsc_getStatus` mostró altura 3, `finalized_height` 3 y el hash del bloque 3. `rsc_getBlockByHeight` devolvió los tres hashes y los padres contiguos. `rsc_getTransaction` marcó cada transferencia como `included` en su altura. `rsc_getBalance` devolvió saldo 1 en las cuatro cuentas. `rsc_getFinality` respondió servicio no disponible. La altura 4 no existía.

### Apagado y recuperación

`SIGTERM` en los cuatro. Cada uno salió con código 0 y sin hijos. Los data-dir se conservaron. El segundo arranque de cada directorio imprimió `recovered 3 87e3196b89e2c2213bf053dd7e52f273b0879478849182a6d16aba8cc53c9359`, `stored 3` con los mismos bytes del bloque 3 y `roots` con el `state_root` y la raíz de ledger del bloque 3. No apareció `installed-genesis`. El proceso reiniciado no reescribió los bloques 1, 2 ni 3, y antes de la transacción nueva no emitió votos, finality ni QC.

RocksDB puede dejar el nombre `LOCK` después de cerrar. La prueba de que el lock quedó libre es que la misma base se volvió a abrir y recuperó el tip.

Después del reinicio, en la misma corrida:

- validator-0, PeerId `12D3KooWMQEZ6w1V9ePwgSHzJs7QvEtkt6BZnF8tjdwNChWLCxoe`, P2P `/ip4/127.0.0.1/tcp/46017`, RPC `127.0.0.1:40363`
- validator-1, PeerId `12D3KooWGdRgzjYTv86JnrypMPocgMWgfAdZkS7ux4tW5LbZoqqu`, P2P `/ip4/127.0.0.1/tcp/45363`, RPC `127.0.0.1:42561`
- validator-2, PeerId `12D3KooWCzK33RRotSDJaGFBwqeKJBU6BNkT2CyRwcm9J4zztbKL`, P2P `/ip4/127.0.0.1/tcp/42679`, RPC `127.0.0.1:40825`
- validator-3, PeerId `12D3KooWFK6y9uvfyYjo5E15iNGJf67tiizZS7uBgzwuKV5BvZGE`, P2P `/ip4/127.0.0.1/tcp/42861`, RPC `127.0.0.1:37243`

Los data-dir fueron los mismos. `rsc_getStatus` volvió a la altura 3 y al hash del bloque 3. La malla P2P volvió a quedar `connected` e `identified`.

La transferencia `5bccad5e4521f7eb94bd8f57a981bc9e60f0f320bb6cf67d143331a72b7bf8a6` produjo el bloque 4 `7321765778f964bb3090105cff7c71a7bed56db424e4f3f6b57dc2ed97b2c14a`. Padre, el bloque 3. Altura 4. `transactions_root` `da3bdbcd5c81955b5fa8b315563f4f7cf369da55d749e48a3f49da9eb28c11c4`. `state_root` `4a1fe7e5fa91241dde902e1a00cf7b494d084728dd3c9e63c1cbb1bdda16964b`. Raíz de ledger `f86b22a9d1cab6cae2475ea21ce67b158618830db58a1cec919067277df78bd2`.

Los cuatro procesos imprimieron esa única línea `committed` en el proceso reiniciado. `rsc_getStatus` mostró altura 4, `finalized_height` 4 y el hash del bloque 4. `rsc_getBlockByHeight` devolvió el bloque 4 con padre el bloque 3, y el bloque 3 siguió presente. `rsc_getTransaction` marcó la transferencia como `included` en la altura 4. `rsc_getBalance` devolvió saldo 0 en V3 y saldo 2 en V0. La altura 5 no existía. `rsc_getFinality` siguió respondiendo servicio no disponible.

Esos PeerId, puertos y data-dir son de esa ejecución. La suite `--locked` pasó con identidades de red nuevas.

### Negativos

`devnet_rejects_mismatched_identity_and_busy_ports` abre una base `devnet` con chain-id 3 y después:

- network-id `mainnet` sobre esa base termina con `network mismatch` y salida distinta de 0;
- chain-id 1 sobre esa base termina con `chain mismatch` y salida distinta de 0. El chain-id es la identidad de génesis que la base ya valida;
- el rol `foreign` compara el registro del validador 0 con la clave del validador 1, los mismos campos que `create_vote`, y sale con `invalid signature` antes de crear una base;
- un puerto RPC ya ocupado sale con `Address already in use`;
- un puerto P2P ya ocupado sale con `listen failed`.

En la corrida principal, la mempool vacía dejó la altura en 0 y la transferencia de monto 0 fue rechazada.

### Comandos

```text
cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 devnet_produces_blocks_restarts_and_continues devnet_rejects_mismatched_identity_and_busy_ports --nocapture
```

`Finished test` en 1m 03s. 2 passed en 50.21s. Exit 0.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 7.01s. Exit 0.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 32.69s. Lib 43/43 en 9.85s. Bin 0. `two_processes` 30/30 en 230.61s. Doc 0. Exit 0.

`rsc-node`, `rsc-chain`, `rsc-storage` y `rsc-integration-tests` no se modificaron. Sus suites no se ejecutaron. `rsc-validator` no se modificó.

### Cargo.lock

Sin cambios. Fecha 2026-09-27 20:45:11, 124448 bytes. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite compiló `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Archivos

- `crates/rsc-p2p/src/bin/rsc-p2p.rs`
- `crates/rsc-p2p/tests/two_processes.rs`
- `docs/rsc-chain.md`

### Abortos

Ninguno.

### Estado

✅ CLOSED. Los cuatro procesos producen los bloques 1, 2 y 3, se detienen, recuperan el bloque 3 y producen el bloque 4 `7321765778f964bb3090105cff7c71a7bed56db424e4f3f6b57dc2ed97b2c14a`, con el mismo tip, los mismos hashes, la misma raíz de estado y la misma raíz de ledger. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.11 — Caída y recuperación

### Objetivo

Con los cuatro procesos de la devnet en marcha, apagar un validador que no hace falta para el quorum, dejar que los otros tres finalicen el bloque siguiente y volver a incorporar al apagado por el block-sync ya existente.

### Comité

El total sigue en 6. V0 y V1 aportan poder 2. V2 y V3 aportan poder 1. V0, V1 y V2 suman 5. `3 * 5 = 15` y `2 * 6 = 12`. `has_quorum` acepta 15 > 12. V3, cuenta `a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a`, sigue sin firmar. Apagarlo deja el poder votante en 5.

### Altura antes de la caída

En `noncritical_validator_outage_keeps_quorum_and_catches_up --nocapture` los cuatro producen el bloque 1 `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`. Padre, el génesis `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`. `state_root` `fb4a3be7462a48a82fa17b7b3b091dfbf5c90ef59038b72059b3098d3d3ef70c`. Raíz de ledger `86910eef67e68df000951eb6ebb4acb7fce418943016602f8e663386e65cc9b2`. Transferencia `7d79fb0fa368f2f5e2f6400937628c522b3faa1251e4967a37bb1fc658f5525c`.

Después de ese commit, `select_proposer` deja la altura 2 en V2 `ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31`. V3 no es el proposer. `SIGTERM` cierra V3 con código 0 y sin hijos. La base conserva `CURRENT`. Abrirla de nuevo con otro network-id o con chain-id 9 falla antes de borrar nada.

### Bloque con V3 apagado

V0, V1 y V2 siguen activos. La transferencia `e491c0f05dfd9778c32132aba5bc80633856b304f3bc3dd93bd763e85a80dba9` produce el bloque 2 `cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464`. Padre, el bloque 1. `state_root` `93695ca3e3d15dd4a3e71024d20b5268e315e30f54165d2e4038e6d4abed7226`. Raíz de ledger `d331cdadfbad9e734c4e59950a6316112253b86367a97bf32b646c5b09b25219`.

En los tres procesos la línea es `proof cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464 0 2 0 5 5 5`. Prepare, PreCommit y Commit quedan en poder 5. Los votantes son V1, V0 y V2. V3 no aparece. `finalized` nombra ese hash. Cada proceso tiene una sola línea `committed` de altura 2 y una sola instalación de génesis. `rsc_getStatus` muestra altura 2, `finalized_height` 2 y el mismo hash.

En esa corrida los procesos que siguieron fueron:

- V0, PeerId `12D3KooWSp1ADSsGsLNfbZBSvZFTwHX97Qg9NKY2SjaE79SZsVLt`, listen `/ip4/127.0.0.1/tcp/41895/p2p/12D3KooWSp1ADSsGsLNfbZBSvZFTwHX97Qg9NKY2SjaE79SZsVLt`
- V1, PeerId `12D3KooWJHnUtiCfrUYANw9Y6sU6NLqe7oStw5QrGDodc1xedLur`, listen `/ip4/127.0.0.1/tcp/33189/p2p/12D3KooWJHnUtiCfrUYANw9Y6sU6NLqe7oStw5QrGDodc1xedLur`
- V2, PeerId `12D3KooWJcyRrFVoM2LCfxKCrQFbHWx3PQ3Bv7nNMJGh3hpLR8U7`, listen `/ip4/127.0.0.1/tcp/44271/p2p/12D3KooWJcyRrFVoM2LCfxKCrQFbHWx3PQ3Bv7nNMJGh3hpLR8U7`

### Restart y block-sync

V3 vuelve con la misma base, el rol `behind` del índice 3 y la misma cuenta. El PeerId de escucha de esa ejecución es `12D3KooWMTZssGtfV6fVMZ4KHCTEfB49cJvVebw4A31d88bwgrvm`, listen `/ip4/127.0.0.1/tcp/43441/p2p/12D3KooWMTZssGtfV6fVMZ4KHCTEfB49cJvVebw4A31d88bwgrvm`. La clave de red sigue sin persistirse. La línea `validator 3` coincide con la de antes del apagado.

El arranque imprime `recovered 1` con el hash del bloque 1 y `stored 1` con los mismos bytes. No imprime `installed-genesis`. Pide la altura 2 por `/rsc/block-sync/1.0.0`, valida el bloque 2 y lo persiste. La línea `committed` y los bytes coinciden con los de V0, V1 y V2. Después pide la altura 3, recibe vacío y cierra con `sync-complete`. No emite votos, `quorum` ni `finalized`. `rsc_getBlockByHeight`, `rsc_getTransaction` y `rsc_getStatus` quedan iguales en los cuatro procesos. La altura 3 no existe.

RocksDB puede dejar el nombre `LOCK` después de cerrar. La misma base se volvió a abrir y recuperó el bloque 1.

### Negativos

Sobre la base de V3, todavía en el bloque 1:

- network-id `mainnet` termina con `network mismatch`;
- chain-id 9 termina con `chain mismatch`. Esa es la validación de identidad de storage para una base de otra cadena.

`catch_up_rejects_corrupt_parent_and_height` deja dos bases en el bloque 1, sirve los payloads inválidos con el rol `gap` y el rol `probe` los rechaza: `invalid block`, `invalid parent`, `invalid height` y `sync-failed`. El tip sigue en el bloque 1. No hay `committed` ni `synced`.

### Limitación observada

`proposer_offline_does_not_change_view` finaliza el bloque 1 y lee el proposer de la altura 2: V2 `ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31`. Apaga ese proceso y envía otra transferencia. Durante 25 s no aparece un bloque de altura 2. El tip sigue en `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`. El engine no elige otro líder. No se agregó view-change ni se cambió `select_proposer`.

V2 también forma parte de los tres firmantes que suman poder 5, porque V3 no vota. La corrida de éxito deja a V2 en línea y apaga a V3.

### Comandos

```text
cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 noncritical_validator_outage_keeps_quorum_and_catches_up proposer_offline_does_not_change_view catch_up_rejects_corrupt_parent_and_height --nocapture
```

`Finished test` en 10.80 s. 3 passed en 88.84 s. Exit 0.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 6.03 s. Exit 0.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 2.14 s. Lib 43/43 en 11.44 s. Bin 0. `two_processes` 33/33 en 361.89 s. Doc 0. Exit 0.

`rsc-node`, `rsc-chain`, `rsc-storage` y `rsc-integration-tests` no se modificaron. Sus suites no se ejecutaron. El binario `rsc-p2p` tampoco se modificó. `rsc-validator` sigue congelado.

### Cargo.lock

Sin cambios. Fecha 2026-09-27 20:45:11, 124448 bytes. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite compiló los tests de `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Archivos

- `crates/rsc-p2p/tests/two_processes.rs`
- `docs/rsc-chain.md`

### Abortos

Ninguno.

### Estado

✅ CLOSED. Con V3 apagado, V0, V1 y V2 finalizan el bloque 2 `cba8cb4683cf09a046f6def65df6f3be3c825e5c481fa54bc81d2f0104d0c464` con QC de poder 5. V3 recupera ese bloque por `/rsc/block-sync/1.0.0` y los cuatro quedan en el mismo tip, con los mismos hashes, la misma raíz de estado y la misma raíz de ledger. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## TASK 9.12 — Corrida larga

### Criterio

La métrica es el número de bloques, no un reloj fijo. `devnet_long_run_stays_converged` usa el mismo arranque de la devnet, con rol `stress-N`. Ese rol es el de producción ya cerrado y su meta de altura es 48, para poder pasar los cuatro bloques de `devnet-N`. El consenso, `has_quorum`, el comité y el block-sync no cambian. No hay view-change.

Los cuatro procesos comparten el génesis `f1fce85c2a9619e1938931e69991682371c01240ece2f62bb6c706b3e8467a36`. Cada bloque se espera con `committed`, `finalized` y la línea `proof` en poder 5, 5 y 5. Los bytes, el padre, el `StateRoot` y la raíz de ledger se comparan entre los procesos vivos antes de seguir.

### Corrida

En `--nocapture`, el reloj interno marcó 336 s. Se enviaron 35 transacciones y se comprometieron 35. Hubo 4 rechazos, 0 reintentos y 1 reinicio. La altura final es 35.

Los bloques 1 a 30 salen de una transferencia por altura, por RPC. El bloque 1 sigue siendo `e9cb8fe3d66fe2d2bf54d43b5da0a61727de782462ed9a834c1f9180906aa0da`. El bloque 30 es `3c083c818223ea720935a54fe999b164b7deb2bbc351c7544f2f2e259ca3b6d9`, con `state_root` `29176b452efed5fb96e107485d54761904b7a9f49a5db0ee14469604231a32d5` y raíz de ledger `0c9e8d04eff7fbb751092bb577a2de2989efa719560710898544016a81911155`.

El burst mete tres transferencias listas a la vez. El mempool las entrega de a una, en el orden `(sender, nonce, hash)`. Quedan los bloques 31 `f3344492b475543624e7b094bfad6dcdb245f7aec44d20d97cc3cbeff90e6289`, 32 `4fc85306e1fa9fb1f2836bd8c5ac550582f8375b27af0610e0e8af492f614ace` y 33 `48af71d4beb0dec66161ddc829fc39299a6c4f4c44ec174bb63d72d13bbcedd5`.

La misma transacción `f5af282d9ce49d4a6b8950706c0bbef2c8b87c25f14ca52af348a8c62e4acae0`, reenviada mientras sigue en la mempool, recibe `transaction is already in the mempool` y entra una sola vez, en el bloque 34 `c064939237e2ed4a996156f1a12ac7cfa62fb19785514df4c121948acdec3fa4`. Después del commit, el pool ya la borró y un reenvío vuelve a responder el mismo hash. Durante 12 s la altura sigue en 34. `rsc_getTransaction` la muestra una vez, en esa altura. No hay segundo append.

Monto 0, firma alterada y payload `00` responden `transaction rejected`. La altura no se mueve.

Con la altura en 34, el proposer del bloque siguiente es V1 `3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c`. Se apaga V3, que no es ese proposer. V0, V1 y V2 finalizan el bloque 35 `64c15db1eb978d34b2d368c83e3be7a9159470d1d40f81e3da6fc4a378e7069a`. `state_root` `5d8c3ea8e001a6381f90ae5427e2b26658536b5304f1d5e9cbb18e4582b8cb09`. Raíz de ledger `fef5f676180301ab6e625ca662c75625eea6d8bd29831b20c5c07ee25efa0fbe`. El QC queda en poder 5. V3 no está entre los votantes.

V3 reabre la misma base, recupera la altura 34 y pide la 35 por `/rsc/block-sync/1.0.0`. Los bytes del bloque 35 coinciden. No emite votos, `quorum` ni `finalized`. Los cuatro procesos responden el mismo hash, el mismo padre y la misma transacción en las alturas 1 a 35. La altura 36 no existe.

### Comandos

```text
cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 devnet_long_run_stays_converged --nocapture
```

`Finished test` en 16.25 s. 1 passed en 336.71 s. Exit 0.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 8.88 s. Exit 0.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 25.73 s. Lib 43/43 en 13.06 s. Bin 0. `two_processes` 34/34 en 864.48 s. Doc 0. Exit 0.

`rsc-node`, `rsc-chain`, `rsc-storage` y `rsc-integration-tests` no se modificaron. Sus suites no se ejecutaron. `rsc-validator` sigue congelado.

### Cargo.lock

Sin cambios. Fecha 2026-09-27 20:45:11, 124448 bytes. `rocksdb` sigue en 0.25.0, checksum `d8d90add70d1d420ee487bce4a1449880a8d147451c6051b2ee5f8354553dcbf`.

### RocksDB

La suite compiló `rsc-p2p`. No listó `librocksdb-sys` ni `*.cc`. El `librocksdb.a` de 457632132 bytes conserva la fecha 2026-09-27 12:31:03 en el `out` de `librocksdb-sys-38b69e303fd1b6b2`. No hubo `cargo clean`. No se cambió `CARGO_TARGET_DIR`. `ROCKSDB_LIB_DIR` apuntó a ese `out`, con `ROCKSDB_STATIC=1`.

### Archivos

- `crates/rsc-p2p/src/bin/rsc-p2p.rs`
- `crates/rsc-p2p/tests/two_processes.rs`
- `docs/rsc-chain.md`

### Abortos

Ninguno.

### Estado

✅ CLOSED. 35 bloques, 35 transacciones comprometidas, 4 rechazos y una recuperación de V3. Los cuatro procesos terminan en `64c15db1eb978d34b2d368c83e3be7a9159470d1d40f81e3da6fc4a378e7069a`, con el mismo historial, el mismo `StateRoot` y la misma raíz de ledger. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

## Dominio — `reeskova-chain.xyz`

### Causa

El nombre no está delegado. `dig @8.8.8.8 reeskova-chain.xyz A`, `dig @8.8.8.8 reeskova-chain.xyz NS` y `dig @1.1.1.1 rpc.reeskova-chain.xyz A` responden `NXDOMAIN`. La autoridad es el SOA de `.xyz`, `ns0.centralnic.net`. `host reeskova-chain.xyz` responde `not found: 3(NXDOMAIN)`. `curl` a `https://rpc.reeskova-chain.xyz/health` sale con código 6, sin resolver el nombre.

No hay resolución DNS pública. El RPC no fue alcanzado. No se ejecutaron `rsc_chainId`, `rsc_getStatus`, `rsc_getBlockByHeight` ni `rsc_getBalance` contra la devnet. La causa es la delegación DNS, no RSC Chain.

### Arquitectura requerida

```text
reeskova-chain.xyz
  → servidor público
  → HTTPS reverse proxy

rpc.reeskova-chain.xyz
  → GET /health
  → POST /rpc
  → RSC RPC interno en 127.0.0.1:8545

bootstrap.reeskova-chain.xyz
  → endpoint público de bootstrap/libp2p
```

El proxy solo reenvía `GET /health` y `POST /rpc`. No publica el directorio de datos ni un puerto de control. El puerto P2P queda fuera de ese virtual host.

`192.168.1.12` es la IP privada de este host, detrás de `192.168.1.1`. Queda descartada como registro A público.

Cuando exista un host público real, los registros son:

```text
reeskova-chain.xyz.            A    <IP pública del servidor>
rpc.reeskova-chain.xyz.        A    <IP pública del proxy HTTPS>
bootstrap.reeskova-chain.xyz.  A    <IP pública del proceso semilla>
```

La comprobación externa, todavía pendiente, es `curl https://rpc.reeskova-chain.xyz/health` y después `rsc_chainId`, `rsc_getStatus`, `rsc_getBlockByHeight` y `rsc_getBalance`.

### Endpoints objetivo

| Nombre | Destino |
| --- | --- |
| `https://rpc.reeskova-chain.xyz/health` | `GET /health` del RPC ya existente |
| `https://rpc.reeskova-chain.xyz/rpc` | `POST /rpc` del RPC ya existente |
| `bootstrap.reeskova-chain.xyz` | Semilla libp2p ya existente |

El multiaddr de bootstrap conserva el formato actual:

```text
/dns4/bootstrap.reeskova-chain.xyz/tcp/<PORT>/p2p/<PEER_ID>
```

`<PORT>` y `<PEER_ID>` salen de la línea `listen` del proceso semilla. `LocalPeerIdentity::generate` no persiste la identidad, así que el `PeerId` cambia en cada arranque. No se escribe un `PeerId` permanente en DNS.

### Código

Sin cambios. No se modificó `rsc-core`, consenso, `rsc-validator`, cadena ni P2P. No se implementó la app móvil.

### Estado

⚠ BLOCKED. `reeskova-chain.xyz` y `rpc.reeskova-chain.xyz` responden `NXDOMAIN`. Falta delegar el dominio, publicar los tres nombres sobre un host público y poner el reverse proxy delante de `127.0.0.1:8545`. Fase 9 sigue OPEN. `rsc-validator` sigue congelado.

### Sonda 2026-09-28 01:42 -03

Los tres nombres ya resuelven. No apuntan a un host de RSC.

`dig @8.8.8.8 reeskova-chain.xyz A` y `dig @1.1.1.1 reeskova-chain.xyz A` responden `NOERROR` con `207.207.210.107` y `207.207.210.229`. `rpc.reeskova-chain.xyz` y `bootstrap.reeskova-chain.xyz` son `CNAME` de `pixie.porkbun.com`, que resuelve a esas mismas direcciones. Es el host de parking del registrador, no el proceso de la devnet.

`curl https://rpc.reeskova-chain.xyz/health` y `POST https://rpc.reeskova-chain.xyz/rpc` fallan en el handshake TLS (`sslv3 alert handshake failure`). `http://rpc.reeskova-chain.xyz/health` responde `301` de `openresty` hacia `http://reeskova-chain.xyz`. No es `{"status":"ok"}` ni un cuerpo JSON-RPC.

No se ejecutaron `rsc_chainId`, `rsc_getStatus`, `rsc_getBlockByHeight` ni `rsc_getBalance`. No hay proxy hacia `127.0.0.1:8545`. Este equipo sigue en `192.168.1.12`; esa dirección no se publica. No hay `PeerId` en DNS. El código de `rsc-core`, consenso, `rsc-validator`, cadena y storage no cambió.

⚠ BLOCKED. Falta un host con IP pública propia, registros A hacia esa IP y el reverse proxy HTTPS delante del RPC interno.

### Identidad de bootstrap — 2026-09-28 01:48 -03

No hay VPS ni IP pública controlada. `dig @1.1.1.1 rpc.reeskova-chain.xyz A` sigue en `CNAME pixie.porkbun.com` hacia `207.207.210.107` y `207.207.210.229`. `192.168.1.12` no se publicó. No hay certificado, reverse proxy ni `curl` externo hacia el RPC de RSC. `rsc_chainId`, `rsc_getStatus`, `rsc_getBlockByHeight` y `rsc_getBalance` no se ejecutaron contra la devnet pública.

El `PeerId` no entra en DNS. El proceso, sin flag, sigue generando la identidad en cada arranque. Con `--persist-peer <ruta>` guarda solo la clave Ed25519 de libp2p, en protobuf, modo `0600`. Si el archivo ya existe, lo reabre. Si está dañado, sale con `network identity` y no lo reemplaza. No es una clave de consenso ni de validador.

`persist_peer_id_survives_restart` arrancó el binario, lo cerró con `SIGTERM` y lo volvió a abrir con el mismo archivo. La línea `peer` coincidió. La línea `listen` llevó ese mismo `PeerId`. El archivo dañado quedó en `not-a-key`.

```text
cargo test --offline --locked -p rsc-p2p --test two_processes -- --test-threads=1 persist_peer_id_survives_restart --nocapture
```

1 passed en 0.26 s. Exit 0.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 7.06 s. Exit 0.

La suite completa de `two_processes` no se reejecutó. `Cargo.lock` sigue en 124448 bytes, fecha 2026-09-27 20:45:11. `librocksdb.a` sigue en 457632132 bytes, fecha 2026-09-27 12:31:03. No hubo `cargo clean`.

Archivos: `crates/rsc-p2p/src/bin/rsc-p2p.rs`, `crates/rsc-p2p/tests/two_processes.rs`, `docs/rsc-chain.md`. No se tocó `rsc-core`, `rsc-consensus`, `rsc-validator`, `rsc-chain` ni `rsc-storage`.

El multiaddr público queda:

```text
/dns4/bootstrap.reeskova-chain.xyz/tcp/<PORT>/p2p/<PEER_ID>
```

`<PORT>` y `<PEER_ID>` salen de `listen` del proceso que usa `--persist-peer`. Hoy ese nombre DNS no llega a ese proceso.

✅ PASS la identidad persistente: `--persist-peer` conserva el `PeerId` entre reinicios. ⚠ BLOCKED el endpoint público. Falta un host con IPv4 pública, registros A de `@`, `rpc` y `bootstrap` hacia esa IP, TLS y un reverse proxy de `GET /health` y `POST /rpc` hacia `127.0.0.1:8545`. El puerto libp2p queda fuera de ese proxy. `rsc-consensus`, `rsc-validator`, `rsc-chain` y `rsc-storage` no se modifican para salir de este bloqueo.

### Estado oficial — 2026-09-28

PUBLIC RPC = ⚠ BLOCKED. `rsc-p2p` y `--persist-peer` quedan PASS. `rsc-validator` sigue congelado. `rsc-consensus`, `rsc-chain` y `rsc-storage` quedan sin cambios. No se programan funciones nuevas de blockchain mientras `rpc.reeskova-chain.xyz` siga en el parking de Porkbun. El paso siguiente es un host con IPv4 pública.

## TASK 9.13 — Presencia autónoma, primer corte

### Alcance

Observación, UPnP y AutoNAT v1. No hay relay, DCUtR, NAT-PMP ni PCP. `rsc-consensus`, `rsc-validator`, `rsc-chain` y `rsc-storage` no cambian. `--persist-peer` sigue igual. El RPC público sigue BLOCKED.

`crates/rsc-p2p/src/presence.rs` clasifica direcciones y traduce eventos. No abre sockets ni crea otro swarm. `libp2p-upnp` 0.7.0 expone el behaviour en `upnp::tokio::Behaviour`. `libp2p-autonat` 0.16.0 expone `autonat::Behaviour` de la v1, con `NatStatus::{Public, Private, Unknown}`.

`kad.add_address` del peer local devuelve `RoutingUpdate::Failed`. La dirección confirmada entra con `Swarm::add_external_address`, que Kademlia ya guarda en `FromSwarm::ExternalAddrConfirmed`.

### Clasificación y estado

`127.0.0.0/8` es loopback. `10.0.0.0/8`, `172.16.0.0/12` y `192.168.0.0/16` son private. `100.64.0.0/10` es CGNAT. `169.254.0.0/16` y `fe80::/10` son link-local. Una unicast fuera de esos rangos es `global` y queda como candidata. `0.0.0.0` y `::` son unspecified.

UPnP `NewExternalAddr` se anota como candidata y se retira del conjunto confirmado del swarm hasta AutoNAT. `ExpiredExternalAddr` la retira. `GatewayNotFound` y `NonRoutableGateway` dejan el proceso en marcha y no crean una dirección externa.

AutoNAT `Public` con dirección global llama `add_external_address` y `kad.set_mode(Server)`. `Private` no publica y pasa Kademlia a `Client`. `Unknown` no publica. En `Unknown` el modo sigue siendo `Server`, que es el DHT de la devnet local, sin anunciar una dirección pública. Una `Public` cuya dirección no es global se ignora.

`--presence auto`, sin `--listen`, pide `/ip4/0.0.0.0/tcp/0`, `/ip6/::/tcp/0` y los dos `quic-v1`. Un `--listen` explícito no cambia. libp2p no deja `0.0.0.0` en el log: expande cada interfaz.

### Pruebas

Unidad, en `presence.rs`: loopback, private, CGNAT, link-local, global sin confirmar, AutoNAT `Unknown`, `Private` en `Client`, `Public` global en `Server` con dirección externa, `Public` privada ignorada, UPnP sin gateway y candidata retirada. 11 pruebas.

Smoke de proceso, `presence_listen_classifies_without_claiming_public`. El primer intento falló porque esperaba el texto `0.0.0.0`. La corrida real mostró `127.0.0.1` loopback, `192.168.1.12` private, IPv6 `2804:…` como `class global` y `presence unknown`. No hubo `presence public`. La segunda corrida pasó en 0.35 s. No se fabricó un resultado público.

`proposer_offline_does_not_change_view` leyó el proposer de la altura 1, `3a922ee8…`, antes de imprimirse el de la altura 2. La espera hasta `ec0b95a0…` lo dejó en 34.58 s. El valor esperado no cambió.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 1.61 s. Lib 54/54 en 8.71 s. Bin 0. `two_processes` 36/36 en 640.01 s. Doc 0. Exit 0.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 4.45 s. Exit 0.

### Cargo.lock y RocksDB

`Cargo.lock` no cambió: 124448 bytes, 2026-09-27 20:45:11. `upnp` y `autonat` ya estaban en el lock como dependencias opcionales de libp2p 0.57.0.

El `librocksdb.a` de `librocksdb-sys-38b69e303fd1b6b2` ya no estaba. La compilación escribió otro de 457632132 bytes en `librocksdb-sys-d02585dc5184b75d/out`, fecha 2026-09-28 13:30:14. `CC=clang-18`, `CXX=clang++-18`. No hubo `cargo clean`. `CARGO_TARGET_DIR` siguió en el caché `/tmp/cursor-sandbox-cache/f672ecd8cb8164aaeffaf3c1f5c27f54/cargo-target`.

### Archivos

- `crates/rsc-p2p/Cargo.toml`
- `crates/rsc-p2p/src/lib.rs`
- `crates/rsc-p2p/src/presence.rs`
- `crates/rsc-p2p/src/network.rs`
- `crates/rsc-p2p/src/bin/rsc-p2p.rs`
- `crates/rsc-p2p/tests/two_processes.rs`
- `docs/rsc-chain.md`

### Estado

✅ CLOSED. El nodo clasifica la conectividad que el entorno le da. Una dirección global no se declara pública. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

## TASK 9.14 — Relay y DCUtR

### Alcance

Relay v2 y DCUtR dentro de `rsc-p2p`. `rsc-consensus`, `rsc-validator`, `rsc-chain` y `rsc-storage` no cambian. Tampoco el RPC, los bloques, el formato de transacción ni la finalidad. `--persist-peer`, Identify, Kademlia, UPnP y AutoNAT v1 siguen. No hay nodo móvil ni nombre nativo.

APIs usadas, de los crates ya fijados:

- `libp2p-relay` 0.22.0: `relay::Behaviour::new` y `set_status(Some(relay::Status::Enable))` en el servidor. El cliente entra con `SwarmBuilder::with_relay_client(noise::Config::new, yamux::Config::default)`, que entrega `relay::client::Behaviour`.
- `libp2p-dcutr` 0.15.0: `dcutr::Behaviour::new(local_peer_id)`. El éxito es `Event.result == Ok`.
- `libp2p` 0.57.0. La reserva se pide con `listen_on` de la multiaddr del relay más `/p2p-circuit`.

En localhost el servidor no tiene una dirección externa confirmada, así que el anuncio automático queda en `Disable`. `--relay server` fija `Enable`. La reserva además exige una lista de direcciones no vacía (`NoAddressesInReservation` si no la hay). El servidor registra su dirección de escucha con `add_external_address`. Esa dirección alimenta el mensaje de reserva. No se imprime como `presence public`.

### Servidor, clientes y estados

`--relay server` arranca el relay. `--relay-client <multiaddr>` incluye `/p2p/<PeerId>` y, al conectar en directo con ese peer, escucha el circuito. `--circuit-dial <multiaddr>` marca el destino después de `ReservationReqAccepted`. `--persist-peer` conserva el PeerId del relay entre reinicios.

`presence relay <multiaddr>` lleva `/p2p-circuit`. `presence public` sigue siendo AutoNAT con una dirección global, el camino directo. `Reach` distingue `Unknown`, `Local`, `PublicDirect` y `Relay`. Una multiaddr de circuito no pasa a IPv4 pública. `connected` es una conexión directa. `connected-relay` es el circuito. `dcutr-ok` solo sale del evento `Ok`. `dcutr-failed` deja el circuito. `relay-lost` vuelve a `presence unknown`.

El cupo por peer pasó de 2 a 8 para que quepan el circuito y el intento directo.

### Prueba en localhost

Tres procesos reales. A es el servidor, con `--persist-peer`. B y C son clientes. El PeerId de A se mantuvo al reiniciar.

Corrida observada:

- A `12D3KooWQeUiCrkuoqTxMCrTGA7S2XEFhxKxD9xuyNXNyPRSECr1`
- B `12D3KooWBKppMNBxYzGwK9arSevSfnj3CqaqGj2fSjPuPTcpfLFv`
- C `12D3KooWMkCu9xq5znDqzkAN3EWmsikCLqik1jquUxofYxNrGAXK`
- B y C obtuvieron `reservation` hacia A, y A imprimió `reservation-accepted` para cada uno.
- B y C quedaron en `connected-relay` y completaron handshake por ese camino.
- DCUtR devolvió `dcutr-ok 12D3KooWMkCu9xq5znDqzkAN3EWmsikCLqik1jquUxofYxNrGAXK`, con `connected` hacia C.

Ese `dcutr-ok` es un upgrade directo en localhost. Los tres procesos comparten la máquina. No demuestra NAT traversal de Internet. Quedan separados: integración del relay, integración de DCUtR, y una prueba de NAT entre redes distintas. Esta última no se ejecutó.

La primera corrida de la reserva no llegó a `reservation` en 20 s. El servidor aceptaba con la lista de direcciones vacía y el cliente cerraba el listener. La segunda, ya con `add_external_address` de la escucha, pasó.

### Negativos

- Relay inalcanzable: `dial-failed`, sin `reservation`, sin `dcutr-ok`.
- Servidor sin `--relay server`: conexión directa al proceso, sin `reservation`.
- `--relay-client` sin `/p2p/`: el proceso sale con `invalid address`.
- `network mismatch` y `chain mismatch`: `rejected`, sin `handshake`, sin `connected-relay`, sin `dcutr-ok`, sin `presence public`.
- Caída del relay: `relay-lost` y `presence unknown`. Sin `dcutr-ok` y sin `presence public`.
- `dcutr-failed` no se fabricó en esta máquina: el intento local devolvió `dcutr-ok`. El código solo escribe `dcutr-ok` cuando `result` es `Ok`.

### Cargo.lock, RocksDB y comandos

`Cargo.lock` no cambió: 124448 bytes, 2026-09-27 20:45:11. `relay` y `dcutr` ya estaban en el lock. `librocksdb.a` sigue en `librocksdb-sys-d02585dc5184b75d/out`, 457632132 bytes, 2026-09-28 13:30:14. No hubo compilación de `*.cc`. `CARGO_TARGET_DIR` siguió en `/tmp/cursor-sandbox-cache/f672ecd8cb8164aaeffaf3c1f5c27f54/cargo-target`. `ROCKSDB_LIB_DIR` apuntó a ese `out`. `CC=clang-18`, `CXX=clang++-18`.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 25.49 s. Lib 55/55 en 9.70 s. Bin 0. `two_processes` 43/43 en 648.16 s. Doc 0. Exit 0. Las pruebas 9.1–9.13 siguen verdes.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 20.60 s. Exit 0.

### Archivos

- `crates/rsc-p2p/Cargo.toml`
- `crates/rsc-p2p/src/presence.rs`
- `crates/rsc-p2p/src/network.rs`
- `crates/rsc-p2p/src/bin/rsc-p2p.rs`
- `crates/rsc-p2p/tests/two_processes.rs`
- `docs/rsc-chain.md`

### Estado

✅ CLOSED. Relay y DCUtR quedan integrados en localhost. El upgrade directo observado es local. La prueba de NAT entre redes distintas queda pendiente de un entorno que la permita. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

## TASK 9.15 — Ficha firmada de presencia

### Alcance

Solo la ficha dentro de `crates/rsc-p2p/src/presence.rs`. No hay Web Bridge, HTTP `/network`, dominio, app, ni cambios de relay, consensus, validator, chain, storage, bloques, transacciones, QC o `FinalityProof`. No se abre un socket ni un swarm nuevo.

La ficha dice que este PeerId afirma su estado de red. No es un bloque, una transacción ni un mensaje de consenso.

### Identidad y verificación

`libp2p-identity` 0.3.0, el que trae libp2p 0.57.0, no tiene una función que saque la clave pública desde un `PeerId`. `PeerId::from_public_key` guarda el protobuf de la clave en un multihash de identidad cuando cabe en 42 bytes. Ed25519 cabe. RSA y un `PeerId::random` no permiten el camino inverso. La ficha incluye `PublicKey::encode_protobuf()`.

La firma es `Keypair::sign` de esa misma clave Ed25519 de `--persist-peer`. No se usa `CryptoDomain::NodeIdentity`. Verificar:

1. `PublicKey::try_decode_protobuf`.
2. `key_type() == Ed25519`.
3. `PeerId::from(&key)` igual al PeerId anunciado. Si no, `identity mismatch`.
4. `PublicKey::verify` sobre el canon. Si no, `invalid signature`.

### Canon

`network_id` es el `String` de `HandshakeOffer`, validado por `validate_network_id`. No es `u32`: la red real se llama `devnet`. `chain_id` sí es `u32`.

Bytes, big-endian, sin JSON:

```text
"RSC-NP" | version u8 = 1
network_id: u8 len + utf-8
chain_id: u32
peer_id: u8 len + PeerId::to_bytes()
public_key: u16 len + protobuf
protocol_count u8 = 4, cada uno u8 len + utf-8
  1.0.0
  /rsc/handshake/1.0.0
  /rsc/block-sync/1.0.0
  /rsc/gossipsub/1.1.0
chain_flag u8
  0 omite la cadena
  1: latest_height u64, latest_block_hash 32, finalized_height u64, epoch u64
reach u8: 0 Unknown, 1 Local, 2 PublicDirect, 3 Relay
addr_count u16, cada una u16 len + multiaddr
issued_at u64, expires_at u64, sequence u64
```

La firma de 64 bytes va al lado del canon. Misma estructura y mismos valores producen los mismos bytes. Cambiar un campo firmado cambia los bytes y rompe la firma.

`Reach::Unknown` y `Local` publican cero direcciones. `PublicDirect` exige `NatView::Public` al firmar y direcciones `global` sin `/p2p-circuit`. Privada, CGNAT, loopback y link-local devuelven `not publishable`. Sin AutoNAT `Public`, `not public`. `Relay` exige `/p2p-circuit` en cada dirección; si falta, `not relay address`. Un relay sin cadena deja `chain_flag` en 0.

### Sequence y anti-replay

La clave protobuf no se modifica. La secuencia vive en `{persist-peer}.seq`, 8 bytes big-endian, modo `0600`. Si el archivo no existe, la secuencia leída es 0. `advance_sequence` suma uno y lo escribe. El reinicio lee el mismo archivo.

`accept` pide `expires_at > now`, `sequence >= latest`, `network_id` y `chain_id` esperados. Igual secuencia se acepta: es la misma ficha en otro lector. Una menor es `stale sequence`. `expires_at == now` es `expired presence`. Los mismos bytes los puede verificar cualquier tercero.

### Pruebas

Unidad en `presence.rs`: canon determinista, firma válida, firma alterada, PeerId alterado, otra network, otra chain, `issued_at` alterado, expiración, secuencia vieja, secuencia que sobrevive y no toca la clave, privada/CGNAT/loopback/link-local, AutoNAT `Public`, relay sin circuito, relay sin campos de cadena. 12 pruebas nuevas. Las 12 anteriores siguen.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 2m 34s. Lib 67/67 en 12.00 s. Bin 0. `two_processes` 43/43 en 649.00 s. Doc 0. Exit 0. Las pruebas 9.1–9.14 siguen verdes.

```text
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

`Finished dev` en 12.03 s. Exit 0.

### Cargo.lock y RocksDB

`Cargo.lock` no cambió: 124448 bytes, 2026-09-27 20:45:11. `librocksdb.a` sigue en `librocksdb-sys-d02585dc5184b75d/out`, 457632132 bytes, 2026-09-28 13:30:14. No hubo compilación de `*.cc`. `CARGO_TARGET_DIR` siguió en `/tmp/cursor-sandbox-cache/f672ecd8cb8164aaeffaf3c1f5c27f54/cargo-target`. `ROCKSDB_LIB_DIR` apuntó a ese `out`.

### Archivos

- `crates/rsc-p2p/src/presence.rs`
- `docs/rsc-chain.md`

### Estado

✅ CLOSED. La ficha se firma y se verifica con el PeerId persistente. El Web Bridge no está implementado. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

## TASK 9.16 — Web Bridge

### Alcance

Proceso aparte que recibe, valida y sirve la ficha firmada de TASK 9.15. Puede apagarse y la red sigue: no produce bloques, no ejecuta consensus, no cambia tip, `StateRoot`, `LedgerRoot` ni `FinalityProof`. No custodia claves y no firma por un nodo.

`rsc-consensus`, `rsc-validator`, `rsc-chain` y `rsc-storage` no se modificaron.

### Arquitectura

```text
Node -- POST /network/presence --> Web Bridge -- fichas públicas --> GET /network --> App
```

El bridge no es autoridad. Dos procesos pueden guardar exactamente los mismos bytes. No hay quorum de bridges ni firma del bridge. La hora del bridge viaja en el header `x-rsc-bridge-time` y queda fuera de la ficha.

### Ficha y endpoints

El cuerpo es el canon `RSC-NP` de TASK 9.15 seguido de la firma Ed25519 de 64 bytes. `SignedPresence::from_wire` vuelve a codificar el canon y exige los mismos bytes. `GET /network` concatena esas fichas vigentes. Una app las separa con `SignedPresence::read_list`, las verifica una a una y distingue `PublicDirect` de `Relay`.

`POST /network/presence` acepta una sola ficha, `Content-Type: application/octet-stream`, cuerpo máximo 64 KiB.

- válida y secuencia mayor: guarda esos bytes y responde `204`
- los mismos bytes y la misma secuencia: `204`, una sola copia
- secuencia igual con otros bytes: `409 duplicate sequence`, se conserva la anterior
- malformada, firma inválida, PeerId distinto, otra red, otra cadena, vencida, secuencia menor, dirección pública inválida o relay sin circuito: `400` con el texto de `PresenceError`

La validación es `SignedPresence::accept`: decodificación canónica, firma, clave Ed25519 embebida, `PeerId` atado a esa clave, `network_id`, `chain_id`, protocolos, `expires_at > now`, `sequence >=` la guardada y direcciones publicables.

### Expiración, secuencia y storage

`expires_at <= now` no entra en `GET /network`. El archivo puede seguir en disco; el listado lo filtra al leer. Por PeerId queda solo la ficha válida de mayor secuencia. El bridge no reescribe ni vuelve a firmar la ficha.

El directorio `--data-dir` tiene un archivo por PeerId con el wire recibido, escrito por rename atómico. No usa `RscDatabase`, ledger ni state. El binario enlaza `rsc-p2p`, así que RocksDB queda en el link, y el proceso no abre una base.

### Publicación desde el nodo

`rsc-p2p --publish-presence http://127.0.0.1:puerto/network/presence` exige `--persist-peer`. Firma con esa clave, `chain: None`, avanza `{clave}.seq` y sale. No abre el swarm ni la base de la cadena. `--presence-reach` es `public`, `relay`, `local` o `unknown`. `--presence-addr` es la multiaddr. `--presence-ttl` es segundos; el default es 300.

### Negativos

Ninguno aparece en `GET /network`:

- `PresenceRecord` malformado: `invalid presence`
- firma inválida: `invalid signature`
- PeerId que no corresponde a la clave: `identity mismatch`
- otra red: `network mismatch`
- otra cadena: `chain mismatch`
- `expires_at <= now`: `expired presence`
- secuencia menor: `stale sequence`
- dirección privada en `PublicDirect`: `not publishable`
- relay sin `/p2p-circuit`: `not relay address`
- misma secuencia con otros bytes: `duplicate sequence`

### Prueba de procesos

Bridge en `127.0.0.1:0`, luego tres procesos `rsc-p2p`:

- A `PublicDirect` `/ip4/203.0.113.10/tcp/4001`
- B `PublicDirect` `/ip4/203.0.113.11/tcp/4001`
- C `Relay` con `/p2p-circuit` hacia A

`GET /network` devolvió las tres fichas, verificables, más la ficha de control del test. A con `--presence-ttl 1` dejó de aparecer después de 3 s. Una publicación posterior salió con secuencia 3 y volvió al listado. Reenviar la ficha de secuencia 1 respondió `stale sequence` y el listado siguió en secuencia 3. `SIGTERM` cerró el bridge con éxito. Los nodos de publicación salieron `0`. Sus directorios no tienen `CURRENT` ni `LOCK`. El directorio del bridge no tiene `.key` ni `.seq`.

Las direcciones `203.0.113.0/24` son globales de documentación. Esta corrida no es NAT de Internet. PUBLIC RPC sigue BLOCKED.

### Comandos

```text
cargo test --offline --locked -p rsc-bridge -- --test-threads=1
```

`Finished test` en 10.90 s. Lib 3/3 en 0.09 s. Bin 0. `three_nodes` 1/1 en 3.56 s. Doc 0. Exit 0.

```text
cargo test --offline --locked -p rsc-p2p -- --test-threads=1
```

`Finished test` en 1 m 51 s. Lib 68/68 en 12.22 s. Bin 0. `two_processes` 43/43 en 672.94 s. Doc 0. Exit 0. Las pruebas 9.1–9.15 siguen verdes.

```text
cargo clippy --offline --locked -p rsc-bridge --all-targets -- -D warnings
cargo clippy --offline --locked -p rsc-p2p --all-targets -- -D warnings
```

Bridge: `Finished dev` en 2.65 s. Exit 0. `rsc-p2p`: `Finished dev` en 18.07 s. Exit 0.

### Cargo.lock y RocksDB

`Cargo.lock` pasó a 124575 bytes, 2026-09-28 17:10:44, por el paquete `rsc-bridge`. No se agregó una dependencia nueva de terceros. `librocksdb.a` sigue en `librocksdb-sys-d02585dc5184b75d/out`, 457632132 bytes, 2026-09-28 13:30:14. No hubo compilación de `*.cc`. `CARGO_TARGET_DIR` siguió en `/tmp/cursor-sandbox-cache/f672ecd8cb8164aaeffaf3c1f5c27f54/cargo-target`. `ROCKSDB_LIB_DIR` apuntó a ese `out`.

### Archivos

- `Cargo.toml`
- `Cargo.lock`
- `crates/rsc-bridge/Cargo.toml`
- `crates/rsc-bridge/src/lib.rs`
- `crates/rsc-bridge/src/main.rs`
- `crates/rsc-bridge/tests/three_nodes.rs`
- `crates/rsc-p2p/src/presence.rs`
- `crates/rsc-p2p/src/bin/rsc-p2p.rs`
- `docs/rsc-chain.md`

### Estado

✅ CLOSED. El bridge recibe, valida y sirve la ficha de TASK 9.15. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

## TASK 9.17 — Dominio público del Web Bridge

### Alcance

Conectar `https://reeskova-chain.xyz/network` al proceso `rsc-bridge` ya cerrado en TASK 9.16. El dominio sigue siendo un localizador. No entra en consensus, producción de bloques, finality, P2P ni storage.

No hay host controlado ni registro DNS hacia ese bridge. No se escribió proxy, certificado ni código. `rsc-consensus`, `rsc-validator`, `rsc-chain`, `rsc-storage` y el resto del núcleo no se modificaron. `192.168.1.12` no se publicó.

### DNS

Sonda 2026-09-28 17:53 -03, desde este equipo.

NS: `curitiba.ns.porkbun.com`, `fortaleza.ns.porkbun.com`, `maceio.ns.porkbun.com`, `salvador.ns.porkbun.com`.

```text
dig @8.8.8.8 reeskova-chain.xyz A
NOERROR
reeskova-chain.xyz.  A  207.207.210.107
reeskova-chain.xyz.  A  207.207.210.229

dig @1.1.1.1 reeskova-chain.xyz A
NOERROR
reeskova-chain.xyz.  A  207.207.210.107
reeskova-chain.xyz.  A  207.207.210.229
```

`dig @1.1.1.1 reeskova-chain.xyz AAAA` no devolvió registros. Esas dos IPv4 son el parking de Porkbun (`pixie`), no un proceso `rsc-bridge`.

### TLS y HTTP

```text
curl -I https://reeskova-chain.xyz/network
curl: (35) error:14094410:SSL routines:ssl3_read_bytes:sslv3 alert handshake failure
```

`openssl s_client -connect reeskova-chain.xyz:443 -servername reeskova-chain.xyz` conecta y reporta `no peer certificate available`. Alert 40. No hay certificado que presentar al cliente.

```text
curl -I http://reeskova-chain.xyz/network
HTTP/1.1 307 Temporary Redirect
Server: openresty
Location: http://reeskova-chain.xyz/
X-Service: pixie-default
```

El cuerpo de `GET /network` no existe: el handshake HTTPS termina antes. No hay bytes de `PresenceRecord` que un cliente pueda verificar.

### Proxy y host

En este equipo no están `caddy`, `nginx`, `cloudflared` ni `certbot`. No hay listener de `rsc-bridge` publicado. El repositorio no tiene credencial de Porkbun ni de un VPS. La dirección WAN vista por `ifconfig.me` (`201.2.190.21`) es la salida de esta LAN. No es un host de RSC y no se usó como registro A.

### Bridge ON/OFF

El ciclo pedido — bridge encendido, ficha real, `curl` externo, firma, apagado, error, recuperación — no se ejecutó contra el dominio. El nombre no llega a `rsc-bridge`, así que apagar o encender el proceso local no cambia `https://reeskova-chain.xyz/network`.

### Resultado

⚠ BLOCKED. Falta un host con IPv4 pública controlada, un registro A de `reeskova-chain.xyz` hacia esa IP, TLS válido y un reverse proxy que reenvíe `GET /network` al `rsc-bridge` existente, sin copiar la ficha. La prueba externa queda pendiente. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

Archivo: `docs/rsc-chain.md`.

### Despliegue autónomo — 2026-09-28 18:05 -03

Se probó usar este PC como host público, sin VPS. No se modificó código. No se añadió un túnel de terceros.

Tipo de WAN. La salida IPv4 que ve Internet es `201.2.190.21`. Está fuera de `100.64.0.0/10`. El PTR es `201-2-190-21.user3p.v-tal.net.br`. El PC solo tiene `192.168.1.12/24`, con gateway `192.168.1.1`. Hay IPv6 global en el PC, prefijo `2804:d51:87ca:9800::/64`, y la salida IPv6 funciona. `192.168.1.12` no se publicó.

Reachability. Nodos externos completan TCP hacia `201.2.190.21:443` (Canadá, Suiza, Kazajistán) y el `GET` HTTPS responde `302`. Desde esta LAN, `https://201.2.190.21/` responde `Server: Boa/1.0.0` y redirige a `/cgi-bin/login.asp`, con certificado `CN=192.168.1.1`. El puerto 9 recibe `Connection refused` desde Francia y Portugal. La entrada IPv4 llega al router. Este PC no escucha en `443`. Informe: `https://check-host.net/check-report/4df86487kada`.

Port forwarding. SSDP a `239.255.255.250:1900` y a `192.168.1.1:1900` no tuvo respuesta. NAT-PMP a `192.168.1.1:5351` agotó el tiempo. El router pide login en `/cgi-bin/login.asp`. No hay credencial. No se creó un reenvío. `ufw` está activo, entrada `deny`, y solo permite `8081/tcp`.

IPv6 de entrada. Los comprobadores usados rechazan una IPv6 literal (`invalid_url`), también para `2606:4700:4700::1111`. No hay AAAA propio con el que pedir la prueba por nombre. La entrada IPv6 hasta el PC queda sin medición externa.

DNS y TLS. Sigue igual a las 18:05 -03:

```text
dig @8.8.8.8 reeskova-chain.xyz A
207.207.210.107
207.207.210.229

curl -I https://reeskova-chain.xyz/network
curl: (35) error:14094410:SSL routines:ssl3_read_bytes:sslv3 alert handshake failure
```

No hay credencial de Porkbun. El registro A no se cambió. `https://reeskova-chain.xyz/network` no entrega PresenceRecords. El ciclo bridge ON/OFF contra el dominio no se ejecutó.

⚠ BLOCKED. La conexión doméstica no está en CGNAT, y la entrada pública termina en el router. Sin reenvío hacia el PC y sin DNS hacia una dirección que llegue a `rsc-bridge`, el nombre no sirve la ficha. No se implementó un acceso centralizado. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

Archivo: `docs/rsc-chain.md`.

### IPv6 directo — 2026-09-28 18:25 -03

Se probó el PC como destino IPv6, sin VPS, sin túnel y sin proxy. `201.2.190.21` no se usó. `192.168.1.12` no se publicó. No se modificó código.

Direcciones en `wlp3s0`, prefijo `2804:d51:87ca:9800::/64`:

- estable, `mngtmpaddr`: `2804:d51:87ca:9800:3163:3b76:2de8:2d5f`
- temporal, la que usa la salida: `2804:d51:87ca:9800:8efe:dd20:2258:2db9` (`valid_lft` 5137 s en la sonda)

`rsc-bridge` escuchó solo en `[2804:d51:87ca:9800:3163:3b76:2de8:2d5f]:48745`. `ufw` siguió activo. Se agregó una regla de entrada solo IPv6, `48745/tcp`, y se borró al terminar. El puerto ya no escucha.

Un `rsc-p2p` publicó una ficha real: `PublicDirect`, `/ip6/2804:d51:87ca:9800:3163:3b76:2de8:2d5f/tcp/4001`, secuencia 1, peer `12D3KooWSAPpMjJ29dUbsgws9UTQCy2WwdTJyUKfkd7NigwfGcdY`. Desde el propio PC, `GET /network` respondió `200`, `content-type: application/octet-stream`, cuerpo `RSC-NP` de 280 bytes. El puerto `4001` de esa ficha no se abrió.

Entrada ICMP, desde routers de Hurricane Electric:

```text
core3.sao1.he.net  2001:470:0:4fd::1  5/5  0% loss  143–187 ms
core1.rio1.he.net  2001:470:0:4f6::1  5/5  0% loss  148–170 ms
core1.for1.he.net  2001:470:0:69a::1  5/5  0% loss  157–181 ms
```

IPv6 direct reachability del PC, ICMP = ✅ PASS. El ISP no tira ese tráfico.

TCP externo hacia `rsc-bridge` no quedó probado. Los nodos de check-host.net publican solo IPv4 y respondieron `Unknown host` o `No such device or address` al nombre AAAA de la dirección. Un comprobador HTTP que contestó `open` en 1–2 ms con `403` dio el mismo resultado para el puerto 9 de esta IPv6 y para `2001:4860:4860::8888:443`; no se usa como evidencia. No hubo `GET /network` externo con el cuerpo `RSC-NP`.

AAAA y `https://reeskova-chain.xyz/network` no se configuraron. El DNS sigue en `207.207.210.107` y `207.207.210.229`. No hay credencial de Porkbun.

⚠ BLOCKED para el criterio del bridge: una máquina externa todavía no mostró TCP a `rsc-bridge`. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

Archivo: `docs/rsc-chain.md`.

### TCP IPv6 externo — 2026-09-28 18:52 -03

La sonda se volvió a abrir en la dirección estable. `rsc-bridge` escucha en `[2804:d51:87ca:9800:3163:3b76:2de8:2d5f]:48745`. `ufw` sigue activo. La regla de entrada es solo IPv6, `48745/tcp`, comentario `rsc-ipv6-probe`. La dirección temporal no se publicó. `201.2.190.21` y `192.168.1.12` no se usaron. No se modificó código.

Ficha servida, secuencia 1, peer `12D3KooWP9GxqXnb8jGUwHG3JLHcFpJAkp3cNXPoJzdkkWs43FVF`, `PublicDirect`, `/ip6/2804:d51:87ca:9800:3163:3b76:2de8:2d5f/tcp/4001`. Desde este PC, `GET /network` responde `200`, `application/octet-stream`, `RSC-NP`, 280 bytes. El puerto `4001` no escucha.

TCP desde `tcp6.ping.pe`, comando entendido `tcp6`, `ipv6_mode` 1, destino resuelto a la dirección estable, 84 nodos:

```text
[2804:d51:87ca:9800:3163:3b76:2de8:2d5f]:48745
83 conexiones
1 fallo (India, Noida, YottaSrc)
São Paulo  20.71 ms, 25.66 ms, 34.97 ms
Fortaleza  163.74 ms
mediana    229.68 ms
```

Control en el mismo destino, puerto 9, que no escucha: 84/84 fallos. El puerto abierto y el cerrado no se parecen. Las latencias de São Paulo y Fortaleza siguen el mismo orden que el ICMP de Hurricane Electric.

Eso demuestra el connect TCP IPv6 hasta el PC. Un `curl` remoto de `GET /network` no se obtuvo. Los fetchers públicos devolvieron 522, 520, 500 o 422, y el lector de URL de esta sesión rechazó la dirección. No hay cuerpo `RSC-NP` visto desde fuera.

AAAA de `reeskova-chain.xyz` no se configuró. TLS y el reverse proxy local no se configuraron. `https://reeskova-chain.xyz/network` sigue en el parking de Porkbun. El listener de `48745` quedó encendido para el `GET` externo que falta.

⚠ BLOCKED. Falta un HTTP IPv6 externo que reciba `200` y la ficha real. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

Archivo: `docs/rsc-chain.md`.

### HTTP externo — 2026-09-28 19:13 -03

La ficha de secuencia 1 había caducado: `GET /network` local respondía `200` con `content-length: 0`. Se republicó con la misma clave, secuencia 2, TTL 7200 s, el mismo peer `12D3KooWP9GxqXnb8jGUwHG3JLHcFpJAkp3cNXPoJzdkkWs43FVF`. El `GET` local volvió a `200`, `RSC-NP`, 280 bytes. `rsc-bridge` sigue en `[2804:d51:87ca:9800:3163:3b76:2de8:2d5f]:48745`. AAAA no se configuró.

`tcpdump -ni wlp3s0 'ip6 and tcp port 48745'`, 150 s en reposo: 0 paquetes.

Durante otro `tcp6` de ping.pe, la misma captura tomó 517 paquetes, todos con `length 0`:

```text
83 SYN de entrada, 83 orígenes IPv6 distintos
83 SYN-ACK de salida
166 FIN y 185 ACK
0 bytes de aplicación
GET /network = 0
HTTP/1.1 = 0
RSC-NP = 0
```

Un origen de ejemplo, `2604:a880:2:d0::baa:d001`, envió SYN y recibió SYN-ACK. No hubo `GET`.

1. TCP llega. El PC completa el saludo.
2. La petición HTTP no llega. Ningún segmento trae `GET /network`.
3. La respuesta HTTP no sale. `rsc-bridge` no emite `200` ni `RSC-NP` hacia esos orígenes.

El `GET` local de 280 bytes no aparece en `wlp3s0`. No hay en este equipo un shell IPv6 remoto desde el que ejecutar el `curl -g -6` pedido. Los únicos `Host` de SSH son de GitHub.

⚠ BLOCKED. El criterio sigue siendo un `curl` IPv6 externo con `200` y la ficha real. Fase 9 sigue OPEN. PUBLIC RPC sigue BLOCKED. `rsc-validator` sigue congelado.

Archivo: `docs/rsc-chain.md`.






