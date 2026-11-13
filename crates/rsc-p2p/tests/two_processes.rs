//! Dos procesos `rsc-p2p` se conectan con `RscNetwork::dial`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsc_core::AccountId;
use rsc_crypto::{HashBuilder, PrivateKey};
use rsc_ledger::transaction::TRANSACTION_VERSION_V1;
use rsc_ledger::{Transaction, TransactionPayload};
use rsc_p2p::PeerId;
use rsc_rpc::encode_hex;

struct Peer {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    lines: Arc<Mutex<Vec<String>>>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    data_dir: PathBuf,
}

impl Drop for Peer {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        eprintln!("fase drop-inicio pid={}", child.id());
        drop(self.stdin.take());
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status();
        }
        let started = Instant::now();
        loop {
            if child.try_wait().ok().flatten().is_some() {
                eprintln!("fase drop-fin pid={}", child.id());
                return;
            }
            if started.elapsed() > Duration::from_secs(5) {
                eprintln!("fase drop-kill pid={}", child.id());
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn bin() -> PathBuf {
    env!("CARGO_BIN_EXE_rsc-p2p").into()
}

fn data_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "rsc-p2p-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn spawn(data: PathBuf, network: &str, chain: &str, dial: Option<&str>) -> Peer {
    launch(data, network, chain, dial, None, None, None, None)
}

fn spawn_bootstrap(data: PathBuf, network: &str, chain: &str, bootstrap: &str) -> Peer {
    launch(data, network, chain, None, Some(bootstrap), None, None, None)
}

fn spawn_tx(
    data: PathBuf,
    bootstrap: Option<&str>,
    rpc: &str,
    signer: &str,
) -> Peer {
    launch(
        data,
        "devnet",
        "3",
        None,
        bootstrap,
        Some(rpc),
        Some(signer),
        None,
    )
}

fn spawn_vote(data: PathBuf, bootstrap: Option<&str>, rpc: &str, index: u8) -> Peer {
    launch(
        data,
        "devnet",
        "3",
        None,
        bootstrap,
        Some(rpc),
        Some(&signer_hex()),
        Some(&format!("vote-{index}")),
    )
}

fn spawn_proposal(data: PathBuf, bootstrap: Option<&str>, rpc: &str, role: &str) -> Peer {
    launch(
        data,
        "devnet",
        "3",
        None,
        bootstrap,
        Some(rpc),
        Some(&signer_hex()),
        Some(role),
    )
}

#[allow(clippy::too_many_arguments)]
fn launch(
    data: PathBuf,
    network: &str,
    chain: &str,
    dial: Option<&str>,
    bootstrap: Option<&str>,
    rpc: Option<&str>,
    signer: Option<&str>,
    consensus: Option<&str>,
) -> Peer {
    let mut args = vec![
        "--data-dir".to_string(),
        data.to_str().unwrap().to_string(),
        "--network-id".to_string(),
        network.to_string(),
        "--chain-id".to_string(),
        chain.to_string(),
    ];
    if let Some(address) = dial {
        args.push("--dial".into());
        args.push(address.to_string());
    }
    if let Some(address) = bootstrap {
        args.push("--bootstrap".into());
        args.push(address.to_string());
    }
    if let Some(address) = rpc {
        args.push("--rpc".into());
        args.push(address.to_string());
    }
    if let Some(key) = signer {
        args.push("--signer-key".into());
        args.push(key.to_string());
    }
    if let Some(role) = consensus {
        args.push("--consensus".into());
        args.push(role.to_string());
    }
    eprintln!("fase spawn-inicio {network} chain={chain}");
    let mut child = Command::new(bin())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    eprintln!("fase spawn-fin pid={}", child.id());
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            sink.lock().unwrap().push(line);
        }
    });
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let err_sink = Arc::clone(&stderr_lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            err_sink.lock().unwrap().push(line);
        }
    });
    Peer {
        child: Some(child),
        stdin,
        lines,
        stderr_lines,
        data_dir: data,
    }
}

fn lines_of(peer: &Peer) -> Vec<String> {
    peer.lines.lock().unwrap().clone()
}

fn stderr_of(peer: &Peer) -> Vec<String> {
    peer.stderr_lines.lock().unwrap().clone()
}

