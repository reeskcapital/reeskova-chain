//! Proceso del registro. El token de administración sale del entorno.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use rsc_registry::server::{RegistryConfig, RegistryServer};

fn main() -> ExitCode {
    let bind = match bind_from_env("RSC_REGISTRY_BIND", "127.0.0.1:8550") {
        Ok(bind) => bind,
        Err(err) => {
            eprintln!("registry: {err}");
            return ExitCode::from(1);
        }
    };
    if !bind.ip().is_loopback()
        && std::env::var("RSC_REGISTRY_ALLOW_PUBLIC_BIND").ok().as_deref() != Some("1")
    {
        eprintln!("registry: el bind debe ser loopback");
        return ExitCode::from(1);
    }
    let db_path = std::env::var("RSC_REGISTRY_DB")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("registry.sqlite"));
    let admin_token = std::env::var("RSC_REGISTRY_ADMIN_TOKEN")
        .ok()
        .filter(|value| !value.is_empty());
    let rpc_hosts = std::env::var("RSC_REGISTRY_RPC_HOSTS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| item.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let config = RegistryConfig {
        bind,
        db_path,
        admin_token,
        allow_private: std::env::var("RSC_REGISTRY_ALLOW_PRIVATE").ok().as_deref() == Some("1"),
        rpc_hosts,
        min_ttl_secs: 15,
        max_ttl_secs: 120,
        rate_per_window: 60,
        rate_window: Duration::from_secs(60),
    };
    let server = match RegistryServer::open(config) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("registry: {err}");
            return ExitCode::from(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("registry: runtime");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = runtime.block_on(server.run()) {
        eprintln!("registry: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn bind_from_env(name: &str, default: &str) -> Result<SocketAddr, String> {
    let raw = std::env::var(name).unwrap_or_else(|_| default.to_string());
    raw.parse().map_err(|_| "bind inválido".to_string())
}
