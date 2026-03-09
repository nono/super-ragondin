use crate::error::Result;
use crate::model::{
    Conflict, ConflictKind, LocalFileId, LocalNode, NodeInfo, NodeType, PlanResult, RemoteId,
    RemoteNode, SyncOp, SyncedRecord, content_matches,
};
use crate::store::TreeStore;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub struct Planner<'a> {
    store: &'a TreeStore,
    sync_root: PathBuf,
}

impl<'a> Planner<'a> {
    #[must_use]
    pub const fn new(store: &'a TreeStore, sync_root: PathBuf) -> Self {
        Self { store, sync_root }
    }

    /// Plan sync operations by comparing all three trees.
    ///
    /// # Errors
    ///
    /// Returns an error if store access fails.
    pub fn plan(&self) -> Result<Vec<PlanResult>> {
        let mut results = Vec::new();

        let remote_nodes = self.store.list_all_remote()?;
        let local_nodes = self.store.list_all_local()?;

        self.reconcile_inodes(&local_nodes)?;

        let synced_records = self.store.list_all_synced()?;

        tracing::info!(
            remote_count = remote_nodes.len(),
            local_count = local_nodes.len(),
            synced_count = synced_records.len(),
            "📋 Planning sync operations"
        );

        let local_by_id: HashMap<LocalFileId, &LocalNode> =
            local_nodes.iter().map(|n| (n.id.clone(), n)).collect();
        let remote_by_id: HashMap<RemoteId, &RemoteNode> =
            remote_nodes.iter().map(|n| (n.id.clone(), n)).collect();
        let synced_by_local: HashMap<LocalFileId, &SyncedRecord> = synced_records
            .iter()
            .map(|r| (r.local_id.clone(), r))
            .collect();
        let synced_by_remote: HashMap<RemoteId, &SyncedRecord> = synced_records
            .iter()
            .map(|r| (r.remote_id.clone(), r))
            .collect();

        let cyclic_remote_ids = Self::find_remote_cycles(&remote_by_id);

        for remote in &remote_nodes {
            if cyclic_remote_ids.contains(&remote.id) {
                tracing::warn!(
                    remote_id = remote.id.as_str(),
                    name = &remote.name,
                    "⚠️ Skipping node involved in parent cycle"
                );
                // If the node was previously synced, delete the local copy
                if let Some(synced) = synced_by_remote.get(&remote.id)
                    && let Some(local) = local_by_id.get(&synced.local_id)
                {
                    results.push(PlanResult::Op(SyncOp::DeleteLocal {
                        local_id: local.id.clone(),
                        local_path: self.compute_local_path_from_local(local),
                        expected_md5: synced.md5sum.clone(),
                    }));
                }
                results.push(PlanResult::Conflict(Conflict {
                    local_id: None,
                    remote_id: Some(remote.id.clone()),
                    local_path: None,
                    reason: "Parent chain forms a cycle".to_string(),
                    kind: ConflictKind::CycleDetected,
                }));
                continue;
            }

            let synced = synced_by_remote.get(&remote.id);
            let local = synced.and_then(|s| local_by_id.get(&s.local_id));

            results.extend(self.plan_remote_node(remote, local.copied(), synced.copied()));
        }

        for local in &local_nodes {
            let synced = synced_by_local.get(&local.id);
            if synced.is_some() {
                continue;
            }
            results.extend(self.plan_local_only(local));
        }

        for synced in &synced_records {
            let local = local_by_id.get(&synced.local_id);
            let remote = remote_by_id.get(&synced.remote_id);

            if let (None, Some(local_node)) = (remote, local) {
                results.push(self.plan_remote_deleted(synced, local_node));
            }

            if let (None, Some(remote_node)) = (local, remote) {
                results.push(Self::plan_local_deleted(synced, remote_node));
            }
        }

        self.resolve_path_collisions(&mut results, &remote_by_id, &local_by_id);

        Self::sort_operations(&mut results);
        let op_count = results
            .iter()
            .filter(|r| matches!(r, PlanResult::Op(_)))
            .count();
        let conflict_count = results
            .iter()
            .filter(|r| matches!(r, PlanResult::Conflict(_)))
            .count();
        tracing::info!(
            operations = op_count,
            conflicts = conflict_count,
            "📋 Planning complete"
        );
        Ok(results)
    }

