//! Checkpoints de RocksDB con manifiesto. No son vistas de lectura en memoria.

use crate::database::{open_read_only, read_identity, reject_parent_components};
use crate::{DatabaseConfig, RscDatabase, StorageError, STORAGE_SCHEMA_VERSION};
use rocksdb::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MANIFEST_FILE: &str = "manifest.json";
const MAX_SNAPSHOT_ID_LEN: usize = 128;
static SNAPSHOT_SEQ: AtomicU64 = AtomicU64::new(1);

/// Metadatos operativos de un checkpoint. No son consenso.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotMetadata {
    /// Altura observada al crear el checkpoint.
    pub height: u64,
    /// Hash de bloque, bytes opacos.
    pub block_hash: Vec<u8>,
    /// Raíz de estado, bytes opacos.
    pub state_root: Vec<u8>,
}

/// Manifiesto publicado junto al checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Identificador del directorio.
    pub snapshot_id: String,
    /// [`STORAGE_SCHEMA_VERSION`] de la base.
    pub schema_version: u32,
    /// Red de la base.
    pub network_id: String,
    /// Cadena de la base.
    pub chain_id: String,
    /// Reloj de pared al publicar. No es tiempo de consenso.
    pub created_at_unix_secs: u64,
    /// Altura copiada de [`SnapshotMetadata`].
    pub height: u64,
    /// Hash copiado de [`SnapshotMetadata`].
    pub block_hash: Vec<u8>,
    /// Raíz copiada de [`SnapshotMetadata`].
    pub state_root: Vec<u8>,
}

/// Checkpoint publicado: manifiesto y directorio final.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Manifiesto validado.
    pub manifest: SnapshotManifest,
    /// Directorio final del checkpoint.
    pub path: PathBuf,
}

