use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Local filesystem identity (`device_id`, `inode`) - stable across renames
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocalFileId {
    pub device_id: u64,
    pub inode: u64,
}

impl LocalFileId {
    #[must_use]
    pub const fn new(device_id: u64, inode: u64) -> Self {
        Self { device_id, inode }
    }

    /// Encode as 16 bytes (big-endian) for use as a stable binary key
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0..8].copy_from_slice(&self.device_id.to_be_bytes());
        buf[8..16].copy_from_slice(&self.inode.to_be_bytes());
        buf
    }

    /// Decode from 16 bytes (big-endian)
    #[must_use]
    pub fn from_bytes(bytes: &[u8; 16]) -> Self {
        let mut device_bytes = [0u8; 8];
        let mut inode_bytes = [0u8; 8];
        device_bytes.copy_from_slice(&bytes[0..8]);
        inode_bytes.copy_from_slice(&bytes[8..16]);
        Self {
            device_id: u64::from_be_bytes(device_bytes),
            inode: u64::from_be_bytes(inode_bytes),
        }
    }
}

/// Remote Cozy document ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteId(pub String);

impl RemoteId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Unique identifier for a node (file or directory)
/// Kept for backward compatibility with existing code
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

impl From<RemoteId> for NodeId {
    fn from(remote_id: RemoteId) -> Self {
        Self(remote_id.0)
    }
}

impl From<&RemoteId> for NodeId {
    fn from(remote_id: &RemoteId) -> Self {
        Self(remote_id.0.clone())
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

/// A node in the local filesystem tree, keyed by `LocalFileId`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalNode {
    /// Stable filesystem identity
    pub id: LocalFileId,
    /// Parent directory identity (None for sync root)
    pub parent_id: Option<LocalFileId>,
    /// File or directory name
    pub name: String,
    /// Node type
    pub node_type: NodeType,
    /// MD5 checksum (files only)
    pub md5sum: Option<String>,
    /// Size in bytes (files only)
    pub size: Option<u64>,
    /// Last modification timestamp (Unix epoch seconds)
    pub mtime: i64,
}

impl LocalNode {
    #[must_use]
    pub const fn is_file(&self) -> bool {
        matches!(self.node_type, NodeType::File)
    }

    #[must_use]
    pub const fn is_dir(&self) -> bool {
        matches!(self.node_type, NodeType::Directory)
    }
}

/// A node in the remote Cozy tree, keyed by `RemoteId`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteNode {
    /// Remote Cozy document ID
    pub id: RemoteId,
    /// Parent directory ID (None for root)
    pub parent_id: Option<RemoteId>,
    /// File or directory name
    pub name: String,
    /// Node type
    pub node_type: NodeType,
    /// MD5 checksum (files only)
    pub md5sum: Option<String>,
    /// Size in bytes (files only)
    pub size: Option<u64>,
    /// Last modification timestamp (Unix epoch seconds)
    pub updated_at: i64,
    /// `CouchDB` revision
    pub rev: String,
}

impl RemoteNode {
    #[must_use]
    pub const fn is_file(&self) -> bool {
        matches!(self.node_type, NodeType::File)
    }

    #[must_use]
    pub const fn is_dir(&self) -> bool {
        matches!(self.node_type, NodeType::Directory)
    }
}

/// A binding record in the Synced tree that links local and remote identities
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedRecord {
    /// Local filesystem identity (`device_id`, `inode`) - stable across renames
    pub local_id: LocalFileId,
    /// Remote Cozy document ID
    pub remote_id: RemoteId,
    /// Relative path at last sync (for debugging/display)
    pub rel_path: String,
    /// Content hash at last sync
    pub md5sum: Option<String>,
    /// Size at last sync
    pub size: Option<u64>,
    /// Remote `CouchDB` revision at last sync
    pub rev: String,
    /// Node type
    pub node_type: NodeType,
}

