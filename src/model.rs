use serde::{Deserialize, Serialize};

/// Unique identifier for a node (file or directory)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Type of filesystem node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    File,
    Directory,
}

/// A node in the filesystem tree (either local or remote)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Unique identifier
    pub id: NodeId,
    /// Parent directory ID (None for root)
    pub parent_id: Option<NodeId>,
    /// Name of the file or directory
    pub name: String,
    /// Type: file or directory
    pub node_type: NodeType,
    /// MD5 checksum (files only)
    pub md5sum: Option<String>,
    /// Size in bytes (files only)
    pub size: Option<u64>,
    /// Last modification timestamp (Unix epoch seconds)
    pub updated_at: i64,
    /// `CouchDB` revision (remote only)
    pub rev: Option<String>,
}

impl Node {
    #[must_use]
    pub const fn is_file(&self) -> bool {
        matches!(self.node_type, NodeType::File)
    }

    #[must_use]
    pub const fn is_dir(&self) -> bool {
        matches!(self.node_type, NodeType::Directory)
    }
}

/// Which tree a node belongs to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeType {
    Remote,
    Local,
    Synced,
}

/// An operation to perform to synchronize trees
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// Download file from remote to local
    Download { node_id: NodeId },
    /// Upload file from local to remote
    Upload { node_id: NodeId },
    /// Create directory locally
    CreateLocalDir { node_id: NodeId },
    /// Create directory on remote
    CreateRemoteDir { node_id: NodeId },
    /// Delete file/dir locally
    DeleteLocal { node_id: NodeId },
    /// Delete file/dir on remote (trash)
    DeleteRemote { node_id: NodeId },
    /// Move/rename locally
    MoveLocal {
        node_id: NodeId,
        new_parent_id: NodeId,
        new_name: String,
    },
    /// Move/rename on remote
    MoveRemote {
        node_id: NodeId,
        new_parent_id: NodeId,
        new_name: String,
    },
    /// Conflict detected, needs resolution
    Conflict { node_id: NodeId, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_creation() {
        let id = NodeId::new("test-123");
        assert_eq!(id.as_str(), "test-123");
    }

    #[test]
    fn node_type_detection() {
        let file_node = Node {
            id: NodeId::new("1"),
            parent_id: None,
            name: "test.txt".to_string(),
            node_type: NodeType::File,
            md5sum: Some("abc123".to_string()),
            size: Some(100),
            updated_at: 1234567890,
            rev: None,
        };

        let dir_node = Node {
            id: NodeId::new("2"),
            parent_id: None,
            name: "docs".to_string(),
            node_type: NodeType::Directory,
            md5sum: None,
            size: None,
            updated_at: 1234567890,
            rev: None,
        };

        assert!(file_node.is_file());
        assert!(!file_node.is_dir());
        assert!(dir_node.is_dir());
        assert!(!dir_node.is_file());
    }

    #[test]
    fn node_serialization() {
        let node = Node {
            id: NodeId::new("file-1"),
            parent_id: Some(NodeId::new("dir-1")),
            name: "document.pdf".to_string(),
            node_type: NodeType::File,
            md5sum: Some("d41d8cd98f00b204e9800998ecf8427e".to_string()),
            size: Some(1024),
            updated_at: 1706886400,
            rev: Some("1-abc".to_string()),
        };

        let json = serde_json::to_string(&node).unwrap();
        let deserialized: Node = serde_json::from_str(&json).unwrap();

        assert_eq!(node, deserialized);
    }
}