/// Fallo de snapshot. No sustituye a [`StorageError`].
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// La capa física rechazó la operación.
    #[error("snapshot storage error")]
    Storage(#[from] StorageError),
    /// El motor de checkpoint falló.
    #[error("snapshot backend error")]
    Backend(#[source] Box<rocksdb::Error>),
    /// Identificador vacío, relativo o con caracteres no permitidos.
    #[error("snapshot id rejected")]
    InvalidId,
    /// Ya existe un snapshot con ese identificador.
    #[error("snapshot id already exists")]
    DuplicateId,
    /// El manifiesto no es JSON válido o no coincide con el checkpoint.
    #[error("snapshot manifest rejected")]
    MalformedManifest,
    /// No hay un directorio público con ese identificador.
    #[error("snapshot not found")]
    NotFound,
    /// La ruta se solapa con la base viva o con la raíz de snapshots.
    #[error("snapshot path overlaps")]
    PathOverlap,
    /// El destino de restore ya existe.
    #[error("snapshot restore target exists")]
    TargetExists,
    /// El directorio no abre como checkpoint RocksDB válido.
    #[error("snapshot checkpoint rejected")]
    CheckpointInvalid,
    /// Fallo de sistema de archivos sin ruta en el mensaje.
    #[error("snapshot io error")]
    Io,
}

/// Crea, lista, valida y restaura checkpoints de una base viva.
pub struct SnapshotManager {
    db: RscDatabase,
    root: PathBuf,
}

impl SnapshotManager {
    /// `root` no puede solaparse con la base viva.
    pub fn new(db: RscDatabase, root: impl Into<PathBuf>) -> Result<Self, SnapshotError> {
        let root = root.into();
        reject_parent_components(&root)?;
        if paths_overlap(db.path(), &root) {
            return Err(SnapshotError::PathOverlap);
        }
        fs::create_dir_all(&root).map_err(|_| SnapshotError::Io)?;
        Ok(Self { db, root })
    }

    /// Identificador único de reloj de pared más contador. No es un hash de bloque.
    pub fn create_snapshot(&self, metadata: SnapshotMetadata) -> Result<Snapshot, SnapshotError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let seq = SNAPSHOT_SEQ.fetch_add(1, Ordering::Relaxed);
        self.create_snapshot_with_id(format!("snap-{nanos}-{seq}"), metadata)
    }

    /// Publica un checkpoint con `snapshot_id` explícito.
    pub fn create_snapshot_with_id(
        &self,
        snapshot_id: impl Into<String>,
        metadata: SnapshotMetadata,
    ) -> Result<Snapshot, SnapshotError> {
        let snapshot_id = snapshot_id.into();
        validate_snapshot_id(&snapshot_id)?;
        let final_path = self.root.join(&snapshot_id);
        if final_path.exists() {
            return Err(SnapshotError::DuplicateId);
        }
        let temp_path = self.root.join(format!(".tmp-{snapshot_id}"));
        if temp_path.exists() {
            fs::remove_dir_all(&temp_path).map_err(|_| SnapshotError::Io)?;
        }

        let checkpoint = Checkpoint::new(self.db.raw_db())
            .map_err(|error| SnapshotError::Backend(Box::new(error)))?;
        checkpoint
            .create_checkpoint(&temp_path)
            .map_err(|error| SnapshotError::Backend(Box::new(error)))?;

        let (schema_version, network_id, chain_id) = read_identity(self.db.raw_db())?;
        let manifest = SnapshotManifest {
            snapshot_id: snapshot_id.clone(),
            schema_version,
            network_id,
            chain_id,
            created_at_unix_secs: unix_secs(),
            height: metadata.height,
            block_hash: metadata.block_hash,
            state_root: metadata.state_root,
        };
        write_manifest(&temp_path.join(MANIFEST_FILE), &manifest)?;
        validate_checkpoint_directory(&temp_path, &manifest, self.db.config())?;
        fs::rename(&temp_path, &final_path).map_err(|_| SnapshotError::Io)?;
        Ok(Snapshot {
            manifest,
            path: final_path,
        })
    }

    /// Snapshots públicos. Los directorios temporales no entran.
    pub fn list_snapshots(&self) -> Result<Vec<Snapshot>, SnapshotError> {
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|_| SnapshotError::Io)? {
            let entry = entry.map_err(|_| SnapshotError::Io)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if is_temp_name(name) || !entry.path().is_dir() {
                continue;
            }
            snapshots.push(self.open_snapshot(name)?);
        }
        snapshots.sort_by(|left, right| left.manifest.snapshot_id.cmp(&right.manifest.snapshot_id));
        Ok(snapshots)
    }

    /// Lee el manifiesto y comprueba que el directorio es un checkpoint real.
    pub fn open_snapshot(&self, snapshot_id: &str) -> Result<Snapshot, SnapshotError> {
        validate_snapshot_id(snapshot_id)?;
        let path = self.root.join(snapshot_id);
        if !path.is_dir() {
            return Err(SnapshotError::NotFound);
        }
        let manifest = read_manifest(&path.join(MANIFEST_FILE))?;
        validate_checkpoint_directory(&path, &manifest, self.db.config())?;
        Ok(Snapshot { manifest, path })
    }

    /// Misma validación que [`Self::open_snapshot`].
    pub fn validate_snapshot(&self, snapshot_id: &str) -> Result<Snapshot, SnapshotError> {
        self.open_snapshot(snapshot_id)
    }

    /// Copia el checkpoint a un temporal, lo reabre y solo entonces lo publica en `target_path`.
    pub fn restore_snapshot(
        &self,
        snapshot_id: &str,
        target_path: impl AsRef<Path>,
    ) -> Result<(), SnapshotError> {
        let target = target_path.as_ref();
        reject_parent_components(target)?;
        if target.exists() {
            return Err(SnapshotError::TargetExists);
        }
        if paths_overlap(target, self.db.path()) || paths_overlap(target, &self.root) {
            return Err(SnapshotError::PathOverlap);
        }
        let snapshot = self.open_snapshot(snapshot_id)?;
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|_| SnapshotError::Io)?;
        let temp = parent.join(format!(".tmp-restore-{snapshot_id}"));
        if temp.exists() {
            fs::remove_dir_all(&temp).map_err(|_| SnapshotError::Io)?;
        }
        copy_dir(&snapshot.path, &temp)?;
        validate_checkpoint_directory(&temp, &snapshot.manifest, self.db.config())?;
        fs::rename(&temp, target).map_err(|_| SnapshotError::Io)?;
        Ok(())
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn validate_snapshot_id(snapshot_id: &str) -> Result<(), SnapshotError> {
    if snapshot_id.is_empty()
        || snapshot_id == "."
        || snapshot_id == ".."
        || snapshot_id.len() > MAX_SNAPSHOT_ID_LEN
        || snapshot_id.starts_with('.')
        || !snapshot_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(SnapshotError::InvalidId);
    }
    Ok(())
}

fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') || name.starts_with("tmp-")
}