    /// Detect files that were atomically saved (write-to-temp, delete, rename).
    ///
    /// When a synced record's `local_id` (inode) no longer exists in the local
    /// tree but a local node with the same name and parent does, re-bind the
    /// synced record to the new inode. This turns what would be a
    /// `DeleteRemote` + `UploadNew` into an `UploadUpdate`, preserving the
    /// remote document ID (and its sharing permissions, history, etc.).
    fn reconcile_inodes(&self, local_nodes: &[LocalNode]) -> Result<()> {
        let local_by_id: HashMap<LocalFileId, &LocalNode> =
            local_nodes.iter().map(|n| (n.id.clone(), n)).collect();

        let synced_local_ids: HashSet<LocalFileId> = self
            .store
            .list_all_synced()?
            .into_iter()
            .map(|r| r.local_id)
            .collect();

        let mut unbound_by_location: HashMap<(Option<LocalFileId>, String, NodeType), LocalFileId> =
            local_nodes
                .iter()
                .filter(|n| !synced_local_ids.contains(&n.id))
                .map(|n| {
                    (
                        (n.parent_id.clone(), n.name.clone(), n.node_type),
                        n.id.clone(),
                    )
                })
                .collect();

        for synced in self.store.list_all_synced()? {
            if local_by_id.contains_key(&synced.local_id) {
                continue;
            }

            let Some(synced_name) = &synced.local_name else {
                continue;
            };

            let key = (
                synced.local_parent_id.clone(),
                synced_name.clone(),
                synced.node_type,
            );

            if let Some(new_local_id) = unbound_by_location.remove(&key) {
                tracing::info!(
                    old_inode = %synced.local_id,
                    new_inode = %new_local_id,
                    name = synced_name,
                    "🔄 Atomic save detected, rebinding inode"
                );

                let new_synced = SyncedRecord {
                    local_id: new_local_id,
                    ..synced.clone()
                };
                self.store.delete_synced(&synced.local_id)?;
                self.store.insert_synced(&new_synced)?;
            }
        }

        Ok(())
    }

    fn plan_remote_node(
        &self,
        remote: &RemoteNode,
        local: Option<&LocalNode>,
        synced: Option<&SyncedRecord>,
    ) -> Vec<PlanResult> {
        if !Self::is_safe_name(&remote.name) {
            tracing::warn!(
                remote_id = remote.id.as_str(),
                name = &remote.name,
                "⚠️ Unsafe remote name, rejecting"
            );
            return vec![PlanResult::Conflict(Conflict {
                local_id: None,
                remote_id: Some(remote.id.clone()),
                local_path: None,
                reason: format!("Unsafe remote file name: {:?}", remote.name),
                kind: ConflictKind::InvalidName,
            })];
        }
        match (local, synced) {
            (Some(local), Some(synced)) => self.plan_all_three(remote, local, synced),
            (Some(local), None) => Self::plan_created_both_sides(remote, local),
            (None, None) => self.plan_remote_only(remote),
            (None, Some(_)) => vec![],
        }
    }