/// Which tree a node belongs to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeType {
    Remote,
    Local,
    Synced,
}

/// An operation to perform to synchronize trees.
/// Operations include preconditions for idempotency and conflict detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// Download a new file from remote to local
    DownloadNew {
        remote_id: RemoteId,
        local_path: PathBuf,
        expected_rev: String,
        expected_md5: String,
    },
    /// Update an existing local file from remote
    DownloadUpdate {
        remote_id: RemoteId,
        local_id: LocalFileId,
        local_path: PathBuf,
        expected_rev: String,
        expected_remote_md5: String,
        expected_local_md5: String,
    },
    /// Create a new file on remote
    UploadNew {
        local_id: LocalFileId,
        local_path: PathBuf,
        parent_remote_id: RemoteId,
        name: String,
        expected_md5: String,
    },
    /// Update an existing remote file
    UploadUpdate {
        local_id: LocalFileId,
        remote_id: RemoteId,
        local_path: PathBuf,
        expected_local_md5: String,
        expected_rev: String,
    },
    /// Create directory locally
    CreateLocalDir {
        remote_id: RemoteId,
        local_path: PathBuf,
    },
    /// Create directory on remote
    CreateRemoteDir {
        local_id: LocalFileId,
        local_path: PathBuf,
        parent_remote_id: RemoteId,
        name: String,
    },
    /// Delete file/dir locally
    DeleteLocal {
        local_id: LocalFileId,
        local_path: PathBuf,
        expected_md5: Option<String>,
    },
    /// Delete file/dir on remote (trash)
    DeleteRemote {
        remote_id: RemoteId,
        expected_rev: String,
    },
    /// Move/rename locally
    MoveLocal {
        local_id: LocalFileId,
        from_path: PathBuf,
        to_path: PathBuf,
        expected_parent_id: LocalFileId,
        expected_name: String,
    },
    /// Move/rename on remote
    MoveRemote {
        remote_id: RemoteId,
        new_parent_id: RemoteId,
        new_name: String,
        expected_rev: String,
    },
}

/// A conflict detected during planning that requires resolution
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Local identity if known
    pub local_id: Option<LocalFileId>,
    /// Remote identity if known
    pub remote_id: Option<RemoteId>,
    /// Human-readable reason
    pub reason: String,
    /// Type of conflict
    pub kind: ConflictKind,
}

/// Types of conflicts that can occur
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides modified the same file
    BothModified,
    /// Local deleted, remote modified
    LocalDeleteRemoteModify,
    /// Local modified, remote deleted
    LocalModifyRemoteDelete,
    /// Parent directory missing
    ParentMissing,
    /// Name collision (different files with same path)
    NameCollision,
}