fn wait_prefix(peer: &mut Peer, prefix: &str) -> String {
    let started = Instant::now();
    loop {
        if let Some(line) = lines_of(peer)
            .into_iter()
            .find(|line| line.starts_with(prefix))
        {
            return line;
        }
        if let Some(status) = peer.child.as_mut().unwrap().try_wait().unwrap() {
            panic!("el proceso salió {status} antes de {prefix}");
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!(
                "no apareció {prefix}: stdout={:?} stderr={:?}",
                lines_of(peer),
                stderr_of(peer)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn peer_of(line: &str) -> PeerId {
    line.split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

fn stop(peer: &mut Peer) {
    let mut child = peer.child.take().unwrap();
    eprintln!("fase stop-inicio pid={}", child.id());
    drop(peer.stdin.take());
    #[cfg(unix)]
    {
        assert!(child_pids(child.id()).is_empty());
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
    }
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            eprintln!("fase stop-fin pid={} status={status}", child.id());
            assert!(status.success(), "{status} stderr={:?}", stderr_of(peer));
            assert!(std::fs::read_dir(&peer.data_dir).unwrap().next().is_none());
            assert!(!peer.data_dir.join("LOCK").exists());
            std::fs::remove_dir(&peer.data_dir).unwrap();
            return;
        }
        if started.elapsed() > Duration::from_secs(10) {
            eprintln!("fase stop-timeout pid={}", child.id());
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "el proceso no cerró: stdout={:?} stderr={:?}",
                lines_of(peer),
                stderr_of(peer)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn child_pids(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return out;
    };
    for task in tasks.flatten() {
        let Ok(text) = std::fs::read_to_string(task.path().join("children")) else {
            continue;
        };
        for id in text.split_whitespace() {
            if let Ok(id) = id.parse() {
                out.push(id);
            }
        }
    }
    out
}

fn wait_rejection(first: &mut Peer, second: &mut Peer, message: &str) {
    eprintln!("fase rechazo-inicio {message}");
    let started = Instant::now();
    loop {
        let left = lines_of(first);
        let right = lines_of(second);
        let rejected = left
            .iter()
            .chain(right.iter())
            .find(|line| line.starts_with("rejected ") && line.contains(message));
        if rejected.is_some() {
            assert!(
                !left.iter().any(|line| line.starts_with("handshake ")),
                "{left:?}"
            );
            assert!(
                !right.iter().any(|line| line.starts_with("handshake ")),
                "{right:?}"
            );
            eprintln!("fase rechazo-fin {message}");
            return;
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!(
                "no apareció rejected {message}: stdout={left:?} {right:?} stderr={:?} {:?}",
                stderr_of(first),
                stderr_of(second)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn listen_of(peer: &mut Peer) -> String {
    eprintln!("fase listen-inicio");
    let line = wait_prefix(peer, "listen ");
    eprintln!("fase listen-fin {line}");
    line
        .split_once(' ')
        .unwrap()
        .1
        .to_string()
}

#[test]
fn two_processes_dial_and_recognize_each_other() {
    let mut first = spawn(data_dir("a"), "devnet", "3", None);
    let first_peer = peer_of(&wait_prefix(&mut first, "peer "));
    let listen = listen_of(&mut first);
    let mut second = spawn(data_dir("b"), "devnet", "3", Some(&listen));
    let second_peer = peer_of(&wait_prefix(&mut second, "peer "));
    assert_ne!(first_peer, second_peer);
    let seen_by_first = peer_of(&wait_prefix(&mut first, &format!("connected {second_peer}")));
    let seen_by_second = peer_of(&wait_prefix(&mut second, &format!("connected {first_peer}")));
    assert_eq!(seen_by_first, second_peer);
    assert_eq!(seen_by_second, first_peer);
    assert_eq!(
        peer_of(&wait_prefix(&mut first, &format!("handshake {second_peer}"))),
        second_peer
    );
    assert_eq!(
        peer_of(&wait_prefix(
            &mut second,
            &format!("handshake {first_peer}")
        )),
        first_peer
    );
    stop(&mut first);
    stop(&mut second);
}

#[test]
fn a_different_network_id_rejects_the_handshake() {
    let mut first = spawn(data_dir("net-a"), "mainnet", "1", None);
    let listen = listen_of(&mut first);
    let mut second = spawn(data_dir("net-b"), "devnet", "3", Some(&listen));
    wait_rejection(&mut first, &mut second, "network mismatch");
    stop(&mut second);
    stop(&mut first);
}

#[test]
fn a_different_chain_id_rejects_the_handshake() {
    let mut first = spawn(data_dir("chain-a"), "devnet", "3", None);
    let listen = listen_of(&mut first);
    let mut second = spawn(data_dir("chain-b"), "devnet", "1", Some(&listen));
    wait_rejection(&mut first, &mut second, "chain mismatch");
    stop(&mut second);
    stop(&mut first);
}

#[test]
fn a_closed_port_does_not_connect() {
    let peer = PeerId::random();
    let mut alone = spawn(
        data_dir("closed"),
        "devnet",
        "3",
        Some(&format!("/ip4/127.0.0.1/tcp/1/p2p/{peer}")),
    );
    let failed = wait_prefix(&mut alone, "dial-failed ");
    assert!(failed.contains("dial failed"), "{failed}");
    assert!(!lines_of(&alone).iter().any(|line| line.starts_with("connected ")));
    assert!(!lines_of(&alone).iter().any(|line| line.starts_with("handshake ")));
    stop(&mut alone);
}

#[test]
fn an_unknown_peer_id_does_not_confirm_a_session() {
    let mut first = spawn(data_dir("real"), "devnet", "3", None);
    let listen = listen_of(&mut first);
    let endpoint = listen.split("/p2p/").next().unwrap();
    let unknown = PeerId::random();
    let mut second = spawn(
        data_dir("unknown"),
        "devnet",
        "3",
        Some(&format!("{endpoint}/p2p/{unknown}")),
    );
    let failed = wait_prefix(&mut second, "dial-failed ");
    assert!(failed.contains("dial failed"), "{failed}");
    assert!(!lines_of(&second)
        .iter()
        .any(|line| line.starts_with("handshake ") || line.starts_with("connected ")));
    stop(&mut second);
    stop(&mut first);
}

fn recognize(peer: &mut Peer, other: PeerId) {
    wait_prefix(peer, &format!("connected {other}"));
    wait_prefix(peer, &format!("identified {other}"));
    wait_prefix(peer, &format!("handshake {other}"));
}

fn learns(line: &str, peer: PeerId) -> bool {
    let id = peer.to_string();
    (line.starts_with("discovered ")
        || line.starts_with("connected ")
        || line.starts_with("identified ")
        || line.starts_with("handshake ")
        || line.starts_with("address "))
        && line.contains(&id)
}

fn wait_cross(left: &mut Peer, right_id: PeerId, right: &mut Peer, left_id: PeerId) -> bool {
    let started = Instant::now();
    loop {
        let sees_right = lines_of(left).iter().any(|line| learns(line, right_id));
        let sees_left = lines_of(right).iter().any(|line| learns(line, left_id));
        if sees_right && sees_left {
            return true;
        }
        if started.elapsed() > Duration::from_secs(12) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assert_no_session(peer: &Peer) {
    let lines = lines_of(peer);
    assert!(!lines.iter().any(|line| line.starts_with("connected ")), "{lines:?}");
    assert!(!lines.iter().any(|line| line.starts_with("identified ")), "{lines:?}");
    assert!(!lines.iter().any(|line| line.starts_with("handshake ")), "{lines:?}");
}

#[test]
fn three_processes_bootstrap_from_the_seed() {
    let mut seed = spawn(data_dir("seed"), "devnet", "3", None);
    let seed_peer = peer_of(&wait_prefix(&mut seed, "peer "));
    let bootstrap = listen_of(&mut seed);
    let mut node_b = spawn_bootstrap(data_dir("node-b"), "devnet", "3", &bootstrap);
    let mut node_c = spawn_bootstrap(data_dir("node-c"), "devnet", "3", &bootstrap);
    let peer_b = peer_of(&wait_prefix(&mut node_b, "peer "));
    let peer_c = peer_of(&wait_prefix(&mut node_c, "peer "));
    assert_ne!(seed_peer, peer_b);
    assert_ne!(seed_peer, peer_c);
    assert_ne!(peer_b, peer_c);
    assert!(!bootstrap.contains(&peer_b.to_string()));
    assert!(!bootstrap.contains(&peer_c.to_string()));
    recognize(&mut node_b, seed_peer);
    recognize(&mut node_c, seed_peer);
    recognize(&mut seed, peer_b);
    recognize(&mut seed, peer_c);
    assert!(
        lines_of(&node_b)
            .iter()
            .any(|line| line == "bootstrap-ok" || line == &format!("discovered {seed_peer}")),
        "{:?}",
        lines_of(&node_b)
    );
    assert!(
        lines_of(&node_c)
            .iter()
            .any(|line| line == "bootstrap-ok" || line == &format!("discovered {seed_peer}")),
        "{:?}",
        lines_of(&node_c)
    );
    assert!(
        wait_cross(&mut node_b, peer_c, &mut node_c, peer_b),
        "B y C no se descubrieron: {:?} {:?}",
        lines_of(&node_b),
        lines_of(&node_c)
    );
    stop(&mut node_b);
    stop(&mut node_c);
    stop(&mut seed);
}

#[test]
fn an_unreachable_bootstrap_does_not_open_a_session() {
    let peer = PeerId::random();
    let mut alone = spawn_bootstrap(
        data_dir("boot-closed"),
        "devnet",
        "3",
        &format!("/ip4/127.0.0.1/tcp/1/p2p/{peer}"),
    );
    let failed = wait_prefix(&mut alone, "dial-failed ");
    assert!(failed.contains("dial failed"), "{failed}");
    assert_no_session(&alone);
    stop(&mut alone);
}

#[test]
fn a_bootstrap_on_another_network_rejects_the_handshake() {
    let mut seed = spawn(data_dir("boot-net-a"), "mainnet", "1", None);
    let listen = listen_of(&mut seed);
    let mut node = spawn_bootstrap(data_dir("boot-net-b"), "devnet", "3", &listen);
    wait_rejection(&mut seed, &mut node, "network mismatch");
    stop(&mut node);
    stop(&mut seed);
}

#[test]
fn a_bootstrap_on_another_chain_rejects_the_handshake() {
    let mut seed = spawn(data_dir("boot-chain-a"), "devnet", "3", None);
    let listen = listen_of(&mut seed);
    let mut node = spawn_bootstrap(data_dir("boot-chain-b"), "devnet", "1", &listen);
    wait_rejection(&mut seed, &mut node, "chain mismatch");
    stop(&mut node);
    stop(&mut seed);
}

#[test]
fn a_bootstrap_peer_id_mismatch_is_not_a_session() {
    let mut seed = spawn(data_dir("boot-real"), "devnet", "3", None);
    let listen = listen_of(&mut seed);
    let endpoint = listen.split("/p2p/").next().unwrap();
    let unknown = PeerId::random();
    let mut node = spawn_bootstrap(
        data_dir("boot-mismatch"),
        "devnet",
        "3",
        &format!("{endpoint}/p2p/{unknown}"),
    );
    let failed = wait_prefix(&mut node, "dial-failed ");
    assert!(failed.contains("dial failed"), "{failed}");
    assert_no_session(&node);
    assert_no_session(&seed);
    stop(&mut node);
    stop(&mut seed);
}

#[test]
fn a_wrong_bootstrap_endpoint_does_not_connect() {
    let peer = PeerId::random();
    let mut alone = spawn_bootstrap(
        data_dir("boot-endpoint"),
        "devnet",
        "3",
        &format!("/ip4/127.0.0.1/tcp/9/p2p/{peer}"),
    );
    let failed = wait_prefix(&mut alone, "dial-failed ");
    assert!(failed.contains("dial failed"), "{failed}");
    assert_no_session(&alone);
    stop(&mut alone);
}

#[test]
fn a_bootstrap_without_peer_id_exits() {
    let dir = data_dir("boot-noid");
    let output = Command::new(bin())
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--bootstrap",
            "/ip4/127.0.0.1/tcp/9",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8(output.stderr).unwrap();
    assert!(err.contains("invalid address"), "{err}");
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    assert!(!dir.join("LOCK").exists());
    std::fs::remove_dir(&dir).unwrap();
}

fn wait_among(nodes: [&Peer; 4], prefix: &str) -> String {
    let started = Instant::now();
    loop {
        for node in nodes {
            if let Some(line) = lines_of(node)
                .into_iter()
                .find(|line| line.starts_with(prefix))
            {
                return line;
            }
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!("no apareció {prefix}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn count_prefix(peer: &Peer, prefix: &str) -> usize {
    lines_of(peer)
        .iter()
        .filter(|line| line.starts_with(prefix))
        .count()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_health(addr: &str) {
    let started = Instant::now();
    loop {
        if let Ok(mut stream) = std::net::TcpStream::connect(addr) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let req = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                let _ = stream.read_to_string(&mut buf);
                if buf.contains("\"status\":\"ok\"") {
                    return;
                }
            }
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!("sin health {addr}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn post_rpc(addr: &str, method: &str, params: &str) -> String {
    let body = if params.is_empty() {
        format!(r#"{{"jsonrpc":"2.0","method":"{method}","id":1}}"#)
    } else {
        format!(r#"{{"jsonrpc":"2.0","method":"{method}","id":1,"params":{params}}}"#)
    };
    let mut last = String::new();
    for _ in 0..50 {
        if let Ok(mut stream) = std::net::TcpStream::connect(addr) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let req = format!(
                "POST /rpc HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                let _ = stream.read_to_string(&mut buf);
                if buf.contains("jsonrpc") {
                    return buf;
                }
                last = buf;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("sin respuesta rpc {addr}: {last}");
}

fn account_of(key: &PrivateKey) -> AccountId {
    let id = key.key_id();
    let mut builder = HashBuilder::new();
    builder.update(b"RSC/STATE/ACCOUNT-ID/V1");
    builder.update(id.as_bytes());
    AccountId::from_slice(&builder.finalize().to_bytes()).unwrap()
}

fn signer_hex() -> String {
    let key = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
    encode_hex(&key.public_key().to_bytes())
}

fn signed_transfer(nonce: u64, amount: u128) -> Transaction {
    signed_from(0x11, 0x22, nonce, amount)
}

fn signed_from(seed: u8, recipient: u8, nonce: u64, amount: u128) -> Transaction {
    let key = PrivateKey::from_bytes(&[seed; 32]).unwrap();
    let recipient = account_of(&PrivateKey::from_bytes(&[recipient; 32]).unwrap());
    let mut tx = Transaction {
        version: TRANSACTION_VERSION_V1,
        sender: account_of(&key),
        nonce,
        payload: TransactionPayload::Transfer { recipient, amount },
        signature: rsc_crypto::Signature::from_bytes(&[0u8; 64]).unwrap(),
    };
    tx.sign(&key).unwrap();
    tx
}

fn assert_chain_unchanged(addr: &str) {
    let status = post_rpc(addr, "rsc_getStatus", "");
    assert!(status.contains("\"latest_height\":0"), "{status}");
    assert!(
        status.contains(&format!(
            "\"latest_block_hash\":\"{}\"",
            "0".repeat(64)
        )),
        "{status}"
    );
    let fin = post_rpc(addr, "rsc_getFinality", "");
    assert!(fin.contains("service unavailable"), "{fin}");
    let block = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":0}"#);
    assert!(block.contains("not found"), "{block}");
}

fn mesh(seed: &mut Peer, node_b: &mut Peer, node_c: &mut Peer) {
    let seed_peer = peer_of(&wait_prefix(seed, "peer "));
    let peer_b = peer_of(&wait_prefix(node_b, "peer "));
    let peer_c = peer_of(&wait_prefix(node_c, "peer "));
    recognize(node_b, seed_peer);
    recognize(node_c, seed_peer);
    recognize(seed, peer_b);
    recognize(seed, peer_c);
    recognize(node_b, peer_c);
    recognize(node_c, peer_b);
    std::thread::sleep(Duration::from_secs(1));
}

#[test]
fn three_processes_propagate_a_transaction() {
    let signer = signer_hex();
    let rpc_a = format!("127.0.0.1:{}", free_port());
    let rpc_b = format!("127.0.0.1:{}", free_port());
    let rpc_c = format!("127.0.0.1:{}", free_port());
    let mut seed = spawn_tx(data_dir("tx-a"), None, &rpc_a, &signer);
    let bootstrap = listen_of(&mut seed);
    wait_prefix(&mut seed, "rpc ");
    let mut node_b = spawn_tx(data_dir("tx-b"), Some(&bootstrap), &rpc_b, &signer);
    let mut node_c = spawn_tx(data_dir("tx-c"), Some(&bootstrap), &rpc_c, &signer);
    mesh(&mut seed, &mut node_b, &mut node_c);
    assert_chain_unchanged(&rpc_a);
    let tx = signed_transfer(0, 7);
    tx.validate(&PrivateKey::from_bytes(&[0x11; 32]).unwrap().public_key())
        .unwrap();
    let id = encode_hex(tx.id().0.as_bytes());
    let hex = encode_hex(&tx.canonical_bytes());
    let sent = post_rpc(
        &rpc_a,
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{hex}"}}"#),
    );
    assert!(sent.contains("\"result\""), "{sent}");
    assert!(sent.contains(&id), "{sent}");
    wait_prefix(&mut seed, &format!("published {id}"));
    wait_prefix(&mut node_b, &format!("admitted {id}"));
    wait_prefix(&mut node_c, &format!("admitted {id}"));
    let again = post_rpc(
        &rpc_a,
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{hex}"}}"#),
    );
    assert!(again.contains("transaction rejected"), "{again}");
    wait_prefix(&mut seed, "rejected transaction is already in the mempool");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(count_prefix(&seed, &format!("admitted {id}")), 1);
    assert_eq!(count_prefix(&node_b, &format!("admitted {id}")), 1);
    assert_eq!(count_prefix(&node_c, &format!("admitted {id}")), 1);
    assert_eq!(count_prefix(&seed, &format!("published {id}")), 1);
    for addr in [&rpc_a, &rpc_b, &rpc_c] {
        assert_chain_unchanged(addr);
    }
    stop(&mut node_b);
    stop(&mut node_c);
    stop(&mut seed);
}

fn assert_rejected_without_propagation(raw_hex: &str, reason: &str) {
    let signer = signer_hex();
    let rpc_a = format!("127.0.0.1:{}", free_port());
    let rpc_b = format!("127.0.0.1:{}", free_port());
    let mut seed = spawn_tx(data_dir("bad-a"), None, &rpc_a, &signer);
    let bootstrap = listen_of(&mut seed);
    let mut node = spawn_tx(data_dir("bad-b"), Some(&bootstrap), &rpc_b, &signer);
    let seed_peer = peer_of(&wait_prefix(&mut seed, "peer "));
    let peer = peer_of(&wait_prefix(&mut node, "peer "));
    recognize(&mut seed, peer);
    recognize(&mut node, seed_peer);
    std::thread::sleep(Duration::from_secs(1));
    let sent = post_rpc(
        &rpc_a,
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{raw_hex}"}}"#),
    );
    assert!(sent.contains("transaction rejected"), "{sent}");
    let rejected = wait_prefix(&mut seed, "rejected ");
    assert!(rejected.contains(reason), "{rejected}");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(count_prefix(&seed, "published "), 0);
    assert_eq!(count_prefix(&node, "admitted "), 0);
    stop(&mut node);
    stop(&mut seed);
}

#[test]
fn an_altered_signature_is_not_propagated() {
    let mut bytes = signed_transfer(0, 7).canonical_bytes();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert_rejected_without_propagation(&encode_hex(&bytes), "transaction validation failed");
}

#[test]
fn a_malformed_payload_is_not_propagated() {
    assert_rejected_without_propagation("00", "transaction is invalid");
}

#[test]
fn three_processes_propagate_a_proposal() {
    let rpc_a = format!("127.0.0.1:{}", free_port());
    let rpc_b = format!("127.0.0.1:{}", free_port());
    let rpc_c = format!("127.0.0.1:{}", free_port());
    let mut seed = spawn_proposal(data_dir("prop-a"), None, &rpc_a, "propose");
    let bootstrap = listen_of(&mut seed);
    wait_prefix(&mut seed, "rpc ");
    let mut node_b = spawn_proposal(data_dir("prop-b"), Some(&bootstrap), &rpc_b, "observe");
    let mut node_c = spawn_proposal(data_dir("prop-c"), Some(&bootstrap), &rpc_c, "observe");
    mesh(&mut seed, &mut node_b, &mut node_c);
    assert_chain_unchanged(&rpc_a);
    assert_chain_unchanged(&rpc_b);
    assert_chain_unchanged(&rpc_c);
    let published = wait_prefix(&mut seed, "published ");
    let hash = published.split_whitespace().nth(1).unwrap().to_string();
    assert_eq!(
        wait_prefix(&mut seed, "proposer "),
        "proposer 5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(count_prefix(&seed, &format!("published {hash}")), 2);
    let accepted_b = wait_prefix(&mut node_b, &format!("accepted {hash}"));
    let accepted_c = wait_prefix(&mut node_c, &format!("accepted {hash}"));
    assert_eq!(accepted_b, format!("accepted {hash}"));
    assert_eq!(accepted_c, format!("accepted {hash}"));
    for node in [&mut node_b, &mut node_c] {
        wait_prefix(node, "proposal-rejected invalid proposer");
        wait_prefix(node, "proposal-rejected wrong view");
        wait_prefix(node, "proposal-rejected invalid block");
        wait_prefix(node, "proposal-rejected invalid encoding");
    }
    std::thread::sleep(Duration::from_millis(400));
    for node in [&node_b, &node_c] {
        let accepted: Vec<_> = lines_of(node)
            .into_iter()
            .filter(|line| line.starts_with("accepted "))
            .collect();
        assert!(!accepted.is_empty());
        assert!(
            accepted.iter().all(|line| line == &format!("accepted {hash}")),
            "{accepted:?}"
        );
        let state = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("state "))
            .unwrap();
        assert!(state.contains("height 0"), "{state}");
        assert!(state.contains("epoch 0"), "{state}");
        assert!(state.contains("view 0"), "{state}");
        assert!(state.contains("phase Propose"), "{state}");
        assert!(state.contains(&format!("proposal {hash}")), "{state}");
        for line in lines_of(node) {
            assert!(!line.contains("phase Prepare"), "{line}");
            assert!(!line.contains("phase PreCommit"), "{line}");
            assert!(!line.contains("phase Commit"), "{line}");
            assert!(!line.contains("phase Decide"), "{line}");
        }
    }
    assert_eq!(count_prefix(&seed, "published-invalid invalid proposer"), 1);
    assert_eq!(count_prefix(&seed, "published-invalid wrong view"), 1);
    assert_eq!(count_prefix(&seed, "published-invalid invalid block"), 1);
    assert_eq!(count_prefix(&seed, "published-invalid invalid encoding"), 1);
    for addr in [&rpc_a, &rpc_b, &rpc_c] {
        assert_chain_unchanged(addr);
    }
    stop(&mut node_b);
    stop(&mut node_c);
    stop(&mut seed);
}

#[test]
fn four_validators_propagate_votes_and_qcs() {
    let rpc: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_vote(data_dir("vote-0"), None, &rpc[0], 0);
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_vote(data_dir("vote-1"), Some(&bootstrap), &rpc[1], 1);
    let mut v2 = spawn_vote(data_dir("vote-2"), Some(&bootstrap), &rpc[2], 2);
    let mut v3 = spawn_vote(data_dir("vote-3"), Some(&bootstrap), &rpc[3], 3);
    let peers = [
        peer_of(&wait_prefix(&mut v0, "peer ")),
        peer_of(&wait_prefix(&mut v1, "peer ")),
        peer_of(&wait_prefix(&mut v2, "peer ")),
        peer_of(&wait_prefix(&mut v3, "peer ")),
    ];
    let mut nodes = [&mut v0, &mut v1, &mut v2, &mut v3];
    for (index, node) in nodes.iter_mut().enumerate() {
        for (other, peer) in peers.iter().enumerate() {
            if index != other {
                recognize(node, *peer);
            }
        }
    }
    std::thread::sleep(Duration::from_secs(1));
    for addr in &rpc {
        assert_chain_unchanged(addr);
    }
    let proposers: Vec<String> = nodes
        .iter_mut()
        .map(|node| wait_prefix(node, "proposer "))
        .collect();
    assert!(proposers.iter().all(|line| line == &proposers[0]), "{proposers:?}");
    let published = wait_among([&v0, &v1, &v2, &v3], "published ");
    let hash = published.split_whitespace().nth(1).unwrap().to_string();
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, &format!("accepted {hash}"));
        wait_prefix(node, "quorum Prepare 5 6 15 12");
        wait_prefix(node, "qc-ok Prepare 5");
        wait_prefix(node, "vote-rejected invalid signature");
        wait_prefix(node, "vote-rejected wrong view");
        wait_prefix(node, "consensus-rejected invalid encoding");
        wait_prefix(node, "qc-rejected ");
        wait_prefix(node, "quorum PreCommit 5 6 15 12");
        wait_prefix(node, "qc-ok PreCommit 5");
        wait_prefix(node, "quorum Commit 5 6 15 12");
        wait_prefix(node, "qc-ok Commit 5");
        wait_prefix(node, &format!("finalized {hash}"));
    }
    std::thread::sleep(Duration::from_millis(400));
    assert!(count_prefix(&v3, "vote-duplicate Prepare ") >= 1);
    for node in [&v0, &v1, &v2, &v3] {
        assert_eq!(count_prefix(node, "quorum Prepare 5 6 15 12"), 1);
        assert_eq!(count_prefix(node, "quorum PreCommit 5 6 15 12"), 1);
        assert_eq!(count_prefix(node, "quorum Commit 5 6 15 12"), 1);
        assert!(count_prefix(node, "vote-rejected invalid signature") >= 3);
        assert!(count_prefix(node, "vote-rejected wrong view") >= 3);
        assert!(count_prefix(node, "qc-rejected ") >= 2);
        let state = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("state "))
            .unwrap();
        assert!(state.contains("height 0"), "{state}");
        assert!(state.contains("epoch 0"), "{state}");
        assert!(state.contains("view 0"), "{state}");
        assert!(state.contains(&format!("proposal {hash}")), "{state}");
    }
    for addr in &rpc {
        assert_chain_unchanged(addr);
    }
    assert_eq!(
        proposers[0],
        "proposer 3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c"
    );
    assert_eq!(
        wait_prefix(&mut v0, "validator 0 "),
        "validator 0 5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f 2"
    );
    assert_eq!(
        wait_prefix(&mut v1, "validator 1 "),
        "validator 1 3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c 2"
    );
    assert_eq!(
        wait_prefix(&mut v2, "validator 2 "),
        "validator 2 ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31 1"
    );
    assert_eq!(
        wait_prefix(&mut v3, "validator 3 "),
        "validator 3 a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a 1"
    );
    stop(&mut v1);
    stop(&mut v2);
    stop(&mut v3);
    stop(&mut v0);
}

const V0_ID: &str = "5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f";
const V1_ID: &str = "3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c";
const V2_ID: &str = "ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31";
const V3_ID: &str = "a50a7d5a5b0ce6621ee072c2ffe0865419936d6878ec5f8453a868b1f8b3901a";
const VOTERS: &str = "3a922ee80518371422fdbb80a9fc8c1eba745b99e26a9b9d59867679fcc0117c 5beec54367059e4865ba34000cb48e28d5ac7e19de6bf4fa0c4b3b426012454f ec0b95a03350d2c413b9ecc90a58e3f91c2e405552ae0d24dd2970c95d0baa31";

#[test]
fn four_validators_decide_from_local_engines() {
    let rpc: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_vote(data_dir("decide-0"), None, &rpc[0], 0);
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_vote(data_dir("decide-1"), Some(&bootstrap), &rpc[1], 1);
    let mut v2 = spawn_vote(data_dir("decide-2"), Some(&bootstrap), &rpc[2], 2);
    let mut v3 = spawn_vote(data_dir("decide-3"), Some(&bootstrap), &rpc[3], 3);
    let peers = [
        peer_of(&wait_prefix(&mut v0, "peer ")),
        peer_of(&wait_prefix(&mut v1, "peer ")),
        peer_of(&wait_prefix(&mut v2, "peer ")),
        peer_of(&wait_prefix(&mut v3, "peer ")),
    ];
    let mut nodes = [&mut v0, &mut v1, &mut v2, &mut v3];
    for (index, node) in nodes.iter_mut().enumerate() {
        for (other, peer) in peers.iter().enumerate() {
            if index != other {
                recognize(node, *peer);
            }
        }
    }
    std::thread::sleep(Duration::from_secs(1));
    let proposers: Vec<String> = nodes
        .iter_mut()
        .map(|node| wait_prefix(node, "proposer "))
        .collect();
    assert!(proposers.iter().all(|line| line == &proposers[0]), "{proposers:?}");
    let published = wait_among([&v0, &v1, &v2, &v3], "published ");
    let hash = published.split_whitespace().nth(1).unwrap().to_string();
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, &format!("accepted {hash}"));
        for phase in ["Prepare", "PreCommit", "Commit"] {
            wait_prefix(node, &format!("quorum {phase} 5 6 15 12"));
            wait_prefix(node, &format!("voters {phase} {VOTERS}"));
            wait_prefix(node, &format!("qc-ok {phase} 5"));
            wait_prefix(node, &format!("proof-voters {phase} {VOTERS} 5"));
        }
        wait_prefix(node, &format!("proof {hash} 0 0 0 5 5 5"));
        wait_prefix(node, &format!("finalized {hash}"));
        wait_prefix(node, "vote-rejected equivocation");
        wait_prefix(node, "vote-rejected invalid signature");
        wait_prefix(node, "vote-rejected wrong view");
        wait_prefix(node, "consensus-rejected invalid encoding");
    }
    std::thread::sleep(Duration::from_millis(400));
    assert!(count_prefix(&v3, "vote-duplicate Prepare ") >= 1);
    for node in [&v0, &v1, &v2, &v3] {
        assert_eq!(count_prefix(node, "quorum Prepare 5 6 15 12"), 1);
        assert_eq!(count_prefix(node, "quorum PreCommit 5 6 15 12"), 1);
        assert_eq!(count_prefix(node, "quorum Commit 5 6 15 12"), 1);
        for phase in ["Prepare", "PreCommit", "Commit"] {
            let voters: Vec<_> = lines_of(node)
                .into_iter()
                .filter(|line| line == &format!("voters {phase} {VOTERS}"))
                .collect();
            assert_eq!(voters.len(), 1, "{phase}");
            assert!(!lines_of(node).iter().any(|line| {
                line.starts_with(&format!("voters {phase} ")) && line.contains(V3_ID)
            }));
            assert!(!lines_of(node).iter().any(|line| {
                line.starts_with(&format!("proof-voters {phase} ")) && line.contains(V3_ID)
            }));
        }
        let state = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("state "))
            .unwrap();
        assert!(state.contains("phase Decide"), "{state}");
        assert!(state.contains("height 0"), "{state}");
        assert!(state.contains("epoch 0"), "{state}");
        assert!(state.contains("view 0"), "{state}");
        assert!(state.contains(&format!("proposal {hash}")), "{state}");
        assert_eq!(count_prefix(node, &format!("proof {hash} 0 0 0 5 5 5")), 1);
    }
    assert!(count_prefix(&v0, &format!("vote-ok Prepare {V0_ID}")) >= 1);
    assert!(count_prefix(&v1, &format!("vote-ok Prepare {V1_ID}")) >= 1);
    assert!(count_prefix(&v2, &format!("vote-ok Prepare {V2_ID}")) >= 1);
    assert_eq!(count_prefix(&v3, &format!("vote-ok Prepare {V3_ID}")), 0);
    for addr in &rpc {
        assert_chain_unchanged(addr);
    }
    eprintln!("proposer {}", proposers[0]);
    eprintln!("block {hash}");
    eprintln!("bootstrap {bootstrap}");
    for (index, node) in [&v0, &v1, &v2, &v3].iter().enumerate() {
        let peer = lines_of(node)
            .into_iter()
            .find(|line| line.starts_with("peer "))
            .unwrap();
        let listen = lines_of(node)
            .into_iter()
            .find(|line| line.starts_with("listen "))
            .unwrap();
        let state = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("state "))
            .unwrap();
        eprintln!("V{index} {peer} {listen} {state}");
    }
    stop(&mut v1);
    stop(&mut v2);
    stop(&mut v3);
    stop(&mut v0);
}

#[test]
fn two_validators_do_not_reach_quorum() {
    let rpc_a = format!("127.0.0.1:{}", free_port());
    let rpc_b = format!("127.0.0.1:{}", free_port());
    let mut v0 = spawn_vote(data_dir("pair-0"), None, &rpc_a, 0);
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_vote(data_dir("pair-1"), Some(&bootstrap), &rpc_b, 1);
    let peer_0 = peer_of(&wait_prefix(&mut v0, "peer "));
    let peer_1 = peer_of(&wait_prefix(&mut v1, "peer "));
    recognize(&mut v0, peer_1);
    recognize(&mut v1, peer_0);
    std::thread::sleep(Duration::from_secs(1));
    let proposer_0 = wait_prefix(&mut v0, "proposer ");
    let proposer_1 = wait_prefix(&mut v1, "proposer ");
    assert_eq!(proposer_0, proposer_1);
    assert_eq!(proposer_0, format!("proposer {V1_ID}"));
    let started = Instant::now();
    let published = loop {
        if let Some(line) = lines_of(&v0)
            .into_iter()
            .chain(lines_of(&v1))
            .find(|line| line.starts_with("published "))
        {
            break line;
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!("no apareció published");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let hash = published.split_whitespace().nth(1).unwrap().to_string();
    for node in [&mut v0, &mut v1] {
        wait_prefix(node, &format!("accepted {hash}"));
        wait_prefix(node, "no-quorum Prepare 4 6 12 12");
        wait_prefix(node, &format!("vote-ok Prepare {V0_ID}"));
        wait_prefix(node, &format!("vote-ok Prepare {V1_ID}"));
    }
    std::thread::sleep(Duration::from_secs(4));
    for node in [&v0, &v1] {
        assert_eq!(count_prefix(node, "quorum "), 0);
        assert_eq!(count_prefix(node, "qc-ok "), 0);
        assert_eq!(count_prefix(node, "finalized "), 0);
        assert_eq!(count_prefix(node, "proof "), 0);
        assert_eq!(count_prefix(node, "vote-ok PreCommit "), 0);
        assert_eq!(count_prefix(node, "vote-ok Commit "), 0);
        assert!(lines_of(node).iter().all(|line| !line.contains("phase Decide")));
        let state = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("state "))
            .unwrap();
        assert!(state.contains("phase Propose"), "{state}");
        assert!(state.contains("height 0"), "{state}");
        assert!(state.contains("epoch 0"), "{state}");
        assert!(state.contains("view 0"), "{state}");
        assert!(state.contains(&format!("proposal {hash}")), "{state}");
    }
    assert_chain_unchanged(&rpc_a);
    assert_chain_unchanged(&rpc_b);
    eprintln!("pair proposer {proposer_0}");
    eprintln!("pair block {hash}");
    eprintln!("pair v0 {}", lines_of(&v0).into_iter().find(|line| line.starts_with("peer ")).unwrap());
    eprintln!("pair v1 {}", lines_of(&v1).into_iter().find(|line| line.starts_with("peer ")).unwrap());
    stop(&mut v1);
    stop(&mut v0);
}

fn spawn_produce(data: PathBuf, bootstrap: Option<&str>, rpc: &str, role: &str) -> Peer {
    launch(
        data,
        "devnet",
        "3",
        None,
        bootstrap,
        Some(rpc),
        Some(&signer_hex()),
        Some(role),
    )
}

fn stop_keep(peer: &mut Peer) {
    let mut child = peer.child.take().unwrap();
    eprintln!("fase stop-keep-inicio pid={}", child.id());
    drop(peer.stdin.take());
    #[cfg(unix)]
    {
        assert!(child_pids(child.id()).is_empty());
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
    }
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            assert!(child_pids(child.id()).is_empty());
            assert!(peer.data_dir.join("CURRENT").is_file());
            return;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("el proceso no cerró con TERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn expect_db_reject(data: &std::path::Path, network: &str, chain: &str, needle: &str) {
    let rpc = format!("127.0.0.1:{}", free_port());
    let signer = signer_hex();
    let mut child = Command::new(bin())
        .args([
            "--data-dir",
            data.to_str().unwrap(),
            "--network-id",
            network,
            "--chain-id",
            chain,
            "--rpc",
            &rpc,
            "--signer-key",
            &signer,
            "--consensus",
            "behind",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success(), "{status}");
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            assert!(err.contains(needle), "{err}");
            return;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("la base no rechazó {needle}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop_chain(peer: &mut Peer) {
    let mut child = peer.child.take().unwrap();
    eprintln!("fase stop-chain-inicio pid={}", child.id());
    drop(peer.stdin.take());
    #[cfg(unix)]
    {
        assert!(child_pids(child.id()).is_empty());
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
    }
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            assert!(child_pids(child.id()).is_empty());
            std::fs::remove_dir_all(&peer.data_dir).unwrap();
            return;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("el proceso no cerró con TERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn mesh_four(nodes: [&mut Peer; 4]) {
    let peers: Vec<PeerId> = nodes
        .iter()
        .map(|node| peer_of(&lines_of(node).into_iter().find(|line| line.starts_with("peer ")).unwrap()))
        .collect();
    for (index, node) in nodes.into_iter().enumerate() {
        for (other, peer) in peers.iter().enumerate() {
            if index != other {
                recognize(node, *peer);
            }
        }
    }
}

#[test]
fn four_processes_commit_one_ready_transaction() {
    let rpc: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_produce(data_dir("prod-0"), None, &rpc[0], "produce-0");
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_produce(data_dir("prod-1"), Some(&bootstrap), &rpc[1], "produce-1");
    let mut v2 = spawn_produce(data_dir("prod-2"), Some(&bootstrap), &rpc[2], "produce-2");
    let mut v3 = spawn_produce(data_dir("prod-3"), Some(&bootstrap), &rpc[3], "produce-3");
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, "peer ");
        wait_prefix(node, "proposer ");
        wait_prefix(node, "tip ");
    }
    mesh_four([&mut v0, &mut v1, &mut v2, &mut v3]);
    std::thread::sleep(Duration::from_secs(2));
    for node in [&v0, &v1, &v2, &v3] {
        assert_eq!(count_prefix(node, "block "), 0);
        let tip = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("tip "))
            .unwrap();
        assert!(tip.ends_with(" 0"), "{tip}");
    }
    for addr in &rpc {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":0"), "{status}");
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":1}"#);
        assert!(missing.contains("not found"), "{missing}");
    }
    let zero = encode_hex(&signed_transfer(0, 0).canonical_bytes());
    let rejected = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{zero}"}}"#),
    );
    assert!(rejected.contains("transaction rejected"), "{rejected}");
    std::thread::sleep(Duration::from_millis(400));
    for node in [&v0, &v1, &v2, &v3] {
        assert_eq!(count_prefix(node, "block "), 0);
        assert_eq!(count_prefix(node, "admitted "), 0);
    }
    let tx = signed_transfer(0, 1);
    let id = encode_hex(tx.id().0.as_bytes());
    let hex = encode_hex(&tx.canonical_bytes());
    let sent = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{hex}"}}"#),
    );
    assert!(sent.contains(&id), "{sent}");
    let mut nodes = [&mut v0, &mut v1, &mut v2, &mut v3];
    let proposers: Vec<String> = nodes
        .iter_mut()
        .map(|node| wait_prefix(node, "proposer "))
        .collect();
    assert!(proposers.iter().all(|line| line == &proposers[0]), "{proposers:?}");
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, &format!("ready {id}"));
    }
    let published = wait_among([&v0, &v1, &v2, &v3], "block ");
    let mut parts = published.split_whitespace();
    assert_eq!(parts.next(), Some("block"));
    let hash = parts.next().unwrap().to_string();
    let previous = parts.next().unwrap().to_string();
    assert_eq!(parts.next(), Some("1"));
    let tx_root = parts.next().unwrap().to_string();
    let state_root = parts.next().unwrap().to_string();
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, &format!("accepted {hash}"));
        for phase in ["Prepare", "PreCommit", "Commit"] {
            wait_prefix(node, &format!("quorum {phase} 5 6 15 12"));
            wait_prefix(node, &format!("voters {phase} {VOTERS}"));
            wait_prefix(node, &format!("qc-ok {phase} 5"));
        }
        wait_prefix(node, &format!("proof {hash} 0 1 0 5 5 5"));
        wait_prefix(node, &format!("finalized {hash}"));
        wait_prefix(
            node,
            &format!("committed {hash} 1 {previous} {tx_root} {state_root} "),
        );
    }
    std::thread::sleep(Duration::from_millis(500));
    let committed = wait_prefix(&mut v0, &format!("committed {hash} 1 {previous} {tx_root} {state_root} "));
    assert!(committed.ends_with(&id), "{committed}");
    for node in [&v0, &v1, &v2, &v3] {
        assert_eq!(count_prefix(node, "committed "), 1, "{:?}", lines_of(node));
        assert_eq!(count_prefix(node, "block "), usize::from(lines_of(node).iter().any(|line| line.starts_with("block "))));
        assert_eq!(count_prefix(node, "children 1"), 1);
        assert_eq!(count_prefix(node, "pool 0"), 1);
        assert!(lines_of(node).iter().any(|line| line == &format!("account {V0_ID} 0 1")));
        assert!(lines_of(node).iter().any(|line| line == &format!("account {V1_ID} 2 0")));
        let tip = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("tip "))
            .unwrap();
        assert_eq!(tip, format!("tip {hash} 1"));
        let state = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("state "))
            .unwrap();
        assert!(state.contains("phase Decide"), "{state}");
        assert!(state.contains("height 1"), "{state}");
        assert!(state.contains("epoch 0"), "{state}");
        assert!(state.contains("view 0"), "{state}");
    }
    assert!(count_prefix(&v3, &format!("accepted {hash}")) >= 2);
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_prefix(node, "proposal-rejected invalid proposer");
        wait_prefix(node, "proposal-rejected invalid block");
    }
    for addr in &rpc {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":1"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash}\"")), "{status}");
        let block = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":1}"#);
        assert!(block.contains(&hash), "{block}");
        assert!(block.contains(&previous), "{block}");
        assert!(block.contains(&id) || block.contains(&hex), "{block}");
        let genesis = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":0}"#);
        assert!(genesis.contains(&previous), "{genesis}");
        let next = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":2}"#);
        assert!(next.contains("not found"), "{next}");
        let sender = post_rpc(addr, "rsc_getAccount", &format!(r#"{{"account":"{V0_ID}"}}"#));
        assert!(sender.contains("\"balance\":\"0\""), "{sender}");
        assert!(sender.contains("\"nonce\":1"), "{sender}");
        let recipient = post_rpc(addr, "rsc_getAccount", &format!(r#"{{"account":"{V1_ID}"}}"#));
        assert!(recipient.contains("\"balance\":\"2\""), "{recipient}");
        assert!(recipient.contains("\"nonce\":0"), "{recipient}");
    }
    eprintln!("proposer {}", proposers[0]);
    eprintln!("tx {id}");
    eprintln!("block {published}");
    eprintln!("committed {committed}");
    for (index, node) in [&v0, &v1, &v2, &v3].iter().enumerate() {
        let peer = lines_of(node).into_iter().find(|line| line.starts_with("peer ")).unwrap();
        let listen = lines_of(node).into_iter().find(|line| line.starts_with("listen ")).unwrap();
        eprintln!("V{index} {peer} {listen}");
    }
    stop_chain(&mut v1);
    stop_chain(&mut v2);
    stop_chain(&mut v3);
    stop_chain(&mut v0);
}

#[test]
fn power_four_does_not_finalize_a_block() {
    let rpc: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_produce(data_dir("pairp-0"), None, &rpc[0], "produce-0");
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_produce(data_dir("pairp-1"), Some(&bootstrap), &rpc[1], "produce-1");
    let mut v2 = spawn_produce(data_dir("pairp-2"), Some(&bootstrap), &rpc[2], "shadow-2");
    let mut v3 = spawn_produce(data_dir("pairp-3"), Some(&bootstrap), &rpc[3], "shadow-3");
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, "peer ");
    }
    mesh_four([&mut v0, &mut v1, &mut v2, &mut v3]);
    std::thread::sleep(Duration::from_secs(1));
    let tx = signed_transfer(0, 1);
    let id = encode_hex(tx.id().0.as_bytes());
    let hex = encode_hex(&tx.canonical_bytes());
    let sent = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{hex}"}}"#),
    );
    assert!(sent.contains(&id), "{sent}");
    let published = wait_among([&v0, &v1, &v2, &v3], "block ");
    let hash = published.split_whitespace().nth(1).unwrap().to_string();
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, &format!("accepted {hash}"));
        wait_prefix(node, "no-quorum Prepare 4 6 12 12");
    }
    std::thread::sleep(Duration::from_secs(4));
    for node in [&v0, &v1, &v2, &v3] {
        assert_eq!(count_prefix(node, "quorum "), 0);
        assert_eq!(count_prefix(node, "finalized "), 0);
        assert_eq!(count_prefix(node, "committed "), 0);
        assert_eq!(count_prefix(node, "proof "), 0);
        let tip = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("tip "))
            .unwrap();
        assert!(tip.ends_with(" 0"), "{tip}");
    }
    for addr in &rpc {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":0"), "{status}");
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":1}"#);
        assert!(missing.contains("not found"), "{missing}");
    }
    eprintln!("pair block {published}");
    stop_chain(&mut v1);
    stop_chain(&mut v2);
    stop_chain(&mut v3);
    stop_chain(&mut v0);
}

