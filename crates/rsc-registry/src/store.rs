//! SQLite del registro. Un PeerId tiene una sola ficha.

use crate::types::NodeRecord;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordWrite {
    pub peer_id: String,
    pub network_id: String,
    pub chain_id: u32,
    pub p2p_multiaddr: Option<String>,
    pub rpc_public_url: Option<String>,
    pub service_url: Option<String>,
    pub node_type: String,
    pub status: String,
    pub software_version: String,
    pub public_key_hex: String,
    pub sequence: u64,
    pub ttl_secs: u64,
    pub primary_rpc: bool,
    pub produces_blocks: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeFilter {
    pub network_id: Option<String>,
    pub chain_id: Option<u32>,
    pub status: Option<String>,
    pub node_type: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("storage")]
    Storage,
}

#[derive(Debug)]
pub struct StaleSequence {
    pub current: u64,
}

#[derive(Debug)]
pub enum UpsertError {
    Stale(StaleSequence),
    Identity,
    Storage,
}

struct Row {
    record: NodeRecord,
    ttl_secs: u64,
}

pub struct RegistryStore {
    conn: Connection,
}

impl RegistryStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|_| StoreError::Storage)?;
            }
        }
        let conn = Connection::open(path).map_err(|_| StoreError::Storage)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| StoreError::Storage)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|_| StoreError::Storage)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS nodes (
                peer_id TEXT PRIMARY KEY,
                network_id TEXT NOT NULL,
                chain_id INTEGER NOT NULL,
                p2p_multiaddr TEXT,
                rpc_public_url TEXT,
                service_url TEXT,
                node_type TEXT NOT NULL,
                status TEXT NOT NULL,
                last_heartbeat INTEGER NOT NULL,
                registered_at INTEGER NOT NULL,
                software_version TEXT NOT NULL,
                public_key_hex TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                ttl_secs INTEGER NOT NULL,
                primary_rpc INTEGER NOT NULL,
                produces_blocks INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS nodes_network ON nodes(network_id, chain_id);",
        )
        .map_err(|_| StoreError::Storage)?;
        Ok(Self { conn })
    }

    pub fn upsert(&mut self, rec: &RecordWrite, now: u64) -> Result<(), UpsertError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|_| UpsertError::Storage)?;
        let existing = load_row(&tx, &rec.peer_id).map_err(|_| UpsertError::Storage)?;
        if let Some(old) = existing {
            if old.record.public_key_hex != rec.public_key_hex {
                return Err(UpsertError::Identity);
            }
            if rec.sequence <= old.record.sequence {
                return Err(UpsertError::Stale(StaleSequence {
                    current: old.record.sequence,
                }));
            }
            tx.execute(
                "UPDATE nodes SET
                    network_id=?1, chain_id=?2, p2p_multiaddr=?3, rpc_public_url=?4,
                    service_url=?5, node_type=?6, status=?7, last_heartbeat=?8,
                    software_version=?9, sequence=?10, ttl_secs=?11, primary_rpc=?12,
                    produces_blocks=?13
                 WHERE peer_id=?14",
                params![
                    rec.network_id,
                    rec.chain_id,
                    rec.p2p_multiaddr,
                    rec.rpc_public_url,
                    rec.service_url,
                    rec.node_type,
                    rec.status,
                    now,
                    rec.software_version,
                    rec.sequence,
                    rec.ttl_secs,
                    i64::from(rec.primary_rpc),
                    i64::from(rec.produces_blocks),
                    rec.peer_id,
                ],
            )
            .map_err(|_| UpsertError::Storage)?;
        } else {
            tx.execute(
                "INSERT INTO nodes (
                    peer_id, network_id, chain_id, p2p_multiaddr, rpc_public_url, service_url,
                    node_type, status, last_heartbeat, registered_at, software_version,
                    public_key_hex, sequence, ttl_secs, primary_rpc, produces_blocks
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    rec.peer_id,
                    rec.network_id,
                    rec.chain_id,
                    rec.p2p_multiaddr,
                    rec.rpc_public_url,
                    rec.service_url,
                    rec.node_type,
                    rec.status,
                    now,
                    now,
                    rec.software_version,
                    rec.public_key_hex,
                    rec.sequence,
                    rec.ttl_secs,
                    i64::from(rec.primary_rpc),
                    i64::from(rec.produces_blocks),
                ],
            )
            .map_err(|_| UpsertError::Storage)?;
        }
        tx.commit().map_err(|_| UpsertError::Storage)?;
        Ok(())
    }

    pub fn list(&self, filter: &NodeFilter, now: u64) -> Result<Vec<NodeRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT peer_id, network_id, chain_id, p2p_multiaddr, rpc_public_url, service_url,
                        node_type, status, last_heartbeat, registered_at, software_version,
                        public_key_hex, sequence, ttl_secs, primary_rpc, produces_blocks
                 FROM nodes ORDER BY registered_at ASC LIMIT 500",
            )
            .map_err(|_| StoreError::Storage)?;
        let rows = stmt
            .query_map([], map_row)
            .map_err(|_| StoreError::Storage)?;
        let mut out = Vec::new();
        for row in rows {
            let record = finish(row.map_err(|_| StoreError::Storage)?, now);
            if let Some(network) = &filter.network_id {
                if &record.network_id != network {
                    continue;
                }
            }
            if let Some(chain) = filter.chain_id {
                if record.chain_id != chain {
                    continue;
                }
            }
            if let Some(kind) = &filter.node_type {
                if &record.node_type != kind {
                    continue;
                }
            }
            if let Some(status) = &filter.status {
                if &record.status != status {
                    continue;
                }
            }
            out.push(record);
        }
        Ok(out)
    }

    pub fn get(&self, peer_id: &str, now: u64) -> Result<Option<NodeRecord>, StoreError> {
        Ok(load_row(&self.conn, peer_id)?.map(|row| finish(row, now)))
    }

    pub fn expire(&mut self, now: u64) -> Result<u64, StoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE nodes SET status='OFFLINE'
                 WHERE status!='OFFLINE' AND (last_heartbeat + ttl_secs) <= ?1",
                params![now],
            )
            .map_err(|_| StoreError::Storage)?;
        Ok(changed as u64)
    }

    pub fn delete(&mut self, peer_id: &str) -> Result<bool, StoreError> {
        let changed = self
            .conn
            .execute("DELETE FROM nodes WHERE peer_id=?1", params![peer_id])
            .map_err(|_| StoreError::Storage)?;
        Ok(changed > 0)
    }
}

