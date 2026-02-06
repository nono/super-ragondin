use super::mock_fs::MockFs;
use super::mock_remote::MockRemote;
use crate::model::{LocalFileId, LocalNode, NodeType, RemoteId, RemoteNode, SyncedRecord};
use crate::planner::Planner;
use crate::store::TreeStore;
use md5::{Digest, Md5};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counter for generating unique local file IDs in simulation
static INODE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_local_id() -> LocalFileId {
    LocalFileId::new(1, INODE_COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn compute_md5(content: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

/// Simulation runner that orchestrates mock local fs, mock remote, and the real planner/store
pub struct SimulationRunner {
    pub local_fs: MockFs,
    pub remote: MockRemote,
    pub store: TreeStore,
    pub sync_root: PathBuf,
    pub last_seq: u64,
    /// Maps `RemoteId` -> `LocalFileId` for synced files
    remote_to_local: std::collections::HashMap<RemoteId, LocalFileId>,
}

/// Actions that can be simulated
#[derive(Debug, Clone)]
pub enum SimAction {
    LocalCreateFile {
        local_id: LocalFileId,
        parent_local_id: Option<LocalFileId>,
        name: String,
        content: Vec<u8>,
    },
    LocalCreateDir {
        local_id: LocalFileId,
        parent_local_id: Option<LocalFileId>,
        name: String,
    },
    LocalDeleteFile {
        local_id: LocalFileId,
    },
    LocalModifyFile {
        local_id: LocalFileId,
        content: Vec<u8>,
    },
    RemoteCreateFile {
        id: RemoteId,
        parent_id: RemoteId,
        name: String,
        content: Vec<u8>,
    },
    RemoteCreateDir {
        id: RemoteId,
        parent_id: Option<RemoteId>,
        name: String,
    },
    RemoteDeleteFile {
        id: RemoteId,
    },
    RemoteModifyFile {
        id: RemoteId,
        content: Vec<u8>,
    },
    Sync,
}

impl SimulationRunner {
    #[must_use]
    pub fn new(store: TreeStore, sync_root: PathBuf) -> Self {
        Self {
            local_fs: MockFs::new(),
            remote: MockRemote::new(),
            store,
            sync_root,
            last_seq: 0,
            remote_to_local: std::collections::HashMap::new(),
        }
    }

    /// Apply a simulation action
    ///
    /// # Errors
    /// Returns an error if the action fails
    #[allow(clippy::too_many_lines)]
    pub fn apply(&mut self, action: SimAction) -> Result<(), String> {
        match action {
            SimAction::LocalCreateFile {
                local_id,
                parent_local_id,
                name,
                content,
            } => {
                let md5sum = compute_md5(&content);
                let node = LocalNode {
                    id: local_id.clone(),
                    parent_id: parent_local_id,
                    name,
                    node_type: NodeType::File,
                    md5sum: Some(md5sum),
                    size: Some(content.len() as u64),
                    mtime: 1000,
                };
                self.local_fs.create_file(local_id, node.clone(), content);
                self.store
                    .insert_local_node(&node)
                    .map_err(|e| e.to_string())?;
            }
            SimAction::LocalCreateDir {
                local_id,
                parent_local_id,
                name,
            } => {
                let node = LocalNode {
                    id: local_id.clone(),
                    parent_id: parent_local_id,
                    name,
                    node_type: NodeType::Directory,
                    md5sum: None,
                    size: None,
                    mtime: 1000,
                };
                self.local_fs.create_dir(local_id, node.clone());
                self.store
                    .insert_local_node(&node)
                    .map_err(|e| e.to_string())?;
            }
            SimAction::LocalDeleteFile { local_id } => {
                self.local_fs.delete(&local_id);
                self.store
                    .delete_local_node(&local_id)
                    .map_err(|e| e.to_string())?;
            }
            SimAction::LocalModifyFile { local_id, content } => {
                let node = self
                    .local_fs
                    .get_node(&local_id)
                    .cloned()
                    .ok_or_else(|| format!("LocalModifyFile: node {local_id:?} not found"))?;
                let md5sum = compute_md5(&content);
                let mut updated = node;
                updated.md5sum = Some(md5sum);
                updated.size = Some(content.len() as u64);
                updated.mtime += 1;
                self.local_fs.create_file(local_id, updated.clone(), content);
                self.store
                    .insert_local_node(&updated)
                    .map_err(|e| e.to_string())?;
            }
            SimAction::RemoteCreateFile {
                id,
                parent_id,
                name,
                content,
            } => {
                let md5sum = compute_md5(&content);
                let node = RemoteNode {
                    id: id.clone(),
                    parent_id: Some(parent_id),
                    name,
                    node_type: NodeType::File,
                    md5sum: Some(md5sum),
                    size: Some(content.len() as u64),
                    updated_at: 1000,
                    rev: format!("1-{}", id.as_str()),
                };
                self.remote.add_node(node, Some(content));
            }
            SimAction::RemoteCreateDir {
                id,
                parent_id,
                name,
            } => {
                let node = RemoteNode {
                    id: id.clone(),
                    parent_id,
                    name,
                    node_type: NodeType::Directory,
                    md5sum: None,
                    size: None,
                    updated_at: 1000,
                    rev: format!("1-{}", id.as_str()),
                };
                self.remote.add_node(node, None);
            }
            SimAction::RemoteDeleteFile { id } => {
                self.remote.delete_node(&id);
            }
            SimAction::RemoteModifyFile { id, content } => {
                let node =
                    self.remote.get_node(&id).cloned().ok_or_else(|| {
                        format!("RemoteModifyFile: node {} not found", id.as_str())
                    })?;
                let md5sum = compute_md5(&content);
                let mut updated = node;
                updated.md5sum = Some(md5sum);
                updated.size = Some(content.len() as u64);
                updated.updated_at += 1;
                // Increment revision
                let rev_num: u32 = updated
                    .rev
                    .split('-')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                updated.rev = format!("{}-{}", rev_num + 1, id.as_str());
                self.remote.add_node(updated, Some(content));
            }
            SimAction::Sync => {
                self.sync()?;
            }
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<(), String> {
        // Step 1: Fetch remote changes into store (including deletions)
        for change in self.remote.get_all_changes_since(self.last_seq) {
            if change.deleted {
                // Handle remote deletion
                self.store
                    .delete_remote_node(&change.remote_id)
                    .map_err(|e| e.to_string())?;
            } else if let Some(node) = self.remote.get_node(&change.remote_id) {
                self.store
                    .insert_remote_node(node)
                    .map_err(|e| e.to_string())?;

                // Handle root directory specially - create binding immediately
                if node.parent_id.is_none() && !self.remote_to_local.contains_key(&node.id) {
                    let local_id = next_local_id();
                    let local_node = LocalNode {
                        id: local_id.clone(),
                        parent_id: None,
                        name: node.name.clone(),
                        node_type: NodeType::Directory,
                        md5sum: None,
                        size: None,
                        mtime: node.updated_at,
                    };
                    self.local_fs
                        .create_dir(local_id.clone(), local_node.clone());
                    self.store
                        .insert_local_node(&local_node)
                        .map_err(|e| e.to_string())?;

                    let synced = SyncedRecord {
                        local_id: local_id.clone(),
                        remote_id: node.id.clone(),
                        rel_path: String::new(),
                        md5sum: None,
                        size: None,
                        rev: node.rev.clone(),
                        node_type: NodeType::Directory,
                    };
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                    self.remote_to_local.insert(node.id.clone(), local_id);
                }
            }
        }
        self.last_seq = self.remote.current_seq();

        // Step 2: Plan
        let planner = Planner::new(&self.store, self.sync_root.clone());
        let results = planner.plan().map_err(|e| e.to_string())?;

        // Step 3: Execute operations
        for result in results {
            if let crate::model::PlanResult::Op(op) = result {
                self.execute_op(op)?;
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn execute_op(&mut self, op: crate::model::SyncOp) -> Result<(), String> {
        use crate::model::SyncOp;

        match op {
            SyncOp::DownloadNew {
                remote_id,
                local_path: _,
                expected_rev: _,
                expected_md5: _,
            } => {
                if let Some(remote_node) = self.remote.get_node(&remote_id).cloned() {
                    let local_id = next_local_id();
                    let content = self
                        .remote
                        .get_content(&remote_id)
                        .cloned()
                        .unwrap_or_default();

                    let local_node = LocalNode {
                        id: local_id.clone(),
                        parent_id: remote_node
                            .parent_id
                            .as_ref()
                            .and_then(|pid| self.remote_to_local.get(pid).cloned()),
                        name: remote_node.name.clone(),
                        node_type: remote_node.node_type,
                        md5sum: remote_node.md5sum.clone(),
                        size: remote_node.size,
                        mtime: remote_node.updated_at,
                    };

                    self.local_fs
                        .create_file(local_id.clone(), local_node.clone(), content);
                    self.store
                        .insert_local_node(&local_node)
                        .map_err(|e| e.to_string())?;

                    // Create synced record
                    let synced = SyncedRecord {
                        local_id: local_id.clone(),
                        remote_id: remote_id.clone(),
                        rel_path: remote_node.name.clone(),
                        md5sum: remote_node.md5sum.clone(),
                        size: remote_node.size,
                        rev: remote_node.rev.clone(),
                        node_type: remote_node.node_type,
                    };
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                    self.remote_to_local.insert(remote_id, local_id);
                }
            }
            SyncOp::CreateLocalDir {
                remote_id,
                local_path: _,
            } => {
                if let Some(remote_node) = self.remote.get_node(&remote_id).cloned() {
                    let local_id = next_local_id();

                    let local_node = LocalNode {
                        id: local_id.clone(),
                        parent_id: remote_node
                            .parent_id
                            .as_ref()
                            .and_then(|pid| self.remote_to_local.get(pid).cloned()),
                        name: remote_node.name.clone(),
                        node_type: NodeType::Directory,
                        md5sum: None,
                        size: None,
                        mtime: remote_node.updated_at,
                    };

                    self.local_fs
                        .create_dir(local_id.clone(), local_node.clone());
                    self.store
                        .insert_local_node(&local_node)
                        .map_err(|e| e.to_string())?;

                    let synced = SyncedRecord {
                        local_id: local_id.clone(),
                        remote_id: remote_id.clone(),
                        rel_path: remote_node.name,
                        md5sum: None,
                        size: None,
                        rev: remote_node.rev,
                        node_type: NodeType::Directory,
                    };
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                    self.remote_to_local.insert(remote_id, local_id);
                }
            }
            SyncOp::UploadNew {
                local_id,
                local_path: _,
                parent_remote_id,
                name,
                expected_md5,
            } => {
                if let Some(local_node) = self.local_fs.get_node(&local_id).cloned() {
                    let content = self
                        .local_fs
                        .read_file(&local_id)
                        .cloned()
                        .unwrap_or_default();
                    let remote_id =
                        RemoteId::new(format!("remote-{}-{}", local_id.device_id, local_id.inode));

                    let remote_node = RemoteNode {
                        id: remote_id.clone(),
                        parent_id: Some(parent_remote_id),
                        name,
                        node_type: local_node.node_type,
                        md5sum: Some(expected_md5.clone()),
                        size: local_node.size,
                        updated_at: local_node.mtime,
                        rev: format!("1-{}", remote_id.as_str()),
                    };

                    self.remote.add_node(remote_node.clone(), Some(content));
                    self.store
                        .insert_remote_node(&remote_node)
                        .map_err(|e| e.to_string())?;

                    let synced = SyncedRecord {
                        local_id: local_id.clone(),
                        remote_id: remote_id.clone(),
                        rel_path: local_node.name.clone(),
                        md5sum: Some(expected_md5),
                        size: local_node.size,
                        rev: remote_node.rev,
                        node_type: local_node.node_type,
                    };
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                    self.remote_to_local.insert(remote_id, local_id);
                }
            }
            SyncOp::CreateRemoteDir {
                local_id,
                local_path: _,
                parent_remote_id,
                name,
            } => {
                if let Some(local_node) = self.local_fs.get_node(&local_id).cloned() {
                    let remote_id =
                        RemoteId::new(format!("remote-{}-{}", local_id.device_id, local_id.inode));

                    let remote_node = RemoteNode {
                        id: remote_id.clone(),
                        parent_id: Some(parent_remote_id),
                        name,
                        node_type: NodeType::Directory,
                        md5sum: None,
                        size: None,
                        updated_at: local_node.mtime,
                        rev: format!("1-{}", remote_id.as_str()),
                    };

                    self.remote.add_node(remote_node.clone(), None);
                    self.store
                        .insert_remote_node(&remote_node)
                        .map_err(|e| e.to_string())?;

                    let synced = SyncedRecord {
                        local_id: local_id.clone(),
                        remote_id: remote_id.clone(),
                        rel_path: local_node.name,
                        md5sum: None,
                        size: None,
                        rev: remote_node.rev,
                        node_type: NodeType::Directory,
                    };
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                    self.remote_to_local.insert(remote_id, local_id);
                }
            }
            SyncOp::DeleteLocal {
                local_id,
                local_path: _,
                expected_md5: _,
            } => {
                self.local_fs.delete(&local_id);
                self.store
                    .delete_local_node(&local_id)
                    .map_err(|e| e.to_string())?;
                self.store
                    .delete_synced(&local_id)
                    .map_err(|e| e.to_string())?;
            }
            SyncOp::DeleteRemote {
                remote_id,
                expected_rev: _,
            } => {
                self.remote.delete_node(&remote_id);
                self.store
                    .delete_remote_node(&remote_id)
                    .map_err(|e| e.to_string())?;
                if let Some(local_id) = self.remote_to_local.remove(&remote_id) {
                    self.store
                        .delete_synced(&local_id)
                        .map_err(|e| e.to_string())?;
                }
            }
            _ => {} // Skip move operations for now
        }
        Ok(())
    }

    /// Check invariant: after sync, local and remote should have same files (by content)
    ///
    /// # Errors
    /// Returns an error describing the convergence mismatch
    pub fn check_convergence(&self) -> Result<(), String> {
        let local_nodes = self.local_fs.list_all();
        let local_by_md5: HashSet<_> = local_nodes
            .iter()
            .filter(|n| n.node_type == NodeType::File)
            .filter_map(|n| n.md5sum.as_ref())
            .collect();

        let remote_by_md5: HashSet<_> = self
            .remote
            .nodes
            .values()
            .filter(|n| n.node_type == NodeType::File)
            .filter_map(|n| n.md5sum.as_ref())
            .collect();

        if local_by_md5 != remote_by_md5 {
            return Err(format!(
                "Convergence failed: local md5s {local_by_md5:?}, remote md5s {remote_by_md5:?}"
            ));
        }

        Ok(())
    }
}
