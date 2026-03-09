use super::mock_fs::MockFs;
use super::mock_remote::MockRemote;
use crate::model::{
    LocalFileId, LocalNode, NodeInfo, NodeType, RemoteId, RemoteNode, SyncedRecord, TRASH_DIR_ID,
};
use crate::planner::Planner;
use crate::store::{StoreSnapshot, TreeStore};
use crate::sync::conflict_name::generate_conflict_name;
use crate::util::compute_md5_from_bytes;
use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

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
    /// Per-instance counter for generating unique local file IDs
    inode_counter: u64,
    /// Number of download ops to skip (simulating transient network failures)
    pending_download_failures: u32,
    /// Number of upload ops to skip (simulating transient network failures)
    pending_upload_failures: u32,
    /// Queue of remote mutations to inject during the next sync execution
    pending_concurrent_remote_ops: Vec<ConcurrentRemoteOp>,
    /// Saved snapshot for rollback testing
    snapshot: Option<RunnerSnapshot>,
}

/// Complete snapshot of the runner state for rollback testing
#[derive(Clone)]
struct RunnerSnapshot {
    local_fs: MockFs,
    remote: MockRemote,
    store_snapshot: StoreSnapshot,
    last_seq: u64,
    remote_to_local: std::collections::HashMap<RemoteId, LocalFileId>,
    stopped: bool,
    inode_counter: u64,
    pending_download_failures: u32,
    pending_upload_failures: u32,
    pending_concurrent_remote_ops: Vec<ConcurrentRemoteOp>,
}

/// Remote mutations injected during sync execution to simulate concurrent
/// changes from other devices (e.g., `CouchDB` changes arriving mid-sync).
#[derive(Debug, Clone)]
pub enum ConcurrentRemoteOp {
    CreateFile {
        id: RemoteId,
        parent_id: RemoteId,
        name: String,
        content: Vec<u8>,
    },
    ModifyFile {
        id: RemoteId,
        content: Vec<u8>,
    },
    DeleteFile {
        id: RemoteId,
    },
    TrashFile {
        id: RemoteId,
    },
}

impl std::fmt::Display for ConcurrentRemoteOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateFile {
                id, name, content, ..
            } => write!(f, "CreateFile(id={id}, name={name}, {}B)", content.len()),
            Self::ModifyFile { id, content } => {
                write!(f, "ModifyFile(id={id}, {}B)", content.len())
            }
            Self::DeleteFile { id } => write!(f, "DeleteFile(id={id})"),
            Self::TrashFile { id } => write!(f, "TrashFile(id={id})"),
        }
    }
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
    /// Move a remote file or directory to the trash (`io.cozy.files.trash-dir`)
    RemoteTrash {
        id: RemoteId,
    },
    RemoteModifyFile {
        id: RemoteId,
        content: Vec<u8>,
    },
    /// Simulate an atomic save: delete the old inode, create a new one with
    /// the same name and parent but new content (and a fresh inode).
    LocalAtomicSave {
        local_id: LocalFileId,
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
    /// Inject a transient download failure (the next download/create-local-dir op will be skipped)
    FailNextDownload,
    /// Inject a transient upload failure (the next upload/create-remote-dir op will be skipped)
    FailNextUpload,
    /// Queue a concurrent remote change that fires during the next sync execution
    ConcurrentRemoteChange(ConcurrentRemoteOp),
    /// Save the entire runner state for later rollback
    SnapshotState,
    /// Restore the runner state to the last snapshot (no-op if none)
    RollbackToSnapshot,
}

fn fmt_local_parent(parent: Option<&LocalFileId>) -> String {
    parent.map_or_else(|| "root".to_string(), ToString::to_string)
}

fn fmt_remote_parent(parent: Option<&RemoteId>) -> String {
    parent.map_or_else(|| "root".to_string(), ToString::to_string)
}

