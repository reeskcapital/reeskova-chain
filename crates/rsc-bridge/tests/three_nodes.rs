//! Bridge, tres nodos `rsc-p2p` y los rechazos que no pueden salir en GET /network.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libp2p::{identity, Multiaddr, PeerId};
use rsc_p2p::presence::{PresenceRecord, Reach, SignedPresence};
use rsc_p2p::{BLOCK_SYNC_PROTOCOL, GOSSIPSUB_PROTOCOL, HANDSHAKE_PROTOCOL, RSC_P2P_PROTOCOL_VERSION};

struct Proc {
    child: std::process::Child,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status();
            let _ = self.child.wait();
        }
    }
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rsc-bridge-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spawn(bin: &PathBuf, args: &[String]) -> Proc {
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    if let Some(stdout) = child.stdout.take() {
        pipe(stdout, Arc::clone(&lines));
    }
    if let Some(stderr) = child.stderr.take() {
        pipe(stderr, Arc::clone(&lines));
    }
    Proc { child, lines }
}

fn pipe(stream: impl Read + Send + 'static, sink: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            sink.lock().unwrap().push(line);
        }
    });
}

fn wait_prefix(proc: &mut Proc, prefix: &str) -> String {
    let started = Instant::now();
    loop {
        if let Some(line) = proc
            .lines
            .lock()
            .unwrap()
            .iter()
            .find(|line| line.starts_with(prefix))
            .cloned()
        {
            return line;
        }
        if proc.child.try_wait().unwrap().is_some() {
            std::thread::sleep(Duration::from_millis(50));
            if let Some(line) = proc
                .lines
                .lock()
                .unwrap()
                .iter()
                .find(|line| line.starts_with(prefix))
                .cloned()
            {
                return line;
            }
            panic!("salió antes de {prefix}: {:?}", proc.lines.lock().unwrap());
        }
        if started.elapsed() > Duration::from_secs(20) {
            panic!("timeout {prefix}: {:?}", proc.lines.lock().unwrap());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_success(proc: &mut Proc) {
    let status = proc.child.wait().unwrap();
    assert!(status.success(), "{status:?} {:?}", proc.lines.lock().unwrap());
}

fn stop(proc: &mut Proc) {
    if proc.child.try_wait().unwrap().is_none() {
        Command::new("kill")
            .args(["-TERM", &proc.child.id().to_string()])
            .status()
            .unwrap();
    }
    wait_success(proc);
}

fn node_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_rsc-bridge"));
    path.pop();
    path.push("rsc-p2p");
    assert!(path.is_file(), "falta {}", path.display());
    path
}

fn http(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|mark| mark == b"\r\n\r\n")
        .expect("respuesta HTTP");
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let status = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let mut length = None;
    for line in head.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
    }
    let mut payload = raw[split + 4..].to_vec();
    if let Some(length) = length {
        payload.truncate(length);
    }
    (status, head.to_string(), payload)
}

fn protocols() -> [String; 4] {
    [
        RSC_P2P_PROTOCOL_VERSION.into(),
        HANDSHAKE_PROTOCOL.into(),
        BLOCK_SYNC_PROTOCOL.into(),
        GOSSIPSUB_PROTOCOL.into(),
    ]
}

fn direct(peer: PeerId, sequence: u64, expires_at: u64, address: &'static str) -> Spec<'static> {
    Spec {
        peer,
        sequence,
        expires_at,
        reach: Reach::PublicDirect,
        address,
        network_id: "devnet",
        chain_id: 3,
    }
}

struct Spec<'a> {
    peer: PeerId,
    sequence: u64,
    expires_at: u64,
    reach: Reach,
    address: &'a str,
    network_id: &'a str,
    chain_id: u32,
}

fn sign(key: &identity::Keypair, spec: Spec<'_>) -> Vec<u8> {
    let record = PresenceRecord {
        network_id: spec.network_id.into(),
        chain_id: spec.chain_id,
        peer_id: spec.peer,
        public_key: key.public().encode_protobuf(),
        protocols: protocols(),
        chain: None,
        reach: spec.reach,
        addresses: vec![spec.address.parse::<Multiaddr>().unwrap()],
        issued_at: 1,
        expires_at: spec.expires_at,
        sequence: spec.sequence,
    };
    let canon = record.canonical_bytes().unwrap();
    let raw = key.sign(&canon).unwrap();
    let mut wire = canon;
    wire.extend_from_slice(&raw);
    wire
}

