use crate::error::{Error, Result};
use crate::local::scanner::Scanner;
use crate::model::{LocalFileId, NodeType, PlanResult, SyncOp, SyncedRecord};
use crate::planner::Planner;
use crate::store::TreeStore;
use crate::util::compute_md5_from_path;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The sync engine orchestrates synchronization between local and remote trees.
///
/// It holds references to the store, sync directory, and staging directory,
/// and provides methods to plan and execute sync operations.
pub struct SyncEngine {
    store: TreeStore,
    sync_dir: PathBuf,
    #[allow(dead_code)]
    staging_dir: PathBuf,
}

impl SyncEngine {
    #[must_use]
    pub const fn new(store: TreeStore, sync_dir: PathBuf, staging_dir: PathBuf) -> Self {
        Self {
            store,
            sync_dir,
            staging_dir,
        }
    }

    /// Returns a reference to the sync directory path.
    #[must_use]
    pub fn sync_dir(&self) -> &Path {
        &self.sync_dir
    }

    /// Returns a reference to the underlying store.
    #[must_use]
    pub const fn store(&self) -> &TreeStore {
        &self.store
    }

    /// Scan the local sync directory and populate the local tree.
    ///
    /// # Errors
    ///
    /// Returns an error if scanning or storage fails.
    pub fn initial_scan(&mut self) -> Result<()> {
        tracing::info!(sync_dir = %self.sync_dir.display(), "🔍 Starting initial scan");
        let scanner = Scanner::new(&self.sync_dir);
        let local_nodes = scanner.scan()?;
        let count = local_nodes.len();

        for node in &local_nodes {
            self.store.insert_local_node(node)?;
        }

        tracing::info!(count, "🔍 Initial scan found nodes");
        self.store.flush()?;
        Ok(())
    }

    /// Plan sync operations by comparing all three trees.
    ///
    /// # Errors
    ///
    /// Returns an error if store access fails.
    pub fn plan(&self) -> Result<Vec<PlanResult>> {
        let planner = Planner::new(&self.store, self.sync_dir.clone());
        planner.plan()
    }

    /// Run a full sync cycle: scan, plan, and execute all operations.
    ///
    /// Returns the plan results for inspection/logging.
    ///
    /// # Errors
    ///
    /// Returns an error if scanning, planning, or execution fails.
    pub fn run_cycle(&mut self) -> Result<Vec<PlanResult>> {
        tracing::info!("🔄 Starting sync cycle");
        self.initial_scan()?;

        let results = self.plan()?;
        let op_count = results
            .iter()
            .filter(|r| matches!(r, PlanResult::Op(_)))
            .count();
        tracing::info!(operations = op_count, "📋 Planned operations");

        for result in &results {
            match result {
                PlanResult::Op(sync_op) => {
                    self.execute_op(sync_op)?;
                }
                PlanResult::Conflict(conflict) => {
                    tracing::warn!(conflict = ?conflict, "⚠️ Conflict");
                }
                PlanResult::NoOp => {}
            }
        }

        tracing::info!("🔄 Sync cycle complete");
        Ok(results)
    }

