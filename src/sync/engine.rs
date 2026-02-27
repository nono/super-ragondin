use crate::error::{Error, Result};
use crate::local::scanner::Scanner;
use crate::model::{LocalFileId, LocalNode, NodeType, PlanResult, RemoteId, SyncOp, SyncedRecord};
use crate::planner::Planner;
use crate::remote::client::CozyClient;
use crate::store::TreeStore;
use crate::util::{compute_md5_from_bytes, compute_md5_from_path};
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

        self.bootstrap_root()?;

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

    /// Bootstrap the root synced record and local node for the sync directory.
    ///
    /// This maps the sync directory's filesystem identity (`device_id`, `inode`)
    /// to the well-known Cozy root remote ID `io.cozy.files.root-dir`.
    fn bootstrap_root(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.sync_dir)?;
        let root_local_id = LocalFileId::new(metadata.dev(), metadata.ino());

        if self.store.get_synced_by_local(&root_local_id)?.is_some() {
            return Ok(());
        }

        tracing::info!("🌱 Bootstrapping root synced record");

        let root_local = LocalNode {
            id: root_local_id.clone(),
            parent_id: None,
            name: String::new(),
            node_type: NodeType::Directory,
            md5sum: None,
            size: None,
            mtime: metadata.mtime(),
        };
        self.store.insert_local_node(&root_local)?;

        let root_synced = SyncedRecord {
            local_id: root_local_id,
            remote_id: RemoteId::new("io.cozy.files.root-dir"),
            rel_path: String::new(),
            md5sum: None,
            size: None,
            rev: String::new(),
            node_type: NodeType::Directory,
            local_name: Some(String::new()),
            local_parent_id: None,
            remote_name: Some(String::new()),
            remote_parent_id: None,
        };
        self.store.insert_synced(&root_synced)?;

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

    /// Run a full sync cycle with async network support: scan, plan, and execute all operations.
    ///
    /// # Errors
    ///
    /// Returns an error if scanning, planning, or execution fails.
    pub async fn run_cycle_async(&mut self, client: &CozyClient) -> Result<Vec<PlanResult>> {
        tracing::info!("🔄 Starting async sync cycle");
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
                    self.execute_op_async(client, sync_op).await?;
                }
                PlanResult::Conflict(conflict) => {
                    tracing::warn!(conflict = ?conflict, "⚠️ Conflict");
                }
                PlanResult::NoOp => {}
            }
        }

        tracing::info!("🔄 Async sync cycle complete");
        Ok(results)
    }

    /// Execute a single sync operation, using the client for network ops.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn execute_op_async(&mut self, client: &CozyClient, op: &SyncOp) -> Result<()> {
        match op {
            SyncOp::CreateLocalDir { .. }
            | SyncOp::DeleteLocal { .. }
            | SyncOp::MoveLocal { .. } => self.execute_op(op),

            SyncOp::DeleteRemote { remote_id, .. } => {
                client.trash(remote_id).await?;
                self.execute_delete_remote(remote_id)
            }

            SyncOp::DownloadNew {
                remote_id,
                local_path,
                expected_md5,
                ..
            } => {
                self.execute_download_new(client, remote_id, local_path, expected_md5)
                    .await
            }

            SyncOp::DownloadUpdate {
                remote_id,
                local_id,
                local_path,
                expected_remote_md5,
                expected_local_md5,
                ..
            } => {
                self.execute_download_update(
                    client,
                    remote_id,
                    local_id,
                    local_path,
                    expected_remote_md5,
                    expected_local_md5,
                )
                .await
            }

            SyncOp::UploadNew {
                local_id,
                local_path,
                parent_remote_id,
                name,
                expected_md5,
            } => {
                self.execute_upload_new(
                    client,
                    local_id,
                    local_path,
                    parent_remote_id,
                    name,
                    expected_md5,
                )
                .await
            }

            SyncOp::UploadUpdate {
                local_id,
                remote_id,
                local_path,
                expected_local_md5,
                expected_rev,
            } => {
                self.execute_upload_update(
                    client,
                    local_id,
                    remote_id,
                    local_path,
                    expected_local_md5,
                    expected_rev,
                )
                .await
            }

            SyncOp::CreateRemoteDir {
                local_id,
                local_path,
                parent_remote_id,
                name,
            } => {
                self.execute_create_remote_dir(client, local_id, local_path, parent_remote_id, name)
                    .await
            }

            SyncOp::MoveRemote {
                remote_id,
                new_parent_id,
                new_name,
                ..
            } => {
                self.execute_move_remote(client, remote_id, new_parent_id, new_name)
                    .await
            }
        }
    }

    /// Execute a single sync operation (local-only; network ops are skipped).
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

    fn execute_delete_remote(&self, remote_id: &RemoteId) -> Result<()> {
        tracing::info!(remote_id = remote_id.as_str(), "🗑️ Deleting remote entry");
        self.store.delete_remote_node(remote_id)?;

        if let Some(synced) = self.store.get_synced_by_remote(remote_id)? {
            self.store.delete_synced(&synced.local_id)?;
        }

        self.store.flush()?;
        Ok(())
    }

    async fn execute_move_remote(
        &self,
        client: &CozyClient,
        remote_id: &RemoteId,
        new_parent_id: &RemoteId,
        new_name: &str,
    ) -> Result<()> {
        tracing::info!(
            remote_id = remote_id.as_str(),
            new_name,
            "📦 Moving remote node"
        );
        let updated = client.move_node(remote_id, new_parent_id, new_name).await?;
        self.store.insert_remote_node(&updated)?;
        if let Some(mut synced) = self.store.get_synced_by_remote(remote_id)? {
            synced.remote_name = Some(updated.name);
            synced.remote_parent_id = updated.parent_id;
            synced.rev = updated.rev;
            self.store.insert_synced(&synced)?;
        }
        self.store.flush()?;
        Ok(())
    }

    // ==================== Async network operations ====================

    async fn execute_download_new(
        &self,
        client: &CozyClient,
        remote_id: &RemoteId,
        local_path: &Path,
        expected_md5: &str,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), remote_id = remote_id.as_str(), "📥 Downloading new file");

        let bytes = client.download_file(remote_id).await?;
        let actual_md5 = compute_md5_from_bytes(&bytes);
        if !expected_md5.is_empty() && actual_md5 != expected_md5 {
            return Err(Error::Conflict(format!(
                "Downloaded file md5 mismatch: expected {expected_md5}, got {actual_md5}"
            )));
        }

        self.write_via_staging(local_path, &bytes)?;

        let metadata = fs::metadata(local_path)?;
        let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

        let remote_node = self
            .store
            .get_remote_node(remote_id)?
            .ok_or_else(|| Error::NotFound(remote_id.as_str().to_string()))?;

        let local_parent_id = remote_node.parent_id.as_ref().and_then(|rpid| {
            self.store
                .get_synced_by_remote(rpid)
                .ok()
                .flatten()
                .map(|s| s.local_id)
        });

        let local_node = LocalNode {
            id: local_id.clone(),
            parent_id: local_parent_id.clone(),
            name: remote_node.name.clone(),
            node_type: NodeType::File,
            md5sum: Some(actual_md5.clone()),
            size: Some(bytes.len() as u64),
            mtime: metadata.mtime(),
        };
        self.store.insert_local_node(&local_node)?;

        let synced = SyncedRecord {
            local_id,
            remote_id: remote_id.clone(),
            rel_path: local_path
                .strip_prefix(&self.sync_dir)
                .unwrap_or(local_path)
                .to_string_lossy()
                .to_string(),
            md5sum: Some(actual_md5),
            size: Some(bytes.len() as u64),
            rev: remote_node.rev.clone(),
            node_type: NodeType::File,
            local_name: Some(remote_node.name.clone()),
            local_parent_id,
            remote_name: Some(remote_node.name),
            remote_parent_id: remote_node.parent_id,
        };
        self.store.insert_synced(&synced)?;
        self.store.flush()?;
        Ok(())
    }

    async fn execute_download_update(
        &self,
        client: &CozyClient,
        remote_id: &RemoteId,
        local_id: &LocalFileId,
        local_path: &Path,
        expected_remote_md5: &str,
        expected_local_md5: &str,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), "📥 Updating local file from remote");

        // Check local file hasn't changed since planning
        if local_path.is_file() && !expected_local_md5.is_empty() {
            let actual = compute_md5_from_path(local_path)?;
            if actual != expected_local_md5 {
                return Err(Error::Conflict(format!(
                    "Local file {} was modified (expected md5 {expected_local_md5}, got {actual})",
                    local_path.display()
                )));
            }
        }

        let bytes = client.download_file(remote_id).await?;
        let actual_md5 = compute_md5_from_bytes(&bytes);
        if !expected_remote_md5.is_empty() && actual_md5 != expected_remote_md5 {
            return Err(Error::Conflict(format!(
                "Downloaded file md5 mismatch: expected {expected_remote_md5}, got {actual_md5}"
            )));
        }

        self.write_via_staging(local_path, &bytes)?;

        if let Some(mut node) = self.store.get_local_node(local_id)? {
            node.md5sum = Some(actual_md5.clone());
            node.size = Some(bytes.len() as u64);
            node.mtime = fs::metadata(local_path)?.mtime();
            self.store.insert_local_node(&node)?;
        }

        if let Some(mut synced) = self.store.get_synced_by_local(local_id)? {
            synced.md5sum = Some(actual_md5);
            synced.size = Some(bytes.len() as u64);
            if let Some(remote) = self.store.get_remote_node(remote_id)? {
                synced.rev = remote.rev;
            }
            self.store.insert_synced(&synced)?;
        }

        self.store.flush()?;
        Ok(())
    }

    async fn execute_upload_new(
        &self,
        client: &CozyClient,
        local_id: &LocalFileId,
        local_path: &Path,
        parent_remote_id: &RemoteId,
        name: &str,
        expected_md5: &str,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), name, "📤 Uploading new file");

        let content = fs::read(local_path)?;
        let actual_md5 = compute_md5_from_bytes(&content);
        if !expected_md5.is_empty() && actual_md5 != expected_md5 {
            return Err(Error::Conflict(format!(
                "Local file {} was modified (expected md5 {expected_md5}, got {actual_md5})",
                local_path.display()
            )));
        }

        let remote_node = client
            .upload_file(parent_remote_id, name, content.clone(), &actual_md5)
            .await?;

        self.store.insert_remote_node(&remote_node)?;

        let synced = SyncedRecord {
            local_id: local_id.clone(),
            remote_id: remote_node.id.clone(),
            rel_path: local_path
                .strip_prefix(&self.sync_dir)
                .unwrap_or(local_path)
                .to_string_lossy()
                .to_string(),
            md5sum: Some(actual_md5),
            size: Some(content.len() as u64),
            rev: remote_node.rev,
            node_type: NodeType::File,
            local_name: Some(name.to_string()),
            local_parent_id: self
                .store
                .get_local_node(local_id)?
                .and_then(|n| n.parent_id),
            remote_name: Some(remote_node.name),
            remote_parent_id: remote_node.parent_id,
        };
        self.store.insert_synced(&synced)?;
        self.store.flush()?;
        Ok(())
    }

    async fn execute_upload_update(
        &self,
        client: &CozyClient,
        local_id: &LocalFileId,
        remote_id: &RemoteId,
        local_path: &Path,
        expected_local_md5: &str,
        expected_rev: &str,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), "📤 Updating remote file");

        let content = fs::read(local_path)?;
        let actual_md5 = compute_md5_from_bytes(&content);
        if !expected_local_md5.is_empty() && actual_md5 != expected_local_md5 {
            return Err(Error::Conflict(format!(
                "Local file {} was modified (expected md5 {expected_local_md5}, got {actual_md5})",
                local_path.display()
            )));
        }

        let updated = client
            .update_file(remote_id, content.clone(), &actual_md5, expected_rev)
            .await?;

        self.store.insert_remote_node(&updated)?;

        if let Some(mut synced) = self.store.get_synced_by_local(local_id)? {
            synced.md5sum = Some(actual_md5);
            synced.size = Some(content.len() as u64);
            synced.rev = updated.rev;
            self.store.insert_synced(&synced)?;
        }

        self.store.flush()?;
        Ok(())
    }

    async fn execute_create_remote_dir(
        &self,
        client: &CozyClient,
        local_id: &LocalFileId,
        local_path: &Path,
        parent_remote_id: &RemoteId,
        name: &str,
    ) -> Result<()> {
        tracing::info!(path = %local_path.display(), name, "📁 Creating remote directory");

        let remote_node = client.create_directory(parent_remote_id, name).await?;
        self.store.insert_remote_node(&remote_node)?;

        let synced = SyncedRecord {
            local_id: local_id.clone(),
            remote_id: remote_node.id.clone(),
            rel_path: local_path
                .strip_prefix(&self.sync_dir)
                .unwrap_or(local_path)
                .to_string_lossy()
                .to_string(),
            md5sum: None,
            size: None,
            rev: remote_node.rev,
            node_type: NodeType::Directory,
            local_name: Some(name.to_string()),
            local_parent_id: self
                .store
                .get_local_node(local_id)?
                .and_then(|n| n.parent_id),
            remote_name: Some(remote_node.name),
            remote_parent_id: remote_node.parent_id,
        };
        self.store.insert_synced(&synced)?;
        self.store.flush()?;
        Ok(())
    }

    // ==================== Helpers ====================

    fn write_via_staging(&self, target: &Path, content: &[u8]) -> Result<()> {
        let staging_path = self.staging_dir.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&self.staging_dir)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&staging_path, content)?;
        fs::rename(&staging_path, target)?;
        Ok(())
    }
}