fn publish(dir: &Tmp, reach: &str, address: &str, ttl: &str, url: &str) -> (String, u64) {
    let key = dir.0.join("peer.key");
    let mut proc = spawn(
        &node_bin(),
        &[
            "--data-dir".into(),
            dir.0.display().to_string(),
            "--network-id".into(),
            "devnet".into(),
            "--chain-id".into(),
            "3".into(),
            "--persist-peer".into(),
            key.display().to_string(),
            "--publish-presence".into(),
            url.into(),
            "--presence-reach".into(),
            reach.into(),
            "--presence-addr".into(),
            address.into(),
            "--presence-ttl".into(),
            ttl.into(),
        ],
    );
    let line = wait_prefix(&mut proc, "published ");
    wait_success(&mut proc);
    assert!(!dir.0.join("CURRENT").exists());
    assert!(!dir.0.join("LOCK").exists());
    let mut parts = line.split_whitespace();
    assert_eq!(parts.next(), Some("published"));
    let peer = parts.next().unwrap().to_string();
    let sequence = parts.next().unwrap().parse().unwrap();
    (peer, sequence)
}

#[test]
fn bridge_serves_three_nodes_and_drops_bad_cards() {
    let root = Tmp::new("live");
    let bridge_dir = root.0.join("bridge");
    std::fs::create_dir(&bridge_dir).unwrap();
    let mut bridge = spawn(
        &PathBuf::from(env!("CARGO_BIN_EXE_rsc-bridge")),
        &[
            "--data-dir".into(),
            bridge_dir.display().to_string(),
            "--bind".into(),
            "127.0.0.1:0".into(),
            "--network-id".into(),
            "devnet".into(),
            "--chain-id".into(),
            "3".into(),
        ],
    );
    let listen = wait_prefix(&mut bridge, "listen ");
    let addr: SocketAddr = listen.split_whitespace().nth(1).unwrap().parse().unwrap();
    let url = format!("http://{addr}/network/presence");
    let key = identity::Keypair::generate_ed25519();
    let peer = PeerId::from(&key.public());
    let addr_a = "/ip4/203.0.113.20/tcp/4001";
    let good = sign(&key, direct(peer, 4, 4_000_000_000, addr_a));
    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    let wrong_peer = direct(PeerId::random(), 5, 4_000_000_000, addr_a);
    let mut other_net = direct(peer, 5, 4_000_000_000, addr_a);
    other_net.network_id = "other";
    let mut other_chain = direct(peer, 5, 4_000_000_000, addr_a);
    other_chain.chain_id = 9;
    let mut relay = direct(peer, 5, 4_000_000_000, "/ip4/203.0.113.20/tcp/1");
    relay.reach = Reach::Relay;
    let rejected = [
        (b"not-a-card".to_vec(), 400, "invalid presence"),
        (flipped, 400, "invalid signature"),
        (sign(&key, wrong_peer), 400, "identity mismatch"),
        (sign(&key, other_net), 400, "network mismatch"),
        (sign(&key, other_chain), 400, "chain mismatch"),
        (
            sign(&key, direct(peer, 5, 1, addr_a)),
            400,
            "expired presence",
        ),
        (
            sign(&key, direct(peer, 5, 4_000_000_000, "/ip4/10.1.1.1/tcp/1")),
            400,
            "not publishable",
        ),
        (sign(&key, relay), 400, "not relay address"),
    ];
    for (body, status, text) in rejected {
        let (code, _, payload) = http(addr, "POST", "/network/presence", &body);
        assert_eq!(code, status, "{}", String::from_utf8_lossy(&payload));
        assert_eq!(String::from_utf8_lossy(&payload), text);
        let (code, _, listing) = http(addr, "GET", "/network", &[]);
        assert_eq!(code, 200);
        assert!(listing.is_empty());
    }
    let (code, _, _) = http(addr, "POST", "/network/presence", &good);
    assert_eq!(code, 204);
    let other = sign(&key, direct(peer, 4, 4_000_000_000, "/ip4/203.0.113.21/tcp/4001"));
    let (code, _, payload) = http(addr, "POST", "/network/presence", &other);
    assert_eq!(code, 409, "{}", String::from_utf8_lossy(&payload));
    assert_eq!(String::from_utf8_lossy(&payload), "duplicate sequence");
    let stale = sign(&key, direct(peer, 2, 4_000_000_000, "/ip4/203.0.113.22/tcp/4001"));
    let (code, _, payload) = http(addr, "POST", "/network/presence", &stale);
    assert_eq!(code, 400, "{}", String::from_utf8_lossy(&payload));
    assert_eq!(String::from_utf8_lossy(&payload), "stale sequence");
    let (code, head, listing) = http(addr, "GET", "/network", &[]);
    assert_eq!(code, 200);
    assert!(head.to_ascii_lowercase().contains("x-rsc-bridge-time:"));
    assert_eq!(listing, good);

    let node_a = Tmp::new("a");
    let node_b = Tmp::new("b");
    let node_c = Tmp::new("c");
    let (peer_a, seq_a) = publish(
        &node_a,
        "public",
        "/ip4/203.0.113.10/tcp/4001",
        "300",
        &url,
    );
    let (peer_b, seq_b) = publish(
        &node_b,
        "public",
        "/ip4/203.0.113.11/tcp/4001",
        "300",
        &url,
    );
    let circuit = format!("/ip4/203.0.113.10/tcp/4001/p2p/{peer_a}/p2p-circuit");
    let (peer_c, seq_c) = publish(&node_c, "relay", &circuit, "300", &url);
    assert_eq!((seq_a, seq_b, seq_c), (1, 1, 1));
    let (code, head, listing) = http(addr, "GET", "/network", &[]);
    assert_eq!(code, 200);
    assert!(head.to_ascii_lowercase().contains("x-rsc-bridge-time:"));
    let cards = SignedPresence::read_list(&listing).unwrap();
    assert!(cards.iter().any(|card| card.record.peer_id.to_string() == peer.to_string()));
    let by_peer = |id: &str| {
        cards
            .iter()
            .find(|card| card.record.peer_id.to_string() == id)
            .unwrap()
            .clone()
    };
    let card_a = by_peer(&peer_a);
    let card_b = by_peer(&peer_b);
    let card_c = by_peer(&peer_c);
    for card in [&card_a, &card_b, &card_c] {
        assert!(card.verify().is_ok());
        assert!(card.record.chain.is_none());
        assert_eq!(card.record.sequence, 1);
    }
    assert_eq!(card_a.record.reach, Reach::PublicDirect);
    assert_eq!(card_b.record.reach, Reach::PublicDirect);
    assert_eq!(card_c.record.reach, Reach::Relay);
    assert!(rsc_p2p::presence::is_circuit(&card_c.record.addresses[0]));
    assert!(!rsc_p2p::presence::is_circuit(&card_a.record.addresses[0]));
    let first_a = card_a.to_wire().unwrap();
    assert!(listing.windows(first_a.len()).any(|chunk| chunk == first_a));

    let (_, seq_short) = publish(
        &node_a,
        "public",
        "/ip4/203.0.113.10/tcp/4001",
        "1",
        &url,
    );
    assert_eq!(seq_short, 2);
    std::thread::sleep(Duration::from_secs(3));
    let (_, _, listing) = http(addr, "GET", "/network", &[]);
    let cards = SignedPresence::read_list(&listing).unwrap();
    assert!(cards.iter().all(|card| card.record.peer_id.to_string() != peer_a));
    assert!(cards.iter().any(|card| card.record.peer_id.to_string() == peer_b));
    assert!(cards.iter().any(|card| card.record.peer_id.to_string() == peer_c));

    let (_, seq_new) = publish(
        &node_a,
        "public",
        "/ip4/203.0.113.10/tcp/4001",
        "300",
        &url,
    );
    assert_eq!(seq_new, 3);
    let (_, _, listing) = http(addr, "GET", "/network", &[]);
    let cards = SignedPresence::read_list(&listing).unwrap();
    let renewed = cards
        .iter()
        .find(|card| card.record.peer_id.to_string() == peer_a)
        .unwrap();
    assert_eq!(renewed.record.sequence, 3);
    assert!(renewed.verify().is_ok());
    let (code, _, payload) = http(addr, "POST", "/network/presence", &first_a);
    assert_eq!(code, 400, "{}", String::from_utf8_lossy(&payload));
    assert_eq!(String::from_utf8_lossy(&payload), "stale sequence");
    let (_, _, listing) = http(addr, "GET", "/network", &[]);
    let cards = SignedPresence::read_list(&listing).unwrap();
    assert_eq!(
        cards
            .iter()
            .find(|card| card.record.peer_id.to_string() == peer_a)
            .unwrap()
            .record
            .sequence,
        3
    );
    for name in std::fs::read_dir(&bridge_dir).unwrap() {
        let name = name.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!name.ends_with(".key"), "{name}");
        assert!(!name.ends_with(".seq"), "{name}");
    }
    stop(&mut bridge);
}