    fn plan_all_three(
        &self,
        remote: &RemoteNode,
        local: &LocalNode,
        synced: &SyncedRecord,
    ) -> Vec<PlanResult> {
        let mut ops = Vec::new();

        let remote_content_changed = !content_matches(remote, synced);
        let local_content_changed = !content_matches(local, synced);

        let remote_loc_changed = Self::remote_location_changed(remote, synced);
        let local_loc_changed = Self::local_location_changed(local, synced);

        if remote_loc_changed && local_loc_changed {
            if self.both_moved_to_same_location(local, remote) {
                // no move op needed
            } else {
                tracing::debug!(remote_id = remote.id.as_str(), "⚠️ Both sides moved");
                ops.push(PlanResult::Conflict(Conflict {
                    local_id: Some(local.id.clone()),
                    remote_id: Some(remote.id.clone()),
                    local_path: Some(self.compute_local_path_from_local(local)),
                    reason: "Moved on both sides to different locations".to_string(),
                    kind: ConflictKind::BothMoved,
                }));
                return ops;
            }
        } else if remote_loc_changed {
            if let Some(move_op) = self.plan_move_local(remote, local, synced) {
                ops.push(move_op);
            } else {
                return ops;
            }
        } else if local_loc_changed {
            if let Some(move_op) = self.plan_move_remote(remote, local, synced) {
                ops.push(move_op);
            } else {
                ops.push(PlanResult::Conflict(Conflict {
                    local_id: Some(local.id.clone()),
                    remote_id: Some(remote.id.clone()),
                    local_path: Some(self.compute_local_path_from_local(local)),
                    reason: "Parent directory not synced for move".to_string(),
                    kind: ConflictKind::ParentMissing,
                }));
                return ops;
            }
        }

        match (remote_content_changed, local_content_changed) {
            (false, false) => {}
            (true, false) => {
                ops.push(PlanResult::Op(SyncOp::DownloadUpdate {
                    remote_id: remote.id.clone(),
                    local_id: local.id.clone(),
                    local_path: self.compute_local_path(remote),
                    expected_rev: remote.rev.clone(),
                    expected_remote_md5: remote.md5sum.clone().unwrap_or_default(),
                    expected_local_md5: local.md5sum.clone().unwrap_or_default(),
                }));
            }
            (false, true) => {
                ops.push(PlanResult::Op(SyncOp::UploadUpdate {
                    local_id: local.id.clone(),
                    remote_id: remote.id.clone(),
                    local_path: self.compute_local_path_from_local(local),
                    expected_local_md5: local.md5sum.clone().unwrap_or_default(),
                    expected_rev: remote.rev.clone(),
                }));
            }
            (true, true) => {
                if !Self::remote_equals_local(remote, local) {
                    tracing::debug!(remote_id = remote.id.as_str(), "⚠️ Both sides modified");
                    ops.push(PlanResult::Conflict(Conflict {
                        local_id: Some(local.id.clone()),
                        remote_id: Some(remote.id.clone()),
                        local_path: Some(self.compute_local_path_from_local(local)),
                        reason: "Modified on both sides".to_string(),
                        kind: ConflictKind::BothModified,
                    }));
                }
            }
        }

        ops
    }

    fn plan_move_local(
        &self,
        remote: &RemoteNode,
        local: &LocalNode,
        _synced: &SyncedRecord,
    ) -> Option<PlanResult> {
        // Defer if the new remote parent hasn't been synced yet
        if let Some(parent_id) = &remote.parent_id {
            self.store.get_synced_by_remote(parent_id).ok().flatten()?;
        }

        let from_path = self.compute_local_path_from_local(local);
        let to_path = self.compute_local_path(remote);

        Some(PlanResult::Op(SyncOp::MoveLocal {
            local_id: local.id.clone(),
            from_path,
            to_path,
            expected_parent_id: local.parent_id.clone(),
            expected_name: local.name.clone(),
        }))
    }

    fn plan_move_remote(
        &self,
        remote: &RemoteNode,
        local: &LocalNode,
        _synced: &SyncedRecord,
    ) -> Option<PlanResult> {
        let new_parent_remote_id = local
            .parent_id
            .as_ref()
            .and_then(|pid| self.store.get_synced_by_local(pid).ok()?)
            .map(|s| s.remote_id)?;

        Some(PlanResult::Op(SyncOp::MoveRemote {
            remote_id: remote.id.clone(),
            new_parent_id: new_parent_remote_id,
            new_name: local.name.clone(),
            expected_rev: remote.rev.clone(),
        }))
    }

    fn plan_created_both_sides(remote: &RemoteNode, local: &LocalNode) -> Vec<PlanResult> {
        if Self::remote_equals_local(remote, local) {
            vec![]
        } else {
            vec![PlanResult::Conflict(Conflict {
                local_id: Some(local.id.clone()),
                remote_id: Some(remote.id.clone()),
                local_path: None,
                reason: "Created on both sides with different content".to_string(),
                kind: ConflictKind::NameCollision,
            })]
        }
    }

    fn plan_remote_only(&self, remote: &RemoteNode) -> Vec<PlanResult> {
        let Some(parent_id) = remote.parent_id.as_ref() else {
            return vec![];
        };

        // Defer if parent directory hasn't been synced yet — it will be
        // handled in a subsequent planning round once the parent is synced.
        if self
            .store
            .get_synced_by_remote(parent_id)
            .ok()
            .flatten()
            .is_none()
        {
            return vec![];
        }

        tracing::debug!(remote_id = remote.id.as_str(), name = &remote.name, node_type = ?remote.node_type, "📋 New remote node, planning download");

        let local_path = self.compute_local_path(remote);

        if remote.is_dir() {
            vec![PlanResult::Op(SyncOp::CreateLocalDir {
                remote_id: remote.id.clone(),
                local_path,
            })]
        } else {
            vec![PlanResult::Op(SyncOp::DownloadNew {
                remote_id: remote.id.clone(),
                local_path,
                expected_rev: remote.rev.clone(),
                expected_md5: remote.md5sum.clone().unwrap_or_default(),
            })]
        }
    }