/// Result of planning sync operations for an item
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanResult {
    /// Operation to execute
    Op(SyncOp),
    /// Conflict requiring resolution
    Conflict(Conflict),
    /// No action needed (already in sync)
    NoOp,
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

    #[test]
    fn local_file_id_creation() {
        let id = LocalFileId::new(12345, 67890);
        assert_eq!(id.device_id, 12345);
        assert_eq!(id.inode, 67890);
    }

    #[test]
    fn local_file_id_equality() {
        let id1 = LocalFileId::new(1, 100);
        let id2 = LocalFileId::new(1, 100);
        let id3 = LocalFileId::new(1, 101);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn remote_id_creation() {
        let id = RemoteId::new("abc-123-def");
        assert_eq!(id.as_str(), "abc-123-def");
    }

    #[test]
    fn remote_id_to_node_id_conversion() {
        let remote = RemoteId::new("remote-123");
        let node_id: NodeId = remote.clone().into();
        assert_eq!(node_id.as_str(), "remote-123");

        let node_id_ref: NodeId = (&remote).into();
        assert_eq!(node_id_ref.as_str(), "remote-123");
    }

    #[test]
    fn synced_record_serialization() {
        let record = SyncedRecord {
            local_id: LocalFileId::new(1, 100),
            remote_id: RemoteId::new("remote-123"),
            rel_path: "docs/file.txt".to_string(),
            md5sum: Some("abc123".to_string()),
            size: Some(1024),
            rev: "1-xyz".to_string(),
            node_type: NodeType::File,
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: SyncedRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(record, deserialized);
    }

    #[test]
    fn sync_op_download_new() {
        let op = SyncOp::DownloadNew {
            remote_id: RemoteId::new("file-1"),
            local_path: PathBuf::from("/sync/docs/file.txt"),
            expected_rev: "2-abc".to_string(),
            expected_md5: "d41d8cd98f00b204e9800998ecf8427e".to_string(),
        };

        if let SyncOp::DownloadNew {
            remote_id,
            local_path,
            expected_rev,
            expected_md5,
        } = op
        {
            assert_eq!(remote_id.as_str(), "file-1");
            assert_eq!(local_path, PathBuf::from("/sync/docs/file.txt"));
            assert_eq!(expected_rev, "2-abc");
            assert_eq!(expected_md5, "d41d8cd98f00b204e9800998ecf8427e");
        } else {
            panic!("Expected DownloadNew variant");
        }
    }

    #[test]
    fn conflict_struct() {
        let conflict = Conflict {
            local_id: Some(LocalFileId::new(1, 100)),
            remote_id: Some(RemoteId::new("remote-1")),
            reason: "Both modified".to_string(),
            kind: ConflictKind::BothModified,
        };

        assert!(conflict.local_id.is_some());
        assert!(conflict.remote_id.is_some());
        assert_eq!(conflict.reason, "Both modified");
        assert_eq!(conflict.kind, ConflictKind::BothModified);
    }

    #[test]
    fn plan_result_variants() {
        let op = PlanResult::Op(SyncOp::DeleteRemote {
            remote_id: RemoteId::new("file-1"),
            expected_rev: "1-abc".to_string(),
        });
        assert!(matches!(op, PlanResult::Op(_)));

        let conflict = PlanResult::Conflict(Conflict {
            local_id: None,
            remote_id: Some(RemoteId::new("file-1")),
            reason: "test".to_string(),
            kind: ConflictKind::ParentMissing,
        });
        assert!(matches!(conflict, PlanResult::Conflict(_)));

        let noop = PlanResult::NoOp;
        assert!(matches!(noop, PlanResult::NoOp));
    }

    #[test]
    fn local_file_id_bytes_roundtrip() {
        let id = LocalFileId::new(0x1234_5678_9ABC_DEF0, 0xFEDC_BA98_7654_3210);
        let bytes = id.to_bytes();
        let decoded = LocalFileId::from_bytes(&bytes);
        assert_eq!(id, decoded);
    }

    #[test]
    fn local_node_serialization() {
        let node = LocalNode {
            id: LocalFileId::new(1, 100),
            parent_id: Some(LocalFileId::new(1, 50)),
            name: "test.txt".to_string(),
            node_type: NodeType::File,
            md5sum: Some("abc123".to_string()),
            size: Some(1024),
            mtime: 1706886400,
        };

        let json = serde_json::to_string(&node).unwrap();
        let deserialized: LocalNode = serde_json::from_str(&json).unwrap();
        assert_eq!(node, deserialized);
    }

    #[test]
    fn remote_node_serialization() {
        let node = RemoteNode {
            id: RemoteId::new("remote-123"),
            parent_id: Some(RemoteId::new("parent-456")),
            name: "doc.pdf".to_string(),
            node_type: NodeType::File,
            md5sum: Some("def456".to_string()),
            size: Some(2048),
            updated_at: 1706886400,
            rev: "2-xyz".to_string(),
        };

        let json = serde_json::to_string(&node).unwrap();
        let deserialized: RemoteNode = serde_json::from_str(&json).unwrap();
        assert_eq!(node, deserialized);
    }
}
