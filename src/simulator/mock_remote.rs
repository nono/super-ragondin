use crate::model::{RemoteId, RemoteNode};
use std::collections::{HashMap, HashSet};

/// A change record for tracking remote changes
#[derive(Debug, Clone)]
pub struct ChangeRecord {
    pub seq: u64,
    pub remote_id: RemoteId,
    pub deleted: bool,
}

/// Mock remote Cozy server for simulation testing
#[derive(Debug, Clone, Default)]
pub struct MockRemote {
    pub nodes: HashMap<RemoteId, RemoteNode>,
    pub file_contents: HashMap<RemoteId, Vec<u8>>,
    seq: u64,
    changes: Vec<ChangeRecord>,
}

impl MockRemote {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: RemoteNode, content: Option<Vec<u8>>) {
        self.seq += 1;
        self.changes.push(ChangeRecord {
            seq: self.seq,
            remote_id: node.id.clone(),
            deleted: false,
        });
        if let Some(c) = content {
            self.file_contents.insert(node.id.clone(), c);
        }
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn delete_node(&mut self, id: &RemoteId) {
        let mut to_remove = Vec::new();
        Self::collect_tree(&self.nodes, id, &mut to_remove, &mut HashSet::new());
        for rid in &to_remove {
            self.seq += 1;
            self.changes.push(ChangeRecord {
                seq: self.seq,
                remote_id: rid.clone(),
                deleted: true,
            });
            self.nodes.remove(rid);
            self.file_contents.remove(rid);
        }
    }

    fn collect_tree(
        nodes: &HashMap<RemoteId, RemoteNode>,
        id: &RemoteId,
        out: &mut Vec<RemoteId>,
        visited: &mut HashSet<RemoteId>,
    ) {
        if !visited.insert(id.clone()) {
            return;
        }
        let children: Vec<RemoteId> = nodes
            .values()
            .filter(|n| n.parent_id.as_ref() == Some(id))
            .map(|n| n.id.clone())
            .collect();
        for child in children {
            Self::collect_tree(nodes, &child, out, visited);
        }
        out.push(id.clone());
    }

    #[must_use]
    pub fn get_node(&self, id: &RemoteId) -> Option<&RemoteNode> {
        self.nodes.get(id)
    }

    #[must_use]
    pub fn get_content(&self, id: &RemoteId) -> Option<&Vec<u8>> {
        self.file_contents.get(id)
    }

    /// Get changes since a given sequence number.
    /// Returns (seq, node, deleted) tuples for non-deleted nodes.
    #[must_use]
    pub fn get_changes_since(&self, since_seq: u64) -> Vec<(u64, &RemoteNode, bool)> {
        self.changes
            .iter()
            .filter(|c| c.seq > since_seq)
            .filter_map(|c| {
                if c.deleted {
                    None
                } else {
                    self.nodes.get(&c.remote_id).map(|n| (c.seq, n, c.deleted))
                }
            })
            .collect()
    }

    /// Get all changes since a given sequence number, including deletions.
    /// For deletions, returns a placeholder node.
    #[must_use]
    pub fn get_all_changes_since(&self, since_seq: u64) -> Vec<ChangeRecord> {
        self.changes
            .iter()
            .filter(|c| c.seq > since_seq)
            .cloned()
            .collect()
    }

    pub fn move_node(&mut self, id: &RemoteId, new_parent_id: RemoteId, new_name: String) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.parent_id = Some(new_parent_id);
            node.name = new_name;
            let rev_num: u32 = node
                .rev
                .split('-')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            node.rev = format!("{}-{}", rev_num + 1, id.as_str());
        }
        self.seq += 1;
        self.changes.push(ChangeRecord {
            seq: self.seq,
            remote_id: id.clone(),
            deleted: false,
        });
    }

    #[must_use]
    pub const fn current_seq(&self) -> u64 {
        self.seq
    }
}
