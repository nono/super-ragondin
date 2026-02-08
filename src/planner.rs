use crate::error::Result;
use crate::model::{
    Conflict, ConflictKind, LocalFileId, LocalNode, NodeType, PlanResult, RemoteId, RemoteNode,
    SyncOp, SyncedRecord,
};
use crate::store::TreeStore;
use std::collections::HashMap;
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

        for remote in &remote_nodes {
            let synced = synced_by_remote.get(&remote.id);
            let local = synced.and_then(|s| local_by_id.get(&s.local_id));

            if let Some(result) = self.plan_remote_node(remote, local.copied(), synced.copied()) {
                results.push(result);
            }
        }

        for local in &local_nodes {
            let synced = synced_by_local.get(&local.id);
            if synced.is_some() {
                continue;
            }
            if let Some(result) = self.plan_local_only(local) {
                results.push(result);
            }
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

    fn plan_remote_node(
        &self,
        remote: &RemoteNode,
        local: Option<&LocalNode>,
        synced: Option<&SyncedRecord>,
    ) -> Option<PlanResult> {
        match (local, synced) {
            (Some(local), Some(synced)) => self.plan_all_three(remote, local, synced),
            (Some(local), None) => Self::plan_created_both_sides(remote, local),
            (None, None) => self.plan_remote_only(remote),
            (None, Some(_)) => None,
        }
    }

    fn plan_all_three(
        &self,
        remote: &RemoteNode,
        local: &LocalNode,
        synced: &SyncedRecord,
    ) -> Option<PlanResult> {
        let remote_changed = !Self::remote_matches_synced(remote, synced);
        let local_changed = !Self::local_matches_synced(local, synced);

        match (remote_changed, local_changed) {
            (false, false) => None,
            (true, false) => Some(PlanResult::Op(SyncOp::DownloadUpdate {
                remote_id: remote.id.clone(),
                local_id: local.id.clone(),
                local_path: self.compute_local_path(remote),
                expected_rev: remote.rev.clone(),
                expected_remote_md5: remote.md5sum.clone().unwrap_or_default(),
                expected_local_md5: local.md5sum.clone().unwrap_or_default(),
            })),
            (false, true) => Some(PlanResult::Op(SyncOp::UploadUpdate {
                local_id: local.id.clone(),
                remote_id: remote.id.clone(),
                local_path: self.compute_local_path(remote),
                expected_local_md5: local.md5sum.clone().unwrap_or_default(),
                expected_rev: remote.rev.clone(),
            })),
            (true, true) => {
                if Self::remote_equals_local(remote, local) {
                    None
                } else {
                    tracing::debug!(remote_id = remote.id.as_str(), "⚠️ Both sides modified");
                    Some(PlanResult::Conflict(Conflict {
                        local_id: Some(local.id.clone()),
                        remote_id: Some(remote.id.clone()),
                        reason: "Modified on both sides".to_string(),
                        kind: ConflictKind::BothModified,
                    }))
                }
            }
        }
    }

    fn plan_created_both_sides(remote: &RemoteNode, local: &LocalNode) -> Option<PlanResult> {
        if Self::remote_equals_local(remote, local) {
            None
        } else {
            Some(PlanResult::Conflict(Conflict {
                local_id: Some(local.id.clone()),
                remote_id: Some(remote.id.clone()),
                reason: "Created on both sides with different content".to_string(),
                kind: ConflictKind::NameCollision,
            }))
        }
    }

    fn plan_remote_only(&self, remote: &RemoteNode) -> Option<PlanResult> {
        // Skip root directory - it maps to sync_root which already exists
        remote.parent_id.as_ref()?;

        tracing::debug!(remote_id = remote.id.as_str(), name = &remote.name, node_type = ?remote.node_type, "📋 New remote node, planning download");

        let local_path = self.compute_local_path(remote);

        if remote.node_type == NodeType::Directory {
            Some(PlanResult::Op(SyncOp::CreateLocalDir {
                remote_id: remote.id.clone(),
                local_path,
            }))
        } else {
            Some(PlanResult::Op(SyncOp::DownloadNew {
                remote_id: remote.id.clone(),
                local_path,
                expected_rev: remote.rev.clone(),
                expected_md5: remote.md5sum.clone().unwrap_or_default(),
            }))
        }
    }

    #[allow(clippy::option_if_let_else)]
    fn plan_local_only(&self, local: &LocalNode) -> Option<PlanResult> {
        let local_path = self.compute_local_path_from_local(local);

        if local.parent_id.is_some() {
            tracing::debug!(name = &local.name, node_type = ?local.node_type, "📋 New local node, planning upload");
            match self.find_parent_remote_id(local.parent_id.as_ref()) {
                Some(parent_remote_id) => {
                    if local.node_type == NodeType::Directory {
                        Some(PlanResult::Op(SyncOp::CreateRemoteDir {
                            local_id: local.id.clone(),
                            local_path,
                            parent_remote_id,
                            name: local.name.clone(),
                        }))
                    } else {
                        Some(PlanResult::Op(SyncOp::UploadNew {
                            local_id: local.id.clone(),
                            local_path,
                            parent_remote_id,
                            name: local.name.clone(),
                            expected_md5: local.md5sum.clone().unwrap_or_default(),
                        }))
                    }
                }
                None => Some(PlanResult::Conflict(Conflict {
                    local_id: Some(local.id.clone()),
                    remote_id: None,
                    reason: "Parent directory not synced".to_string(),
                    kind: ConflictKind::ParentMissing,
                })),
            }
        } else {
            None
        }
    }

    fn plan_remote_deleted(&self, synced: &SyncedRecord, local: &LocalNode) -> PlanResult {
        let local_changed = !Self::local_matches_synced(local, synced);

        if local_changed {
            tracing::debug!(
                remote_id = synced.remote_id.as_str(),
                "⚠️ Remote deleted but local modified"
            );
            PlanResult::Conflict(Conflict {
                local_id: Some(local.id.clone()),
                remote_id: Some(synced.remote_id.clone()),
                reason: "Remote deleted but local modified".to_string(),
                kind: ConflictKind::LocalModifyRemoteDelete,
            })
        } else {
            PlanResult::Op(SyncOp::DeleteLocal {
                local_id: local.id.clone(),
                local_path: self.sync_root.join(&synced.rel_path),
                expected_md5: synced.md5sum.clone(),
            })
        }
    }

    fn plan_local_deleted(synced: &SyncedRecord, remote: &RemoteNode) -> PlanResult {
        let remote_changed = !Self::remote_matches_synced(remote, synced);

        if remote_changed {
            tracing::debug!(
                remote_id = remote.id.as_str(),
                "⚠️ Local deleted but remote modified"
            );
            PlanResult::Conflict(Conflict {
                local_id: Some(synced.local_id.clone()),
                remote_id: Some(remote.id.clone()),
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

    fn find_parent_remote_id(&self, parent_local_id: Option<&LocalFileId>) -> Option<RemoteId> {
        let parent_id = parent_local_id?;
        let synced = self.store.get_synced_by_local(parent_id).ok()??;
        Some(synced.remote_id)
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
        while let Some(ref parent_id) = current_parent {
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
        while let Some(ref parent_id) = current_parent {
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

    fn remote_matches_synced(remote: &RemoteNode, synced: &SyncedRecord) -> bool {
        if remote.node_type != synced.node_type {
            return false;
        }
        if remote.node_type == NodeType::File {
            remote.md5sum == synced.md5sum
        } else {
            true
        }
    }

    fn local_matches_synced(local: &LocalNode, synced: &SyncedRecord) -> bool {
        if local.node_type != synced.node_type {
            return false;
        }
        if local.node_type == NodeType::File {
            local.md5sum == synced.md5sum
        } else {
            true
        }
    }

    fn remote_equals_local(remote: &RemoteNode, local: &LocalNode) -> bool {
        if remote.node_type != local.node_type {
            return false;
        }
        if remote.name != local.name {
            return false;
        }
        if remote.node_type == NodeType::File {
            remote.md5sum == local.md5sum
        } else {
            true
        }
    }

    fn sort_operations(results: &mut [PlanResult]) {
        results.sort_by_key(|r| match r {
            PlanResult::Op(SyncOp::CreateLocalDir { .. } | SyncOp::CreateRemoteDir { .. }) => 0,
            PlanResult::Op(
                SyncOp::DownloadNew { .. }
                | SyncOp::DownloadUpdate { .. }
                | SyncOp::UploadNew { .. }
                | SyncOp::UploadUpdate { .. },
            ) => 1,
            PlanResult::Op(SyncOp::MoveLocal { .. } | SyncOp::MoveRemote { .. }) => 2,
            PlanResult::Op(SyncOp::DeleteLocal { .. } | SyncOp::DeleteRemote { .. }) => 3,
            PlanResult::Conflict(_) => 4,
            PlanResult::NoOp => 5,
        });
    }
}
