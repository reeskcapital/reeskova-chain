# API universal — `rpc.reeskova-chain.xyz`

Un solo dominio para hablar con la cadena y activar nodos. Las apps **no**
usan `127.0.0.1:8545/8546/8547`; eso queda detrás del proxy.

## URLs públicas

| Uso | Método | URL |
|-----|--------|-----|
| Explorer | GET | `http://rpc.reeskova-chain.xyz/` |
| RPC JSON-RPC | POST | `http://rpc.reeskova-chain.xyz/rpc` |
| Health | GET | `http://rpc.reeskova-chain.xyz/health` |
| Discovery | GET | `http://rpc.reeskova-chain.xyz/network` |
| Community link | POST | `http://rpc.reeskova-chain.xyz/community/link` |
| Community unlink | POST | `http://rpc.reeskova-chain.xyz/community/unlink` |
| Mobile mine (1/4…4/4) | POST | `http://rpc.reeskova-chain.xyz/mobile/mine` |
| Mobile stop | POST | `http://rpc.reeskova-chain.xyz/mobile/stop` |

## Community (link cortito — así es la API)

Sin certificado. HTTP simple:

```bash
# ver estado
curl http://rpc.reeskova-chain.xyz/community

# ligar
curl -X POST http://rpc.reeskova-chain.xyz/community/link \
  -H 'Content-Type: application/json' \
  -d '{"user":"rsc…"}'

# soltar
curl -X POST http://rpc.reeskova-chain.xyz/community/unlink \
  -H 'Content-Type: application/json' \
  -d '{"user":"rsc…"}'
```


## Arranque en este host

```powershell
powershell -File deploy\public-rpc\start-universal-api.ps1
```

Levanta: `rsc-node` + `rsc-mobile-node` + `rsc-community-node` + Caddy.

Prueba local sin TLS: `http://127.0.0.1:8880/...` (mismas rutas).

## DNS + puertos (obligatorio para HTTPS público)

1. Porkbun: `rpc` **A** → IPv4 pública, `rpc` **AAAA** → IPv6 pública.
2. Router: reenviar **TCP 80 y 443** → IP LAN de este PC (ej. `192.168.1.10`).
3. Caddy obtiene el certificado solo si Let's Encrypt puede llegar a 80/443.

Sin el reenvío, el dominio no responde desde internet aunque el stack local esté OK.