    #[allow(clippy::option_if_let_else)]
    fn plan_local_only(&self, local: &LocalNode) -> Vec<PlanResult> {
        if !Self::is_safe_name(&local.name) {
            tracing::warn!(name = &local.name, "⚠️ Unsafe local name, rejecting");
            return vec![PlanResult::Conflict(Conflict {
                local_id: Some(local.id.clone()),
                remote_id: None,
                local_path: None,
                reason: format!("Unsafe local file name: {:?}", local.name),
                kind: ConflictKind::InvalidName,
            })];
        }

        let local_path = self.compute_local_path_from_local(local);

        if local.parent_id.is_some() {
            tracing::debug!(name = &local.name, node_type = ?local.node_type, "📋 New local node, planning upload");
            match self.find_parent_remote_id(local.parent_id.as_ref()) {
                Some(parent_remote_id) => {
                    if local.is_dir() {
                        vec![PlanResult::Op(SyncOp::CreateRemoteDir {
                            local_id: local.id.clone(),
                            local_path,
                            parent_remote_id,
                            name: local.name.clone(),
                        })]
                    } else {
                        vec![PlanResult::Op(SyncOp::UploadNew {
                            local_id: local.id.clone(),
                            local_path,
                            parent_remote_id,
                            name: local.name.clone(),
                            expected_md5: local.md5sum.clone().unwrap_or_default(),
                        })]
                    }
                }
                None => vec![PlanResult::Conflict(Conflict {
                    local_id: Some(local.id.clone()),
                    remote_id: None,
                    local_path: Some(local_path),
                    reason: "Parent directory not synced".to_string(),
                    kind: ConflictKind::ParentMissing,
                })],
            }
        } else {
            vec![]
        }
    }

    fn plan_remote_deleted(&self, synced: &SyncedRecord, local: &LocalNode) -> PlanResult {
        let local_changed = !content_matches(local, synced);

        if local_changed {
            tracing::debug!(
                remote_id = synced.remote_id.as_str(),
                "⚠️ Remote deleted but local modified"
            );
            PlanResult::Conflict(Conflict {
                local_id: Some(local.id.clone()),
                remote_id: Some(synced.remote_id.clone()),
                local_path: Some(self.compute_local_path_from_local(local)),
                reason: "Remote deleted but local modified".to_string(),
                kind: ConflictKind::LocalModifyRemoteDelete,
            })
        } else {
            PlanResult::Op(SyncOp::DeleteLocal {
                local_id: local.id.clone(),
                local_path: self.compute_local_path_from_local(local),
                expected_md5: synced.md5sum.clone(),
            })
        }
    }

    fn plan_local_deleted(synced: &SyncedRecord, remote: &RemoteNode) -> PlanResult {
        let remote_changed = !content_matches(remote, synced);

        if remote_changed {
            tracing::debug!(
                remote_id = remote.id.as_str(),
                "⚠️ Local deleted but remote modified"
            );
            PlanResult::Conflict(Conflict {
                local_id: Some(synced.local_id.clone()),
                remote_id: Some(remote.id.clone()),
                local_path: None,
                reason: "Local deleted but remote modified".to_string(),
                kind: ConflictKind::LocalDeleteRemoteModify,
            })
        } else {
            PlanResult::Op(SyncOp::DeleteRemote {
                remote_id: remote.id.clone(),
                expected_rev: synced.rev.clone(),
            })
        }
    }

    fn remote_location_changed(remote: &RemoteNode, synced: &SyncedRecord) -> bool {
        let Some(synced_name) = &synced.remote_name else {
            return false;
        };
        let Some(synced_parent) = &synced.remote_parent_id else {
            return synced_name.as_str() != remote.name;
        };
        synced_name.as_str() != remote.name || remote.parent_id.as_ref() != Some(synced_parent)
    }

    fn local_location_changed(local: &LocalNode, synced: &SyncedRecord) -> bool {
        let Some(synced_name) = &synced.local_name else {
            return false;
        };
        let Some(synced_parent) = &synced.local_parent_id else {
            return synced_name.as_str() != local.name;
        };
        synced_name.as_str() != local.name || local.parent_id.as_ref() != Some(synced_parent)
    }

