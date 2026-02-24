use crate::error::Result;
use crate::model::{LocalFileId, LocalNode, RemoteId, RemoteNode, SyncedRecord};
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle};
use std::path::Path;

/// In-memory snapshot of all key-value pairs in the store
#[derive(Debug, Clone)]
pub struct StoreSnapshot {
    remote: Vec<(Vec<u8>, Vec<u8>)>,
    local: Vec<(Vec<u8>, Vec<u8>)>,
    synced_by_local: Vec<(Vec<u8>, Vec<u8>)>,
    synced_by_remote: Vec<(Vec<u8>, Vec<u8>)>,
    local_children: Vec<(Vec<u8>, Vec<u8>)>,
    remote_children: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Persistent storage for the 3 trees using fjall.
///
/// Each tree uses appropriate keying:
/// - Remote: keyed by `RemoteId` (string bytes)
/// - Local: keyed by `LocalFileId` (16-byte binary)
/// - Synced: keyed by `LocalFileId` with reverse index by `RemoteId`
pub struct TreeStore {
    keyspace: Keyspace,
    remote: PartitionHandle,
    local: PartitionHandle,
    synced_by_local: PartitionHandle,
    synced_by_remote: PartitionHandle,
    local_children: PartitionHandle,
    remote_children: PartitionHandle,
}

impl TreeStore {
    /// Open or create a tree store at the given path
    ///
    /// # Errors
    /// Returns an error if the keyspace cannot be opened
    pub fn open(path: &Path) -> Result<Self> {
        tracing::info!(path = %path.display(), "💾 Opening tree store");
        let keyspace = Config::new(path).open()?;

        let remote = keyspace.open_partition("remote", PartitionCreateOptions::default())?;
        let local = keyspace.open_partition("local", PartitionCreateOptions::default())?;
        let synced_by_local =
            keyspace.open_partition("synced_by_local", PartitionCreateOptions::default())?;
        let synced_by_remote =
            keyspace.open_partition("synced_by_remote", PartitionCreateOptions::default())?;
        let local_children =
            keyspace.open_partition("local_children", PartitionCreateOptions::default())?;
        let remote_children =
            keyspace.open_partition("remote_children", PartitionCreateOptions::default())?;

        Ok(Self {
            keyspace,
            remote,
            local,
            synced_by_local,
            synced_by_remote,
            local_children,
            remote_children,
        })
    }

    // ==================== Remote Tree ====================

    /// Insert a node into the remote tree
    ///
    /// # Errors
    /// Returns an error if serialization or storage fails
    pub fn insert_remote_node(&self, node: &RemoteNode) -> Result<()> {
        if let Some(old_node) = self.get_remote_node(&node.id)?
            && let Some(old_parent) = &old_node.parent_id
        {
            let old_child_key = make_child_key_remote(old_parent, &old_node.name);
            self.remote_children.remove(old_child_key)?;
        }

        let key = node.id.as_str().as_bytes();
        let value = serde_json::to_vec(node)?;
        self.remote.insert(key, value)?;

        if let Some(parent_id) = &node.parent_id {
            let child_key = make_child_key_remote(parent_id, &node.name);
            self.remote_children
                .insert(child_key, node.id.as_str().as_bytes())?;
        }
        tracing::trace!(
            id = node.id.as_str(),
            name = &node.name,
            "💾 Inserted remote node"
        );
        Ok(())
    }

