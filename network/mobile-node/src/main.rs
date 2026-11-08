//! API del botón minar.
//!
//! Arranca sin ningún nodo ligado. El primer `POST /mobile/mine` abre el
//! enlace en 1/4. No hace falta tener la cadena puesta en una terminal.

#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rsc_mobile_node::{MineStep, MobileNodes, StoredNode, StoredNodes};

const QUARTERS_PER_NODE: u32 = rsc_mobile_node::QUARTERS_PER_NODE;

struct Config {
    api: String,
    bind: String,
    state: PathBuf,
}

fn main() -> ExitCode {
    let config = match parse_args(env::args().skip(1)) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let nodes = match open_nodes(&config) {
        Ok(nodes) => nodes,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let listener = match TcpListener::bind(&config.bind) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("no se pudo escuchar en {}: {err}", config.bind);
            return ExitCode::from(1);
        }
    };
    println!("minar {} en {}", nodes.api(), config.bind);
    let _ = std::io::stdout().flush();
    let shared = Arc::new(Mutex::new(nodes));
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        let shared = Arc::clone(&shared);
        let state = config.state.clone();
        thread::spawn(move || {
            let _ = serve_client(stream, &shared, &state);
        });
    }
    ExitCode::SUCCESS
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut api = None;
    let mut bind = None;
    let mut state = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("falta el valor de {flag}"))?;
        match flag.as_str() {
            "--api" => assign(&mut api, value, "api")?,
            "--bind" => assign(&mut bind, value, "bind")?,
            "--state" => assign(&mut state, value, "state")?,
            other => return Err(format!("argumento desconocido {other}")),
        }
    }
    Ok(Config {
        api: api.ok_or("falta api")?,
        bind: bind.unwrap_or_else(|| "127.0.0.1:8546".to_string()),
        state: PathBuf::from(state.ok_or("falta state")?),
    })
}

fn assign(slot: &mut Option<String>, value: String, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} repetido"));
    }
    if value.is_empty() {
        return Err(format!("{name} vacío"));
    }
    *slot = Some(value);
    Ok(())
}

fn open_nodes(config: &Config) -> Result<MobileNodes, String> {
    if !config.state.exists() {
        return MobileNodes::link(&config.api).map_err(|err| err.to_string());
    }
    let text = fs::read_to_string(&config.state).map_err(|err| err.to_string())?;
    let stored = decode_stored(&text)?;
    MobileNodes::load(&config.api, stored).map_err(|err| err.to_string())
}

fn serve_client(mut stream: TcpStream, nodes: &Mutex<MobileNodes>, state: &PathBuf) -> Result<(), ()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let (method, path, body) = read_request(&mut stream)?;
    let (status, payload) = {
        let mut nodes = nodes.lock().map_err(|_| ())?;
        let outcome = handle(&method, &path, &body, &mut nodes);
        if outcome.0 == 200 && (path_name(&path) == "/mine" || path_name(&path) == "/stop") {
            save_nodes(state, &nodes).map_err(|_| ())?;
        }
        outcome
    };
    write_response(&mut stream, status, &payload)
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, String), ()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut tmp).map_err(|_| ())?;
        if read == 0 {
            return Err(());
        }
        buf.extend_from_slice(&tmp[..read]);
        if let Some(position) = buf.windows(4).position(|mark| mark == b"\r\n\r\n") {
            break position;
        }
        if buf.len() > 8192 {
            return Err(());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let request = lines.next().ok_or(())?;
    let mut parts = request.split_whitespace();
    let method = parts.next().ok_or(())?.to_string();
    let path = parts.next().ok_or(())?.to_string();
    let mut length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().map_err(|_| ())?;
        }
    }
    if length > 4096 {
        return Err(());
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut tmp).map_err(|_| ())?;
        if read == 0 {
            return Err(());
        }
        body.extend_from_slice(&tmp[..read]);
    }
    body.truncate(length);
    let body = String::from_utf8(body).map_err(|_| ())?;
    Ok((method, path, body))
}

fn handle(method: &str, path: &str, body: &str, nodes: &mut MobileNodes) -> (u16, String) {
    match (method, path_name(path)) {
        ("GET", "/mobile") => (200, state_json(nodes, None)),
        ("POST", "/mine") => post_mine(body, nodes),
        ("POST", "/stop") => post_stop(body, nodes),
        _ if path_name(path) == "/mobile" || path_name(path) == "/mine" || path_name(path) == "/stop" => {
            (405, error_json("método"))
        }
        _ => (404, error_json("ruta")),
    }
}

fn path_name(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or(path);
    match path {
        "/mobile" | "/mobile/" | "/" | "/status" => "/mobile",
        "/mine" | "/mobile/mine" => "/mine",
        "/stop" | "/mobile/stop" => "/stop",
        other => other,
    }
}