fn write_manifest(path: &Path, manifest: &SnapshotManifest) -> Result<(), SnapshotError> {
    let bytes =
        serde_json::to_vec_pretty(manifest).map_err(|_| SnapshotError::MalformedManifest)?;
    let mut file = File::create(path).map_err(|_| SnapshotError::Io)?;
    file.write_all(&bytes).map_err(|_| SnapshotError::Io)?;
    file.sync_all().map_err(|_| SnapshotError::Io)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<SnapshotManifest, SnapshotError> {
    let bytes = fs::read(path).map_err(|_| SnapshotError::MalformedManifest)?;
    serde_json::from_slice(&bytes).map_err(|_| SnapshotError::MalformedManifest)
}

fn validate_checkpoint_directory(
    path: &Path,
    manifest: &SnapshotManifest,
    config: &DatabaseConfig,
) -> Result<(), SnapshotError> {
    if manifest.schema_version != STORAGE_SCHEMA_VERSION {
        return Err(SnapshotError::Storage(StorageError::SchemaMismatch));
    }
    if manifest.network_id != config.network_id {
        return Err(SnapshotError::Storage(StorageError::NetworkMismatch));
    }
    if manifest.chain_id != config.chain_id {
        return Err(SnapshotError::Storage(StorageError::ChainMismatch));
    }
    validate_snapshot_id(&manifest.snapshot_id)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(SnapshotError::CheckpointInvalid)?;
    let public_name = file_name
        .strip_prefix(".tmp-restore-")
        .or_else(|| file_name.strip_prefix(".tmp-"))
        .unwrap_or(file_name);
    if public_name != manifest.snapshot_id {
        return Err(SnapshotError::MalformedManifest);
    }
    let db = open_read_only(path).map_err(|_| SnapshotError::CheckpointInvalid)?;
    let (schema, network, chain) =
        read_identity(&db).map_err(|_| SnapshotError::CheckpointInvalid)?;
    if schema != manifest.schema_version
        || network != manifest.network_id
        || chain != manifest.chain_id
    {
        return Err(SnapshotError::MalformedManifest);
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = normalize(left);
    let right = normalize(right);
    left == right || left.starts_with(&right) || right.starts_with(&left)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), SnapshotError> {
    fs::create_dir_all(to).map_err(|_| SnapshotError::Io)?;
    for entry in fs::read_dir(from).map_err(|_| SnapshotError::Io)? {
        let entry = entry.map_err(|_| SnapshotError::Io)?;
        let target = to.join(entry.file_name());
        let file_type = entry.file_type().map_err(|_| SnapshotError::Io)?;
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|_| SnapshotError::Io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseConfig, RscDatabase, StorageError};

    fn config() -> DatabaseConfig {
        DatabaseConfig {
            max_value_size: 1024,
            network_id: "devnet".to_string(),
            chain_id: "rsc-dev".to_string(),
        }
    }

    fn metadata() -> SnapshotMetadata {
        SnapshotMetadata {
            height: 7,
            block_hash: b"block".to_vec(),
            state_root: b"state".to_vec(),
        }
    }

    fn manager() -> (tempfile::TempDir, SnapshotManager) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("live");
        let snap_root = dir.path().join("snaps");
        let db = RscDatabase::open(&db_path, config()).unwrap();
        db.put(crate::ColumnFamily::Blocks, b"k", b"v").unwrap();
        let manager = SnapshotManager::new(db, snap_root).unwrap();
        (dir, manager)
    }

    #[test]
    fn create_snapshot_returns_snapshot() {
        let (_dir, manager) = manager();
        let snapshot = manager.create_snapshot(metadata()).unwrap();
        assert_eq!(snapshot.manifest.height, 7);
        assert!(snapshot.path.is_dir());
    }

    #[test]
    fn open_snapshot_returns_same_manifest_identity() {
        let (_dir, manager) = manager();
        let created = manager
            .create_snapshot_with_id("snap-a", metadata())
            .unwrap();
        let opened = manager.open_snapshot("snap-a").unwrap();
        assert_eq!(opened.manifest, created.manifest);
        assert_eq!(opened.path, created.path);
    }

    #[test]
    fn snapshot_manifest_id_matches_directory() {
        let (_dir, manager) = manager();
        let snapshot = manager
            .create_snapshot_with_id("snap-b", metadata())
            .unwrap();
        assert_eq!(
            snapshot.path.file_name().unwrap(),
            snapshot.manifest.snapshot_id.as_str()
        );
    }

    #[test]
    fn snapshot_schema_version_is_v1() {
        let (_dir, manager) = manager();
        let snapshot = manager.create_snapshot(metadata()).unwrap();
        assert_eq!(snapshot.manifest.schema_version, STORAGE_SCHEMA_VERSION);
    }

    #[test]
    fn snapshot_network_identity_is_validated() {
        let (_dir, manager) = manager();
        let snapshot = manager
            .create_snapshot_with_id("snap-net", metadata())
            .unwrap();
        let manifest_path = snapshot.path.join(MANIFEST_FILE);
        let mut manifest = read_manifest(&manifest_path).unwrap();
        manifest.network_id = "other".to_string();
        write_manifest(&manifest_path, &manifest).unwrap();
        let error = manager.validate_snapshot("snap-net").unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::Storage(StorageError::NetworkMismatch)
        ));
    }

    #[test]
    fn snapshot_chain_identity_is_validated() {
        let (_dir, manager) = manager();
        let snapshot = manager
            .create_snapshot_with_id("snap-chain", metadata())
            .unwrap();
        let manifest_path = snapshot.path.join(MANIFEST_FILE);
        let mut manifest = read_manifest(&manifest_path).unwrap();
        manifest.chain_id = "other-chain".to_string();
        write_manifest(&manifest_path, &manifest).unwrap();
        let error = manager.validate_snapshot("snap-chain").unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::Storage(StorageError::ChainMismatch)
        ));
    }

    #[test]
    fn malformed_manifest_is_rejected() {
        let (_dir, manager) = manager();
        let snapshot = manager
            .create_snapshot_with_id("snap-bad", metadata())
            .unwrap();
        fs::write(snapshot.path.join(MANIFEST_FILE), b"{not-json").unwrap();
        let error = manager.open_snapshot("snap-bad").unwrap_err();
        assert!(matches!(error, SnapshotError::MalformedManifest));
    }

    #[test]
    fn duplicate_snapshot_id_is_rejected() {
        let (_dir, manager) = manager();
        manager
            .create_snapshot_with_id("snap-dup", metadata())
            .unwrap();
        let error = manager
            .create_snapshot_with_id("snap-dup", metadata())
            .unwrap_err();
        assert!(matches!(error, SnapshotError::DuplicateId));
    }

    #[test]
    fn invalid_snapshot_id_is_rejected() {
        let (_dir, manager) = manager();
        for id in ["", ".", "..", "a/b", "a\\b", ".hidden", "has space"] {
            let error = manager.create_snapshot_with_id(id, metadata()).unwrap_err();
            assert!(matches!(error, SnapshotError::InvalidId), "{id}");
        }
    }

    #[test]
    fn restore_result_reopens_successfully() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("live");
        let snap_root = dir.path().join("snaps");
        let target = dir.path().join("restored");
        let db = RscDatabase::open(&db_path, config()).unwrap();
        db.put(crate::ColumnFamily::Blocks, b"k", b"v").unwrap();
        let manager = SnapshotManager::new(db, snap_root).unwrap();
        manager
            .create_snapshot_with_id("snap-restore", metadata())
            .unwrap();
        manager.restore_snapshot("snap-restore", &target).unwrap();
        let restored = RscDatabase::open(&target, config()).unwrap();
        assert_eq!(
            restored
                .get(crate::ColumnFamily::Blocks, b"k")
                .unwrap()
                .unwrap()
                .as_bytes(),
            b"v"
        );
    }

    #[test]
    fn snapshot_temp_directories_are_not_public_snapshots() {
        let (_dir, manager) = manager();
        fs::create_dir_all(manager.root.join(".tmp-hidden")).unwrap();
        fs::create_dir_all(manager.root.join("tmp-hidden")).unwrap();
        manager
            .create_snapshot_with_id("snap-public", metadata())
            .unwrap();
        let listed = manager.list_snapshots().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.snapshot_id, "snap-public");
    }

    #[test]
    fn restore_path_security() {
        let (dir, manager) = manager();
        manager
            .create_snapshot_with_id("snap-sec", metadata())
            .unwrap();
        let nested = dir.path().join("live").join("nested");
        let error = manager.restore_snapshot("snap-sec", &nested).unwrap_err();
        assert!(matches!(error, SnapshotError::PathOverlap));
        let escape = dir.path().join("..").join("escape");
        let error = manager.restore_snapshot("snap-sec", &escape).unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::Storage(StorageError::InvalidPath)
        ));
    }
}