    fn both_moved_to_same_location(&self, local: &LocalNode, remote: &RemoteNode) -> bool {
        if local.name != remote.name {
            return false;
        }
        match (&local.parent_id, &remote.parent_id) {
            (Some(local_pid), Some(remote_pid)) => {
                if let Ok(Some(synced)) = self.store.get_synced_by_local(local_pid) {
                    &synced.remote_id == remote_pid
                } else {
                    false
                }
            }
            (None, None) => true,
            _ => false,
        }
    }

    fn find_parent_remote_id(&self, parent_local_id: Option<&LocalFileId>) -> Option<RemoteId> {
        let parent_id = parent_local_id?;
        let synced = self.store.get_synced_by_local(parent_id).ok()??;
        Some(synced.remote_id)
    }

    /// Find all remote node IDs whose ancestor chain does not reach root.
    ///
    /// This includes nodes directly in a cycle (e.g., A→B→A) and nodes
    /// whose ancestor is in a cycle (unreachable descendants).
    fn find_remote_cycles(remote_by_id: &HashMap<RemoteId, &RemoteNode>) -> HashSet<RemoteId> {
        let mut unreachable = HashSet::new();

        for id in remote_by_id.keys() {
            let mut visited = HashSet::new();
            let mut current = Some(id.clone());
            let mut reaches_root = false;
            while let Some(ref cid) = current {
                if !visited.insert(cid.clone()) {
                    break; // cycle detected
                }
                match remote_by_id.get(cid).and_then(|n| n.parent_id.as_ref()) {
                    None => {
                        reaches_root = true;
                        break;
                    }
                    Some(pid) => current = Some(pid.clone()),
                }
            }
            if !reaches_root {
                unreachable.insert(id.clone());
            }
        }

        unreachable
    }

    fn compute_local_path(&self, remote: &RemoteNode) -> PathBuf {
        let rel_path = self.compute_remote_rel_path(remote);
        self.sync_root.join(rel_path)
    }

    #[allow(clippy::assigning_clones)]
    fn compute_remote_rel_path(&self, remote: &RemoteNode) -> PathBuf {
        let mut components = Vec::new();

        if !remote.name.is_empty() {
            components.push(remote.name.clone());
        }

        let mut current_parent = remote.parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(ref parent_id) = current_parent {
            if !visited.insert(parent_id.clone()) {
                break;
            }
            if let Ok(Some(parent)) = self.store.get_remote_node(parent_id) {
                if !parent.name.is_empty() {
                    components.push(parent.name.clone());
                }
                current_parent = parent.parent_id.clone();
            } else {
                break;
            }
        }

        components.reverse();
        components.iter().collect()
    }

    fn compute_local_path_from_local(&self, local: &LocalNode) -> PathBuf {
        let rel_path = self.compute_local_rel_path(local);
        self.sync_root.join(rel_path)
    }

    #[allow(clippy::assigning_clones)]
    fn compute_local_rel_path(&self, local: &LocalNode) -> PathBuf {
        let mut components = Vec::new();

        if !local.name.is_empty() {
            components.push(local.name.clone());
        }

        let mut current_parent = local.parent_id.clone();
        let mut visited = HashSet::new();
        while let Some(ref parent_id) = current_parent {
            if !visited.insert(parent_id.clone()) {
                break;
            }
            if let Ok(Some(parent)) = self.store.get_local_node(parent_id) {
                if !parent.name.is_empty() {
                    components.push(parent.name.clone());
                }
                current_parent = parent.parent_id.clone();
            } else {
                break;
            }
        }

        components.reverse();
        components.iter().collect()
    }

    fn remote_equals_local(remote: &RemoteNode, local: &LocalNode) -> bool {
        remote.name() == local.name() && content_matches(remote, local)
    }

    fn is_safe_name(name: &str) -> bool {
        if name.is_empty() {
            return true;
        }
        if name == "." || name == ".." {
            return false;
        }
        if name.contains('/') || name.contains('\\') || name.contains('\0') {
            return false;
        }
        true
    }

