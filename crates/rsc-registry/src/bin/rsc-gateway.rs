//! Proceso del gateway. Los backends salen del entorno, no del cliente.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use rsc_registry::gateway::{serve, GatewayConfig};

fn main() -> ExitCode {
    let bind = match std::env::var("RSC_GATEWAY_BIND").unwrap_or_else(|_| "127.0.0.1:8544".into()).parse::<SocketAddr>() {
        Ok(bind) => bind,
        Err(_) => {
            eprintln!("gateway: bind inválido");
            return ExitCode::from(1);
        }
    };
    let primary = std::env::var("RSC_GATEWAY_RPC_BACKEND")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8545".into());
    let mut rpc_backends = vec![primary];
    if let Ok(failover) = std::env::var("RSC_GATEWAY_RPC_FAILOVER") {
        if !failover.is_empty() {
            rpc_backends.push(failover);
        }
    }
    let mobile_backend = optional_backend("RSC_GATEWAY_MOBILE_BACKEND", "http://127.0.0.1:8546");
    let community_backend = optional_backend("RSC_GATEWAY_COMMUNITY_BACKEND", "http://127.0.0.1:8547");
    let timeout = std::env::var("RSC_GATEWAY_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= 30)
        .unwrap_or(8);
    let config = GatewayConfig {
        rpc_backends,
        mobile_backend,
        community_backend,
        timeout: Duration::from_secs(timeout),
        health_ttl: Duration::from_secs(2),
        rate_per_window: 120,
        rate_window: Duration::from_secs(60),
        max_body: 1024 * 1024,
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("gateway: runtime");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = runtime.block_on(serve(bind, config)) {
        eprintln!("gateway: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn optional_backend(name: &str, default: &str) -> Option<String> {
    if std::env::var(format!("{name}_DISABLE")).ok().as_deref() == Some("1") {
        return None;
    }
    Some(
        std::env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.to_string()),
    )
}