impl std::fmt::Display for SimAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalCreateFile {
                local_id,
                parent_local_id,
                name,
                content,
            } => write!(
                f,
                "LocalCreateFile(id={local_id}, parent={}, name={name}, {}B)",
                fmt_local_parent(parent_local_id.as_ref()),
                content.len()
            ),
            Self::LocalCreateDir {
                local_id,
                parent_local_id,
                name,
            } => write!(
                f,
                "LocalCreateDir(id={local_id}, parent={}, name={name})",
                fmt_local_parent(parent_local_id.as_ref())
            ),
            Self::LocalDeleteFile { local_id } => {
                write!(f, "LocalDeleteFile(id={local_id})")
            }
            Self::LocalModifyFile { local_id, content } => {
                write!(f, "LocalModifyFile(id={local_id}, {}B)", content.len())
            }
            Self::RemoteCreateFile {
                id,
                parent_id,
                name,
                content,
            } => write!(
                f,
                "RemoteCreateFile(id={id}, parent={parent_id}, name={name}, {}B)",
                content.len()
            ),
            Self::RemoteCreateDir {
                id,
                parent_id,
                name,
            } => write!(
                f,
                "RemoteCreateDir(id={id}, parent={}, name={name})",
                fmt_remote_parent(parent_id.as_ref())
            ),
            Self::RemoteDeleteFile { id } => write!(f, "RemoteDeleteFile(id={id})"),
            Self::RemoteTrash { id } => write!(f, "RemoteTrash(id={id})"),
            Self::RemoteModifyFile { id, content } => {
                write!(f, "RemoteModifyFile(id={id}, {}B)", content.len())
            }
            Self::LocalAtomicSave { local_id, content } => {
                write!(f, "LocalAtomicSave(id={local_id}, {}B)", content.len())
            }
            Self::LocalMove {
                local_id,
                new_parent_local_id,
                new_name,
            } => write!(
                f,
                "LocalMove(id={local_id}, new_parent={}, new_name={new_name})",
                fmt_local_parent(new_parent_local_id.as_ref())
            ),
            Self::RemoteMove {
                id,
                new_parent_id,
                new_name,
            } => write!(
                f,
                "RemoteMove(id={id}, new_parent={new_parent_id}, new_name={new_name})"
            ),
            Self::Sync => write!(f, "Sync"),
            Self::StopClient => write!(f, "StopClient"),
            Self::RestartClient => write!(f, "RestartClient"),
            Self::FailNextDownload => write!(f, "FailNextDownload"),
            Self::FailNextUpload => write!(f, "FailNextUpload"),
            Self::ConcurrentRemoteChange(op) => {
                write!(f, "ConcurrentRemoteChange({op})")
            }
            Self::SnapshotState => write!(f, "SnapshotState"),
            Self::RollbackToSnapshot => write!(f, "RollbackToSnapshot"),
        }
    }
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
            inode_counter: 0,
            pending_download_failures: 0,
            pending_upload_failures: 0,
            pending_concurrent_remote_ops: Vec::new(),
            snapshot: None,
        }
    }

    const fn next_local_id(&mut self) -> LocalFileId {
        self.inode_counter += 1;
        LocalFileId::new(1, self.inode_counter)
    }

    /// Apply a simulation action
    ///
    /// # Errors
    /// Returns an error if the action fails
    pub fn apply(&mut self, action: SimAction) -> Result<(), String> {
        match action {
            SimAction::LocalCreateFile {
                local_id,
                parent_local_id,
                name,
                content,
            } => self.apply_local_create_file(local_id, parent_local_id, name, content),
            SimAction::LocalCreateDir {
                local_id,
                parent_local_id,
                name,
            } => self.apply_local_create_dir(local_id, parent_local_id, name),
            SimAction::LocalDeleteFile { local_id } => self.apply_local_delete(&local_id),
            SimAction::LocalModifyFile { local_id, content } => {
                self.apply_local_modify(local_id, content)
            }
            SimAction::LocalAtomicSave { local_id, content } => {
                self.apply_local_atomic_save(&local_id, content)
            }
            SimAction::RemoteCreateFile {
                id,
                parent_id,
                name,
                content,
            } => {
                self.apply_remote_create_file(&id, &parent_id, name, content);
                Ok(())
            }
            SimAction::RemoteCreateDir {
                id,
                parent_id,
                name,
            } => {
                self.apply_remote_create_dir(&id, parent_id.as_ref(), name);
                Ok(())
            }
            SimAction::RemoteDeleteFile { id } => {
                self.remote.delete_node(&id);
                Ok(())
            }
            SimAction::RemoteTrash { id } => {
                self.apply_remote_trash(&id);
                Ok(())
            }
            SimAction::RemoteModifyFile { id, content } => self.apply_remote_modify(&id, content),
            SimAction::LocalMove {
                local_id,
                new_parent_local_id,
                new_name,
            } => self.apply_local_move(&local_id, new_parent_local_id, new_name),
            SimAction::RemoteMove {
                id,
                new_parent_id,
                new_name,
            } => {
                self.remote.move_node(&id, new_parent_id, new_name);
                Ok(())
            }
            SimAction::Sync => {
                if !self.stopped {
                    self.sync()?;
                }
                Ok(())
            }
            SimAction::StopClient => {
                self.stopped = true;
                Ok(())
            }
            SimAction::RestartClient => {
                self.stopped = false;
                self.reconcile_local()
            }
            SimAction::FailNextDownload => {
                self.pending_download_failures += 1;
                Ok(())
            }
            SimAction::FailNextUpload => {
                self.pending_upload_failures += 1;
                Ok(())
            }
            SimAction::ConcurrentRemoteChange(op) => {
                self.pending_concurrent_remote_ops.push(op);
                Ok(())
            }
            SimAction::SnapshotState => self.take_snapshot(),
            SimAction::RollbackToSnapshot => self.rollback_to_snapshot(),
        }
    }

    fn take_snapshot(&mut self) -> Result<(), String> {
        let store_snapshot = self.store.snapshot().map_err(|e| e.to_string())?;
        self.snapshot = Some(RunnerSnapshot {
            local_fs: self.local_fs.clone(),
            remote: self.remote.clone(),
            store_snapshot,
            last_seq: self.last_seq,
            remote_to_local: self.remote_to_local.clone(),
            stopped: self.stopped,
            inode_counter: self.inode_counter,
            pending_download_failures: self.pending_download_failures,
            pending_upload_failures: self.pending_upload_failures,
            pending_concurrent_remote_ops: self.pending_concurrent_remote_ops.clone(),
        });
        Ok(())
    }

    fn rollback_to_snapshot(&mut self) -> Result<(), String> {
        let Some(snap) = self.snapshot.take() else {
            return Ok(());
        };
        self.local_fs = snap.local_fs;
        self.remote = snap.remote;
        self.store
            .restore(&snap.store_snapshot)
            .map_err(|e| e.to_string())?;
        self.last_seq = snap.last_seq;
        self.remote_to_local = snap.remote_to_local;
        self.stopped = snap.stopped;
        self.inode_counter = snap.inode_counter;
        self.pending_download_failures = snap.pending_download_failures;
        self.pending_upload_failures = snap.pending_upload_failures;
        self.pending_concurrent_remote_ops = snap.pending_concurrent_remote_ops;
        Ok(())
    }

    fn apply_local_create_file(
        &mut self,
        local_id: LocalFileId,
        parent_local_id: Option<LocalFileId>,
        name: String,
        content: Vec<u8>,
    ) -> Result<(), String> {
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
        Ok(())
    }

    fn apply_local_create_dir(
        &mut self,
        local_id: LocalFileId,
        parent_local_id: Option<LocalFileId>,
        name: String,
    ) -> Result<(), String> {
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
        Ok(())
    }

    fn apply_local_delete(&mut self, local_id: &LocalFileId) -> Result<(), String> {
        self.local_fs.delete(local_id);
        if !self.stopped {
            self.delete_local_recursive(local_id)?;
        }
        Ok(())
    }

    fn apply_local_modify(
        &mut self,
        local_id: LocalFileId,
        content: Vec<u8>,
    ) -> Result<(), String> {
        let Some(node) = self.local_fs.get_node(&local_id).cloned() else {
            return Ok(());
        };
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
        Ok(())
    }

    fn apply_local_atomic_save(
        &mut self,
        local_id: &LocalFileId,
        content: Vec<u8>,
    ) -> Result<(), String> {
        let Some(node) = self.local_fs.get_node(local_id).cloned() else {
            return Ok(());
        };
        if node.node_type != NodeType::File {
            return Ok(());
        }

        let name = node.name;
        let parent_id = node.parent_id;

        // Delete old inode
        self.local_fs.delete(local_id);
        if !self.stopped {
            self.store
                .delete_local_node(local_id)
                .map_err(|e| e.to_string())?;
        }

        // Create new inode with same name and parent
        let new_local_id = self.next_local_id();
        let md5sum = compute_md5(&content);
        let new_node = LocalNode {
            id: new_local_id.clone(),
            parent_id,
            name,
            node_type: NodeType::File,
            md5sum: Some(md5sum),
            size: Some(content.len() as u64),
            mtime: 1000,
        };
        self.local_fs
            .create_file(new_local_id, new_node.clone(), content);
        if !self.stopped {
            self.store
                .insert_local_node(&new_node)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn apply_remote_create_file(
        &mut self,
        id: &RemoteId,
        parent_id: &RemoteId,
        name: String,
        content: Vec<u8>,
    ) {
        let md5sum = compute_md5(&content);
        let node = RemoteNode {
            id: id.clone(),
            parent_id: Some(parent_id.clone()),
            name,
            node_type: NodeType::File,
            md5sum: Some(md5sum),
            size: Some(content.len() as u64),
            updated_at: 1000,
            rev: format!("1-{}", id.as_str()),
        };
        self.remote.add_node(node, Some(content));
    }

    fn apply_remote_create_dir(
        &mut self,
        id: &RemoteId,
        parent_id: Option<&RemoteId>,
        name: String,
    ) {
        let node = RemoteNode {
            id: id.clone(),
            parent_id: parent_id.cloned(),
            name,
            node_type: NodeType::Directory,
            md5sum: None,
            size: None,
            updated_at: 1000,
            rev: format!("1-{}", id.as_str()),
        };
        self.remote.add_node(node, None);
    }

    fn apply_remote_modify(&mut self, id: &RemoteId, content: Vec<u8>) -> Result<(), String> {
        let node = self
            .remote
            .get_node(id)
            .cloned()
            .ok_or_else(|| format!("RemoteModifyFile: node {} not found", id.as_str()))?;
        let md5sum = compute_md5(&content);
        let mut updated = node;
        updated.md5sum = Some(md5sum);
        updated.size = Some(content.len() as u64);
        updated.updated_at += 1;
        let rev_num: u32 = updated
            .rev
            .split('-')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        updated.rev = format!("{}-{}", rev_num + 1, id.as_str());
        self.remote.add_node(updated, Some(content));
        Ok(())
    }

    fn apply_remote_trash(&mut self, id: &RemoteId) {
        self.remote.trash_node(id);
    }

    /// Check whether a remote node is under the trash directory
    fn is_under_trash(&self, node: &RemoteNode) -> bool {
        let trash_id = RemoteId::new(TRASH_DIR_ID);
        let mut current = node.parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(ref pid) = current {
            if *pid == trash_id {
                return true;
            }
            if !visited.insert(pid.clone()) {
                return false;
            }
            if let Some(parent) = self.remote.get_node(pid) {
                current.clone_from(&parent.parent_id);
            } else {
                return false;
            }
        }
        false
    }

    /// Check whether a remote node is the trash directory or under it
    fn is_trashed_or_trash_dir(&self, node: &RemoteNode) -> bool {
        node.id.as_str() == TRASH_DIR_ID || self.is_under_trash(node)
    }

    fn apply_local_move(
        &mut self,
        local_id: &LocalFileId,
        new_parent_local_id: Option<LocalFileId>,
        new_name: String,
    ) -> Result<(), String> {
        self.local_fs
            .move_node(local_id, new_parent_local_id, new_name);
        if !self.stopped
            && let Some(node) = self.local_fs.get_node(local_id).cloned()
        {
            self.store
                .insert_local_node(&node)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn delete_local_recursive(&self, id: &LocalFileId) -> Result<(), String> {
        let mut to_delete = Vec::new();
        let mut stack = vec![id.clone()];
        let mut visited = HashSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            let children = self
                .store
                .list_local_children(&current)
                .map_err(|e| e.to_string())?;
            for child in &children {
                stack.push(child.id.clone());
            }
            to_delete.push(current);
        }
        // Delete in reverse order (children before parents)
        for del_id in to_delete.iter().rev() {
            self.store
                .delete_local_node(del_id)
                .map_err(|e| e.to_string())?;
        }
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

    fn sync(&mut self) -> Result<(), String> {
        self.fetch_remote_changes()?;
        self.plan_and_execute_loop()?;
        self.refresh_remote_to_local()
    }

    /// Rebuild `remote_to_local` from the synced records in the store.
    ///
    /// This is needed after the planner's inode reconciliation (atomic save
    /// detection) which updates synced records directly in the store,
    /// bypassing the runner's `remote_to_local` map.
    fn refresh_remote_to_local(&mut self) -> Result<(), String> {
        let synced_records = self.store.list_all_synced().map_err(|e| e.to_string())?;
        self.remote_to_local.clear();
        for record in synced_records {
            self.remote_to_local
                .insert(record.remote_id, record.local_id);
        }
        Ok(())
    }

    fn fetch_remote_changes(&mut self) -> Result<(), String> {
        let trash_id = RemoteId::new(TRASH_DIR_ID);
        for change in self.remote.get_all_changes_since(self.last_seq) {
            if change.deleted {
                self.store
                    .delete_remote_node(&change.remote_id)
                    .map_err(|e| e.to_string())?;
            } else if let Some(node) = self.remote.get_node(&change.remote_id).cloned() {
                // Skip the trash directory itself — it should never be synced
                if node.id == trash_id {
                    continue;
                }

                // Nodes moved under the trash are treated as remote deletions
                if self.is_under_trash(&node) {
                    self.delete_remote_tree_from_store(&node.id)?;
                    continue;
                }

                self.store
                    .insert_remote_node(&node)
                    .map_err(|e| e.to_string())?;

                if node.parent_id.is_none() && !self.remote_to_local.contains_key(&node.id) {
                    self.bind_root_directory(&node)?;
                }
            }
        }
        self.last_seq = self.remote.current_seq();
        Ok(())
    }

    /// Delete a remote node and all its descendants from the store's remote tree.
    /// Used when a node is moved to trash — children don't get individual change
    /// records, so we recursively clean them up.
    fn delete_remote_tree_from_store(&self, id: &RemoteId) -> Result<(), String> {
        let all_remote = self.store.list_all_remote().map_err(|e| e.to_string())?;
        let mut to_delete = vec![id.clone()];
        let mut i = 0;
        while i < to_delete.len() {
            let current = to_delete[i].clone();
            for node in &all_remote {
                if node.parent_id.as_ref() == Some(&current) && !to_delete.contains(&node.id) {
                    to_delete.push(node.id.clone());
                }
            }
            i += 1;
        }
        for del_id in &to_delete {
            self.store
                .delete_remote_node(del_id)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn bind_root_directory(&mut self, node: &RemoteNode) -> Result<(), String> {
        let local_id = self.next_local_id();
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
        Ok(())
    }

    /// Plan and execute sync operations in a loop until convergence.
    ///
    /// Each pass is split into two phases:
    ///   Phase A: execute creates, moves, downloads, and uploads (non-delete ops)
    ///   Phase B: re-plan, then execute deletes
    /// This ensures moves are executed before cascade deletes can orphan files
    /// (e.g., a file moved out of a directory that is then deleted).
    ///
    /// After the first batch of non-delete ops, any queued concurrent remote
    /// changes are applied to `MockRemote` (but not fetched into the store).
    /// This simulates `CouchDB` changes arriving mid-sync from other devices.
    fn plan_and_execute_loop(&mut self) -> Result<(), String> {
        let max_rounds = 10;
        for _ in 0..max_rounds {
            let ops = self.plan_sync_ops()?;
            if ops.is_empty() {
                break;
            }

            let (non_delete_ops, delete_ops) = Self::partition_ops(ops);

            for op in non_delete_ops {
                self.execute_op(op)?;
            }

            // Inject concurrent remote changes after non-delete ops execute.
            // These modify MockRemote but are NOT fetched into the store,
            // simulating changes that arrive while sync is in progress.
            self.apply_concurrent_remote_ops();

            if !delete_ops.is_empty() {
                // Re-plan before deletes: moves that were blocked (ParentMissing)
                // in the initial plan may now be possible after creates executed.
                let fresh_ops = self.plan_sync_ops()?;
                let (new_non_delete, new_delete) = Self::partition_ops(fresh_ops);

                for op in new_non_delete {
                    self.execute_op(op)?;
                }
                for op in new_delete {
                    self.execute_op(op)?;
                }
            }
        }

        // Drain any remaining concurrent changes even when the planner found
        // no work (e.g., everything was already in sync). They'll be picked
        // up by the next sync call's fetch_remote_changes.
        self.apply_concurrent_remote_ops();

        Ok(())
    }

    fn apply_concurrent_remote_ops(&mut self) {
        let ops: Vec<_> = self.pending_concurrent_remote_ops.drain(..).collect();
        for op in ops {
            match op {
                ConcurrentRemoteOp::CreateFile {
                    id,
                    parent_id,
                    name,
                    content,
                } => {
                    // Only create if parent still exists (a real server would
                    // reject creates into a deleted directory)
                    if self.remote.get_node(&parent_id).is_some() {
                        self.apply_remote_create_file(&id, &parent_id, name, content);
                    }
                }
                ConcurrentRemoteOp::ModifyFile { id, content } => {
                    // Silently ignore if the file no longer exists (concurrent deletion)
                    let _ = self.apply_remote_modify(&id, content);
                }
                ConcurrentRemoteOp::DeleteFile { id } => {
                    self.remote.delete_node(&id);
                }
                ConcurrentRemoteOp::TrashFile { id } => {
                    self.apply_remote_trash(&id);
                }
            }
        }
    }

    fn plan_sync_ops(&mut self) -> Result<Vec<crate::model::SyncOp>, String> {
        let planner = Planner::new(&self.store, self.sync_root.clone());
        let results = planner.plan().map_err(|e| e.to_string())?;
        let mut ops = Vec::new();
        for r in results {
            match r {
                crate::model::PlanResult::Op(op) => ops.push(op),
                crate::model::PlanResult::Conflict(ref conflict)
                    if conflict.kind == crate::model::ConflictKind::NameCollision =>
                {
                    self.resolve_name_collision(conflict)?;
                }
                _ => {}
            }
        }
        Ok(ops)
    }

    /// Resolve a `NameCollision` conflict by renaming the local file to a
    /// conflict copy (simulated by deleting and re-creating with a different
    /// name). This frees the path for the remote version to be downloaded.
    fn resolve_name_collision(&mut self, conflict: &crate::model::Conflict) -> Result<(), String> {
        let Some(local_id) = &conflict.local_id else {
            return Ok(());
        };
        let Some(node) = self.local_fs.get_node(local_id).cloned() else {
            return Ok(());
        };
        let content = self
            .local_fs
            .read_file(local_id)
            .cloned()
            .unwrap_or_default();

        // Create a conflict copy using the same naming logic as the real engine
        let conflict_local_id = self.next_local_id();
        let conflict_name = generate_conflict_name(&PathBuf::from(&node.name));
        let conflict_node = LocalNode {
            id: conflict_local_id.clone(),
            parent_id: node.parent_id.clone(),
            name: conflict_name,
            node_type: node.node_type,
            md5sum: node.md5sum.clone(),
            size: node.size,
            mtime: node.mtime,
        };
        self.local_fs
            .create_file(conflict_local_id, conflict_node.clone(), content);
        self.store
            .insert_local_node(&conflict_node)
            .map_err(|e| e.to_string())?;

        // Delete the original local file
        self.local_fs.delete(local_id);
        self.store
            .delete_local_node(local_id)
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    fn partition_ops(
        ops: Vec<crate::model::SyncOp>,
    ) -> (Vec<crate::model::SyncOp>, Vec<crate::model::SyncOp>) {
        ops.into_iter().partition(|op| {
            !matches!(
                op,
                crate::model::SyncOp::DeleteLocal { .. }
                    | crate::model::SyncOp::DeleteRemote { .. }
            )
        })
    }

    fn execute_op(&mut self, op: crate::model::SyncOp) -> Result<(), String> {
        use crate::model::SyncOp;

        // Inject transient failures: skip the operation so the planner retries later
        match &op {
            SyncOp::DownloadNew { .. }
            | SyncOp::DownloadUpdate { .. }
            | SyncOp::CreateLocalDir { .. } => {
                if self.pending_download_failures > 0 {
                    self.pending_download_failures -= 1;
                    return Ok(());
                }
            }
            SyncOp::UploadNew { .. }
            | SyncOp::UploadUpdate { .. }
            | SyncOp::CreateRemoteDir { .. } => {
                if self.pending_upload_failures > 0 {
                    self.pending_upload_failures -= 1;
                    return Ok(());
                }
            }
            _ => {}
        }

        match op {
            SyncOp::DownloadNew { remote_id, .. } => self.execute_download_new(&remote_id),
            SyncOp::CreateLocalDir { remote_id, .. } => self.execute_create_local_dir(&remote_id),
            SyncOp::UploadNew {
                local_id,
                parent_remote_id,
                name,
                expected_md5,
                ..
            } => self.execute_upload_new(&local_id, &parent_remote_id, &name, &expected_md5),
            SyncOp::CreateRemoteDir {
                local_id,
                parent_remote_id,
                name,
                ..
            } => self.execute_create_remote_dir(&local_id, &parent_remote_id, &name),
            SyncOp::DeleteLocal { local_id, .. } => self.execute_delete_local(&local_id),
            SyncOp::DeleteRemote { remote_id, .. } => self.execute_delete_remote(&remote_id),
            SyncOp::MoveLocal { local_id, .. } => self.execute_move_local(&local_id),
            SyncOp::MoveRemote {
                remote_id,
                new_parent_id,
                new_name,
                ..
            } => self.execute_move_remote(&remote_id, &new_parent_id, &new_name),
            SyncOp::DownloadUpdate {
                remote_id,
                local_id,
                ..
            } => self.execute_download_update(&remote_id, &local_id),
            SyncOp::UploadUpdate {
                local_id,
                remote_id,
                ..
            } => self.execute_upload_update(&local_id, &remote_id),
            SyncOp::BindExisting {
                local_id,
                remote_id,
                ..
            } => self.execute_bind_existing(&local_id, &remote_id),
        }
    }

    fn execute_download_new(&mut self, remote_id: &RemoteId) -> Result<(), String> {
        let Some(remote_node) = self.remote.get_node(remote_id).cloned() else {
            return Ok(());
        };
        let local_id = self.next_local_id();
        let content = self
            .remote
            .get_content(remote_id)
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
        self.remote_to_local.insert(remote_id.clone(), local_id);
        Ok(())
    }

    fn execute_create_local_dir(&mut self, remote_id: &RemoteId) -> Result<(), String> {
        let Some(remote_node) = self.remote.get_node(remote_id).cloned() else {
            return Ok(());
        };
        let local_id = self.next_local_id();

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
        self.remote_to_local.insert(remote_id.clone(), local_id);
        Ok(())
    }

    fn execute_upload_new(
        &mut self,
        local_id: &LocalFileId,
        parent_remote_id: &RemoteId,
        name: &str,
        expected_md5: &str,
    ) -> Result<(), String> {
        let Some(local_node) = self.local_fs.get_node(local_id).cloned() else {
            return Ok(());
        };
        let content = self
            .local_fs
            .read_file(local_id)
            .cloned()
            .unwrap_or_default();
        let remote_id = RemoteId::new(format!("remote-{}-{}", local_id.device_id, local_id.inode));

        let remote_node = RemoteNode {
            id: remote_id.clone(),
            parent_id: Some(parent_remote_id.clone()),
            name: name.to_string(),
            node_type: local_node.node_type,
            md5sum: Some(expected_md5.to_string()),
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
            md5sum: Some(expected_md5.to_string()),
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
        self.remote_to_local.insert(remote_id, local_id.clone());
        Ok(())
    }

    fn execute_create_remote_dir(
        &mut self,
        local_id: &LocalFileId,
        parent_remote_id: &RemoteId,
        name: &str,
    ) -> Result<(), String> {
        let Some(local_node) = self.local_fs.get_node(local_id).cloned() else {
            return Ok(());
        };
        let remote_id = RemoteId::new(format!("remote-{}-{}", local_id.device_id, local_id.inode));

        let remote_node = RemoteNode {
            id: remote_id.clone(),
            parent_id: Some(parent_remote_id.clone()),
            name: name.to_string(),
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
        self.remote_to_local.insert(remote_id, local_id.clone());
        Ok(())
    }

    fn execute_delete_local(&mut self, local_id: &LocalFileId) -> Result<(), String> {
        if let Some(synced) = self.store.get_synced_by_local(local_id).ok().flatten() {
            self.remote_to_local.remove(&synced.remote_id);
        }
        self.local_fs.delete(local_id);
        self.store
            .delete_local_node(local_id)
            .map_err(|e| e.to_string())?;
        self.store
            .delete_synced(local_id)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn execute_delete_remote(&mut self, remote_id: &RemoteId) -> Result<(), String> {
        self.remote.delete_node(remote_id);
        self.store
            .delete_remote_node(remote_id)
            .map_err(|e| e.to_string())?;
        if let Some(local_id) = self.remote_to_local.remove(remote_id) {
            self.store
                .delete_synced(&local_id)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn execute_move_local(&mut self, local_id: &LocalFileId) -> Result<(), String> {
        let Some(remote_node) = self
            .store
            .get_synced_by_local(local_id)
            .ok()
            .flatten()
            .and_then(|s| self.remote.get_node(&s.remote_id).cloned())
        else {
            return Ok(());
        };
        let new_parent_local = remote_node
            .parent_id
            .as_ref()
            .and_then(|pid| self.remote_to_local.get(pid).cloned());
        self.local_fs
            .move_node(local_id, new_parent_local.clone(), remote_node.name.clone());
        if let Some(node) = self.local_fs.get_node(local_id).cloned() {
            self.store
                .insert_local_node(&node)
                .map_err(|e| e.to_string())?;
        }
        if let Some(mut synced) = self.store.get_synced_by_local(local_id).ok().flatten() {
            synced.local_name = Some(remote_node.name.clone());
            synced.local_parent_id = new_parent_local;
            synced.remote_name = Some(remote_node.name);
            synced.remote_parent_id = remote_node.parent_id;
            synced.rel_path = self
                .local_fs
                .get_node(local_id)
                .map(|n| n.name.clone())
                .unwrap_or_default();
            self.store
                .insert_synced(&synced)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn execute_move_remote(
        &mut self,
        remote_id: &RemoteId,
        new_parent_id: &RemoteId,
        new_name: &str,
    ) -> Result<(), String> {
        self.remote
            .move_node(remote_id, new_parent_id.clone(), new_name.to_string());
        if let Some(node) = self.remote.get_node(remote_id).cloned() {
            self.store
                .insert_remote_node(&node)
                .map_err(|e| e.to_string())?;
        }
        if let Some(mut synced) = self.store.get_synced_by_remote(remote_id).ok().flatten() {
            synced.remote_name = Some(new_name.to_string());
            synced.remote_parent_id = Some(new_parent_id.clone());
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
        Ok(())
    }

    fn execute_download_update(
        &mut self,
        remote_id: &RemoteId,
        local_id: &LocalFileId,
    ) -> Result<(), String> {
        let Some(remote_node) = self.remote.get_node(remote_id).cloned() else {
            return Ok(());
        };
        let content = self
            .remote
            .get_content(remote_id)
            .cloned()
            .unwrap_or_default();
        if let Some(mut local_node) = self.local_fs.get_node(local_id).cloned() {
            local_node.md5sum.clone_from(&remote_node.md5sum);
            local_node.size = remote_node.size;
            local_node.mtime = remote_node.updated_at;
            self.local_fs
                .create_file(local_id.clone(), local_node, content);
            if let Some(node) = self.local_fs.get_node(local_id).cloned() {
                self.store
                    .insert_local_node(&node)
                    .map_err(|e| e.to_string())?;
            }
        }
        if let Some(mut synced) = self.store.get_synced_by_local(local_id).ok().flatten() {
            synced.md5sum = remote_node.md5sum;
            synced.size = remote_node.size;
            synced.rev = remote_node.rev;
            self.store
                .insert_synced(&synced)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn execute_upload_update(
        &mut self,
        local_id: &LocalFileId,
        remote_id: &RemoteId,
    ) -> Result<(), String> {
        let Some(local_node) = self.local_fs.get_node(local_id).cloned() else {
            return Ok(());
        };
        let content = self
            .local_fs
            .read_file(local_id)
            .cloned()
            .unwrap_or_default();
        if let Some(mut remote_node) = self.remote.get_node(remote_id).cloned() {
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
        if let Some(mut synced) = self.store.get_synced_by_local(local_id).ok().flatten() {
            synced.md5sum = local_node.md5sum;
            synced.size = local_node.size;
            self.store
                .insert_synced(&synced)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn execute_bind_existing(
        &self,
        local_id: &LocalFileId,
        remote_id: &RemoteId,
    ) -> Result<(), String> {
        let local_node = self
            .local_fs
            .get_node(local_id)
            .cloned()
            .ok_or_else(|| format!("BindExisting: local node {local_id} not found"))?;
        let remote_node = self
            .remote
            .get_node(remote_id)
            .cloned()
            .ok_or_else(|| format!("BindExisting: remote node {remote_id} not found"))?;

        let rel_path = self.local_path(&local_node);
        let synced = SyncedRecord {
            local_id: local_id.clone(),
            remote_id: remote_id.clone(),
            rel_path,
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

    /// Check whether a remote node's ancestor chain reaches root without a cycle.
    /// Nodes involved in or under a cycle are "unreachable" and excluded from
    /// convergence checks because the planner reports them as conflicts.
    fn remote_node_reachable(&self, node: &RemoteNode) -> bool {
        if node.parent_id.is_none() {
            return true;
        }
        let mut current = node.parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(ref pid) = current {
            if !visited.insert(pid.clone()) {
                return false;
            }
            if let Some(parent) = self.remote.get_node(pid) {
                if parent.parent_id.is_none() {
                    return true;
                }
                current.clone_from(&parent.parent_id);
            } else {
                return false;
            }
        }
        true
    }

    /// Check invariant: after sync, local and remote should have same files
    /// and directories (same paths and same content for files).
    /// Remote nodes that are unreachable due to parent cycles are excluded.
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
            .filter(|n| {
                n.is_file() && self.remote_node_reachable(n) && !self.is_trashed_or_trash_dir(n)
            })
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
            .filter(|n| {
                n.is_dir()
                    && !n.name.is_empty()
                    && self.remote_node_reachable(n)
                    && !self.is_trashed_or_trash_dir(n)
            })
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
        // CycleDetected conflicts are persistent (they remain until the
        // server resolves the cycle), so they don't indicate non-idempotency.
        let conflicts: Vec<_> = results
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    crate::model::PlanResult::Conflict(c)
                        if c.kind != crate::model::ConflictKind::CycleDetected
                )
            })
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