    /// Detect and resolve path collisions where a `DownloadNew` and an
    /// `UploadNew` target the same local path.
    ///
    /// When a new remote file and a new local file appear at the same path:
    /// - Same content → both ops are removed (the files are already identical,
    ///   only a synced record needs to be created on the next cycle).
    /// - Different content → both ops are replaced with a `NameCollision`
    ///   conflict.
    #[allow(clippy::unused_self)]
    fn resolve_path_collisions(
        &self,
        results: &mut Vec<PlanResult>,
        remote_by_id: &HashMap<RemoteId, &RemoteNode>,
        local_by_id: &HashMap<LocalFileId, &LocalNode>,
    ) {
        // Build index: local_path → index for remote-side and local-side new ops
        let mut remote_new_by_path: HashMap<&PathBuf, usize> = HashMap::new();
        let mut local_new_by_path: HashMap<&PathBuf, usize> = HashMap::new();

        for (i, result) in results.iter().enumerate() {
            match result {
                PlanResult::Op(
                    SyncOp::DownloadNew { local_path, .. }
                    | SyncOp::CreateLocalDir { local_path, .. },
                ) => {
                    remote_new_by_path.insert(local_path, i);
                }
                PlanResult::Op(
                    SyncOp::UploadNew { local_path, .. }
                    | SyncOp::CreateRemoteDir { local_path, .. },
                ) => {
                    local_new_by_path.insert(local_path, i);
                }
                _ => {}
            }
        }

        // Find paths that appear in both maps
        let mut indices_to_remove = Vec::new();
        let mut replacements: Vec<(usize, PlanResult)> = Vec::new();

        for (path, &remote_idx) in &remote_new_by_path {
            if let Some(&local_idx) = local_new_by_path.get(path) {
                let (remote_id, local_id) = match (&results[remote_idx], &results[local_idx]) {
                    (
                        PlanResult::Op(
                            SyncOp::DownloadNew { remote_id, .. }
                            | SyncOp::CreateLocalDir { remote_id, .. },
                        ),
                        PlanResult::Op(
                            SyncOp::UploadNew { local_id, .. }
                            | SyncOp::CreateRemoteDir { local_id, .. },
                        ),
                    ) => (remote_id.clone(), local_id.clone()),
                    _ => continue,
                };

                let remote = remote_by_id.get(&remote_id);
                let local = local_by_id.get(&local_id);

                match (remote, local) {
                    (Some(remote), Some(local)) if Self::remote_equals_local(remote, local) => {
                        tracing::info!(
                            path = %path.display(),
                            "✅ New remote and local at same path with same content, binding"
                        );
                        indices_to_remove.push(local_idx);
                        replacements.push((
                            remote_idx,
                            PlanResult::Op(SyncOp::BindExisting {
                                local_id,
                                remote_id,
                                local_path: (*path).clone(),
                            }),
                        ));
                    }
                    (Some(_), Some(_)) => {
                        tracing::debug!(
                            path = %path.display(),
                            "⚠️ New remote and local at same path with different content"
                        );
                        indices_to_remove.push(local_idx);
                        replacements.push((
                            remote_idx,
                            PlanResult::Conflict(Conflict {
                                local_id: Some(local_id),
                                remote_id: Some(remote_id),
                                local_path: Some((*path).clone()),
                                reason: "Created on both sides with different content".to_string(),
                                kind: ConflictKind::NameCollision,
                            }),
                        ));
                    }
                    _ => {}
                }
            }
        }

        // Apply replacements first (before removing)
        for (idx, replacement) in replacements {
            results[idx] = replacement;
        }

        // Remove colliding ops (in reverse order to preserve indices)
        indices_to_remove.sort_unstable();
        indices_to_remove.dedup();
        for idx in indices_to_remove.into_iter().rev() {
            results.remove(idx);
        }
    }

    fn sort_operations(results: &mut [PlanResult]) {
        results.sort_by_key(|r| match r {
            PlanResult::Op(SyncOp::CreateLocalDir { .. } | SyncOp::CreateRemoteDir { .. }) => 0,
            PlanResult::Op(SyncOp::BindExisting { .. }) => 1,
            PlanResult::Op(SyncOp::MoveLocal { .. } | SyncOp::MoveRemote { .. }) => 2,
            PlanResult::Op(
                SyncOp::DownloadNew { .. }
                | SyncOp::DownloadUpdate { .. }
                | SyncOp::UploadNew { .. }
                | SyncOp::UploadUpdate { .. },
            ) => 3,
            PlanResult::Op(SyncOp::DeleteLocal { .. } | SyncOp::DeleteRemote { .. }) => 4,
            PlanResult::Conflict(_) => 5,
            PlanResult::NoOp => 6,
        });
    }
}