fn load_row(conn: &Connection, peer_id: &str) -> Result<Option<Row>, StoreError> {
    conn.query_row(
        "SELECT peer_id, network_id, chain_id, p2p_multiaddr, rpc_public_url, service_url,
                node_type, status, last_heartbeat, registered_at, software_version,
                public_key_hex, sequence, ttl_secs, primary_rpc, produces_blocks
         FROM nodes WHERE peer_id=?1",
        params![peer_id],
        map_row,
    )
    .optional()
    .map_err(|_| StoreError::Storage)
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let ttl = row.get::<_, i64>(13)? as u64;
    Ok(Row {
        ttl_secs: ttl,
        record: NodeRecord {
            peer_id: row.get(0)?,
            network_id: row.get(1)?,
            chain_id: row.get::<_, i64>(2)? as u32,
            p2p_multiaddr: row.get(3)?,
            rpc_public_url: row.get(4)?,
            service_url: row.get(5)?,
            node_type: row.get(6)?,
            status: row.get(7)?,
            last_heartbeat: row.get::<_, i64>(8)? as u64,
            registered_at: row.get::<_, i64>(9)? as u64,
            software_version: row.get(10)?,
            public_key_hex: row.get(11)?,
            sequence: row.get::<_, i64>(12)? as u64,
            primary_rpc: row.get::<_, i64>(14)? != 0,
            produces_blocks: row.get::<_, i64>(15)? != 0,
        },
    })
}

fn finish(row: Row, now: u64) -> NodeRecord {
    let mut record = row.record;
    record.status = effective_status(&record.status, record.last_heartbeat, row.ttl_secs, now);
    record
}

fn effective_status(status: &str, last: u64, ttl: u64, now: u64) -> String {
    if status == "OFFLINE" {
        return "OFFLINE".to_string();
    }
    if ttl > 0 && now >= last.saturating_add(ttl) {
        "OFFLINE".to_string()
    } else {
        status.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(sequence: u64) -> RecordWrite {
        RecordWrite {
            peer_id: "12D3KooWTestPeer".into(),
            network_id: "rsc-public".into(),
            chain_id: 3,
            p2p_multiaddr: None,
            rpc_public_url: Some("https://rpc.reeskova-chain.xyz/rpc".into()),
            service_url: None,
            node_type: "full".into(),
            status: "ONLINE".into(),
            software_version: "0.1.0".into(),
            public_key_hex: "aa".into(),
            sequence,
            ttl_secs: 30,
            primary_rpc: true,
            produces_blocks: true,
        }
    }

    #[test]
    fn duplicate_peer_updates_and_stale_sequence_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = RegistryStore::open(&dir.path().join("nodes.sqlite")).unwrap();
        store.upsert(&sample(1), 1_000).unwrap();
        store.upsert(&sample(2), 1_010).unwrap();
        let err = store.upsert(&sample(2), 1_020).unwrap_err();
        match err {
            UpsertError::Stale(stale) => assert_eq!(stale.current, 2),
            other => panic!("unexpected {other:?}"),
        }
        let mut other = sample(3);
        other.public_key_hex = "bb".into();
        assert!(matches!(
            store.upsert(&other, 1_030).unwrap_err(),
            UpsertError::Identity
        ));
        let listed = store
            .list(
                &NodeFilter {
                    network_id: Some("rsc-public".into()),
                    chain_id: Some(3),
                    ..NodeFilter::default()
                },
                1_010,
            )
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].sequence, 2);
        assert!(listed[0].primary_rpc);
    }

    #[test]
    fn expired_heartbeat_is_offline_before_and_after_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = RegistryStore::open(&dir.path().join("nodes.sqlite")).unwrap();
        let mut rec = sample(1);
        rec.ttl_secs = 10;
        store.upsert(&rec, 100).unwrap();
        let seen = store.get(&rec.peer_id, 200).unwrap().unwrap();
        assert_eq!(seen.status, "OFFLINE");
        assert_eq!(store.expire(200).unwrap(), 1);
        let swept = store.get(&rec.peer_id, 200).unwrap().unwrap();
        assert_eq!(swept.status, "OFFLINE");
        store.upsert(&sample(4), 300).unwrap();
        assert!(store.delete(&rec.peer_id).unwrap());
        assert!(store.get(&rec.peer_id, 300).unwrap().is_none());
    }
}