fn post_mine(body: &str, nodes: &mut MobileNodes) -> (u16, String) {
    let user = match user_from(body) {
        Ok(user) => user,
        Err(message) => return (400, error_json(message)),
    };
    match nodes.mine(&user) {
        Ok(step) => (200, state_json(nodes, Some(&step))),
        Err(_) => (400, error_json("usuario")),
    }
}

fn post_stop(body: &str, nodes: &mut MobileNodes) -> (u16, String) {
    let user = match user_from(body) {
        Ok(user) => user,
        Err(message) => return (400, error_json(message)),
    };
    match nodes.stop(&user) {
        Ok(step) => (200, state_json(nodes, Some(&step))),
        Err(_) => (400, error_json("usuario")),
    }
}

fn user_from(body: &str) -> Result<String, &'static str> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| "json")?;
    let user = value
        .get("user")
        .and_then(|item| item.as_str())
        .ok_or("usuario")?;
    let user = user.trim();
    if user.is_empty() || user.len() > 128 {
        return Err("usuario");
    }
    Ok(user.to_string())
}

fn state_json(nodes: &MobileNodes, step: Option<&MineStep>) -> String {
    let state = nodes.state();
    let links = state
        .links
        .iter()
        .map(|link| {
            serde_json::json!({
                "index": link.index,
                "miners": link.miners,
                "quarters": link.quarters,
                "open": link.open,
                "autonomous": link.autonomous,
                "api": link.api,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "changed": step.map(|step| step.changed),
        "opened": step.and_then(|step| step.opened),
        "miners": state.miners,
        "open_nodes": state.open_nodes,
        "partial_quarters": state.partial_quarters,
        "quarters_per_node": QUARTERS_PER_NODE,
        "api": nodes.api(),
        "links": links,
    })
    .to_string()
}

fn error_json(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

fn save_nodes(path: &PathBuf, nodes: &MobileNodes) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let stored = nodes.export();
    let sealed = stored
        .sealed
        .iter()
        .map(|node| {
            serde_json::json!({
                "index": node.index,
                "miners": node.miners,
            })
        })
        .collect::<Vec<_>>();
    let value = serde_json::json!({
        "sealed": sealed,
        "committed": stored.committed,
        "pending": stored.pending,
    });
    let text = serde_json::to_string(&value).map_err(|err| err.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text).map_err(|err| err.to_string())?;
    fs::rename(&temporary, path).map_err(|err| err.to_string())
}

fn decode_stored(text: &str) -> Result<StoredNodes, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let sealed = value
        .get("sealed")
        .and_then(|item| item.as_array())
        .ok_or("estado inválido")?
        .iter()
        .map(|node| {
            let miners = string_list(node.get("miners"))?;
            let index = node
                .get("index")
                .and_then(|item| item.as_u64())
                .ok_or("estado inválido")?;
            Ok(StoredNode {
                index: u32::try_from(index).map_err(|_| "estado inválido".to_string())?,
                miners,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(StoredNodes {
        sealed,
        committed: string_list(value.get("committed"))?,
        pending: string_list(value.get("pending"))?,
    })
}

fn string_list(value: Option<&serde_json::Value>) -> Result<Vec<String>, String> {
    value
        .and_then(|item| item.as_array())
        .ok_or("estado inválido")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(|text| text.to_string())
                .ok_or_else(|| "estado inválido".to_string())
        })
        .collect()
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), ()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let payload = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(payload.as_bytes()).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{handle, path_name};
    use rsc_mobile_node::MobileNodes;

    fn fresh() -> MobileNodes {
        MobileNodes::link("https://rpc.reeskova-chain.xyz/rpc").unwrap()
    }

    #[test]
    fn the_first_button_starts_one_quarter() {
        let mut nodes = fresh();
        let (status, idle) = handle("GET", "/mobile", "", &mut nodes);
        assert_eq!(status, 200);
        let idle: serde_json::Value = serde_json::from_str(&idle).unwrap();
        assert_eq!(idle["open_nodes"], 0);
        assert_eq!(idle["partial_quarters"], 0);
        assert_eq!(idle["links"].as_array().unwrap().len(), 0);

        let (status, body) = handle(
            "POST",
            "/mobile/mine",
            r#"{"user":"rsc-button"}"#,
            &mut nodes,
        );
        assert_eq!(status, 200);
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["changed"], true);
        assert!(body["opened"].is_null());
        assert_eq!(body["partial_quarters"], 1);
        assert_eq!(body["open_nodes"], 0);
        assert_eq!(body["links"][0]["quarters"], 1);
        assert_eq!(body["links"][0]["open"], false);
        assert_eq!(path_name("/mobile/stop"), "/stop");
    }
}
