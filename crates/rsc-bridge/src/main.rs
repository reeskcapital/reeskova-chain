//! Proceso del Web Bridge. Solo escucha HTTP y guarda fichas públicas.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use rsc_bridge::Bridge;

struct Config {
    data_dir: PathBuf,
    bind: SocketAddr,
    network_id: String,
    chain_id: u32,
}

fn main() -> ExitCode {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = runtime.block_on(serve(config)) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut data_dir = None;
    let mut bind = None;
    let mut network_id = None;
    let mut chain_id = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("falta el valor de {flag}"))?;
        match flag.as_str() {
            "--data-dir" => set(&mut data_dir, value, "data-dir")?,
            "--bind" => set(&mut bind, value, "bind")?,
            "--network-id" => set(&mut network_id, value, "network-id")?,
            "--chain-id" => {
                if chain_id.is_some() {
                    return Err("chain-id repetido".into());
                }
                if value.is_empty() {
                    return Err("chain-id vacío".into());
                }
                chain_id = Some(value.parse::<u32>().map_err(|_| "chain-id no es u32".to_string())?);
            }
            other => return Err(format!("argumento desconocido {other}")),
        }
    }
    Ok(Config {
        data_dir: PathBuf::from(data_dir.ok_or("falta data-dir")?),
        bind: bind.ok_or("falta bind")?.parse().map_err(|_| "bind".to_string())?,
        network_id: network_id.ok_or("falta network-id")?,
        chain_id: chain_id.ok_or("falta chain-id")?,
    })
}

fn set(slot: &mut Option<String>, value: String, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} repetido"));
    }
    if value.is_empty() {
        return Err(format!("{name} vacío"));
    }
    *slot = Some(value);
    Ok(())
}

async fn serve(config: Config) -> Result<(), String> {
    let bridge = Bridge::open(&config.data_dir, &config.network_id, config.chain_id)
        .map_err(|err| err.to_string())?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    println!("listen {addr}");
    axum::serve(listener, rsc_bridge::router(Arc::new(bridge)))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|err| err.to_string())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}
