use cozy_desktop::model::{
    Conflict, ConflictKind, LocalFileId, LocalNode, NodeType, PlanResult, RemoteId, RemoteNode,
    SyncOp, SyncedRecord,
};
use cozy_desktop::planner::Planner;
use cozy_desktop::store::TreeStore;
use std::path::PathBuf;
use tempfile::tempdir;

fn local_id(device: u64, inode: u64) -> LocalFileId {
    LocalFileId::new(device, inode)
}

fn remote_id(id: &str) -> RemoteId {
    RemoteId::new(id)
}

fn make_local_file(
    id: LocalFileId,
    parent: Option<LocalFileId>,
    name: &str,
    md5: &str,
) -> LocalNode {
    LocalNode {
        id,
        parent_id: parent,
        name: name.to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.to_string()),
        size: Some(100),
        mtime: 1000,
    }
}

fn make_local_dir(id: LocalFileId, parent: Option<LocalFileId>, name: &str) -> LocalNode {
    LocalNode {
        id,
        parent_id: parent,
        name: name.to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 1000,
    }
}

fn make_remote_file(id: RemoteId, parent: Option<RemoteId>, name: &str, md5: &str) -> RemoteNode {
    RemoteNode {
        id,
        parent_id: parent,
        name: name.to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.to_string()),
        size: Some(100),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    }
}

fn make_remote_dir(id: RemoteId, parent: Option<RemoteId>, name: &str) -> RemoteNode {
    RemoteNode {
        id,
        parent_id: parent,
        name: name.to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-abc".to_string(),
    }
}

fn make_synced(
    local_id: LocalFileId,
    remote_id: RemoteId,
    path: &str,
    md5: Option<&str>,
    node_type: NodeType,
) -> SyncedRecord {
    SyncedRecord {
        local_id,
        remote_id,
        rel_path: path.to_string(),
        md5sum: md5.map(String::from),
        size: Some(100),
        rev: "1-abc".to_string(),
        node_type,
    }
}

#[test]
fn test_new_remote_file_generates_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let remote_file = make_remote_file(
        remote_id("f1"),
        Some(remote_id("root")),
        "doc.txt",
        "abc123",
    );
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::DownloadNew {
            remote_id,
            local_path,
            expected_rev,
            expected_md5,
        }) => {
            assert_eq!(remote_id.as_str(), "f1");
            assert_eq!(expected_rev, "1-abc");
            assert_eq!(expected_md5, "abc123");
            assert!(local_path.to_string_lossy().contains("doc.txt"));
        }
        other => panic!("Expected DownloadNew, got {:?}", other),
    }
}

#[test]
fn test_new_local_file_generates_upload() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let root_local = local_id(1, 1);
    let root_remote = remote_id("root");

    let synced_root = make_synced(
        root_local.clone(),
        root_remote.clone(),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();

    let local_file = make_local_file(local_id(1, 100), Some(root_local), "doc.txt", "abc123");
    store.insert_local_node(&local_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::UploadNew {
            local_id,
            parent_remote_id,
            name,
            expected_md5,
            ..
        }) => {
            assert_eq!(local_id.inode, 100);
            assert_eq!(parent_remote_id.as_str(), "root");
            assert_eq!(name, "doc.txt");
            assert_eq!(expected_md5, "abc123");
        }
        other => panic!("Expected UploadNew, got {:?}", other),
    }
}

#[test]
fn test_synced_file_no_ops() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let local_file = make_local_file(lid.clone(), None, "doc.txt", "abc123");
    let remote_file = make_remote_file(rid.clone(), None, "doc.txt", "abc123");
    let synced = make_synced(lid, rid, "doc.txt", Some("abc123"), NodeType::File);

    store.insert_local_node(&local_file).unwrap();
    store.insert_remote_node(&remote_file).unwrap();
    store.insert_synced(&synced).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert!(ops.is_empty());
}

#[test]
fn test_remote_modified_generates_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let local_file = make_local_file(lid.clone(), None, "doc.txt", "old_hash");
    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "doc.txt",
        Some("old_hash"),
        NodeType::File,
    );

    let mut remote_file = make_remote_file(rid.clone(), None, "doc.txt", "new_hash");
    remote_file.rev = "2-xyz".to_string();

    store.insert_local_node(&local_file).unwrap();
    store.insert_synced(&synced).unwrap();
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::DownloadUpdate {
            remote_id,
            local_id,
            expected_rev,
            expected_remote_md5,
            expected_local_md5,
            ..
        }) => {
            assert_eq!(remote_id.as_str(), "f1");
            assert_eq!(local_id.inode, 100);
            assert_eq!(expected_rev, "2-xyz");
            assert_eq!(expected_remote_md5, "new_hash");
            assert_eq!(expected_local_md5, "old_hash");
        }
        other => panic!("Expected DownloadUpdate, got {:?}", other),
    }
}

