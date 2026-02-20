use super::mock_fs::MockFs;
use super::mock_remote::MockRemote;
use crate::model::{
    LocalFileId, LocalNode, NodeInfo, NodeType, RemoteId, RemoteNode, SyncedRecord,
};
use crate::planner::Planner;
use crate::store::TreeStore;
use crate::util::compute_md5_from_bytes;
use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counter for generating unique local file IDs in simulation
static INODE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_local_id() -> LocalFileId {
    LocalFileId::new(1, INODE_COUNTER.fetch_add(1, Ordering::SeqCst))
}

fn compute_md5(content: &[u8]) -> String {
    compute_md5_from_bytes(content)
}

/// Simulation runner that orchestrates mock local fs, mock remote, and the real planner/store
pub struct SimulationRunner {
    pub local_fs: MockFs,
    pub remote: MockRemote,
    pub store: TreeStore,
    pub sync_root: PathBuf,
    pub last_seq: u64,
    /// Maps `RemoteId` -> `LocalFileId` for synced files
    pub remote_to_local: std::collections::HashMap<RemoteId, LocalFileId>,
    stopped: bool,
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
    LocalMove {
        local_id: LocalFileId,
        new_parent_local_id: Option<LocalFileId>,
        new_name: String,
    },
    RemoteMove {
        id: RemoteId,
        new_parent_id: RemoteId,
        new_name: String,
    },
    Sync,
    StopClient,
    RestartClient,
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
            stopped: false,
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
                if !self.stopped {
                    self.store
                        .insert_local_node(&node)
                        .map_err(|e| e.to_string())?;
                }
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
                if !self.stopped {
                    self.store
                        .insert_local_node(&node)
                        .map_err(|e| e.to_string())?;
                }
            }
            SimAction::LocalDeleteFile { local_id } => {
                self.local_fs.delete(&local_id);
                if !self.stopped {
                    self.delete_local_recursive(&local_id)?;
                }
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
                self.local_fs
                    .create_file(local_id, updated.clone(), content);
                if !self.stopped {
                    self.store
                        .insert_local_node(&updated)
                        .map_err(|e| e.to_string())?;
                }
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
            SimAction::LocalMove {
                local_id,
                new_parent_local_id,
                new_name,
            } => {
                self.local_fs
                    .move_node(&local_id, new_parent_local_id, new_name);
                if !self.stopped
                    && let Some(node) = self.local_fs.get_node(&local_id).cloned()
                {
                    self.store
                        .insert_local_node(&node)
                        .map_err(|e| e.to_string())?;
                }
            }
            SimAction::RemoteMove {
                id,
                new_parent_id,
                new_name,
            } => {
                self.remote.move_node(&id, new_parent_id, new_name);
            }
            SimAction::Sync => {
                if !self.stopped {
                    self.sync()?;
                }
            }
            SimAction::StopClient => {
                self.stopped = true;
            }
            SimAction::RestartClient => {
                self.stopped = false;
                self.reconcile_local()?;
            }
        }
        Ok(())
    }

    fn delete_local_recursive(&self, id: &LocalFileId) -> Result<(), String> {
        let children = self
            .store
            .list_local_children(id)
            .map_err(|e| e.to_string())?;
        for child in &children {
            self.delete_local_recursive(&child.id)?;
        }
        self.store
            .delete_local_node(id)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn reconcile_local(&self) -> Result<(), String> {
        let fs_ids: HashSet<_> = self.local_fs.nodes.keys().cloned().collect();

        let store_nodes = self.store.list_all_local().map_err(|e| e.to_string())?;
        let store_ids: HashSet<_> = store_nodes.iter().map(|n| n.id.clone()).collect();

        for id in store_ids.difference(&fs_ids) {
            self.store
                .delete_local_node(id)
                .map_err(|e| e.to_string())?;
        }

        for node in self.local_fs.list_all() {
            self.store
                .insert_local_node(node)
                .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
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
                        local_name: Some(node.name.clone()),
                        local_parent_id: None,
                        remote_name: Some(node.name.clone()),
                        remote_parent_id: node.parent_id.clone(),
                    };
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                    self.remote_to_local.insert(node.id.clone(), local_id);
                }
            }
        }
        self.last_seq = self.remote.current_seq();

        // Step 2+3: Plan and execute in a loop until no more ops are generated.
        // We split each pass into two phases:
        //   Phase A: execute creates, moves, downloads, and uploads (non-delete ops)
        //   Phase B: re-plan, then execute deletes
        // This ensures moves are executed before cascade deletes can orphan files
        // (e.g., a file moved out of a directory that is then deleted).
        let max_rounds = 10;
        for _ in 0..max_rounds {
            let planner = Planner::new(&self.store, self.sync_root.clone());
            let results = planner.plan().map_err(|e| e.to_string())?;

            let ops: Vec<_> = results
                .into_iter()
                .filter_map(|r| {
                    if let crate::model::PlanResult::Op(op) = r {
                        Some(op)
                    } else {
                        None
                    }
                })
                .collect();

            if ops.is_empty() {
                break;
            }

            let (non_delete_ops, delete_ops): (Vec<_>, Vec<_>) = ops.into_iter().partition(|op| {
                !matches!(
                    op,
                    crate::model::SyncOp::DeleteLocal { .. }
                        | crate::model::SyncOp::DeleteRemote { .. }
                )
            });

            for op in non_delete_ops {
                self.execute_op(op)?;
            }

            if !delete_ops.is_empty() {
                // Re-plan before deletes: moves that were blocked (ParentMissing)
                // in the initial plan may now be possible after creates executed.
                let planner = Planner::new(&self.store, self.sync_root.clone());
                let results = planner.plan().map_err(|e| e.to_string())?;
                // Execute any newly-unblocked non-delete ops first
                let (new_non_delete, new_delete): (Vec<_>, Vec<_>) = results
                    .into_iter()
                    .filter_map(|r| {
                        if let crate::model::PlanResult::Op(op) = r {
                            Some(op)
                        } else {
                            None
                        }
                    })
                    .partition(|op| {
                        !matches!(
                            op,
                            crate::model::SyncOp::DeleteLocal { .. }
                                | crate::model::SyncOp::DeleteRemote { .. }
                        )
                    });

                for op in new_non_delete {
                    self.execute_op(op)?;
                }

                // Now execute the deletes from the fresh plan
                for op in new_delete {
                    self.execute_op(op)?;
                }
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

                    let synced = SyncedRecord {
                        local_id: local_id.clone(),
                        remote_id: remote_id.clone(),
                        rel_path: remote_node.name.clone(),
                        md5sum: remote_node.md5sum.clone(),
                        size: remote_node.size,
                        rev: remote_node.rev.clone(),
                        node_type: remote_node.node_type,
                        local_name: Some(local_node.name.clone()),
                        local_parent_id: local_node.parent_id,
                        remote_name: Some(remote_node.name.clone()),
                        remote_parent_id: remote_node.parent_id,
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
                        rel_path: remote_node.name.clone(),
                        md5sum: None,
                        size: None,
                        rev: remote_node.rev,
                        node_type: NodeType::Directory,
                        local_name: Some(local_node.name.clone()),
                        local_parent_id: local_node.parent_id,
                        remote_name: Some(remote_node.name),
                        remote_parent_id: remote_node.parent_id,
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
                        local_name: Some(local_node.name),
                        local_parent_id: local_node.parent_id,
                        remote_name: Some(remote_node.name),
                        remote_parent_id: remote_node.parent_id,
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
                        rel_path: local_node.name.clone(),
                        md5sum: None,
                        size: None,
                        rev: remote_node.rev,
                        node_type: NodeType::Directory,
                        local_name: Some(local_node.name),
                        local_parent_id: local_node.parent_id,
                        remote_name: Some(remote_node.name),
                        remote_parent_id: remote_node.parent_id,
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
                if let Some(synced) = self.store.get_synced_by_local(&local_id).ok().flatten() {
                    self.remote_to_local.remove(&synced.remote_id);
                }
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
            SyncOp::MoveLocal {
                local_id,
                from_path: _,
                to_path: _,
                expected_parent_id: _,
                expected_name: _,
            } => {
                if let Some(remote_node) = self
                    .store
                    .get_synced_by_local(&local_id)
                    .ok()
                    .flatten()
                    .and_then(|s| self.remote.get_node(&s.remote_id).cloned())
                {
                    let new_parent_local = remote_node
                        .parent_id
                        .as_ref()
                        .and_then(|pid| self.remote_to_local.get(pid).cloned());
                    self.local_fs.move_node(
                        &local_id,
                        new_parent_local.clone(),
                        remote_node.name.clone(),
                    );
                    if let Some(node) = self.local_fs.get_node(&local_id).cloned() {
                        self.store
                            .insert_local_node(&node)
                            .map_err(|e| e.to_string())?;
                    }
                    if let Some(mut synced) =
                        self.store.get_synced_by_local(&local_id).ok().flatten()
                    {
                        synced.local_name = Some(remote_node.name.clone());
                        synced.local_parent_id = new_parent_local;
                        synced.remote_name = Some(remote_node.name);
                        synced.remote_parent_id = remote_node.parent_id;
                        synced.rel_path = self
                            .local_fs
                            .get_node(&local_id)
                            .map(|n| n.name.clone())
                            .unwrap_or_default();
                        self.store
                            .insert_synced(&synced)
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            SyncOp::MoveRemote {
                remote_id,
                new_parent_id,
                new_name,
                expected_rev: _,
            } => {
                self.remote
                    .move_node(&remote_id, new_parent_id.clone(), new_name.clone());
                if let Some(node) = self.remote.get_node(&remote_id).cloned() {
                    self.store
                        .insert_remote_node(&node)
                        .map_err(|e| e.to_string())?;
                }
                if let Some(mut synced) = self.store.get_synced_by_remote(&remote_id).ok().flatten()
                {
                    synced.remote_name = Some(new_name);
                    synced.remote_parent_id = Some(new_parent_id);
                    if let Some(local_node) = self.local_fs.get_node(&synced.local_id) {
                        synced.local_name = Some(local_node.name.clone());
                        synced.local_parent_id = local_node.parent_id.clone();
                    }
                    synced.rel_path = self
                        .local_fs
                        .get_node(&synced.local_id)
                        .map(|n| n.name.clone())
                        .unwrap_or_default();
                    self.store
                        .insert_synced(&synced)
                        .map_err(|e| e.to_string())?;
                }
            }
            SyncOp::DownloadUpdate {
                remote_id,
                local_id,
                local_path: _,
                expected_rev: _,
                expected_remote_md5: _,
                expected_local_md5: _,
            } => {
                if let Some(remote_node) = self.remote.get_node(&remote_id).cloned() {
                    let content = self
                        .remote
                        .get_content(&remote_id)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(mut local_node) = self.local_fs.get_node(&local_id).cloned() {
                        local_node.md5sum.clone_from(&remote_node.md5sum);
                        local_node.size = remote_node.size;
                        local_node.mtime = remote_node.updated_at;
                        self.local_fs
                            .create_file(local_id.clone(), local_node, content);
                        if let Some(node) = self.local_fs.get_node(&local_id).cloned() {
                            self.store
                                .insert_local_node(&node)
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    if let Some(mut synced) =
                        self.store.get_synced_by_local(&local_id).ok().flatten()
                    {
                        synced.md5sum = remote_node.md5sum;
                        synced.size = remote_node.size;
                        synced.rev = remote_node.rev;
                        self.store
                            .insert_synced(&synced)
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            SyncOp::UploadUpdate {
                local_id,
                remote_id,
                local_path: _,
                expected_local_md5: _,
                expected_rev: _,
            } => {
                if let Some(local_node) = self.local_fs.get_node(&local_id).cloned() {
                    let content = self
                        .local_fs
                        .read_file(&local_id)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(mut remote_node) = self.remote.get_node(&remote_id).cloned() {
                        remote_node.md5sum.clone_from(&local_node.md5sum);
                        remote_node.size = local_node.size;
                        remote_node.updated_at = local_node.mtime;
                        let rev_num: u32 = remote_node
                            .rev
                            .split('-')
                            .next()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(1);
                        remote_node.rev = format!("{}-{}", rev_num + 1, remote_id.as_str());
                        self.remote.add_node(remote_node.clone(), Some(content));
                        self.store
                            .insert_remote_node(&remote_node)
                            .map_err(|e| e.to_string())?;
                    }
                    if let Some(mut synced) =
                        self.store.get_synced_by_local(&local_id).ok().flatten()
                    {
                        synced.md5sum = local_node.md5sum;
                        synced.size = local_node.size;
                        self.store
                            .insert_synced(&synced)
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }

    fn local_path(&self, node: &LocalNode) -> String {
        let mut parts = vec![node.name.clone()];
        let mut current = node.parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(ref pid) = current {
            if !visited.insert(pid.clone()) {
                break;
            }
            if let Some(parent) = self.local_fs.get_node(pid) {
                if parent.name.is_empty() {
                    break;
                }
                parts.push(parent.name.clone());
                current.clone_from(&parent.parent_id);
            } else {
                break;
            }
        }
        parts.reverse();
        parts.join("/")
    }

    fn remote_path(&self, node: &RemoteNode) -> String {
        let mut parts = vec![node.name.clone()];
        let mut current = node.parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(ref pid) = current {
            if !visited.insert(pid.clone()) {
                break;
            }
            if let Some(parent) = self.remote.get_node(pid) {
                if parent.name.is_empty() {
                    break;
                }
                parts.push(parent.name.clone());
                current.clone_from(&parent.parent_id);
            } else {
                break;
            }
        }
        parts.reverse();
        parts.join("/")
    }

    /// Check invariant: after sync, local and remote should have same files
    /// and directories (same paths and same content for files)
    ///
    /// # Errors
    /// Returns an error describing the convergence mismatch
    pub fn check_convergence(&self) -> Result<(), String> {
        let local_files: BTreeSet<(String, String)> = self
            .local_fs
            .list_all()
            .iter()
            .filter(|n| n.is_file())
            .filter_map(|n| {
                n.md5sum
                    .as_ref()
                    .map(|md5| (self.local_path(n), md5.clone()))
            })
            .collect();

        let remote_files: BTreeSet<(String, String)> = self
            .remote
            .nodes
            .values()
            .filter(|n| n.is_file())
            .filter_map(|n| {
                n.md5sum
                    .as_ref()
                    .map(|md5| (self.remote_path(n), md5.clone()))
            })
            .collect();

        if local_files != remote_files {
            let local_only: BTreeSet<_> = local_files.difference(&remote_files).collect();
            let remote_only: BTreeSet<_> = remote_files.difference(&local_files).collect();
            return Err(format!(
                "File convergence failed:\n  local only: {local_only:?}\n  remote only: {remote_only:?}"
            ));
        }

        let local_dirs: BTreeSet<String> = self
            .local_fs
            .list_all()
            .iter()
            .filter(|n| n.is_dir() && !n.name.is_empty())
            .map(|n| self.local_path(n))
            .collect();

        let remote_dirs: BTreeSet<String> = self
            .remote
            .nodes
            .values()
            .filter(|n| n.is_dir() && !n.name.is_empty())
            .map(|n| self.remote_path(n))
            .collect();

        if local_dirs != remote_dirs {
            let local_only: BTreeSet<_> = local_dirs.difference(&remote_dirs).collect();
            let remote_only: BTreeSet<_> = remote_dirs.difference(&local_dirs).collect();
            return Err(format!(
                "Directory convergence failed:\n  local only: {local_only:?}\n  remote only: {remote_only:?}"
            ));
        }

        Ok(())
    }

    /// Check invariant: after sync, the `TreeStore` is internally consistent.
    /// Every synced record references a valid local node and remote node in
    /// the store, and `remote_to_local` matches the synced table.
    ///
    /// # Errors
    /// Returns an error describing the consistency violation
    pub fn check_store_consistency(&self) -> Result<(), String> {
        let synced_records = self.store.list_all_synced().map_err(|e| e.to_string())?;

        let mut errors = Vec::new();

        for record in &synced_records {
            // Every synced record must have a corresponding local node in the store
            match self.store.get_local_node(&record.local_id) {
                Ok(None) => errors.push(format!(
                    "synced record '{}' (remote={}) has local node missing in store",
                    record.rel_path,
                    record.remote_id.as_str()
                )),
                Err(e) => errors.push(format!(
                    "synced record '{}': error reading local node: {e}",
                    record.rel_path
                )),
                Ok(Some(_)) => {}
            }

            // Every synced record must have a corresponding remote node in the store
            match self.store.get_remote_node(&record.remote_id) {
                Ok(None) => errors.push(format!(
                    "synced record '{}' (local={:?}) has remote node missing in store",
                    record.rel_path, record.local_id
                )),
                Err(e) => errors.push(format!(
                    "synced record '{}': error reading remote node: {e}",
                    record.rel_path
                )),
                Ok(Some(_)) => {}
            }

            // remote_to_local must map this record's remote_id to the correct local_id
            match self.remote_to_local.get(&record.remote_id) {
                None => errors.push(format!(
                    "synced record '{}': remote_to_local missing entry for remote={}",
                    record.rel_path,
                    record.remote_id.as_str()
                )),
                Some(local_id) if *local_id != record.local_id => errors.push(format!(
                    "synced record '{}': remote_to_local maps remote={} to {:?} but synced has {:?}",
                    record.rel_path,
                    record.remote_id.as_str(),
                    local_id,
                    record.local_id
                )),
                Some(_) => {}
            }
        }

        // Check reverse: every remote_to_local entry should have a synced record
        for (remote_id, local_id) in &self.remote_to_local {
            match self.store.get_synced_by_remote(remote_id) {
                Ok(None) => errors.push(format!(
                    "remote_to_local has entry remote={} -> {:?} but no synced record exists",
                    remote_id.as_str(),
                    local_id
                )),
                Ok(Some(synced)) if synced.local_id != *local_id => errors.push(format!(
                    "remote_to_local maps remote={} to {:?} but synced record has {:?}",
                    remote_id.as_str(),
                    local_id,
                    synced.local_id
                )),
                Err(e) => errors.push(format!(
                    "remote_to_local remote={}: error reading synced record: {e}",
                    remote_id.as_str()
                )),
                Ok(Some(_)) => {}
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Store consistency check failed ({} errors):\n  {}",
                errors.len(),
                errors.join("\n  ")
            ))
        }
    }

    /// Check invariant: after sync, planning again should produce zero
    /// operations. This catches bugs where sync creates side effects
    /// that trigger further sync.
    ///
    /// # Errors
    /// Returns an error listing unexpected operations or conflicts
    pub fn check_idempotency(&self) -> Result<(), String> {
        let planner = Planner::new(&self.store, self.sync_root.clone());
        let results = planner.plan().map_err(|e| e.to_string())?;

        let ops: Vec<_> = results
            .iter()
            .filter(|r| matches!(r, crate::model::PlanResult::Op(_)))
            .collect();
        let conflicts: Vec<_> = results
            .iter()
            .filter(|r| matches!(r, crate::model::PlanResult::Conflict(_)))
            .collect();

        if ops.is_empty() && conflicts.is_empty() {
            return Ok(());
        }

        let mut msg = String::from("Sync not idempotent — second plan produced:\n");
        for op in &ops {
            let _ = writeln!(msg, "  op: {op:?}");
        }
        for conflict in &conflicts {
            let _ = writeln!(msg, "  conflict: {conflict:?}");
        }
        Err(msg)
    }
}