    /// Execute a single sync operation.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn execute_op(&mut self, op: &SyncOp) -> Result<()> {
        match op {
            SyncOp::CreateLocalDir {
                remote_id,
                local_path,
            } => self.execute_create_local_dir(remote_id, local_path),

            SyncOp::DeleteLocal {
                local_id,
                local_path,
                expected_md5,
            } => self.execute_delete_local(local_id, local_path, expected_md5.as_deref()),

            SyncOp::DeleteRemote {
                remote_id,
                expected_rev: _,
            } => self.execute_delete_remote(remote_id),

            SyncOp::MoveLocal {
                local_id,
                from_path,
                to_path,
                expected_parent_id: _,
                expected_name: _,
            } => self.execute_move_local(local_id, from_path, to_path),

            // These operations require async/network - not implemented in this phase
            SyncOp::DownloadNew { .. }
            | SyncOp::DownloadUpdate { .. }
            | SyncOp::UploadNew { .. }
            | SyncOp::UploadUpdate { .. }
            | SyncOp::CreateRemoteDir { .. }
            | SyncOp::MoveRemote { .. } => {
                tracing::warn!(op = ?op, "⏭️ Operation requires async execution, skipping");
                Ok(())
            }
        }
    }

    fn execute_create_local_dir(
        &self,
        remote_id: &crate::model::RemoteId,
        local_path: &Path,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), remote_id = remote_id.as_str(), "📁 Creating local directory");
        let remote_node = self
            .store
            .get_remote_node(remote_id)?
            .ok_or_else(|| Error::NotFound(remote_id.as_str().to_string()))?;

        fs::create_dir_all(local_path)?;

        let metadata = fs::metadata(local_path)?;
        let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

        let local_parent_id = remote_node.parent_id.as_ref().and_then(|rpid| {
            self.store
                .get_synced_by_remote(rpid)
                .ok()
                .flatten()
                .map(|s| s.local_id)
        });

        let synced = SyncedRecord {
            local_id: local_id.clone(),
            remote_id: remote_id.clone(),
            rel_path: local_path
                .strip_prefix(&self.sync_dir)
                .unwrap_or(local_path)
                .to_string_lossy()
                .to_string(),
            md5sum: None,
            size: None,
            rev: remote_node.rev.clone(),
            node_type: NodeType::Directory,
            local_name: Some(remote_node.name.clone()),
            local_parent_id: local_parent_id.clone(),
            remote_name: Some(remote_node.name.clone()),
            remote_parent_id: remote_node.parent_id.clone(),
        };

        let local_node = crate::model::LocalNode {
            id: local_id,
            parent_id: local_parent_id,
            name: remote_node.name,
            node_type: NodeType::Directory,
            md5sum: None,
            size: None,
            mtime: metadata.mtime(),
        };

        self.store.insert_local_node(&local_node)?;
        self.store.insert_synced(&synced)?;
        self.store.flush()?;

        Ok(())
    }

    fn execute_delete_local(
        &self,
        local_id: &LocalFileId,
        local_path: &Path,
        expected_md5: Option<&str>,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), "🗑️ Deleting local entry");

        if local_path.is_file() {
            if let Some(expected) = expected_md5 {
                let actual = compute_md5_from_path(local_path)?;
                if actual != expected {
                    return Err(Error::Conflict(format!(
                        "File {} was modified (expected md5 {expected}, got {actual})",
                        local_path.display()
                    )));
                }
            }
            fs::remove_file(local_path)?;
        } else if local_path.is_dir() {
            fs::remove_dir_all(local_path)?;
        }

        self.store.delete_local_node(local_id)?;
        self.store.delete_synced(local_id)?;
        self.store.flush()?;

        Ok(())
    }

    fn execute_move_local(
        &self,
        local_id: &LocalFileId,
        from_path: &Path,
        to_path: &Path,
    ) -> Result<()> {
        tracing::info!(from = %from_path.display(), to = %to_path.display(), "📦 Moving local file");

        if from_path.exists() {
            let metadata = fs::symlink_metadata(from_path)?;
            let actual_id = LocalFileId::new(metadata.dev(), metadata.ino());
            if actual_id != *local_id {
                return Err(Error::Conflict(format!(
                    "File {} identity changed (expected {:?}, got {:?})",
                    from_path.display(),
                    local_id,
                    actual_id
                )));
            }
        }

        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from_path, to_path)?;

        if let Some(mut node) = self.store.get_local_node(local_id)? {
            node.name = to_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(parent_path) = to_path.parent() {
                let parent_meta = fs::metadata(parent_path)?;
                node.parent_id = Some(LocalFileId::new(parent_meta.dev(), parent_meta.ino()));
            }
            self.store.insert_local_node(&node)?;

            if let Some(mut synced) = self.store.get_synced_by_local(local_id)? {
                synced.local_name = Some(node.name.clone());
                synced.local_parent_id.clone_from(&node.parent_id);
                if let Ok(Some(remote_node)) = self.store.get_remote_node(&synced.remote_id) {
                    synced.remote_name = Some(remote_node.name.clone());
                    synced.remote_parent_id.clone_from(&remote_node.parent_id);
                }
                synced.rel_path = to_path
                    .strip_prefix(&self.sync_dir)
                    .unwrap_or(to_path)
                    .to_string_lossy()
                    .to_string();
                self.store.insert_synced(&synced)?;
            }
        }

        self.store.flush()?;
        Ok(())
    }

    fn execute_delete_remote(&self, remote_id: &crate::model::RemoteId) -> Result<()> {
        tracing::info!(remote_id = remote_id.as_str(), "🗑️ Deleting remote entry");
        self.store.delete_remote_node(remote_id)?;

        // Also remove synced record if it exists
        if let Some(synced) = self.store.get_synced_by_remote(remote_id)? {
            self.store.delete_synced(&synced.local_id)?;
        }

        self.store.flush()?;
        Ok(())
    }
}