#[test]
fn test_local_modified_generates_upload() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let remote_file = make_remote_file(rid.clone(), None, "doc.txt", "old_hash");
    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "doc.txt",
        Some("old_hash"),
        NodeType::File,
    );

    let local_file = make_local_file(lid.clone(), None, "doc.txt", "new_hash");

    store.insert_remote_node(&remote_file).unwrap();
    store.insert_synced(&synced).unwrap();
    store.insert_local_node(&local_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::UploadUpdate {
            local_id,
            remote_id,
            expected_local_md5,
            expected_rev,
            ..
        }) => {
            assert_eq!(local_id.inode, 100);
            assert_eq!(remote_id.as_str(), "f1");
            assert_eq!(expected_local_md5, "new_hash");
            assert_eq!(expected_rev, "1-abc");
        }
        other => panic!("Expected UploadUpdate, got {:?}", other),
    }
}

#[test]
fn test_both_modified_generates_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "doc.txt",
        Some("original"),
        NodeType::File,
    );

    let mut remote_file = make_remote_file(rid.clone(), None, "doc.txt", "remote_change");
    remote_file.rev = "2-remote".to_string();

    let local_file = make_local_file(lid.clone(), None, "doc.txt", "local_change");

    store.insert_synced(&synced).unwrap();
    store.insert_remote_node(&remote_file).unwrap();
    store.insert_local_node(&local_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Conflict(Conflict {
            local_id,
            remote_id,
            kind,
            ..
        }) => {
            assert!(local_id.is_some());
            assert!(remote_id.is_some());
            assert_eq!(kind, &ConflictKind::BothModified);
        }
        other => panic!("Expected Conflict, got {:?}", other),
    }
}

#[test]
fn test_remote_deleted_generates_local_delete() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "doc.txt",
        Some("hash"),
        NodeType::File,
    );
    let local_file = make_local_file(lid.clone(), None, "doc.txt", "hash");

    store.insert_synced(&synced).unwrap();
    store.insert_local_node(&local_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::DeleteLocal {
            local_id,
            expected_md5,
            ..
        }) => {
            assert_eq!(local_id.inode, 100);
            assert_eq!(*expected_md5, Some("hash".to_string()));
        }
        other => panic!("Expected DeleteLocal, got {:?}", other),
    }
}

#[test]
fn test_local_deleted_generates_remote_delete() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "doc.txt",
        Some("hash"),
        NodeType::File,
    );
    let remote_file = make_remote_file(rid.clone(), None, "doc.txt", "hash");

    store.insert_synced(&synced).unwrap();
    store.insert_remote_node(&remote_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::DeleteRemote {
            remote_id,
            expected_rev,
        }) => {
            assert_eq!(remote_id.as_str(), "f1");
            assert_eq!(expected_rev, "1-abc");
        }
        other => panic!("Expected DeleteRemote, got {:?}", other),
    }
}

#[test]
fn test_new_remote_directory_generates_create_local_dir() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let remote_dir = make_remote_dir(remote_id("d1"), Some(remote_id("root")), "docs");
    store.insert_remote_node(&remote_dir).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::CreateLocalDir {
            remote_id,
            local_path,
        }) => {
            assert_eq!(remote_id.as_str(), "d1");
            assert!(local_path.to_string_lossy().contains("docs"));
        }
        other => panic!("Expected CreateLocalDir, got {:?}", other),
    }
}

#[test]
fn test_new_local_directory_generates_create_remote_dir() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let root_local = local_id(1, 1);
    let root_remote = remote_id("root");

    let synced_root = make_synced(
        root_local.clone(),
        root_remote.clone(),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();

    let local_dir = make_local_dir(local_id(1, 200), Some(root_local), "docs");
    store.insert_local_node(&local_dir).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::CreateRemoteDir {
            local_id,
            parent_remote_id,
            name,
            ..
        }) => {
            assert_eq!(local_id.inode, 200);
            assert_eq!(parent_remote_id.as_str(), "root");
            assert_eq!(name, "docs");
        }
        other => panic!("Expected CreateRemoteDir, got {:?}", other),
    }
}

#[test]
fn test_local_file_without_synced_parent_generates_parent_missing_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let local_file = make_local_file(local_id(1, 100), Some(local_id(1, 50)), "doc.txt", "abc123");
    store.insert_local_node(&local_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Conflict(Conflict { kind, .. }) => {
            assert_eq!(kind, &ConflictKind::ParentMissing);
        }
        other => panic!("Expected ParentMissing conflict, got {:?}", other),
    }
}

#[test]
fn test_upload_update_uses_remote_rev_not_synced_rev() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    let mut remote_file = make_remote_file(rid.clone(), None, "doc.txt", "old_hash");
    remote_file.rev = "3-current".to_string();

    let synced = SyncedRecord {
        local_id: lid.clone(),
        remote_id: rid.clone(),
        rel_path: "doc.txt".to_string(),
        md5sum: Some("old_hash".to_string()),
        size: Some(100),
        rev: "1-old".to_string(),
        node_type: NodeType::File,
    };

    let local_file = make_local_file(lid.clone(), None, "doc.txt", "new_hash");

    store.insert_remote_node(&remote_file).unwrap();
    store.insert_synced(&synced).unwrap();
    store.insert_local_node(&local_file).unwrap();

    let planner = Planner::new(&store, PathBuf::from("/sync"));
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::UploadUpdate { expected_rev, .. }) => {
            assert_eq!(expected_rev, "3-current");
        }
        other => panic!("Expected UploadUpdate, got {:?}", other),
    }
}