    /// Get a node from the remote tree by ID
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn get_remote_node(&self, id: &RemoteId) -> Result<Option<RemoteNode>> {
        let key = id.as_str().as_bytes();
        match self.remote.get(key)? {
            Some(bytes) => {
                let node: RemoteNode = serde_json::from_slice(&bytes)?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    /// Delete a node from the remote tree
    ///
    /// # Errors
    /// Returns an error if storage access fails
    pub fn delete_remote_node(&self, id: &RemoteId) -> Result<()> {
        if let Some(node) = self.get_remote_node(id)?
            && let Some(parent_id) = &node.parent_id
        {
            let child_key = make_child_key_remote(parent_id, &node.name);
            self.remote_children.remove(child_key)?;
        }
        self.remote.remove(id.as_str().as_bytes())?;
        tracing::trace!(id = id.as_str(), "💾 Deleted remote node");
        Ok(())
    }

    /// List children of a remote node
    ///
    /// # Errors
    /// Returns an error if storage access fails
    pub fn list_remote_children(&self, parent_id: &RemoteId) -> Result<Vec<RemoteNode>> {
        let prefix = format!("{}:", parent_id.as_str());
        let mut children = Vec::new();

        for item in self.remote_children.prefix(prefix.as_bytes()) {
            let (_, child_id_bytes) = item?;
            let child_id = String::from_utf8_lossy(&child_id_bytes);
            if let Some(node) = self.get_remote_node(&RemoteId::new(child_id.as_ref()))? {
                children.push(node);
            }
        }
        Ok(children)
    }

    /// List all nodes in the remote tree
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn list_all_remote(&self) -> Result<Vec<RemoteNode>> {
        let mut nodes = Vec::new();
        for item in self.remote.iter() {
            let (_, value) = item?;
            let node: RemoteNode = serde_json::from_slice(&value)?;
            nodes.push(node);
        }
        Ok(nodes)
    }

    // ==================== Local Tree ====================

    /// Insert a node into the local tree (keyed by `LocalFileId`)
    ///
    /// # Errors
    /// Returns an error if serialization or storage fails
    pub fn insert_local_node(&self, node: &LocalNode) -> Result<()> {
        if let Some(old_node) = self.get_local_node(&node.id)?
            && let Some(old_parent) = &old_node.parent_id
        {
            let old_child_key = make_child_key_local(old_parent, &old_node.name);
            self.local_children.remove(old_child_key)?;
        }

        let key = node.id.to_bytes();
        let value = serde_json::to_vec(node)?;
        self.local.insert(key, value)?;

        if let Some(parent_id) = &node.parent_id {
            let child_key = make_child_key_local(parent_id, &node.name);
            self.local_children.insert(child_key, key)?;
        }
        tracing::trace!(name = &node.name, "💾 Inserted local node");
        Ok(())
    }

    /// Get a node from the local tree by `LocalFileId`
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn get_local_node(&self, id: &LocalFileId) -> Result<Option<LocalNode>> {
        let key = id.to_bytes();
        match self.local.get(key)? {
            Some(bytes) => {
                let node: LocalNode = serde_json::from_slice(&bytes)?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    /// Delete a node from the local tree
    ///
    /// # Errors
    /// Returns an error if storage access fails
    pub fn delete_local_node(&self, id: &LocalFileId) -> Result<()> {
        if let Some(node) = self.get_local_node(id)?
            && let Some(parent_id) = &node.parent_id
        {
            let child_key = make_child_key_local(parent_id, &node.name);
            self.local_children.remove(child_key)?;
        }
        self.local.remove(id.to_bytes())?;
        tracing::trace!("💾 Deleted local node");
        Ok(())
    }

    /// List children of a local node
    ///
    /// # Errors
    /// Returns an error if storage access fails
    pub fn list_local_children(&self, parent_id: &LocalFileId) -> Result<Vec<LocalNode>> {
        let prefix = parent_id.to_bytes();
        let mut children = Vec::new();

        for item in self.local_children.prefix(prefix) {
            let (_, child_id_bytes) = item?;
            if let Ok(bytes) = <[u8; 16]>::try_from(&child_id_bytes[..]) {
                let child_id = LocalFileId::from_bytes(&bytes);
                if let Some(node) = self.get_local_node(&child_id)? {
                    children.push(node);
                }
            }
        }
        Ok(children)
    }

    /// List all nodes in the local tree
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn list_all_local(&self) -> Result<Vec<LocalNode>> {
        let mut nodes = Vec::new();
        for item in self.local.iter() {
            let (_, value) = item?;
            let node: LocalNode = serde_json::from_slice(&value)?;
            nodes.push(node);
        }
        Ok(nodes)
    }

    // ==================== Synced Tree ====================

    /// Insert a synced record (updates both indices)
    ///
    /// # Errors
    /// Returns an error if serialization or storage fails
    pub fn insert_synced(&self, record: &SyncedRecord) -> Result<()> {
        let local_key = record.local_id.to_bytes();
        let remote_key = record.remote_id.as_str().as_bytes();
        let value = serde_json::to_vec(record)?;

        self.synced_by_local.insert(local_key, &value)?;
        self.synced_by_remote.insert(remote_key, local_key)?;
        tracing::trace!(rel_path = &record.rel_path, "💾 Inserted synced record");
        Ok(())
    }

    /// Get a synced record by local ID
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn get_synced_by_local(&self, local_id: &LocalFileId) -> Result<Option<SyncedRecord>> {
        let key = local_id.to_bytes();
        match self.synced_by_local.get(key)? {
            Some(bytes) => {
                let record: SyncedRecord = serde_json::from_slice(&bytes)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Get a synced record by remote ID
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn get_synced_by_remote(&self, remote_id: &RemoteId) -> Result<Option<SyncedRecord>> {
        let key = remote_id.as_str().as_bytes();
        let Some(local_id_bytes) = self.synced_by_remote.get(key)? else {
            return Ok(None);
        };
        let Ok(bytes) = <[u8; 16]>::try_from(&local_id_bytes[..]) else {
            return Ok(None);
        };
        let local_id = LocalFileId::from_bytes(&bytes);
        self.get_synced_by_local(&local_id)
    }

    /// Delete a synced record
    ///
    /// # Errors
    /// Returns an error if storage access fails
    pub fn delete_synced(&self, local_id: &LocalFileId) -> Result<()> {
        if let Some(record) = self.get_synced_by_local(local_id)? {
            self.synced_by_remote
                .remove(record.remote_id.as_str().as_bytes())?;
        }
        self.synced_by_local.remove(local_id.to_bytes())?;
        tracing::trace!("💾 Deleted synced record");
        Ok(())
    }

    /// List all synced records
    ///
    /// # Errors
    /// Returns an error if deserialization or storage access fails
    pub fn list_all_synced(&self) -> Result<Vec<SyncedRecord>> {
        let mut records = Vec::new();
        for item in self.synced_by_local.iter() {
            let (_, value) = item?;
            let record: SyncedRecord = serde_json::from_slice(&value)?;
            records.push(record);
        }
        Ok(records)
    }

    /// Flush all pending writes to disk
    ///
    /// # Errors
    /// Returns an error if persistence fails
    pub fn flush(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        tracing::debug!("💾 Store flushed to disk");
        Ok(())
    }

    /// Take an in-memory snapshot of all key-value pairs in all partitions
    ///
    /// # Errors
    /// Returns an error if reading from any partition fails
    pub fn snapshot(&self) -> Result<StoreSnapshot> {
        Ok(StoreSnapshot {
            remote: snapshot_partition(&self.remote)?,
            local: snapshot_partition(&self.local)?,
            synced_by_local: snapshot_partition(&self.synced_by_local)?,
            synced_by_remote: snapshot_partition(&self.synced_by_remote)?,
            local_children: snapshot_partition(&self.local_children)?,
            remote_children: snapshot_partition(&self.remote_children)?,
        })
    }

    /// Restore all partitions from a previously taken snapshot, clearing
    /// any data added after the snapshot was taken.
    ///
    /// # Errors
    /// Returns an error if clearing or restoring any partition fails
    pub fn restore(&self, snapshot: &StoreSnapshot) -> Result<()> {
        restore_partition(&self.remote, &snapshot.remote)?;
        restore_partition(&self.local, &snapshot.local)?;
        restore_partition(&self.synced_by_local, &snapshot.synced_by_local)?;
        restore_partition(&self.synced_by_remote, &snapshot.synced_by_remote)?;
        restore_partition(&self.local_children, &snapshot.local_children)?;
        restore_partition(&self.remote_children, &snapshot.remote_children)?;
        tracing::debug!("💾 Store restored from snapshot");
        Ok(())
    }
}

fn snapshot_partition(partition: &PartitionHandle) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut entries = Vec::new();
    for item in partition.iter() {
        let (key, value) = item?;
        entries.push((key.to_vec(), value.to_vec()));
    }
    Ok(entries)
}

fn restore_partition(partition: &PartitionHandle, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<()> {
    // Collect existing keys to remove
    let existing_keys: Vec<Vec<u8>> = partition
        .iter()
        .filter_map(|item| item.ok().map(|(k, _)| k.to_vec()))
        .collect();
    for key in existing_keys {
        partition.remove(&key)?;
    }
    // Re-insert snapshot entries
    for (key, value) in entries {
        partition.insert(key, value)?;
    }
    Ok(())
}

fn make_child_key_local(parent_id: &LocalFileId, name: &str) -> Vec<u8> {
    let mut key = parent_id.to_bytes().to_vec();
    key.push(b':');
    key.extend_from_slice(name.as_bytes());
    key
}

fn make_child_key_remote(parent_id: &RemoteId, name: &str) -> String {
    format!("{}:{}", parent_id.as_str(), name)
}
