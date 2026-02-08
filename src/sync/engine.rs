use crate::error::{Error, Result};
use crate::local::scanner::Scanner;
use crate::model::{LocalFileId, NodeType, PlanResult, SyncOp, SyncedRecord};
use crate::planner::Planner;
use crate::store::TreeStore;
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
                expected_md5: _,
            } => self.execute_delete_local(local_id, local_path),

            SyncOp::DeleteRemote {
                remote_id,
                expected_rev: _,
            } => self.execute_delete_remote(remote_id),

            // These operations require async/network - not implemented in this phase
            SyncOp::DownloadNew { .. }
            | SyncOp::DownloadUpdate { .. }
            | SyncOp::UploadNew { .. }
            | SyncOp::UploadUpdate { .. }
            | SyncOp::CreateRemoteDir { .. }
            | SyncOp::MoveLocal { .. }
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

        // Get the inode of the newly created directory
        let metadata = fs::metadata(local_path)?;
        let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

        // Create synced record
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
        };

        // Create local node
        let local_node = crate::model::LocalNode {
            id: local_id,
            parent_id: None, // TODO: compute parent
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

    fn execute_delete_local(&self, local_id: &LocalFileId, local_path: &Path) -> Result<()> {
        tracing::info!(path = %local_path.display(), "🗑️ Deleting local entry");
        if local_path.is_dir() {
            fs::remove_dir_all(local_path)?;
        } else if local_path.exists() {
            fs::remove_file(local_path)?;
        }

        self.store.delete_local_node(local_id)?;
        self.store.delete_synced(local_id)?;
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