#[test]
fn a_zero_amount_is_not_propagated() {
    let hex = encode_hex(&signed_transfer(0, 0).canonical_bytes());
    assert_rejected_without_propagation(&hex, "transaction validation failed");
}

fn mesh_nodes(nodes: &mut [&mut Peer]) {
    let peers: Vec<PeerId> = nodes
        .iter()
        .map(|node| {
            peer_of(
                &lines_of(node)
                    .into_iter()
                    .find(|line| line.starts_with("peer "))
                    .unwrap(),
            )
        })
        .collect();
    for (index, node) in nodes.iter_mut().enumerate() {
        for (other, peer) in peers.iter().enumerate() {
            if index != other {
                recognize(node, *peer);
            }
        }
    }
}

fn wait_secs(peer: &mut Peer, prefix: &str, secs: u64) -> String {
    let started = Instant::now();
    loop {
        if let Some(line) = lines_of(peer)
            .into_iter()
            .find(|line| line.starts_with(prefix))
        {
            return line;
        }
        if let Some(status) = peer.child.as_mut().unwrap().try_wait().unwrap() {
            panic!("el proceso salió {status} antes de {prefix}");
        }
        if started.elapsed() > Duration::from_secs(secs) {
            panic!("no apareció {prefix}: {:?}", lines_of(peer));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn block_at(nodes: &[&Peer], height: &str) -> String {
    let started = Instant::now();
    loop {
        for node in nodes {
            if let Some(line) = lines_of(node).into_iter().find(|line| {
                let mut parts = line.split_whitespace();
                parts.next() == Some("block") && parts.nth(2) == Some(height)
            }) {
                return line;
            }
        }
        if started.elapsed() > Duration::from_secs(40) {
            for node in nodes {
                eprintln!("---- node");
                for line in lines_of(node) {
                    if line.starts_with("proposer ")
                        || line.starts_with("block ")
                        || line.starts_with("committed ")
                        || line.starts_with("advance-")
                        || line.starts_with("publish-")
                        || line.starts_with("state ")
                        || line.starts_with("ready ")
                        || line.starts_with("admitted ")
                        || line.starts_with("transaction ")
                    {
                        eprintln!("{line}");
                    }
                }
            }
            panic!("no apareció block {height}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn owned_prefix(lines: &[String], prefix: &str) -> String {
    lines
        .iter()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("falta {prefix}"))
        .clone()
}

fn send_tx(addr: &str, tx: &Transaction) -> String {
    let id = encode_hex(tx.id().0.as_bytes());
    let hex = encode_hex(&tx.canonical_bytes());
    let sent = post_rpc(
        addr,
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{hex}"}}"#),
    );
    assert!(sent.contains(&id), "{sent}");
    id
}

#[test]
fn behind_node_syncs_two_committed_blocks() {
    let rpc: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_produce(data_dir("sync-0"), None, &rpc[0], "sync-0");
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_produce(data_dir("sync-1"), Some(&bootstrap), &rpc[1], "sync-1");
    let mut v2 = spawn_produce(data_dir("sync-2"), Some(&bootstrap), &rpc[2], "sync-2");
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_prefix(node, "peer ");
        wait_prefix(node, "tip ");
    }
    mesh_nodes(&mut [&mut v0, &mut v1, &mut v2]);
    let genesis = lines_of(&v0)
        .into_iter()
        .find(|line| line.starts_with("tip ") && line.ends_with(" 0"))
        .unwrap();
    let genesis_hash = genesis.split_whitespace().nth(1).unwrap().to_string();
    let first = send_tx(&rpc[0], &signed_transfer(0, 1));
    let published = block_at(&[&v0, &v1, &v2], "1");
    let mut parts = published.split_whitespace();
    assert_eq!(parts.next(), Some("block"));
    let hash1 = parts.next().unwrap().to_string();
    let prev1 = parts.next().unwrap().to_string();
    assert_eq!(parts.next(), Some("1"));
    assert_eq!(prev1, genesis_hash);
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_secs(node, &format!("committed {hash1} 1 {prev1} "), 40);
    }
    assert!(lines_of(&v0).iter().any(|line| line == &format!("account {V0_ID} 0 1")));
    let second = send_tx(&rpc[0], &signed_from(0x22, 0x33, 0, 1));
    let published2 = block_at(&[&v0, &v1, &v2], "2");
    let mut parts = published2.split_whitespace();
    assert_eq!(parts.next(), Some("block"));
    let hash2 = parts.next().unwrap().to_string();
    let prev2 = parts.next().unwrap().to_string();
    assert_eq!(parts.next(), Some("2"));
    assert_eq!(prev2, hash1);
    let mut committed2 = String::new();
    for node in [&mut v0, &mut v1, &mut v2] {
        committed2 = wait_secs(node, &format!("committed {hash2} 2 {prev2} "), 40);
        assert_eq!(count_prefix(node, "committed "), 2, "{:?}", lines_of(node));
    }
    let state_root = committed2.split_whitespace().nth(5).unwrap().to_string();
    let ledger_root = committed2.split_whitespace().nth(6).unwrap().to_string();
    for node in [&v0, &v1, &v2] {
        let tip = lines_of(node)
            .into_iter()
            .rev()
            .find(|line| line.starts_with("tip "))
            .unwrap();
        assert_eq!(tip, format!("tip {hash2} 2"));
        assert!(lines_of(node).iter().any(|line| {
            line.starts_with(&format!("committed {hash2} 2 {hash1} "))
                && line.split_whitespace().nth(5) == Some(state_root.as_str())
                && line.split_whitespace().nth(6) == Some(ledger_root.as_str())
        }));
    }
    for addr in &rpc[..3] {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":2"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash2}\"")), "{status}");
    }
    let v0_lines = lines_of(&v0);
    let bytes1 = owned_prefix(&v0_lines, "bytes 1 ");
    let bytes2 = owned_prefix(&v0_lines, "bytes 2 ");
    let mut v3 = spawn_produce(data_dir("sync-3"), Some(&bootstrap), &rpc[3], "behind");
    wait_prefix(&mut v3, "sync-protocol /rsc/block-sync/1.0.0");
    wait_prefix(&mut v3, "peer ");
    wait_prefix(&mut v3, "tip ");
    let early = lines_of(&v3)
        .into_iter()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert!(early.ends_with(" 0"), "{early}");
    assert!(early.contains(&genesis_hash), "{early}");
    mesh_nodes(&mut [&mut v0, &mut v1, &mut v2, &mut v3]);
    wait_secs(&mut v3, "sync-request 1", 40);
    wait_secs(&mut v3, &format!("synced 1 {hash1} {genesis_hash}"), 40);
    wait_secs(&mut v3, "sync-request 2", 40);
    wait_secs(&mut v3, &format!("synced 2 {hash2} {hash1}"), 40);
    wait_secs(&mut v3, "sync-request 3", 40);
    wait_secs(&mut v3, "sync-empty 3", 40);
    wait_secs(&mut v3, "sync-complete", 40);
    assert_eq!(owned_prefix(&lines_of(&v3), "bytes 1 "), bytes1);
    assert_eq!(owned_prefix(&lines_of(&v3), "bytes 2 "), bytes2);
    assert_eq!(count_prefix(&v3, "block "), 0, "{:?}", lines_of(&v3));
    assert_eq!(count_prefix(&v3, "finalized "), 0);
    assert_eq!(count_prefix(&v3, "quorum "), 0);
    assert_eq!(count_prefix(&v3, "admitted "), 0);
    assert_eq!(count_prefix(&v3, "committed "), 2);
    let tip = lines_of(&v3)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert_eq!(tip, format!("tip {hash2} 2"));
    let status = post_rpc(&rpc[3], "rsc_getStatus", "");
    assert!(status.contains("\"latest_height\":2"), "{status}");
    assert!(status.contains(&format!("\"latest_block_hash\":\"{hash2}\"")), "{status}");
    let missing = post_rpc(&rpc[3], "rsc_getBlockByHeight", r#"{"height":3}"#);
    assert!(missing.contains("not found"), "{missing}");
    let sender = post_rpc(&rpc[3], "rsc_getAccount", &format!(r#"{{"account":"{V0_ID}"}}"#));
    assert!(sender.contains("\"balance\":\"0\""), "{sender}");
    let mid = post_rpc(&rpc[3], "rsc_getAccount", &format!(r#"{{"account":"{V1_ID}"}}"#));
    assert!(mid.contains("\"balance\":\"1\""), "{mid}");
    assert!(mid.contains("\"nonce\":1"), "{mid}");
    let recipient = post_rpc(&rpc[3], "rsc_getAccount", &format!(r#"{{"account":"{V2_ID}"}}"#));
    assert!(recipient.contains("\"balance\":\"2\""), "{recipient}");
    eprintln!("genesis {genesis_hash}");
    eprintln!("tx1 {first}");
    eprintln!("block1 {published}");
    eprintln!("tx2 {second}");
    eprintln!("block2 {published2}");
    eprintln!("committed2 {committed2}");
    eprintln!("bytes1 {bytes1}");
    eprintln!("bytes2 {bytes2}");
    for (index, node) in [&v0, &v1, &v2, &v3].iter().enumerate() {
        let peer = lines_of(node)
            .into_iter()
            .find(|line| line.starts_with("peer "))
            .unwrap();
        let listen = lines_of(node)
            .into_iter()
            .find(|line| line.starts_with("listen "))
            .unwrap();
        eprintln!("V{index} {peer} {listen}");
    }
    let served = [&v0, &v1, &v2].iter().any(|node| {
        lines_of(node).iter().any(|line| line.starts_with(&format!("sync-serve 1 {hash1}")))
            && lines_of(node).iter().any(|line| line.starts_with(&format!("sync-serve 2 {hash2}")))
    });
    assert!(served, "ningún peer sirvió los dos bloques");
    stop_chain(&mut v3);
    stop_chain(&mut v2);
    stop_chain(&mut v1);
    stop_chain(&mut v0);
}

#[test]
fn block_sync_rejects_invalid_payloads() {
    let rpc: Vec<String> = (0..2).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut fault = spawn_produce(data_dir("fault"), None, &rpc[0], "fault");
    let bootstrap = listen_of(&mut fault);
    wait_prefix(&mut fault, "rpc ");
    let mut audit = spawn_produce(data_dir("audit"), Some(&bootstrap), &rpc[1], "audit");
    wait_prefix(&mut audit, "sync-protocol /rsc/block-sync/1.0.0");
    wait_prefix(&mut audit, "peer ");
    mesh_nodes(&mut [&mut fault, &mut audit]);
    for marker in [
        "sync-rejected invalid encoding",
        "sync-rejected invalid parent",
        "sync-rejected invalid height",
        "sync-rejected invalid block",
        "sync-rejected state root distinto",
        "sync-empty 9",
        "sync-failed",
    ] {
        wait_secs(&mut audit, marker, 20);
    }
    assert_eq!(count_prefix(&audit, "committed "), 0, "{:?}", lines_of(&audit));
    assert_eq!(count_prefix(&audit, "synced "), 0);
    let tip = lines_of(&audit)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert!(tip.ends_with(" 0"), "{tip}");
    let status = post_rpc(&rpc[1], "rsc_getStatus", "");
    assert!(status.contains("\"latest_height\":0"), "{status}");
    stop_chain(&mut audit);
    stop_chain(&mut fault);
}

#[test]
fn a_missing_block_fails_sync_without_writing() {
    let rpc: Vec<String> = (0..2).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut ahead = spawn_produce(data_dir("miss-0"), None, &rpc[0], "sync-0");
    let bootstrap = listen_of(&mut ahead);
    wait_prefix(&mut ahead, "rpc ");
    let mut behind = spawn_produce(data_dir("miss-3"), Some(&bootstrap), &rpc[1], "behind");
    wait_prefix(&mut behind, "peer ");
    mesh_nodes(&mut [&mut ahead, &mut behind]);
    wait_secs(&mut behind, "sync-request 1", 20);
    wait_secs(&mut behind, "sync-empty 1", 20);
    wait_secs(&mut behind, "sync-failed", 20);
    assert_eq!(count_prefix(&behind, "sync-complete"), 0);
    assert_eq!(count_prefix(&behind, "committed "), 0);
    assert_eq!(count_prefix(&behind, "synced "), 0);
    let tip = lines_of(&behind)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert!(tip.ends_with(" 0"), "{tip}");
    stop_chain(&mut behind);
    stop_chain(&mut ahead);
}

#[test]
fn block_sync_rejects_a_different_network_or_chain() {
    let rpc: Vec<String> = (0..3).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut honest = spawn_produce(data_dir("syncnet"), None, &rpc[0], "sync-0");
    let listen = listen_of(&mut honest);
    let mut other_net = launch(
        data_dir("syncnet-b"),
        "mainnet",
        "3",
        None,
        Some(&listen),
        Some(&rpc[1]),
        Some(&signer_hex()),
        Some("behind"),
    );
    wait_rejection(&mut honest, &mut other_net, "network mismatch");
    assert_eq!(count_prefix(&other_net, "sync-complete"), 0);
    assert_eq!(count_prefix(&other_net, "synced "), 0);
    stop_chain(&mut other_net);
    let mut other_chain = launch(
        data_dir("syncchain-b"),
        "devnet",
        "1",
        None,
        Some(&listen),
        Some(&rpc[2]),
        Some(&signer_hex()),
        Some("behind"),
    );
    wait_rejection(&mut honest, &mut other_chain, "chain mismatch");
    assert_eq!(count_prefix(&other_chain, "sync-complete"), 0);
    assert_eq!(count_prefix(&other_chain, "synced "), 0);
    stop_chain(&mut other_chain);
    stop_chain(&mut honest);
}

fn line_tail(line: &str, nth: usize) -> &str {
    line.split_whitespace().nth(nth).unwrap()
}

#[test]
fn restart_catches_up_blocks_two_and_three() {
    let rpc: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_produce(data_dir("catch-0"), None, &rpc[0], "catch-0");
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_produce(data_dir("catch-1"), Some(&bootstrap), &rpc[1], "catch-1");
    let mut v2 = spawn_produce(data_dir("catch-2"), Some(&bootstrap), &rpc[2], "catch-2");
    let mut v3 = spawn_produce(data_dir("catch-3"), Some(&bootstrap), &rpc[3], "catch-3");
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_prefix(node, "peer ");
        wait_prefix(node, "installed-genesis ");
        wait_prefix(node, "tip ");
    }
    mesh_nodes(&mut [&mut v0, &mut v1, &mut v2, &mut v3]);
    let genesis = line_tail(&wait_prefix(&mut v0, "installed-genesis "), 1).to_string();
    let first = send_tx(&rpc[0], &signed_transfer(0, 1));
    let published = block_at(&[&v0, &v1, &v2, &v3], "1");
    let hash1 = line_tail(&published, 1).to_string();
    let prev1 = line_tail(&published, 2).to_string();
    assert_eq!(prev1, genesis);
    for node in [&mut v0, &mut v1, &mut v2, &mut v3] {
        wait_secs(node, &format!("committed {hash1} 1 {prev1} "), 40);
    }
    let bytes1 = owned_prefix(&lines_of(&v3), "bytes 1 ");
    let validator = owned_prefix(&lines_of(&v3), &format!("validator 3 {V3_ID} "));
    stop_keep(&mut v3);
    let kept = v3.data_dir.clone();
    let second = send_tx(&rpc[0], &signed_from(0x22, 0x33, 0, 1));
    let published2 = block_at(&[&v0, &v1, &v2], "2");
    let hash2 = line_tail(&published2, 1).to_string();
    let prev2 = line_tail(&published2, 2).to_string();
    assert_eq!(prev2, hash1);
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_secs(node, &format!("committed {hash2} 2 {prev2} "), 40);
    }
    let third = send_tx(&rpc[0], &signed_from(0x33, 0x11, 0, 1));
    let published3 = block_at(&[&v0, &v1, &v2], "3");
    let hash3 = line_tail(&published3, 1).to_string();
    let prev3 = line_tail(&published3, 2).to_string();
    assert_eq!(prev3, hash2);
    let mut committed3 = String::new();
    for node in [&mut v0, &mut v1, &mut v2] {
        committed3 = wait_secs(node, &format!("committed {hash3} 3 {prev3} "), 40);
        assert_eq!(count_prefix(node, "committed "), 3, "{:?}", lines_of(node));
    }
    let state_root = line_tail(&committed3, 5).to_string();
    let ledger_root = line_tail(&committed3, 6).to_string();
    expect_db_reject(&kept, "mainnet", "3", "network mismatch");
    expect_db_reject(&kept, "devnet", "1", "chain mismatch");
    let rpc3 = format!("127.0.0.1:{}", free_port());
    let mut resumed = spawn_produce(kept.clone(), Some(&bootstrap), &rpc3, "behind");
    wait_secs(&mut resumed, &format!("recovered 1 {hash1}"), 20);
    assert_eq!(count_prefix(&resumed, "installed-genesis "), 0);
    let again = wait_secs(&mut resumed, &format!("validator 3 {V3_ID} "), 20);
    assert_eq!(again, validator);
    let stored = owned_prefix(&lines_of(&resumed), "stored 1 ");
    assert_eq!(line_tail(&stored, 2), line_tail(&bytes1, 2));
    let early = lines_of(&resumed)
        .into_iter()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert_eq!(early, format!("tip {hash1} 1"));
    wait_prefix(&mut resumed, "peer ");
    mesh_nodes(&mut [&mut v0, &mut v1, &mut v2, &mut resumed]);
    wait_secs(&mut resumed, "sync-request 2", 40);
    wait_secs(&mut resumed, &format!("synced 2 {hash2} {hash1}"), 40);
    wait_secs(&mut resumed, "sync-request 3", 40);
    wait_secs(&mut resumed, &format!("synced 3 {hash3} {hash2}"), 40);
    wait_secs(&mut resumed, "sync-request 4", 40);
    wait_secs(&mut resumed, "sync-empty 4", 40);
    wait_secs(&mut resumed, "sync-complete", 40);
    assert_eq!(count_prefix(&resumed, "sync-request 1"), 0);
    assert_eq!(count_prefix(&resumed, "block "), 0);
    assert_eq!(count_prefix(&resumed, "finalized "), 0);
    assert_eq!(count_prefix(&resumed, "quorum "), 0);
    assert_eq!(count_prefix(&resumed, "admitted "), 0);
    assert_eq!(count_prefix(&resumed, "bytes 1 "), 0);
    assert_eq!(count_prefix(&resumed, "committed "), 2);
    assert!(lines_of(&resumed).iter().any(|line| {
        line.starts_with(&format!("committed {hash3} 3 {hash2} "))
            && line_tail(line, 5) == state_root
            && line_tail(line, 6) == ledger_root
    }));
    assert_eq!(
        owned_prefix(&lines_of(&resumed), "bytes 2 "),
        owned_prefix(&lines_of(&v0), "bytes 2 ")
    );
    assert_eq!(
        owned_prefix(&lines_of(&resumed), "bytes 3 "),
        owned_prefix(&lines_of(&v0), "bytes 3 ")
    );
    let tip = lines_of(&resumed)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert_eq!(tip, format!("tip {hash3} 3"));
    std::thread::sleep(Duration::from_secs(1));
    for node in [&v0, &v1, &v2] {
        assert_eq!(count_prefix(node, "committed "), 3);
    }
    for addr in [&rpc[0], &rpc[1], &rpc[2], &rpc3] {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":3"), "{status}");
        assert!(
            status.contains(&format!("\"latest_block_hash\":\"{hash3}\"")),
            "{status}"
        );
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":4}"#);
        assert!(missing.contains("not found"), "{missing}");
    }
    let sender = post_rpc(&rpc3, "rsc_getAccount", &format!(r#"{{"account":"{V0_ID}"}}"#));
    assert!(sender.contains("\"balance\":\"1\""), "{sender}");
    assert!(sender.contains("\"nonce\":1"), "{sender}");
    let mid = post_rpc(&rpc3, "rsc_getAccount", &format!(r#"{{"account":"{V1_ID}"}}"#));
    assert!(mid.contains("\"balance\":\"1\""), "{mid}");
    assert!(mid.contains("\"nonce\":1"), "{mid}");
    let recipient = post_rpc(&rpc3, "rsc_getAccount", &format!(r#"{{"account":"{V2_ID}"}}"#));
    assert!(recipient.contains("\"balance\":\"1\""), "{recipient}");
    assert!(recipient.contains("\"nonce\":1"), "{recipient}");
    eprintln!("genesis {genesis}");
    eprintln!("tx1 {first}");
    eprintln!("block1 {published}");
    eprintln!("tx2 {second}");
    eprintln!("block2 {published2}");
    eprintln!("tx3 {third}");
    eprintln!("block3 {published3}");
    eprintln!("committed3 {committed3}");
    eprintln!("validator {validator}");
    for (index, node) in [&v0, &v1, &v2, &resumed].iter().enumerate() {
        let peer = lines_of(node)
            .into_iter()
            .find(|line| line.starts_with("peer "))
            .unwrap();
        let listen = lines_of(node)
            .into_iter()
            .find(|line| line.starts_with("listen "))
            .unwrap();
        eprintln!("V{index} {peer} {listen}");
    }
    stop_chain(&mut resumed);
    stop_chain(&mut v2);
    stop_chain(&mut v1);
    stop_chain(&mut v0);
}

#[test]
fn restart_rejects_bad_payloads_and_a_short_peer() {
    let rpc: Vec<String> = (0..3).map(|_| format!("127.0.0.1:{}", free_port())).collect();
    let mut v0 = spawn_produce(data_dir("gap-0"), None, &rpc[0], "produce-0");
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    let mut v1 = spawn_produce(data_dir("gap-1"), Some(&bootstrap), &rpc[1], "produce-1");
    let mut v2 = spawn_produce(data_dir("gap-2"), Some(&bootstrap), &rpc[2], "produce-2");
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_prefix(node, "peer ");
    }
    mesh_nodes(&mut [&mut v0, &mut v1, &mut v2]);
    let first = send_tx(&rpc[0], &signed_transfer(0, 1));
    let published = block_at(&[&v0, &v1, &v2], "1");
    let hash1 = line_tail(&published, 1).to_string();
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_secs(node, &format!("committed {hash1} 1 "), 40);
    }
    let kept1 = v1.data_dir.clone();
    stop_keep(&mut v1);
    let rpc_short = format!("127.0.0.1:{}", free_port());
    let mut behind = spawn_produce(kept1.clone(), Some(&bootstrap), &rpc_short, "behind");
    wait_secs(&mut behind, &format!("recovered 1 {hash1}"), 20);
    wait_prefix(&mut behind, "peer ");
    mesh_nodes(&mut [&mut v0, &mut behind]);
    wait_secs(&mut behind, "sync-request 2", 20);
    wait_secs(&mut behind, "sync-empty 2", 20);
    wait_secs(&mut behind, "sync-failed", 20);
    assert_eq!(count_prefix(&behind, "sync-complete"), 0);
    assert_eq!(count_prefix(&behind, "synced "), 0);
    assert_eq!(count_prefix(&behind, "committed "), 0);
    let tip = lines_of(&behind)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("tip "))
        .unwrap();
    assert_eq!(tip, format!("tip {hash1} 1"));
    stop_keep(&mut behind);
    stop_chain(&mut v0);
    let kept2 = v2.data_dir.clone();
    stop_keep(&mut v2);
    let rpc_gap = format!("127.0.0.1:{}", free_port());
    let mut gap = spawn_produce(kept2, None, &rpc_gap, "gap");
    let gap_listen = listen_of(&mut gap);
    wait_secs(&mut gap, &format!("recovered 1 {hash1}"), 20);
    let rpc_probe = format!("127.0.0.1:{}", free_port());
    let mut probe = spawn_produce(kept1, Some(&gap_listen), &rpc_probe, "probe");
    wait_secs(&mut probe, &format!("recovered 1 {hash1}"), 20);
    wait_prefix(&mut probe, "peer ");
    mesh_nodes(&mut [&mut gap, &mut probe]);
    for marker in [
        "sync-rejected invalid block",
        "sync-rejected invalid parent",
        "sync-rejected invalid height",
        "sync-failed",
    ] {
        wait_secs(&mut probe, marker, 20);
    }
    assert_eq!(count_prefix(&probe, "committed "), 0, "{:?}", lines_of(&probe));
    assert_eq!(count_prefix(&probe, "synced "), 0);
    let status = post_rpc(&rpc_probe, "rsc_getStatus", "");
    assert!(status.contains("\"latest_height\":1"), "{status}");
    assert!(status.contains(&format!("\"latest_block_hash\":\"{hash1}\"")), "{status}");
    let missing = post_rpc(&rpc_probe, "rsc_getBlockByHeight", r#"{"height":2}"#);
    assert!(missing.contains("not found"), "{missing}");
    eprintln!("short-peer block1 {hash1} tx {first}");
    stop_chain(&mut probe);
    stop_chain(&mut gap);
}

fn committed_line(nodes: &[&Peer], height: &str) -> String {
    let mut found = Vec::new();
    for node in nodes {
        let line = lines_of(node).into_iter().find(|line| {
            let mut parts = line.split_whitespace();
            parts.next() == Some("committed") && parts.nth(1) == Some(height)
        });
        found.push(line.unwrap_or_else(|| panic!("falta committed {height}")));
    }
    for line in &found[1..] {
        assert_eq!(line, &found[0], "{found:?}");
    }
    found.remove(0)
}

fn start_devnet(dirs: &[PathBuf; 4], rpc: &[String; 4]) -> [Peer; 4] {
    start_role("devnet", dirs, rpc)
}

fn start_role(prefix: &str, dirs: &[PathBuf; 4], rpc: &[String; 4]) -> [Peer; 4] {
    let mut v0 = spawn_produce(dirs[0].clone(), None, &rpc[0], &format!("{prefix}-0"));
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    wait_health(&rpc[0]);
    let v1 = spawn_produce(dirs[1].clone(), Some(&bootstrap), &rpc[1], &format!("{prefix}-1"));
    let v2 = spawn_produce(dirs[2].clone(), Some(&bootstrap), &rpc[2], &format!("{prefix}-2"));
    let v3 = spawn_produce(dirs[3].clone(), Some(&bootstrap), &rpc[3], &format!("{prefix}-3"));
    let mut nodes = [v0, v1, v2, v3];
    for (node, addr) in nodes.iter_mut().zip(rpc) {
        wait_prefix(node, "peer ");
        wait_prefix(node, "validator ");
        wait_prefix(node, "rpc ");
        wait_health(addr);
    }
    mesh_nodes(&mut nodes.iter_mut().collect::<Vec<_>>());
    nodes
}

fn expect_process_reject(args: &[&str], needles: &[&str]) {
    let mut child = Command::new(bin())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success(), "{status}");
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            assert!(
                needles.iter().any(|needle| err.contains(needle)),
                "{err}"
            );
            return;
        }
        if started.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("el proceso no rechazó {needles:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn devnet_produces_blocks_restarts_and_continues() {
    let dirs = [
        data_dir("devnet-0"),
        data_dir("devnet-1"),
        data_dir("devnet-2"),
        data_dir("devnet-3"),
    ];
    let rpc = [
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    ];
    let mut nodes = start_devnet(&dirs, &rpc);
    let genesis = line_tail(&wait_prefix(&mut nodes[0], "installed-genesis "), 1).to_string();
    for node in &nodes {
        assert!(lines_of(node).iter().any(|line| line == &format!("installed-genesis {genesis}")));
        assert_eq!(count_prefix(node, "block "), 0);
        assert_eq!(count_prefix(node, "committed "), 0);
    }
    for addr in &rpc {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":0"), "{status}");
        assert!(status.contains("\"finalized_height\":0"), "{status}");
        let fin = post_rpc(addr, "rsc_getFinality", "");
        assert!(fin.contains("service unavailable"), "{fin}");
    }
    let zero = encode_hex(&signed_transfer(0, 0).canonical_bytes());
    let rejected = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{zero}"}}"#),
    );
    assert!(rejected.contains("transaction rejected"), "{rejected}");
    for node in &nodes {
        assert_eq!(count_prefix(node, "block "), 0);
    }
    let ids = [
        V0_ID, V1_ID, V2_ID, V3_ID,
    ];
    let identities: Vec<String> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| owned_prefix(&lines_of(node), &format!("validator {index} {} ", ids[index])))
        .collect();
    let peers: Vec<String> = nodes
        .iter()
        .map(|node| owned_prefix(&lines_of(node), "peer "))
        .collect();
    let listens: Vec<String> = nodes
        .iter()
        .map(|node| owned_prefix(&lines_of(node), "listen "))
        .collect();
    let tx1 = send_tx(&rpc[0], &signed_from(0x11, 0x22, 0, 1));
    wait_prefix(&mut nodes[0], &format!("ready {tx1}"));
    let published1 = block_at(&nodes.iter().collect::<Vec<_>>(), "1");
    let hash1 = line_tail(&published1, 1).to_string();
    assert_eq!(line_tail(&published1, 2), genesis);
    for node in &mut nodes {
        wait_secs(node, &format!("committed {hash1} 1 "), 40);
    }
    let block1 = committed_line(&nodes.iter().collect::<Vec<_>>(), "1");
    assert_eq!(line_tail(&block1, 3), genesis);
    let tx2 = send_tx(&rpc[0], &signed_from(0x22, 0x33, 0, 1));
    wait_prefix(&mut nodes[0], &format!("ready {tx2}"));
    let published2 = block_at(&nodes.iter().collect::<Vec<_>>(), "2");
    let hash2 = line_tail(&published2, 1).to_string();
    assert_eq!(line_tail(&published2, 2), hash1);
    for node in &mut nodes {
        wait_secs(node, &format!("committed {hash2} 2 "), 40);
    }
    let block2 = committed_line(&nodes.iter().collect::<Vec<_>>(), "2");
    assert_eq!(line_tail(&block2, 3), hash1);
    let tx3 = send_tx(&rpc[0], &signed_from(0x33, 0x11, 0, 1));
    wait_prefix(&mut nodes[0], &format!("ready {tx3}"));
    let published3 = block_at(&nodes.iter().collect::<Vec<_>>(), "3");
    let hash3 = line_tail(&published3, 1).to_string();
    assert_eq!(line_tail(&published3, 2), hash2);
    for node in &mut nodes {
        wait_secs(node, &format!("committed {hash3} 3 "), 40);
    }
    let block3 = committed_line(&nodes.iter().collect::<Vec<_>>(), "3");
    assert_eq!(line_tail(&block3, 1), hash3);
    assert_eq!(line_tail(&block3, 3), hash2);
    assert_eq!(line_tail(&block3, 2), "3");
    let state3 = line_tail(&block3, 5).to_string();
    let ledger3 = line_tail(&block3, 6).to_string();
    let bytes3 = owned_prefix(&lines_of(&nodes[0]), "bytes 3 ");
    for addr in &rpc {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":3"), "{status}");
        assert!(status.contains("\"finalized_height\":3"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash3}\"")), "{status}");
        let fin = post_rpc(addr, "rsc_getFinality", "");
        assert!(fin.contains("service unavailable"), "{fin}");
        for (height, hash, parent) in [
            (1u64, hash1.as_str(), genesis.as_str()),
            (2, hash2.as_str(), hash1.as_str()),
            (3, hash3.as_str(), hash2.as_str()),
        ] {
            let body = post_rpc(addr, "rsc_getBlockByHeight", &format!(r#"{{"height":{height}}}"#));
            assert!(body.contains(&format!("\"hash\":\"{hash}\"")), "{body}");
            assert!(body.contains(&format!("\"parent_hash\":\"{parent}\"")), "{body}");
        }
        for (id, height) in [(&tx1, 1u64), (&tx2, 2), (&tx3, 3)] {
            let body = post_rpc(addr, "rsc_getTransaction", &format!(r#"{{"transaction_hash":"{id}"}}"#));
            assert!(body.contains("\"status\":\"included\""), "{body}");
            assert!(body.contains(&format!("\"block_height\":{height}")), "{body}");
            assert!(body.contains(&format!("\"hash\":\"{id}\"")), "{body}");
        }
        for account in [V0_ID, V1_ID, V2_ID, V3_ID] {
            let body = post_rpc(addr, "rsc_getBalance", &format!(r#"{{"account":"{account}"}}"#));
            assert!(body.contains("\"balance\":\"1\""), "{body}");
        }
    }
    let next = post_rpc(&rpc[0], "rsc_getBlockByHeight", r#"{"height":4}"#);
    assert!(next.contains("not found"), "{next}");
    for node in &mut nodes {
        stop_keep(node);
    }
    let rpc2 = [
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    ];
    let mut again = start_devnet(&dirs, &rpc2);
    for (index, node) in again.iter_mut().enumerate() {
        let recovered = wait_secs(node, &format!("recovered 3 {hash3}"), 20);
        assert_eq!(line_tail(&recovered, 1), "3");
        assert!(lines_of(node).iter().all(|line| !line.starts_with("installed-genesis ")));
        let stored = wait_secs(node, "stored 3 ", 20);
        assert_eq!(line_tail(&stored, 2), line_tail(&bytes3, 2));
        let roots = wait_secs(node, "roots ", 20);
        assert_eq!(line_tail(&roots, 1), state3);
        assert_eq!(line_tail(&roots, 2), ledger3);
        let identity = wait_secs(node, &format!("validator {index} "), 20);
        assert_eq!(identity, identities[index]);
        assert_eq!(count_prefix(node, "committed "), 0);
        assert_eq!(count_prefix(node, "bytes 1 "), 0);
        assert_eq!(count_prefix(node, "bytes 2 "), 0);
        assert_eq!(count_prefix(node, "bytes 3 "), 0);
        assert_eq!(count_prefix(node, "vote "), 0);
        assert_eq!(count_prefix(node, "finalized "), 0);
        assert_eq!(count_prefix(node, "proof "), 0);
        assert_eq!(count_prefix(node, "block "), 0);
    }
    for addr in &rpc2 {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":3"), "{status}");
        assert!(status.contains("\"finalized_height\":3"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash3}\"")), "{status}");
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":4}"#);
        assert!(missing.contains("not found"), "{missing}");
    }
    let tx4 = send_tx(&rpc2[0], &signed_from(0x44, 0x11, 0, 1));
    wait_prefix(&mut again[0], &format!("ready {tx4}"));
    let published4 = block_at(&again.iter().collect::<Vec<_>>(), "4");
    let hash4 = line_tail(&published4, 1).to_string();
    assert_eq!(line_tail(&published4, 2), hash3);
    for node in &mut again {
        wait_secs(node, &format!("committed {hash4} 4 "), 40);
    }
    let block4 = committed_line(&again.iter().collect::<Vec<_>>(), "4");
    assert_eq!(line_tail(&block4, 1), hash4);
    assert_eq!(line_tail(&block4, 2), "4");
    assert_eq!(line_tail(&block4, 3), hash3);
    for node in &again {
        assert_eq!(count_prefix(node, "committed "), 1, "{:?}", lines_of(node));
        assert_eq!(count_prefix(node, "installed-genesis "), 0);
    }
    for addr in &rpc2 {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":4"), "{status}");
        assert!(status.contains("\"finalized_height\":4"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash4}\"")), "{status}");
        let body = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":4}"#);
        assert!(body.contains(&format!("\"hash\":\"{hash4}\"")), "{body}");
        assert!(body.contains(&format!("\"parent_hash\":\"{hash3}\"")), "{body}");
        let earlier = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":3}"#);
        assert!(earlier.contains(&format!("\"hash\":\"{hash3}\"")), "{earlier}");
        let tx = post_rpc(addr, "rsc_getTransaction", &format!(r#"{{"transaction_hash":"{tx4}"}}"#));
        assert!(tx.contains("\"status\":\"included\""), "{tx}");
        assert!(tx.contains("\"block_height\":4"), "{tx}");
        let sender = post_rpc(addr, "rsc_getBalance", &format!(r#"{{"account":"{V3_ID}"}}"#));
        assert!(sender.contains("\"balance\":\"0\""), "{sender}");
        let recipient = post_rpc(addr, "rsc_getBalance", &format!(r#"{{"account":"{V0_ID}"}}"#));
        assert!(recipient.contains("\"balance\":\"2\""), "{recipient}");
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":5}"#);
        assert!(missing.contains("not found"), "{missing}");
        let fin = post_rpc(addr, "rsc_getFinality", "");
        assert!(fin.contains("service unavailable"), "{fin}");
    }
    let resumed_peers: Vec<String> = again
        .iter()
        .map(|node| owned_prefix(&lines_of(node), "peer "))
        .collect();
    let resumed_listens: Vec<String> = again
        .iter()
        .map(|node| owned_prefix(&lines_of(node), "listen "))
        .collect();
    eprintln!("devnet genesis {genesis}");
    eprintln!("devnet tx {tx1} {tx2} {tx3} {tx4}");
    eprintln!("devnet {block1}");
    eprintln!("devnet {block2}");
    eprintln!("devnet {block3}");
    eprintln!("devnet {block4}");
    for (index, line) in peers.iter().enumerate() {
        eprintln!("devnet before {index} {line} {}", listens[index]);
    }
    for (index, line) in resumed_peers.iter().enumerate() {
        eprintln!("devnet after {index} {line} {}", resumed_listens[index]);
    }
    for addr in &rpc {
        eprintln!("devnet rpc-before {addr}");
    }
    for addr in &rpc2 {
        eprintln!("devnet rpc-after {addr}");
    }
    for dir in &dirs {
        eprintln!("devnet dir {}", dir.display());
    }
    for node in &mut again {
        stop_chain(node);
    }
}

#[test]
fn devnet_rejects_mismatched_identity_and_busy_ports() {
    let dir = data_dir("devnet-reject");
    let rpc = format!("127.0.0.1:{}", free_port());
    let mut node = spawn_produce(dir.clone(), None, &rpc, "devnet-0");
    wait_prefix(&mut node, "installed-genesis ");
    wait_health(&rpc);
    stop_keep(&mut node);
    expect_db_reject(&dir, "mainnet", "3", "network mismatch");
    expect_db_reject(&dir, "devnet", "1", "chain mismatch");
    let foreign = data_dir("devnet-foreign");
    expect_process_reject(
        &[
            "--data-dir",
            foreign.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--consensus",
            "foreign",
        ],
        &["invalid signature"],
    );
    let (hold_rpc, rpc_port) = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    };
    let busy_rpc = data_dir("devnet-busy-rpc");
    let p2p_rpc = format!("127.0.0.1:{rpc_port}");
    expect_process_reject(
        &[
            "--data-dir",
            busy_rpc.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--rpc",
            &p2p_rpc,
            "--consensus",
            "devnet-0",
        ],
        &["Address already in use", "os error 10048"],
    );
    drop(hold_rpc);
    let (hold_p2p, p2p_port) = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    };
    let busy_p2p = data_dir("devnet-busy-p2p");
    let listen = format!("/ip4/127.0.0.1/tcp/{p2p_port}");
    let free_rpc = format!("127.0.0.1:{}", free_port());
    expect_process_reject(
        &[
            "--data-dir",
            busy_p2p.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--listen",
            &listen,
            "--rpc",
            &free_rpc,
            "--consensus",
            "devnet-0",
        ],
        &["listen failed"],
    );
    drop(hold_p2p);
    std::fs::remove_dir_all(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&foreign);
    let _ = std::fs::remove_dir_all(&busy_rpc);
    let _ = std::fs::remove_dir_all(&busy_p2p);
}

fn line_after(lines: &[String], marker: &str, prefix: &str) -> String {
    let start = lines
        .iter()
        .position(|line| line.starts_with(marker))
        .unwrap_or_else(|| panic!("falta {marker}"));
    lines[start..]
        .iter()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("falta {prefix} después de {marker}"))
        .clone()
}

fn latest_proposer(node: &Peer) -> String {
    lines_of(node)
        .into_iter()
        .rev()
        .find(|line| line.starts_with("proposer "))
        .unwrap()
}

#[test]
fn noncritical_validator_outage_keeps_quorum_and_catches_up() {
    let dirs = [
        data_dir("outage-0"),
        data_dir("outage-1"),
        data_dir("outage-2"),
        data_dir("outage-3"),
    ];
    let rpc = [
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    ];
    let mut nodes = start_devnet(&dirs, &rpc);
    let genesis = line_tail(&wait_prefix(&mut nodes[0], "installed-genesis "), 1).to_string();
    let tx1 = send_tx(&rpc[0], &signed_from(0x11, 0x22, 0, 1));
    wait_prefix(&mut nodes[0], &format!("ready {tx1}"));
    let published1 = block_at(&nodes.iter().collect::<Vec<_>>(), "1");
    let hash1 = line_tail(&published1, 1).to_string();
    assert_eq!(line_tail(&published1, 2), genesis);
    for node in &mut nodes {
        wait_secs(node, &format!("committed {hash1} 1 "), 40);
        wait_secs(node, &format!("finalized {hash1}"), 40);
    }
    let block1 = committed_line(&nodes.iter().collect::<Vec<_>>(), "1");
    let next = line_tail(&latest_proposer(&nodes[0]), 1).to_string();
    assert_eq!(next, V2_ID, "la altura 2 sigue proponiendo el validador en línea");
    assert_ne!(next, V3_ID);
    let identity = owned_prefix(&lines_of(&nodes[3]), &format!("validator 3 {V3_ID} "));
    let bytes1 = owned_prefix(&lines_of(&nodes[3]), "bytes 1 ");
    let dir3 = nodes[3].data_dir.clone();
    stop_keep(&mut nodes[3]);
    expect_db_reject(&dir3, "mainnet", "3", "network mismatch");
    expect_db_reject(&dir3, "devnet", "9", "chain mismatch");
    let prepare_before = count_prefix(&nodes[0], "quorum Prepare ");
    let tx2 = send_tx(&rpc[0], &signed_from(0x22, 0x33, 0, 1));
    wait_prefix(&mut nodes[0], &format!("ready {tx2}"));
    let live = [&nodes[0], &nodes[1], &nodes[2]];
    let published2 = block_at(&live, "2");
    let hash2 = line_tail(&published2, 1).to_string();
    assert_eq!(line_tail(&published2, 2), hash1);
    for node in &mut nodes[..3] {
        wait_secs(node, &format!("committed {hash2} 2 "), 40);
        wait_secs(node, &format!("finalized {hash2}"), 40);
        let proof = owned_prefix(&lines_of(node), &format!("proof {hash2} "));
        assert_eq!(proof, format!("proof {hash2} 0 2 0 5 5 5"));
        for phase in ["Prepare", "PreCommit", "Commit"] {
            let voters = line_after(
                &lines_of(node),
                &format!("proof {hash2} "),
                &format!("proof-voters {phase} "),
            );
            assert!(voters.ends_with(" 5"), "{voters}");
            assert!(voters.contains(V0_ID) && voters.contains(V1_ID) && voters.contains(V2_ID), "{voters}");
            assert!(!voters.contains(V3_ID), "{voters}");
        }
        assert_eq!(count_prefix(node, &format!("committed {hash2} 2 ")), 1);
        assert_eq!(count_prefix(node, "installed-genesis "), 1);
    }
    assert!(count_prefix(&nodes[0], "quorum Prepare ") > prepare_before);
    let block2 = committed_line(&[&nodes[0], &nodes[1], &nodes[2]], "2");
    let state2 = line_tail(&block2, 5).to_string();
    let ledger2 = line_tail(&block2, 6).to_string();
    let bytes2 = owned_prefix(&lines_of(&nodes[0]), "bytes 2 ");
    for addr in &rpc[..3] {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":2"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash2}\"")), "{status}");
        assert!(status.contains("\"finalized_height\":2"), "{status}");
    }
    let bootstrap = owned_prefix(&lines_of(&nodes[0]), "listen ");
    let bootstrap = bootstrap["listen ".len()..].to_string();
    let rpc3 = format!("127.0.0.1:{}", free_port());
    let mut resumed = spawn_produce(dir3, Some(&bootstrap), &rpc3, "behind");
    let recovered = wait_secs(&mut resumed, &format!("recovered 1 {hash1}"), 20);
    assert_eq!(line_tail(&recovered, 2), hash1);
    assert!(lines_of(&resumed).iter().all(|line| !line.starts_with("installed-genesis ")));
    let stored = wait_secs(&mut resumed, "stored 1 ", 20);
    assert_eq!(line_tail(&stored, 2), line_tail(&bytes1, 2));
    let again = wait_secs(&mut resumed, &format!("validator 3 {V3_ID} "), 20);
    assert_eq!(again, identity);
    wait_prefix(&mut resumed, "peer ");
    wait_health(&rpc3);
    {
        let mut mesh = nodes[..3].iter_mut().collect::<Vec<_>>();
        mesh.push(&mut resumed);
        mesh_nodes(&mut mesh);
    }
    wait_secs(&mut resumed, "sync-request 2", 20);
    wait_secs(&mut resumed, &format!("synced 2 {hash2} {hash1}"), 40);
    wait_secs(&mut resumed, "sync-request 3", 20);
    wait_secs(&mut resumed, "sync-empty 3", 20);
    wait_secs(&mut resumed, "sync-complete", 20);
    assert_eq!(count_prefix(&resumed, "sync-request 1"), 0);
    assert_eq!(count_prefix(&resumed, "bytes 1 "), 0);
    assert_eq!(count_prefix(&resumed, "vote "), 0);
    assert_eq!(count_prefix(&resumed, "finalized "), 0);
    assert_eq!(count_prefix(&resumed, "quorum "), 0);
    let returned = owned_prefix(&lines_of(&resumed), &format!("committed {hash2} 2 "));
    assert_eq!(returned, block2);
    assert_eq!(line_tail(&owned_prefix(&lines_of(&resumed), "bytes 2 "), 2), line_tail(&bytes2, 2));
    let status = post_rpc(&rpc3, "rsc_getStatus", "");
    assert!(status.contains("\"latest_height\":2"), "{status}");
    assert!(status.contains(&format!("\"latest_block_hash\":\"{hash2}\"")), "{status}");
    for addr in [&rpc[0], &rpc[1], &rpc[2], &rpc3] {
        for (height, hash, parent) in [(1u64, hash1.as_str(), genesis.as_str()), (2, hash2.as_str(), hash1.as_str())] {
            let body = post_rpc(addr, "rsc_getBlockByHeight", &format!(r#"{{"height":{height}}}"#));
            assert!(body.contains(&format!("\"hash\":\"{hash}\"")), "{body}");
            assert!(body.contains(&format!("\"parent_hash\":\"{parent}\"")), "{body}");
        }
        for (id, height) in [(&tx1, 1u64), (&tx2, 2)] {
            let body = post_rpc(addr, "rsc_getTransaction", &format!(r#"{{"transaction_hash":"{id}"}}"#));
            assert!(body.contains("\"status\":\"included\""), "{body}");
            assert!(body.contains(&format!("\"block_height\":{height}")), "{body}");
        }
    }
    let missing = post_rpc(&rpc3, "rsc_getBlockByHeight", r#"{"height":3}"#);
    assert!(missing.contains("not found"), "{missing}");
    eprintln!("outage genesis {genesis}");
    eprintln!("outage proposer-next {next}");
    eprintln!("outage {block1}");
    eprintln!("outage {block2}");
    eprintln!("outage state {state2} ledger {ledger2}");
    eprintln!("outage tx {tx1} {tx2}");
    eprintln!("outage offline validator 3 {V3_ID}");
    for (index, node) in nodes.iter().take(3).enumerate() {
        eprintln!(
            "outage live {index} {} {}",
            owned_prefix(&lines_of(node), "peer "),
            owned_prefix(&lines_of(node), "listen ")
        );
    }
    eprintln!(
        "outage resumed {} {}",
        owned_prefix(&lines_of(&resumed), "peer "),
        owned_prefix(&lines_of(&resumed), "listen ")
    );
    stop_chain(&mut resumed);
    stop_chain(&mut nodes[2]);
    stop_chain(&mut nodes[1]);
    stop_chain(&mut nodes[0]);
}

#[test]
fn proposer_offline_does_not_change_view() {
    let dirs = [
        data_dir("leader-0"),
        data_dir("leader-1"),
        data_dir("leader-2"),
        data_dir("leader-3"),
    ];
    let rpc = [
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    ];
    let mut nodes = start_devnet(&dirs, &rpc);
    let tx1 = send_tx(&rpc[0], &signed_from(0x11, 0x22, 0, 1));
    wait_prefix(&mut nodes[0], &format!("ready {tx1}"));
    let hash1 = line_tail(&block_at(&nodes.iter().collect::<Vec<_>>(), "1"), 1).to_string();
    for node in &mut nodes {
        wait_secs(node, &format!("committed {hash1} 1 "), 40);
    }
    let started_proposer = Instant::now();
    let next = loop {
        let id = line_tail(&latest_proposer(&nodes[0]), 1).to_string();
        if id == V2_ID {
            break id;
        }
        if started_proposer.elapsed() > Duration::from_secs(5) {
            panic!("el proposer de la altura 2 sigue en {id}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    stop_keep(&mut nodes[2]);
    let _tx2 = send_tx(&rpc[0], &signed_from(0x22, 0x33, 0, 1));
    wait_prefix(&mut nodes[0], "ready ");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(25) {
        for node in [&nodes[0], &nodes[1], &nodes[3]] {
            let proposed = lines_of(node).iter().any(|line| {
                let mut parts = line.split_whitespace();
                parts.next() == Some("block") && parts.nth(2) == Some("2")
            });
            assert!(!proposed, "el proposer apagado no debe ser reemplazado");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for addr in [&rpc[0], &rpc[1], &rpc[3]] {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":1"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{hash1}\"")), "{status}");
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":2}"#);
        assert!(missing.contains("not found"), "{missing}");
    }
    eprintln!("leader-offline proposer {next} tip {hash1} waited 25s");
    stop_chain(&mut nodes[3]);
    stop_chain(&mut nodes[1]);
    stop_chain(&mut nodes[0]);
    std::fs::remove_dir_all(&dirs[2]).unwrap();
}

#[test]
fn catch_up_rejects_corrupt_parent_and_height() {
    let dirs = [
        data_dir("fault-0"),
        data_dir("fault-1"),
        data_dir("fault-2"),
    ];
    let rpc = [
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    ];
    let mut v0 = spawn_produce(dirs[0].clone(), None, &rpc[0], "devnet-0");
    let bootstrap = listen_of(&mut v0);
    wait_prefix(&mut v0, "rpc ");
    wait_health(&rpc[0]);
    let mut v1 = spawn_produce(dirs[1].clone(), Some(&bootstrap), &rpc[1], "devnet-1");
    let mut v2 = spawn_produce(dirs[2].clone(), Some(&bootstrap), &rpc[2], "devnet-2");
    for (node, addr) in [&mut v0, &mut v1, &mut v2].into_iter().zip(&rpc) {
        wait_prefix(node, "peer ");
        wait_prefix(node, "rpc ");
        wait_health(addr);
    }
    mesh_nodes(&mut [&mut v0, &mut v1, &mut v2]);
    let tx1 = send_tx(&rpc[0], &signed_from(0x11, 0x22, 0, 1));
    wait_prefix(&mut v0, &format!("ready {tx1}"));
    let hash1 = line_tail(&block_at(&[&v0, &v1, &v2], "1"), 1).to_string();
    for node in [&mut v0, &mut v1, &mut v2] {
        wait_secs(node, &format!("committed {hash1} 1 "), 40);
    }
    let kept1 = v1.data_dir.clone();
    let kept2 = v2.data_dir.clone();
    stop_keep(&mut v1);
    stop_keep(&mut v2);
    stop_chain(&mut v0);
    let rpc_gap = format!("127.0.0.1:{}", free_port());
    let mut gap = spawn_produce(kept2, None, &rpc_gap, "gap");
    let gap_listen = listen_of(&mut gap);
    wait_secs(&mut gap, &format!("recovered 1 {hash1}"), 20);
    let rpc_probe = format!("127.0.0.1:{}", free_port());
    let mut probe = spawn_produce(kept1, Some(&gap_listen), &rpc_probe, "probe");
    wait_secs(&mut probe, &format!("recovered 1 {hash1}"), 20);
    wait_prefix(&mut probe, "peer ");
    mesh_nodes(&mut [&mut gap, &mut probe]);
    for marker in [
        "sync-rejected invalid block",
        "sync-rejected invalid parent",
        "sync-rejected invalid height",
        "sync-failed",
    ] {
        wait_secs(&mut probe, marker, 20);
    }
    assert_eq!(count_prefix(&probe, "committed "), 0, "{:?}", lines_of(&probe));
    assert_eq!(count_prefix(&probe, "synced "), 0);
    let status = post_rpc(&rpc_probe, "rsc_getStatus", "");
    assert!(status.contains("\"latest_height\":1"), "{status}");
    assert!(status.contains(&format!("\"latest_block_hash\":\"{hash1}\"")), "{status}");
    let missing = post_rpc(&rpc_probe, "rsc_getBlockByHeight", r#"{"height":2}"#);
    assert!(missing.contains("not found"), "{missing}");
    stop_chain(&mut probe);
    stop_chain(&mut gap);
}

const SEEDS: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
const IDS: [&str; 4] = [V0_ID, V1_ID, V2_ID, V3_ID];

struct Books {
    balance: [u128; 4],
    nonce: [u64; 4],
}

fn note_tx(seen: &mut Vec<String>, id: &str) {
    assert!(
        seen.iter().all(|kept| kept != id),
        "TransactionId duplicado {id}"
    );
    seen.push(id.to_string());
}

fn transfer(sender: usize, recipient: usize, nonce: u64) -> Transaction {
    signed_from(SEEDS[sender], SEEDS[recipient], nonce, 1)
}

fn settle(nodes: &mut [Peer], height: u64, prev: &str, txid: &str) -> (String, String, String) {
    let refs: Vec<&Peer> = nodes.iter().collect();
    let published = block_at(&refs, &height.to_string());
    let hash = line_tail(&published, 1).to_string();
    assert_eq!(line_tail(&published, 2), prev, "padre distinto en altura {height}");
    for node in nodes.iter_mut() {
        wait_secs(node, &format!("committed {hash} {height} "), 60);
        wait_secs(node, &format!("finalized {hash}"), 60);
    }
    let refs: Vec<&Peer> = nodes.iter().collect();
    let committed = committed_line(&refs, &height.to_string());
    assert_eq!(line_tail(&committed, 3), prev);
    assert_eq!(line_tail(&committed, 7), txid, "{committed}");
    let state = line_tail(&committed, 5).to_string();
    let ledger = line_tail(&committed, 6).to_string();
    let proof = owned_prefix(&lines_of(&nodes[0]), &format!("proof {hash} "));
    assert!(proof.ends_with(" 5 5 5"), "{proof}");
    let bytes = owned_prefix(&lines_of(&nodes[0]), &format!("bytes {height} "));
    for node in nodes.iter() {
        assert_eq!(owned_prefix(&lines_of(node), &format!("proof {hash} ")), proof);
        assert_eq!(owned_prefix(&lines_of(node), &format!("bytes {height} ")), bytes);
        assert_eq!(count_prefix(node, &format!("bytes {height} ")), 1);
        let voters = line_after(
            &lines_of(node),
            &format!("proof {hash} "),
            "proof-voters Prepare ",
        );
        assert!(voters.ends_with(" 5"), "{voters}");
        assert!(!voters.contains(V3_ID), "{voters}");
    }
    (hash, state, ledger)
}

fn assert_tip_holds(addrs: &[&str], height: u64, hash: &str, secs: u64) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(secs) {
        for addr in addrs {
            let status = post_rpc(addr, "rsc_getStatus", "");
            assert!(
                status.contains(&format!("\"latest_height\":{height}")),
                "{status}"
            );
            assert!(
                status.contains(&format!("\"latest_block_hash\":\"{hash}\"")),
                "{status}"
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn devnet_long_run_stays_converged() {
    let started = Instant::now();
    let dirs = [
        data_dir("long-0"),
        data_dir("long-1"),
        data_dir("long-2"),
        data_dir("long-3"),
    ];
    let rpc = [
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
        format!("127.0.0.1:{}", free_port()),
    ];
    let mut nodes = start_role("stress", &dirs, &rpc);
    let genesis = line_tail(&wait_prefix(&mut nodes[0], "installed-genesis "), 1).to_string();
    let mut books = Books {
        balance: [1, 1, 1, 1],
        nonce: [0, 0, 0, 0],
    };
    let mut seen = Vec::new();
    let mut tip = genesis.clone();
    let mut sent = 0u64;
    let mut rejected = 0u64;
    for height in 1..=30 {
        let sender = (height as usize - 1) % 4;
        let recipient = (sender + 1) % 4;
        assert!(books.balance[sender] >= 1, "saldo {height}");
        let id = send_tx(&rpc[0], &transfer(sender, recipient, books.nonce[sender]));
        note_tx(&mut seen, &id);
        books.balance[sender] -= 1;
        books.balance[recipient] += 1;
        books.nonce[sender] += 1;
        sent += 1;
        wait_prefix(&mut nodes[0], &format!("ready {id}"));
        let (hash, state, ledger) = settle(&mut nodes, height, &tip, &id);
        eprintln!("long h {height} {hash} {state} {ledger} {id}");
        tip = hash;
        let _ = (state, ledger);
    }
    assert_eq!(books.balance, [0, 1, 2, 1]);
    assert_eq!(books.nonce, [8, 8, 7, 7]);
    let mut burst = Vec::new();
    for sender in [1usize, 2, 3] {
        let tx = transfer(sender, 0, books.nonce[sender]);
        let id = send_tx(&rpc[0], &tx);
        burst.push((IDS[sender].to_string(), id, sender));
    }
    burst.sort_by(|left, right| left.0.cmp(&right.0));
    for (_, id, sender) in &burst {
        note_tx(&mut seen, id);
        books.balance[*sender] -= 1;
        books.balance[0] += 1;
        books.nonce[*sender] += 1;
        sent += 1;
        wait_prefix(&mut nodes[0], &format!("ready {id}"));
    }
    for (offset, (_, id, _)) in burst.iter().enumerate() {
        let height = 31 + offset as u64;
        let (hash, state, ledger) = settle(&mut nodes, height, &tip, id);
        eprintln!("long burst {height} {hash} {state} {ledger} {id}");
        tip = hash;
    }
    let duplicate = transfer(0, 1, books.nonce[0]);
    let duplicate_id = encode_hex(duplicate.id().0.as_bytes());
    let duplicate_hex = encode_hex(&duplicate.canonical_bytes());
    let first = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{duplicate_hex}"}}"#),
    );
    assert!(first.contains(&duplicate_id), "{first}");
    let second = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{duplicate_hex}"}}"#),
    );
    assert!(second.contains("transaction rejected"), "{second}");
    wait_prefix(&mut nodes[0], "rejected transaction is already in the mempool");
    rejected += 1;
    note_tx(&mut seen, &duplicate_id);
    books.balance[0] -= 1;
    books.balance[1] += 1;
    books.nonce[0] += 1;
    sent += 1;
    wait_prefix(&mut nodes[0], &format!("ready {duplicate_id}"));
    let (hash, state, ledger) = settle(&mut nodes, 34, &tip, &duplicate_id);
    eprintln!("long dup-block {hash} {state} {ledger} {duplicate_id}");
    tip = hash;
    let replay = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{duplicate_hex}"}}"#),
    );
    eprintln!("long post-commit {replay}");
    assert_tip_holds(&[&rpc[0], &rpc[1], &rpc[2], &rpc[3]], 34, &tip, 12);
    let once = post_rpc(
        &rpc[0],
        "rsc_getTransaction",
        &format!(r#"{{"transaction_hash":"{duplicate_id}"}}"#),
    );
    assert!(once.contains("\"block_height\":34"), "{once}");
    assert_eq!(
        once.matches(&format!("\"hash\":\"{duplicate_id}\"")).count(),
        1,
        "{once}"
    );
    let zero = encode_hex(&signed_from(0x11, 0x22, books.nonce[0], 0).canonical_bytes());
    let rejected_zero = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{zero}"}}"#),
    );
    assert!(rejected_zero.contains("transaction rejected"), "{rejected_zero}");
    rejected += 1;
    let mut altered = transfer(0, 1, books.nonce[0]).canonical_bytes();
    let last = altered.len() - 1;
    altered[last] ^= 0xff;
    let rejected_sig = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        &format!(r#"{{"signed_transaction":"{}"}}"#, encode_hex(&altered)),
    );
    assert!(rejected_sig.contains("transaction rejected"), "{rejected_sig}");
    wait_prefix(&mut nodes[0], "rejected transaction validation failed");
    rejected += 1;
    let rejected_raw = post_rpc(
        &rpc[0],
        "rsc_sendTransaction",
        r#"{"signed_transaction":"00"}"#,
    );
    assert!(rejected_raw.contains("transaction rejected"), "{rejected_raw}");
    wait_prefix(&mut nodes[0], "rejected transaction is invalid");
    rejected += 1;
    assert_tip_holds(&[&rpc[0]], 34, &tip, 8);
    let next = line_tail(&latest_proposer(&nodes[0]), 1).to_string();
    assert_eq!(next, V1_ID, "el bloque 35 lo propone un validador en línea");
    let identity = owned_prefix(&lines_of(&nodes[3]), &format!("validator 3 {V3_ID} "));
    let dir3 = nodes[3].data_dir.clone();
    for height in 1..=34 {
        assert_eq!(
            owned_prefix(&lines_of(&nodes[3]), &format!("bytes {height} ")),
            owned_prefix(&lines_of(&nodes[0]), &format!("bytes {height} "))
        );
    }
    stop_keep(&mut nodes[3]);
    let sender = (0..4).find(|index| books.balance[*index] >= 1).unwrap();
    let outage_id = send_tx(
        &rpc[0],
        &transfer(sender, (sender + 1) % 4, books.nonce[sender]),
    );
    note_tx(&mut seen, &outage_id);
    sent += 1;
    wait_prefix(&mut nodes[0], &format!("ready {outage_id}"));
    let (hash, state, ledger) = settle(&mut nodes[..3], 35, &tip, &outage_id);
    eprintln!("long outage {hash} {state} {ledger} {outage_id} proposer {next}");
    tip = hash;
    let bootstrap = owned_prefix(&lines_of(&nodes[0]), "listen ");
    let bootstrap = bootstrap["listen ".len()..].to_string();
    let rpc3 = format!("127.0.0.1:{}", free_port());
    let mut resumed = spawn_produce(dir3, Some(&bootstrap), &rpc3, "behind");
    wait_secs(&mut resumed, "recovered 34 ", 20);
    assert!(lines_of(&resumed)
        .iter()
        .all(|line| !line.starts_with("installed-genesis ")));
    let again = wait_secs(&mut resumed, &format!("validator 3 {V3_ID} "), 20);
    assert_eq!(again, identity);
    wait_prefix(&mut resumed, "peer ");
    wait_health(&rpc3);
    {
        let mut mesh = nodes[..3].iter_mut().collect::<Vec<_>>();
        mesh.push(&mut resumed);
        mesh_nodes(&mut mesh);
    }
    wait_secs(&mut resumed, "sync-request 35", 20);
    wait_secs(&mut resumed, &format!("synced 35 {tip} "), 40);
    wait_secs(&mut resumed, "sync-empty 36", 20);
    wait_secs(&mut resumed, "sync-complete", 20);
    assert_eq!(count_prefix(&resumed, "vote "), 0);
    assert_eq!(count_prefix(&resumed, "finalized "), 0);
    assert_eq!(count_prefix(&resumed, "quorum "), 0);
    assert_eq!(
        owned_prefix(&lines_of(&resumed), "bytes 35 "),
        owned_prefix(&lines_of(&nodes[0]), "bytes 35 ")
    );
    let addrs = [&rpc[0], &rpc[1], &rpc[2], rpc3.as_str()];
    let mut parent = genesis.clone();
    for height in 1..=35 {
        let mut body = String::new();
        for addr in addrs {
            let next_body = post_rpc(
                addr,
                "rsc_getBlockByHeight",
                &format!(r#"{{"height":{height}}}"#),
            );
            if body.is_empty() {
                body = next_body;
            } else {
                let hash = next_body
                    .split("\"hash\":\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
                    .to_string();
                assert!(body.contains(&format!("\"hash\":\"{hash}\"")), "{next_body}");
                assert!(
                    next_body.contains(&format!("\"parent_hash\":\"{parent}\"")),
                    "{next_body}"
                );
            }
        }
        assert!(body.contains(&format!("\"parent_hash\":\"{parent}\"")), "{body}");
        parent = body
            .split("\"hash\":\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let id = &seen[height as usize - 1];
        for addr in addrs {
            let tx = post_rpc(
                addr,
                "rsc_getTransaction",
                &format!(r#"{{"transaction_hash":"{id}"}}"#),
            );
            assert!(tx.contains(&format!("\"block_height\":{height}")), "{tx}");
            assert_eq!(tx.matches(&format!("\"hash\":\"{id}\"")).count(), 1, "{tx}");
        }
    }
    assert_eq!(parent, tip);
    for addr in addrs {
        let status = post_rpc(addr, "rsc_getStatus", "");
        assert!(status.contains("\"latest_height\":35"), "{status}");
        assert!(status.contains(&format!("\"latest_block_hash\":\"{tip}\"")), "{status}");
        assert!(status.contains("\"finalized_height\":35"), "{status}");
        let missing = post_rpc(addr, "rsc_getBlockByHeight", r#"{"height":36}"#);
        assert!(missing.contains("not found"), "{missing}");
    }
    let elapsed = started.elapsed().as_secs();
    eprintln!(
        "long-run blocks 35 sent {sent} committed {} rejected {rejected} elapsed_s {elapsed} retries 0 restarts 1",
        seen.len()
    );
    eprintln!("long-run genesis {genesis}");
    eprintln!("long-run tip {tip} state {state} ledger {ledger}");
    eprintln!("long-run offline validator 3 {V3_ID} proposer-kept {next}");
    stop_chain(&mut resumed);
    stop_chain(&mut nodes[2]);
    stop_chain(&mut nodes[1]);
    stop_chain(&mut nodes[0]);
}

#[test]
fn persist_peer_id_survives_restart() {
    let dir = data_dir("peer-keep");
    let key = dir.join("p2p.key");
    std::fs::create_dir_all(&dir).unwrap();
    let mut first = spawn_persisted(&dir, &key);
    let peer = wait_prefix(&mut first, "peer ");
    let listen = wait_prefix(&mut first, "listen ");
    assert!(listen.contains(peer.split_whitespace().nth(1).unwrap()), "{listen}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
    term_files(&mut first);
    let mut second = spawn_persisted(&dir, &key);
    let again = wait_prefix(&mut second, "peer ");
    assert_eq!(again, peer);
    let listen_again = wait_prefix(&mut second, "listen ");
    assert!(
        listen_again.contains(peer.split_whitespace().nth(1).unwrap()),
        "{listen_again}"
    );
    term_files(&mut second);
    std::fs::write(&key, b"not-a-key").unwrap();
    let mut broken = Command::new(bin())
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--persist-peer",
            key.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = broken.try_wait().unwrap() {
            assert!(!status.success(), "{status}");
            let mut err = String::new();
            broken.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            assert!(err.contains("network identity"), "{err}");
            assert!(!err.contains("not-a-key"), "{err}");
            break;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = broken.kill();
            panic!("la identidad dañada no cerró");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read(&key).unwrap(), b"not-a-key");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn spawn_persisted(dir: &std::path::Path, key: &std::path::Path) -> Peer {
    let mut child = Command::new(bin())
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--persist-peer",
            key.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            sink.lock().unwrap().push(line);
        }
    });
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let err_sink = Arc::clone(&stderr_lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            err_sink.lock().unwrap().push(line);
        }
    });
    Peer {
        child: Some(child),
        stdin,
        lines,
        stderr_lines,
        data_dir: dir.to_path_buf(),
    }
}

#[test]
fn presence_listen_classifies_without_claiming_public() {
    let dir = data_dir("presence");
    let mut node = spawn_presence(&dir);
    let peer = wait_prefix(&mut node, "peer ");
    assert!(peer.starts_with("peer "));
    let started = Instant::now();
    let mut saw_tcp = false;
    let mut saw_quic = false;
    let mut saw_loopback = false;
    let mut saw_unknown = false;
    loop {
        for line in lines_of(&node) {
            if line.starts_with("listen ") && line.contains("/tcp/") && !line.contains("quic-v1") {
                saw_tcp = true;
            }
            if line.starts_with("listen ") && line.contains("quic-v1") {
                saw_quic = true;
            }
            if line.starts_with("class ") && line.ends_with(" loopback") {
                saw_loopback = true;
            }
            if line == "presence unknown" {
                saw_unknown = true;
            }
            assert!(
                !line.starts_with("presence public"),
                "el smoke no puede declarar alcance público: {line}"
            );
        }
        if saw_tcp && saw_quic && saw_loopback && saw_unknown {
            break;
        }
        if let Some(status) = node.child.as_mut().unwrap().try_wait().unwrap() {
            panic!("el proceso salió {status}: {:?}", lines_of(&node));
        }
        if started.elapsed() > Duration::from_secs(8) {
            panic!("falta listen o clasificación: {:?}", lines_of(&node));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop(&mut node);
}

fn spawn_presence(dir: &std::path::Path) -> Peer {
    let mut child = Command::new(bin())
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--presence",
            "auto",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            sink.lock().unwrap().push(line);
        }
    });
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let err_sink = Arc::clone(&stderr_lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            err_sink.lock().unwrap().push(line);
        }
    });
    Peer {
        child: Some(child),
        stdin,
        lines,
        stderr_lines,
        data_dir: dir.to_path_buf(),
    }
}

fn has_prefix(peer: &Peer, prefix: &str) -> bool {
    lines_of(peer).iter().any(|line| line.starts_with(prefix))
}

fn wait_dcutr(mut peers: [&mut Peer; 2]) -> String {
    let started = Instant::now();
    loop {
        for peer in &peers {
            if let Some(line) = lines_of(peer)
                .into_iter()
                .find(|line| line.starts_with("dcutr-ok ") || line.starts_with("dcutr-failed "))
            {
                return line;
            }
        }
        for peer in &mut peers {
            if let Some(status) = peer.child.as_mut().unwrap().try_wait().unwrap() {
                panic!("el proceso salió {status}: {:?}", lines_of(peer));
            }
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!(
                "no hubo evento DCUtR: {:?} {:?}",
                lines_of(peers[0]),
                lines_of(peers[1])
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_no_public_direct(peers: &[&Peer]) {
    for peer in peers {
        for line in lines_of(peer) {
            assert!(
                !line.starts_with("presence public"),
                "relay no es IPv4 pública: {line}"
            );
        }
    }
}

fn finish_files(peer: &mut Peer) {
    term_files(peer);
    std::fs::remove_dir_all(&peer.data_dir).unwrap();
}

fn spawn_relay(
    dir: &std::path::Path,
    network: &str,
    chain: &str,
    extra: &[&str],
) -> Peer {
    let mut args = vec![
        "--data-dir".to_string(),
        dir.to_str().unwrap().to_string(),
        "--network-id".to_string(),
        network.to_string(),
        "--chain-id".to_string(),
        chain.to_string(),
    ];
    args.extend(extra.iter().map(|part| (*part).to_string()));
    let mut child = Command::new(bin())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            sink.lock().unwrap().push(line);
        }
    });
    let stderr_lines = Arc::new(Mutex::new(Vec::new()));
    let err_sink = Arc::clone(&stderr_lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            err_sink.lock().unwrap().push(line);
        }
    });
    Peer {
        child: Some(child),
        stdin,
        lines,
        stderr_lines,
        data_dir: dir.to_path_buf(),
    }
}

#[test]
fn relay_reservation_and_dcutr_on_localhost() {
    let dir_a = data_dir("relay-a");
    let key = dir_a.join("peer.key");
    let key_arg = key.to_str().unwrap().to_string();
    let mut server = spawn_relay(
        &dir_a,
        "devnet",
        "3",
        &["--relay", "server", "--persist-peer", &key_arg],
    );
    let first_peer = wait_prefix(&mut server, "peer ");
    let id_a = peer_of(&first_peer);
    wait_prefix(&mut server, "relay-server");
    term_files(&mut server);
    let mut server = spawn_relay(
        &dir_a,
        "devnet",
        "3",
        &["--relay", "server", "--persist-peer", &key_arg],
    );
    let restarted = wait_prefix(&mut server, "peer ");
    assert_eq!(peer_of(&restarted), id_a, "el relay pierde el PeerId");
    wait_prefix(&mut server, "relay-server");
    let listen = wait_prefix(&mut server, "listen ");
    let addr_a = listen.trim_start_matches("listen ").to_string();
    assert!(addr_a.contains(&id_a.to_string()), "{addr_a}");

    let dir_c = data_dir("relay-c");
    let mut client_c = spawn_relay(
        &dir_c,
        "devnet",
        "3",
        &["--dial", &addr_a, "--relay-client", &addr_a],
    );
    let id_c = peer_of(&wait_prefix(&mut client_c, "peer "));
    let reserved_c = wait_prefix(&mut client_c, "reservation ");
    assert!(reserved_c.contains(&id_a.to_string()), "{reserved_c}");
    let relay_addr = wait_prefix(&mut client_c, "presence relay ");
    assert!(relay_addr.contains("p2p-circuit"), "{relay_addr}");
    let accepted_c = wait_prefix(&mut server, &format!("reservation-accepted {id_c}"));
    assert!(accepted_c.contains(&id_c.to_string()));

    let circuit = format!("{addr_a}/p2p-circuit/p2p/{id_c}");
    let dir_b = data_dir("relay-b");
    let mut client_b = spawn_relay(
        &dir_b,
        "devnet",
        "3",
        &[
            "--dial",
            &addr_a,
            "--relay-client",
            &addr_a,
            "--circuit-dial",
            &circuit,
        ],
    );
    let id_b = peer_of(&wait_prefix(&mut client_b, "peer "));
    let reserved_b = wait_prefix(&mut client_b, "reservation ");
    assert!(reserved_b.contains(&id_a.to_string()), "{reserved_b}");
    wait_prefix(&mut server, &format!("reservation-accepted {id_b}"));
    wait_prefix(&mut client_b, &format!("connected-relay {id_c}"));
    wait_prefix(&mut client_c, &format!("connected-relay {id_b}"));
    let started = Instant::now();
    loop {
        let spoke = lines_of(&client_b)
            .iter()
            .chain(lines_of(&client_c).iter())
            .any(|line| {
                line == &format!("handshake-relay {id_b}")
                    || line == &format!("handshake-relay {id_c}")
                    || line == &format!("handshake {id_b}")
                    || line == &format!("handshake {id_c}")
            });
        if spoke {
            break;
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!(
                "sin handshake por el circuito: {:?} {:?}",
                lines_of(&client_b),
                lines_of(&client_c)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let dcutr = wait_dcutr([&mut client_b, &mut client_c]);
    if dcutr.starts_with("dcutr-ok ") {
        let upgraded = dcutr.split_whitespace().nth(1).unwrap();
        let other = if upgraded == id_c.to_string() {
            &client_b
        } else {
            &client_c
        };
        assert!(
            has_prefix(other, &format!("connected {upgraded}")),
            "DCUtR ok sin conexión directa: {dcutr} {:?}",
            lines_of(other)
        );
    } else {
        assert!(dcutr.starts_with("dcutr-failed "), "{dcutr}");
        assert!(
            has_prefix(&client_b, &format!("connected-relay {id_c}")),
            "el fallo de DCUtR no puede borrar el relay"
        );
    }
    assert_no_public_direct(&[&server, &client_b, &client_c]);
    eprintln!("relay-peers server={id_a} b={id_b} c={id_c} {dcutr}");
    finish_files(&mut client_b);
    finish_files(&mut client_c);
    finish_files(&mut server);
}

#[test]
fn relay_unreachable_does_not_reserve() {
    let dir = data_dir("relay-down");
    let peer = rsc_p2p::PeerId::random();
    let addr = format!("/ip4/127.0.0.1/tcp/1/p2p/{peer}");
    let mut client = spawn_relay(
        &dir,
        "devnet",
        "3",
        &["--dial", &addr, "--relay-client", &addr],
    );
    let failed = wait_prefix(&mut client, "dial-failed ");
    assert!(failed.contains("dial failed"), "{failed}");
    std::thread::sleep(Duration::from_secs(1));
    assert!(!has_prefix(&client, "reservation "));
    assert!(!has_prefix(&client, "dcutr-ok "));
    assert!(!has_prefix(&client, "connected-relay "));
    assert_no_public_direct(&[&client]);
    finish_files(&mut client);
}

#[test]
fn relay_without_server_does_not_reserve() {
    let dir_a = data_dir("relay-off-a");
    let mut server = spawn_relay(&dir_a, "devnet", "3", &[]);
    let addr = wait_prefix(&mut server, "listen ")
        .trim_start_matches("listen ")
        .to_string();
    let dir_b = data_dir("relay-off-b");
    let mut client = spawn_relay(
        &dir_b,
        "devnet",
        "3",
        &["--dial", &addr, "--relay-client", &addr],
    );
    wait_prefix(&mut client, "connected ");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(6) {
        assert!(!has_prefix(&client, "reservation "));
        assert!(!has_prefix(&client, "dcutr-ok "));
        assert!(!has_prefix(&client, "presence public"));
        if has_prefix(&client, "reservation-failed") || has_prefix(&server, "reservation-denied ")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!has_prefix(&client, "reservation "), "{:?}", lines_of(&client));
    assert_no_public_direct(&[&server, &client]);
    finish_files(&mut client);
    finish_files(&mut server);
}

#[test]
fn relay_invalid_address_exits() {
    let dir = data_dir("relay-bad");
    let mut child = Command::new(bin())
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--relay-client",
            "/ip4/127.0.0.1/tcp/9",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success(), "{status}");
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            assert!(err.contains("invalid address"), "{err}");
            break;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("la dirección de relay inválida no cerró");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn relay_network_mismatch_does_not_upgrade() {
    relay_reject("other", "3", "network mismatch");
}

#[test]
fn relay_chain_mismatch_does_not_upgrade() {
    relay_reject("devnet", "1", "chain mismatch");
}

fn relay_reject(network: &str, chain: &str, message: &str) {
    let dir_a = data_dir("relay-reject-a");
    let mut server = spawn_relay(&dir_a, "devnet", "3", &["--relay", "server"]);
    wait_prefix(&mut server, "relay-server");
    let addr = wait_prefix(&mut server, "listen ")
        .trim_start_matches("listen ")
        .to_string();
    let dir_b = data_dir("relay-reject-b");
    let mut client = spawn_relay(
        &dir_b,
        network,
        chain,
        &["--dial", &addr, "--relay-client", &addr],
    );
    let started = Instant::now();
    loop {
        let left = lines_of(&server);
        let right = lines_of(&client);
        if left
            .iter()
            .chain(right.iter())
            .any(|line| line.starts_with("rejected ") && line.contains(message))
        {
            assert!(left.iter().chain(right.iter()).all(|line| {
                !line.starts_with("handshake ")
                    && !line.starts_with("handshake-relay ")
                    && !line.starts_with("dcutr-ok ")
                    && !line.starts_with("presence public")
                    && !line.starts_with("connected-relay ")
            }));
            break;
        }
        if started.elapsed() > Duration::from_secs(15) {
            panic!("sin rechazo {message}: {left:?} {right:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    finish_files(&mut client);
    finish_files(&mut server);
}

#[test]
fn relay_loss_does_not_become_direct() {
    let dir_a = data_dir("relay-loss-a");
    let mut server = spawn_relay(&dir_a, "devnet", "3", &["--relay", "server"]);
    let id_a = peer_of(&wait_prefix(&mut server, "peer "));
    let addr = wait_prefix(&mut server, "listen ")
        .trim_start_matches("listen ")
        .to_string();
    let dir_b = data_dir("relay-loss-b");
    let mut client = spawn_relay(
        &dir_b,
        "devnet",
        "3",
        &["--dial", &addr, "--relay-client", &addr],
    );
    wait_prefix(&mut client, "reservation ");
    term_files(&mut server);
    wait_prefix(&mut client, &format!("relay-lost {id_a}"));
    assert!(has_prefix(&client, "presence unknown"));
    assert!(!has_prefix(&client, "dcutr-ok "));
    assert!(!has_prefix(&client, "presence public"));
    assert!(!has_prefix(&client, "connected-relay "));
    finish_files(&mut client);
    std::fs::remove_dir_all(&dir_a).unwrap();
}

fn term_files(peer: &mut Peer) {
    let mut child = peer.child.take().unwrap();
    eprintln!("fase term-inicio pid={}", child.id());
    drop(peer.stdin.take());
    #[cfg(unix)]
    {
        assert!(child_pids(child.id()).is_empty());
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
    }
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            assert!(child_pids(child.id()).is_empty());
            return;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("el proceso no cerró con TERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
