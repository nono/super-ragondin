use crate::error::{Error, Result};
use crate::ignore::IgnoreRules;
use crate::local::scanner::Scanner;
use crate::model::{
    Conflict, ConflictKind, LocalFileId, LocalNode, NodeType, PlanResult, RemoteId, RemoteNode,
    SyncOp, SyncedRecord, TRASH_DIR_ID,
};
use crate::planner::Planner;
use crate::remote::client::CozyClient;
use crate::store::TreeStore;
use crate::sync::conflict_name::generate_conflict_name;
use crate::util::{compute_md5_from_bytes, compute_md5_from_path};
use std::collections::HashSet;
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
    rules: IgnoreRules,
}

impl SyncEngine {
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(
        store: TreeStore,
        sync_dir: PathBuf,
        staging_dir: PathBuf,
        rules: IgnoreRules,
    ) -> Self {
        Self {
            store,
            sync_dir,
            staging_dir,
            rules,
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

    /// Returns a reference to the ignore rules.
    #[must_use]
    pub const fn rules(&self) -> &IgnoreRules {
        &self.rules
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
        let local_nodes = scanner.scan_with_ignore(&self.rules)?;
        let count = local_nodes.len();

        // Safety check: if the scanner found zero files but we have previously
        // synced records, the sync directory is likely on an unmounted drive or
        // otherwise inaccessible.  Abort to prevent the planner from generating
        // DeleteRemote ops for every known file, which would wipe the user's
        // remote data.
        let non_root_synced = self
            .store
            .list_all_synced()?
            .into_iter()
            .filter(|r| !r.rel_path.is_empty())
            .count();
        if local_nodes.is_empty() && non_root_synced > 0 {
            tracing::error!(
                synced_count = non_root_synced,
                "🚨 Sync directory appears empty but {non_root_synced} files are known — aborting"
            );
            return Err(Error::EmptySyncDir {
                synced_count: non_root_synced,
            });
        }

        for node in &local_nodes {
            self.store.insert_local_node(node)?;
        }

        // Remove stale local nodes — nodes that were in the store from a previous
        // session but are no longer present on disk (e.g. deleted while stopped).
        // The root node is not returned by the scanner so we preserve it explicitly.
        let root_meta = fs::symlink_metadata(&self.sync_dir)?;
        let root_local_id = LocalFileId::new(root_meta.dev(), root_meta.ino());
        let scanned_ids: HashSet<LocalFileId> = local_nodes
            .iter()
            .map(|n| n.id.clone())
            .chain(std::iter::once(root_local_id))
            .collect();
        let stale_ids: Vec<LocalFileId> = self
            .store
            .list_all_local()?
            .into_iter()
            .map(|n| n.id)
            .filter(|id| !scanned_ids.contains(id))
            .collect();
        if !stale_ids.is_empty() {
            tracing::info!(count = stale_ids.len(), "🧹 Removing stale local nodes");
            for id in &stale_ids {
                self.store.delete_local_node(id)?;
                // Clear location fields on the associated synced record so that
                // reconcile_inodes (keyed on local_name + local_parent_id) does
                // not mistakenly treat a new local node at the same path as an
                // atomic save.  The synced record is kept so the planner can
                // still generate DeleteRemote for the now-missing local file.
                if let Some(synced) = self.store.get_synced_by_local(id)? {
                    let updated = SyncedRecord {
                        local_name: None,
                        local_parent_id: None,
                        ..synced
                    };
                    self.store.delete_synced(id)?;
                    self.store.insert_synced(&updated)?;
                }
            }
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
        let planner = Planner::new(&self.store, self.sync_dir.clone(), &self.rules);
        planner.plan()
    }

    /// Run a full sync cycle: scan, plan, and execute all operations.
    ///
    /// Execution uses two phases to prevent cascade deletions from orphaning
    /// files that were moved out of a directory being deleted:
    ///   Phase 1: execute creates, moves, downloads, uploads (non-delete ops)
    ///   Phase 2: re-plan, then execute all remaining ops including deletes
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

        let mut all_results = Vec::new();
        let (non_delete, delete) = Self::partition_results(results);

        // Phase 1: resolve conflicts first, then re-plan and execute non-delete ops
        let conflicts = Self::extract_conflicts(&non_delete);

        let phase1_results = if conflicts.is_empty() {
            non_delete
        } else {
            for conflict in &conflicts {
                self.resolve_conflict(conflict)?;
            }
            self.initial_scan()?;
            let replanned = self.plan()?;
            let (replanned_non_delete, _) = Self::partition_results(replanned);
            replanned_non_delete
        };

        for result in &phase1_results {
            if let PlanResult::Op(sync_op) = result {
                self.execute_op(sync_op)?;
            }
        }
        all_results.extend(phase1_results);

        // Phase 2: re-plan before deletes so that moves/creates executed in
        // phase 1 are taken into account (e.g. a file moved out of a directory
        // that is about to be deleted).
        if !delete.is_empty() {
            self.initial_scan()?;
            let fresh = self.plan()?;

            let fresh_conflicts = Self::extract_conflicts(&fresh);

            let phase2_results = if fresh_conflicts.is_empty() {
                fresh
            } else {
                for conflict in &fresh_conflicts {
                    self.resolve_conflict(conflict)?;
                }
                self.initial_scan()?;
                self.plan()?
            };

            for result in &phase2_results {
                if let PlanResult::Op(sync_op) = result {
                    self.execute_op(sync_op)?;
                }
            }
            all_results.extend(phase2_results);
        }

        tracing::info!("🔄 Sync cycle complete");
        Ok(all_results)
    }

    /// Fetch remote changes and apply them to the remote tree.
    ///
    /// Inserts new/updated nodes and removes deleted ones. Returns the new
    /// `last_seq` value for incremental fetches.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request or store operations fail.
    #[tracing::instrument(skip(self, client), fields(since = ?since))]
    pub async fn fetch_and_apply_remote_changes(
        &self,
        client: &CozyClient,
        since: Option<&str>,
    ) -> Result<String> {
        let changes = client.fetch_changes(since).await?;

        for result in &changes.results {
            if result.deleted {
                self.store.delete_remote_node(&result.node.id)?;
            } else if result.node.id.as_str() == TRASH_DIR_ID {
                // Skip the trash directory itself — it should never be synced
                tracing::debug!("🗑️ Skipping trash directory");
            } else if self.is_in_trash(&result.node) {
                // Nodes moved under the trash are treated as remote deletions
                tracing::debug!(
                    id = result.node.id.as_str(),
                    "🗑️ Node is in trash, treating as deletion"
                );
                self.store.delete_remote_node(&result.node.id)?;
            } else if result.node.node_type == NodeType::File && result.node.md5sum.is_none() {
                // Skip files without a checksum — they represent incomplete
                // uploads by another client and would produce garbage if
                // downloaded.  They will reappear in a future changes feed
                // once the upload completes and the server computes the hash.
                tracing::debug!(
                    id = result.node.id.as_str(),
                    name = &result.node.name,
                    "⏭️ Skipping remote file without checksum (incomplete upload)"
                );
            } else {
                self.ensure_remote_parent_exists(&result.node)?;
                self.store.insert_remote_node(&result.node)?;
            }
        }

        self.store.flush()?;
        tracing::debug!(changes_applied = changes.results.len(), last_seq = %changes.last_seq, "applied remote changes");
        Ok(changes.last_seq)
    }

    /// Check whether a remote node is inside the trash directory.
    ///
    /// Walks up the parent chain to detect nodes directly or transitively
    /// under the trash.
    fn is_in_trash(&self, node: &RemoteNode) -> bool {
        let trash_id = RemoteId::new(TRASH_DIR_ID);
        let mut current = node.parent_id.clone();
        while let Some(ref pid) = current {
            if *pid == trash_id {
                return true;
            }
            match self.store.get_remote_node(pid) {
                Ok(Some(parent)) => current.clone_from(&parent.parent_id),
                _ => return false,
            }
        }
        false
    }

    /// Ensure that a remote node's parent directory exists in the store.
    ///
    /// When the parent is the well-known root directory and is not yet in the
    /// store, a synthetic root node is created so that child nodes are never
    /// orphaned.
    fn ensure_remote_parent_exists(&self, node: &RemoteNode) -> Result<()> {
        let Some(parent_id) = &node.parent_id else {
            return Ok(());
        };
        if self.store.get_remote_node(parent_id)?.is_some() {
            return Ok(());
        }
        if parent_id.as_str() == "io.cozy.files.root-dir" {
            let root = RemoteNode {
                id: parent_id.clone(),
                parent_id: None,
                name: String::new(),
                node_type: NodeType::Directory,
                md5sum: None,
                size: None,
                updated_at: 0,
                rev: String::new(),
            };
            self.store.insert_remote_node(&root)?;
        }
        Ok(())
    }

    /// Run a full sync cycle with async network support: scan, plan, and execute all operations.
    ///
    /// Execution uses two phases to prevent cascade deletions from orphaning
    /// files that were moved out of a directory being deleted:
    ///   Phase 1: execute creates, moves, downloads, uploads (non-delete ops)
    ///   Phase 2: re-plan, then execute all remaining ops including deletes
    ///
    /// # Errors
    ///
    /// Returns an error if scanning, planning, or execution fails.
    #[tracing::instrument(skip(self, client))]
    pub async fn run_cycle_async(&mut self, client: &CozyClient) -> Result<Vec<PlanResult>> {
        tracing::info!("🔄 Starting async sync cycle");
        self.initial_scan()?;

        let results = self.plan()?;
        let op_count = results
            .iter()
            .filter(|r| matches!(r, PlanResult::Op(_)))
            .count();
        tracing::info!(operations = op_count, "📋 Planned operations");

        let mut all_results = Vec::new();

        // Resolve conflicts first, then re-plan
        let conflicts = Self::extract_conflicts(&results);

        let plan_results = if conflicts.is_empty() {
            results
        } else {
            for conflict in &conflicts {
                self.resolve_conflict_async(client, conflict).await?;
            }
            self.plan()?
        };

        let (non_delete, delete) = Self::partition_results(plan_results);

        // Phase 1: execute non-delete ops
        let non_delete_ops = Self::extract_ops(&non_delete);
        self.execute_ops_async(client, &non_delete_ops).await?;
        all_results.extend(non_delete);

        // Phase 2: re-plan before deletes
        if !delete.is_empty() {
            self.initial_scan()?;
            let fresh = self.plan()?;

            let fresh_conflicts = Self::extract_conflicts(&fresh);

            let phase2_results = if fresh_conflicts.is_empty() {
                fresh
            } else {
                for conflict in &fresh_conflicts {
                    self.resolve_conflict_async(client, conflict).await?;
                }
                self.initial_scan()?;
                self.plan()?
            };

            for result in &phase2_results {
                match result {
                    PlanResult::Op(op) => {
                        self.execute_op_async(client, op).await?;
                    }
                    PlanResult::Conflict(conflict) => {
                        self.resolve_conflict_async(client, conflict).await?;
                    }
                    PlanResult::NoOp => {}
                }
            }
            all_results.extend(phase2_results);
        }

        tracing::info!("🔄 Async sync cycle complete");
        Ok(all_results)
    }

    /// Execute a batch of sync operations, running transfers concurrently.
    async fn execute_ops_async(&self, client: &CozyClient, ops: &[SyncOp]) -> Result<()> {
        use futures::stream::{self, StreamExt as _};

        let mut i = 0;
        while i < ops.len() {
            if ops[i].is_transfer() {
                let start = i;
                while i < ops.len() && ops[i].is_transfer() {
                    i += 1;
                }
                let engine_ref = self;
                let mut stream = stream::iter(ops[start..i].iter())
                    .map(|op| engine_ref.execute_op_async(client, op))
                    .buffer_unordered(2);
                while let Some(result) = stream.next().await {
                    result?;
                }
            } else {
                self.execute_op_async(client, &ops[i]).await?;
                i += 1;
            }
        }
        Ok(())
    }

    /// Execute a single sync operation, using the client for network ops.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub async fn execute_op_async(&self, client: &CozyClient, op: &SyncOp) -> Result<()> {
        tracing::debug!(op = ?op, "executing sync op");
        match op {
            SyncOp::CreateLocalDir { .. }
            | SyncOp::DeleteLocal { .. }
            | SyncOp::MoveLocal { .. }
            | SyncOp::BindExisting { .. }
            | SyncOp::DeleteSynced { .. } => self.execute_op(op),

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
    pub fn execute_op(&self, op: &SyncOp) -> Result<()> {
        tracing::debug!(op = ?op, "executing sync op");
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

            SyncOp::BindExisting {
                local_id,
                remote_id,
                local_path,
            } => self.execute_bind_existing(local_id, remote_id, local_path),

            SyncOp::DeleteSynced { local_id } => {
                tracing::debug!(%local_id, "🧹 Deleting orphaned synced record");
                self.store.delete_synced(local_id)
            }

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

    fn execute_bind_existing(
        &self,
        local_id: &LocalFileId,
        remote_id: &RemoteId,
        local_path: &Path,
    ) -> Result<()> {
        tracing::info!(
            path = %local_path.display(),
            remote_id = remote_id.as_str(),
            "🔗 Binding existing local and remote nodes"
        );

        let remote_node = self
            .store
            .get_remote_node(remote_id)?
            .ok_or_else(|| Error::NotFound(remote_id.as_str().to_string()))?;

        let local_node = self
            .store
            .get_local_node(local_id)?
            .ok_or_else(|| Error::NotFound(format!("{local_id}")))?;

        let synced = SyncedRecord {
            local_id: local_id.clone(),
            remote_id: remote_id.clone(),
            rel_path: local_path
                .strip_prefix(&self.sync_dir)
                .unwrap_or(local_path)
                .to_string_lossy()
                .to_string(),
            md5sum: remote_node.md5sum.clone(),
            size: remote_node.size,
            rev: remote_node.rev.clone(),
            node_type: remote_node.node_type,
            local_name: Some(local_node.name.clone()),
            local_parent_id: local_node.parent_id,
            remote_name: Some(remote_node.name.clone()),
            remote_parent_id: remote_node.parent_id,
        };

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

        // Try to reuse an existing local file with the same checksum, falling back to download.
        let (actual_md5, file_size) =
            if !expected_md5.is_empty() && self.try_reuse_local_file(expected_md5, local_path)? {
                let meta = fs::metadata(local_path)?;
                (expected_md5.to_string(), meta.len())
            } else {
                let bytes = client.download_file(remote_id).await?;
                let actual_md5 = compute_md5_from_bytes(&bytes);
                if !expected_md5.is_empty() && actual_md5 != expected_md5 {
                    return Err(Error::Conflict(format!(
                        "Downloaded file md5 mismatch: expected {expected_md5}, got {actual_md5}"
                    )));
                }
                let size = bytes.len() as u64;
                self.write_via_staging(local_path, &bytes)?;
                (actual_md5, size)
            };

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
            size: Some(file_size),
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
            size: Some(file_size),
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

        // Try to reuse an existing local file with the same checksum, falling back to download.
        let (actual_md5, file_size) = if !expected_remote_md5.is_empty()
            && self.try_reuse_local_file(expected_remote_md5, local_path)?
        {
            let meta = fs::metadata(local_path)?;
            (expected_remote_md5.to_string(), meta.len())
        } else {
            let bytes = client.download_file(remote_id).await?;
            let actual_md5 = compute_md5_from_bytes(&bytes);
            if !expected_remote_md5.is_empty() && actual_md5 != expected_remote_md5 {
                return Err(Error::Conflict(format!(
                    "Downloaded file md5 mismatch: expected {expected_remote_md5}, got {actual_md5}"
                )));
            }
            let size = bytes.len() as u64;
            self.write_via_staging(local_path, &bytes)?;
            (actual_md5, size)
        };

        if let Some(mut node) = self.store.get_local_node(local_id)? {
            node.md5sum = Some(actual_md5.clone());
            node.size = Some(file_size);
            node.mtime = fs::metadata(local_path)?.mtime();
            self.store.insert_local_node(&node)?;
        }

        if let Some(mut synced) = self.store.get_synced_by_local(local_id)? {
            synced.md5sum = Some(actual_md5);
            synced.size = Some(file_size);
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

    // ==================== Conflict resolution ====================

    /// Resolve a conflict with versioning awareness.
    ///
    /// For `BothModified` conflicts: checks whether the local file's content
    /// was already stored as an old version on the remote. If so, the conflict
    /// is a false positive (e.g., edit-save-edit-before-sync-completes) and
    /// the remote version is silently accepted without creating a conflict copy.
    ///
    /// Falls back to `resolve_conflict` (rename to conflict copy) when no
    /// version match is found or for non-`BothModified` conflicts.
    ///
    /// # Errors
    ///
    /// Returns an error if network requests or file operations fail.
    pub async fn resolve_conflict_async(
        &self,
        client: &CozyClient,
        conflict: &Conflict,
    ) -> Result<()> {
        if conflict.kind != ConflictKind::BothModified {
            return self.resolve_conflict(conflict);
        }

        let (Some(local_path), Some(remote_id)) = (&conflict.local_path, &conflict.remote_id)
        else {
            return self.resolve_conflict(conflict);
        };

        if !local_path.is_file() {
            return self.resolve_conflict(conflict);
        }

        let Ok(local_md5) = compute_md5_from_path(local_path) else {
            return self.resolve_conflict(conflict);
        };

        let old_md5sums = match client.fetch_old_version_md5sums(remote_id).await {
            Ok(sums) => sums,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "⚠️ Failed to fetch old versions, falling back to conflict copy"
                );
                return self.resolve_conflict(conflict);
            }
        };

        if old_md5sums.iter().any(|md5| md5 == &local_md5) {
            tracing::info!(
                path = %local_path.display(),
                local_md5 = &local_md5,
                "✅ Local content matches an old remote version, accepting remote"
            );

            // Delete the local file first so the next cycle downloads the remote version.
            // We do this before updating the store: if the fs operation fails, the store
            // remains consistent and we avoid spurious uploads of a still-existing file.
            if local_path.is_file() {
                fs::remove_file(local_path)?;
            }

            if let Some(local_id) = &conflict.local_id {
                self.store.delete_synced(local_id)?;
                self.store.delete_local_node(local_id)?;
                self.store.flush()?;
            }

            return Ok(());
        }

        self.resolve_conflict(conflict)
    }

    /// Resolve a conflict by creating a renamed copy of the local file.
    ///
    /// For `BothModified` conflicts: renames the local file to a conflict copy
    /// (e.g., `file-conflict-2024-01-15T12_30_45.123Z.ext`), freeing the
    /// original path for the remote version to be downloaded on the next cycle.
    ///
    /// # Errors
    ///
    /// Returns an error if the file rename fails.
    pub fn resolve_conflict(&self, conflict: &Conflict) -> Result<()> {
        // ParentMissing is a transient condition: the parent directory hasn't
        // been synced yet. The file will be uploaded on the next cycle once the
        // parent exists — no rename needed.
        if conflict.kind == ConflictKind::ParentMissing {
            tracing::info!(
                conflict = ?conflict,
                "⏳ Parent not synced yet, deferring to next cycle"
            );
            return Ok(());
        }

        let Some(local_path) = &conflict.local_path else {
            tracing::warn!(conflict = ?conflict, "⚠️ Conflict (no local path to resolve)");
            return Ok(());
        };

        if !local_path.is_file() {
            tracing::warn!(
                path = %local_path.display(),
                kind = ?conflict.kind,
                "⚠️ Conflict local path is not a file, skipping rename"
            );
            return Ok(());
        }

        let conflict_filename = generate_conflict_name(local_path);
        let conflict_path = local_path.with_file_name(&conflict_filename);

        tracing::info!(
            from = %local_path.display(),
            to = %conflict_path.display(),
            kind = ?conflict.kind,
            "📝 Creating conflict copy"
        );

        fs::rename(local_path, &conflict_path)?;

        // Remove the synced record so the next planning cycle treats the
        // original path as missing locally and re-downloads from the remote.
        if let Some(local_id) = &conflict.local_id {
            self.store.delete_synced(local_id)?;
            self.store.delete_local_node(local_id)?;
            self.store.flush()?;
        }

        Ok(())
    }

    // ==================== Two-phase helpers ====================

    /// Partition plan results into (non-delete, delete) groups.
    ///
    /// Delete operations (`DeleteLocal`, `DeleteRemote`, `DeleteSynced`) are
    /// separated so they can be deferred until after a re-plan.
    fn partition_results(results: Vec<PlanResult>) -> (Vec<PlanResult>, Vec<PlanResult>) {
        results.into_iter().partition(|r| match r {
            PlanResult::Op(op) => !op.is_delete(),
            _ => true,
        })
    }

    fn extract_conflicts(results: &[PlanResult]) -> Vec<Conflict> {
        results
            .iter()
            .filter_map(|r| match r {
                PlanResult::Conflict(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    fn extract_ops(results: &[PlanResult]) -> Vec<SyncOp> {
        results
            .iter()
            .filter_map(|r| match r {
                PlanResult::Op(op) => Some(op.clone()),
                _ => None,
            })
            .collect()
    }

    // ==================== Helpers ====================

    /// Try to reuse an existing local file with the same MD5 checksum instead
    /// of downloading.
    ///
    /// Looks up the synced tree for a file record whose checksum matches
    /// `expected_md5`, verifies the source file still exists on disk and has
    /// not been modified, then copies it to `target_path` via the staging
    /// directory.
    ///
    /// Returns `true` if the file was successfully reused, `false` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if reading the source file or writing to staging fails.
    pub(crate) fn try_reuse_local_file(
        &self,
        expected_md5: &str,
        target_path: &Path,
    ) -> Result<bool> {
        let Some(donor) = self.store.find_synced_by_md5(expected_md5)? else {
            return Ok(false);
        };

        let source_path = self.sync_dir.join(&donor.rel_path);
        if !source_path.is_file() {
            tracing::debug!(
                path = %source_path.display(),
                "📋 Donor file no longer exists on disk"
            );
            return Ok(false);
        }

        let actual_md5 = compute_md5_from_path(&source_path)?;
        if actual_md5 != expected_md5 {
            tracing::debug!(
                path = %source_path.display(),
                expected = expected_md5,
                actual = actual_md5,
                "📋 Donor file content has changed"
            );
            return Ok(false);
        }

        tracing::info!(
            source = %source_path.display(),
            target = %target_path.display(),
            md5 = expected_md5,
            "♻️ Reusing local file instead of downloading"
        );

        self.copy_via_staging(&source_path, target_path)?;
        Ok(true)
    }

    fn via_staging(&self, target: &Path, write_fn: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        let staging_path = self.staging_dir.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&self.staging_dir)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        write_fn(&staging_path)?;
        fs::rename(&staging_path, target)?;
        Ok(())
    }

    fn write_via_staging(&self, target: &Path, content: &[u8]) -> Result<()> {
        self.via_staging(target, |staging| Ok(fs::write(staging, content)?))
    }

    fn copy_via_staging(&self, source: &Path, target: &Path) -> Result<()> {
        self.via_staging(target, |staging| {
            fs::copy(source, staging)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn try_reuse_local_file_copies_existing_file() {
        let store_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let store = TreeStore::open(store_dir.path()).unwrap();

        let existing_content = b"hello world, I am a synced file";
        let existing_path = sync_dir.path().join("existing.txt");
        fs::write(&existing_path, existing_content).unwrap();

        let existing_md5 = compute_md5_from_bytes(existing_content);

        let metadata = fs::metadata(&existing_path).unwrap();
        let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

        let synced = SyncedRecord {
            local_id,
            remote_id: RemoteId::new("remote-existing"),
            rel_path: "existing.txt".to_string(),
            md5sum: Some(existing_md5.clone()),
            size: Some(existing_content.len() as u64),
            rev: "1-abc".to_string(),
            node_type: NodeType::File,
            local_name: Some("existing.txt".to_string()),
            local_parent_id: None,
            remote_name: Some("existing.txt".to_string()),
            remote_parent_id: None,
        };
        store.insert_synced(&synced).unwrap();
        store.flush().unwrap();

        let engine = SyncEngine::new(
            store,
            sync_dir.path().to_path_buf(),
            sync_dir.path().join(".staging"),
            IgnoreRules::none(),
        );

        let target_path = sync_dir.path().join("copy.txt");
        let reused = engine
            .try_reuse_local_file(&existing_md5, &target_path)
            .unwrap();
        assert!(reused, "Should reuse the existing local file");
        assert!(target_path.exists(), "Target file should exist after reuse");
        assert_eq!(
            fs::read(&target_path).unwrap(),
            existing_content,
            "Content should match"
        );
    }

    #[test]
    fn try_reuse_local_file_returns_false_when_no_match() {
        let store_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let store = TreeStore::open(store_dir.path()).unwrap();

        let engine = SyncEngine::new(
            store,
            sync_dir.path().to_path_buf(),
            sync_dir.path().join(".staging"),
            IgnoreRules::none(),
        );

        let target_path = sync_dir.path().join("new.txt");
        let reused = engine
            .try_reuse_local_file("nonexistent_md5", &target_path)
            .unwrap();
        assert!(!reused, "Should not reuse when no match found");
        assert!(!target_path.exists(), "Target should not exist");
    }

    #[test]
    fn try_reuse_local_file_returns_false_when_source_deleted() {
        let store_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let store = TreeStore::open(store_dir.path()).unwrap();

        let synced = SyncedRecord {
            local_id: LocalFileId::new(1, 999),
            remote_id: RemoteId::new("remote-gone"),
            rel_path: "gone.txt".to_string(),
            md5sum: Some("deadbeef".to_string()),
            size: Some(100),
            rev: "1-x".to_string(),
            node_type: NodeType::File,
            local_name: Some("gone.txt".to_string()),
            local_parent_id: None,
            remote_name: Some("gone.txt".to_string()),
            remote_parent_id: None,
        };
        store.insert_synced(&synced).unwrap();
        store.flush().unwrap();

        let engine = SyncEngine::new(
            store,
            sync_dir.path().to_path_buf(),
            sync_dir.path().join(".staging"),
            IgnoreRules::none(),
        );

        let target_path = sync_dir.path().join("target.txt");
        let reused = engine
            .try_reuse_local_file("deadbeef", &target_path)
            .unwrap();
        assert!(
            !reused,
            "Should not reuse when source file is missing from disk"
        );
    }

    #[test]
    fn try_reuse_local_file_returns_false_when_content_changed() {
        let store_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let store = TreeStore::open(store_dir.path()).unwrap();

        let original_content = b"original content";
        let original_md5 = compute_md5_from_bytes(original_content);

        let file_path = sync_dir.path().join("changed.txt");
        fs::write(&file_path, original_content).unwrap();
        let metadata = fs::metadata(&file_path).unwrap();
        let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

        let synced = SyncedRecord {
            local_id,
            remote_id: RemoteId::new("remote-changed"),
            rel_path: "changed.txt".to_string(),
            md5sum: Some(original_md5.clone()),
            size: Some(original_content.len() as u64),
            rev: "1-x".to_string(),
            node_type: NodeType::File,
            local_name: Some("changed.txt".to_string()),
            local_parent_id: None,
            remote_name: Some("changed.txt".to_string()),
            remote_parent_id: None,
        };
        store.insert_synced(&synced).unwrap();
        store.flush().unwrap();

        fs::write(&file_path, b"modified content").unwrap();

        let engine = SyncEngine::new(
            store,
            sync_dir.path().to_path_buf(),
            sync_dir.path().join(".staging"),
            IgnoreRules::none(),
        );

        let target_path = sync_dir.path().join("target.txt");
        let reused = engine
            .try_reuse_local_file(&original_md5, &target_path)
            .unwrap();
        assert!(!reused, "Should not reuse when file content has changed");
    }
}
