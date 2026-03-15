use std::path::PathBuf;
use super_ragondin_sync::ignore::IgnoreRules;
use super_ragondin_sync::model::{
    Conflict, ConflictKind, LocalFileId, LocalNode, NodeType, PlanResult, RemoteId, RemoteNode,
    SyncOp, SyncedRecord,
};
use super_ragondin_sync::planner::Planner;
use super_ragondin_sync::store::TreeStore;
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
        local_name: None,
        local_parent_id: None,
        remote_name: None,
        remote_parent_id: None,
    }
}

fn make_synced_with_location(
    local_id: LocalFileId,
    remote_id: RemoteId,
    path: &str,
    md5: Option<&str>,
    node_type: NodeType,
    local_name: &str,
    local_parent_id: Option<LocalFileId>,
    remote_name: &str,
    remote_parent_id: Option<RemoteId>,
) -> SyncedRecord {
    SyncedRecord {
        local_id,
        remote_id,
        rel_path: path.to_string(),
        md5sum: md5.map(String::from),
        size: Some(100),
        rev: "1-abc".to_string(),
        node_type,
        local_name: Some(local_name.to_string()),
        local_parent_id,
        remote_name: Some(remote_name.to_string()),
        remote_parent_id,
    }
}

#[test]
fn test_new_remote_file_generates_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // Root directory must be synced so the planner can resolve the parent
    let synced_root = make_synced(
        local_id(1, 1),
        remote_id("root"),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(local_id(1, 1), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(remote_id("root"), None, ""))
        .unwrap();

    let remote_file = make_remote_file(
        remote_id("f1"),
        Some(remote_id("root")),
        "doc.txt",
        "abc123",
    );
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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
    store
        .insert_local_node(&make_local_dir(root_local.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(root_remote, None, ""))
        .unwrap();

    let local_file = make_local_file(local_id(1, 100), Some(root_local), "doc.txt", "abc123");
    store.insert_local_node(&local_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    // Root directory must be synced so the planner can resolve the parent
    let synced_root = make_synced(
        local_id(1, 1),
        remote_id("root"),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(local_id(1, 1), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(remote_id("root"), None, ""))
        .unwrap();

    let remote_dir = make_remote_dir(remote_id("d1"), Some(remote_id("root")), "docs");
    store.insert_remote_node(&remote_dir).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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
    store
        .insert_local_node(&make_local_dir(root_local.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(root_remote, None, ""))
        .unwrap();

    let local_dir = make_local_dir(local_id(1, 200), Some(root_local), "docs");
    store.insert_local_node(&local_dir).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
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
        local_name: None,
        local_parent_id: None,
        remote_name: None,
        remote_parent_id: None,
    };

    let local_file = make_local_file(lid.clone(), None, "doc.txt", "new_hash");

    store.insert_remote_node(&remote_file).unwrap();
    store.insert_synced(&synced).unwrap();
    store.insert_local_node(&local_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PlanResult::Op(SyncOp::UploadUpdate { expected_rev, .. }) => {
            assert_eq!(expected_rev, "3-current");
        }
        other => panic!("Expected UploadUpdate, got {:?}", other),
    }
}

#[test]
fn test_remote_rename_generates_move_local() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();

    let parent_local = make_local_dir(parent_lid.clone(), None, "");
    let parent_remote = make_remote_dir(parent_rid.clone(), None, "");
    store.insert_local_node(&parent_local).unwrap();
    store.insert_remote_node(&parent_remote).unwrap();

    let synced = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "old.txt",
        Some("hash"),
        NodeType::File,
        "old.txt",
        Some(parent_lid.clone()),
        "old.txt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "old.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "new.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let move_op = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })));
    assert!(move_op.is_some(), "Should plan MoveLocal for remote rename");

    if let Some(PlanResult::Op(SyncOp::MoveLocal {
        local_id: op_lid,
        from_path,
        to_path,
        ..
    })) = move_op
    {
        assert_eq!(*op_lid, lid);
        assert!(from_path.to_string_lossy().contains("old.txt"));
        assert!(to_path.to_string_lossy().contains("new.txt"));
    }
}

#[test]
fn test_remote_move_to_different_dir_generates_move_local() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent1_lid = local_id(1, 1);
    let parent1_rid = remote_id("dir1");
    let parent2_lid = local_id(1, 2);
    let parent2_rid = remote_id("dir2");
    let root_lid = local_id(1, 0);
    let root_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        root_lid.clone(),
        root_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    let root_local = make_local_dir(root_lid.clone(), None, "");
    let root_remote = make_remote_dir(root_rid.clone(), None, "");
    store.insert_local_node(&root_local).unwrap();
    store.insert_remote_node(&root_remote).unwrap();

    let synced_p1 = make_synced_with_location(
        parent1_lid.clone(),
        parent1_rid.clone(),
        "dir1",
        None,
        NodeType::Directory,
        "dir1",
        Some(root_lid.clone()),
        "dir1",
        Some(root_rid.clone()),
    );
    store.insert_synced(&synced_p1).unwrap();
    let p1_local = make_local_dir(parent1_lid.clone(), Some(root_lid.clone()), "dir1");
    let p1_remote = make_remote_dir(parent1_rid.clone(), Some(root_rid.clone()), "dir1");
    store.insert_local_node(&p1_local).unwrap();
    store.insert_remote_node(&p1_remote).unwrap();

    let synced_p2 = make_synced_with_location(
        parent2_lid.clone(),
        parent2_rid.clone(),
        "dir2",
        None,
        NodeType::Directory,
        "dir2",
        Some(root_lid.clone()),
        "dir2",
        Some(root_rid.clone()),
    );
    store.insert_synced(&synced_p2).unwrap();
    let p2_local = make_local_dir(parent2_lid.clone(), Some(root_lid.clone()), "dir2");
    let p2_remote = make_remote_dir(parent2_rid.clone(), Some(root_rid.clone()), "dir2");
    store.insert_local_node(&p2_local).unwrap();
    store.insert_remote_node(&p2_remote).unwrap();

    let synced_file = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "dir1/file.txt",
        Some("hash"),
        NodeType::File,
        "file.txt",
        Some(parent1_lid.clone()),
        "file.txt",
        Some(parent1_rid.clone()),
    );
    store.insert_synced(&synced_file).unwrap();

    let local_file = make_local_file(lid.clone(), Some(parent1_lid.clone()), "file.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    let remote_file = make_remote_file(rid.clone(), Some(parent2_rid.clone()), "file.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let move_op = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })));
    assert!(
        move_op.is_some(),
        "Should plan MoveLocal for remote directory move"
    );

    if let Some(PlanResult::Op(SyncOp::MoveLocal { to_path, .. })) = move_op {
        assert!(
            to_path.to_string_lossy().contains("dir2"),
            "Should move to dir2"
        );
    }
}

#[test]
fn test_local_rename_generates_move_remote() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    let parent_local = make_local_dir(parent_lid.clone(), None, "");
    let parent_remote = make_remote_dir(parent_rid.clone(), None, "");
    store.insert_local_node(&parent_local).unwrap();
    store.insert_remote_node(&parent_remote).unwrap();

    let synced = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "old.txt",
        Some("hash"),
        NodeType::File,
        "old.txt",
        Some(parent_lid.clone()),
        "old.txt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "new.txt", "hash");
    store.insert_local_node(&local_file).unwrap();

    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "old.txt", "hash");
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let move_op = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::MoveRemote { .. })));
    assert!(move_op.is_some(), "Should plan MoveRemote for local rename");

    if let Some(PlanResult::Op(SyncOp::MoveRemote {
        remote_id: op_rid,
        new_name,
        ..
    })) = move_op
    {
        assert_eq!(op_rid.as_str(), "f1");
        assert_eq!(new_name, "new.txt");
    }
}

#[test]
fn test_both_moved_to_same_location_is_noop() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    let synced = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "old.txt",
        Some("hash"),
        NodeType::File,
        "old.txt",
        Some(parent_lid.clone()),
        "old.txt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "new.txt", "hash");
    let remote_file = make_remote_file(rid.clone(), Some(parent_rid.clone()), "new.txt", "hash");
    store.insert_local_node(&local_file).unwrap();
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let move_ops: Vec<_> = ops
        .iter()
        .filter(|r| {
            matches!(
                r,
                PlanResult::Op(SyncOp::MoveLocal { .. } | SyncOp::MoveRemote { .. })
            )
        })
        .collect();
    assert!(
        move_ops.is_empty(),
        "No move ops when both sides moved to same location"
    );
}

#[test]
fn test_both_moved_to_different_locations_is_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    let synced = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "old.txt",
        Some("hash"),
        NodeType::File,
        "old.txt",
        Some(parent_lid.clone()),
        "old.txt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    let local_file = make_local_file(
        lid.clone(),
        Some(parent_lid.clone()),
        "local_name.txt",
        "hash",
    );
    store.insert_local_node(&local_file).unwrap();

    let remote_file = make_remote_file(
        rid.clone(),
        Some(parent_rid.clone()),
        "remote_name.txt",
        "hash",
    );
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let conflict = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Conflict(c) if c.kind == ConflictKind::BothMoved));
    assert!(conflict.is_some(), "Should produce BothMoved conflict");
}

#[test]
fn test_remote_rename_and_content_change_generates_move_and_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    let synced = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "old.txt",
        Some("old_hash"),
        NodeType::File,
        "old.txt",
        Some(parent_lid.clone()),
        "old.txt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "old.txt", "old_hash");
    store.insert_local_node(&local_file).unwrap();

    let mut remote_file =
        make_remote_file(rid.clone(), Some(parent_rid.clone()), "new.txt", "new_hash");
    remote_file.rev = "2-abc".to_string();
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let has_move = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })));
    let has_download = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadUpdate { .. })));

    assert!(has_move, "Should plan MoveLocal");
    assert!(has_download, "Should plan DownloadUpdate");

    let move_idx = ops
        .iter()
        .position(|r| matches!(r, PlanResult::Op(SyncOp::MoveLocal { .. })))
        .unwrap();
    let download_idx = ops
        .iter()
        .position(|r| matches!(r, PlanResult::Op(SyncOp::DownloadUpdate { .. })))
        .unwrap();
    assert!(
        move_idx < download_idx,
        "MoveLocal should be sorted before DownloadUpdate"
    );
}

#[test]
fn test_local_rename_and_content_change_generates_move_and_upload() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    let synced = make_synced_with_location(
        lid.clone(),
        rid.clone(),
        "old.txt",
        Some("old_hash"),
        NodeType::File,
        "old.txt",
        Some(parent_lid.clone()),
        "old.txt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    let local_file = make_local_file(lid.clone(), Some(parent_lid.clone()), "new.txt", "new_hash");
    store.insert_local_node(&local_file).unwrap();

    let remote_file =
        make_remote_file(rid.clone(), Some(parent_rid.clone()), "old.txt", "old_hash");
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let has_move = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::MoveRemote { .. })));
    let has_upload = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::UploadUpdate { .. })));

    assert!(has_move, "Should plan MoveRemote");
    assert!(has_upload, "Should plan UploadUpdate");

    let move_idx = ops
        .iter()
        .position(|r| matches!(r, PlanResult::Op(SyncOp::MoveRemote { .. })))
        .unwrap();
    let upload_idx = ops
        .iter()
        .position(|r| matches!(r, PlanResult::Op(SyncOp::UploadUpdate { .. })))
        .unwrap();
    assert!(
        move_idx < upload_idx,
        "MoveRemote should be sorted before UploadUpdate"
    );

    if let Some(PlanResult::Op(SyncOp::UploadUpdate { local_path, .. })) = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::UploadUpdate { .. })))
    {
        assert_eq!(
            local_path,
            &PathBuf::from("/sync/new.txt"),
            "UploadUpdate should use post-move local path"
        );
    }
}

#[test]
fn test_remote_name_dotdot_is_rejected_as_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    store
        .insert_remote_node(&make_remote_dir(remote_id("root"), None, ""))
        .unwrap();
    let remote_file = make_remote_file(remote_id("f1"), Some(remote_id("root")), "..", "abc123");
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let has_download = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })));
    assert!(!has_download, "Should NOT plan a download for '..' name");

    let has_conflict = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Conflict(c) if c.kind == ConflictKind::InvalidName));
    assert!(
        has_conflict,
        "Should produce InvalidName conflict for '..' name"
    );
}

#[test]
fn test_remote_name_with_slash_is_rejected_as_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    store
        .insert_remote_node(&make_remote_dir(remote_id("root"), None, ""))
        .unwrap();
    let remote_file = make_remote_file(
        remote_id("f1"),
        Some(remote_id("root")),
        "a/../../etc/passwd",
        "abc123",
    );
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let has_download = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })));
    assert!(
        !has_download,
        "Should NOT plan a download for name with '/'"
    );

    let has_conflict = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Conflict(c) if c.kind == ConflictKind::InvalidName));
    assert!(
        has_conflict,
        "Should produce InvalidName conflict for name with '/'"
    );
}

#[test]
fn test_remote_name_dot_is_rejected_as_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let remote_file = make_remote_file(remote_id("f1"), Some(remote_id("root")), ".", "abc123");
    store.insert_remote_node(&remote_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let has_download = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })));
    assert!(!has_download, "Should NOT plan a download for '.' name");
}

#[test]
fn test_atomic_save_rebinds_inode_and_generates_upload_update() {
    // Simulate "atomic save": app writes to temp file, deletes original,
    // renames temp to original. The file gets a new inode but same path.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let old_lid = local_id(1, 100); // original inode
    let new_lid = local_id(1, 200); // new inode after atomic save
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    // Set up root
    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    // Synced record points to old inode
    let synced = make_synced_with_location(
        old_lid.clone(),
        rid.clone(),
        "report.odt",
        Some("old_hash"),
        NodeType::File,
        "report.odt",
        Some(parent_lid.clone()),
        "report.odt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Old inode is GONE (not in local tree) — deleted by atomic save
    // New inode appears at same path with new content
    let new_local = make_local_file(
        new_lid.clone(),
        Some(parent_lid.clone()),
        "report.odt",
        "new_hash",
    );
    store.insert_local_node(&new_local).unwrap();

    // Remote is unchanged
    let remote = make_remote_file(
        rid.clone(),
        Some(parent_rid.clone()),
        "report.odt",
        "old_hash",
    );
    store.insert_remote_node(&remote).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should NOT produce DeleteRemote + UploadNew
    let has_delete = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DeleteRemote { .. })));
    assert!(!has_delete, "Should NOT delete remote file on atomic save");

    let has_upload_new = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::UploadNew { .. })));
    assert!(
        !has_upload_new,
        "Should NOT upload as new file on atomic save"
    );

    // Should produce UploadUpdate with the new local ID
    let upload_update = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::UploadUpdate { .. })));
    assert!(
        upload_update.is_some(),
        "Should produce UploadUpdate for atomic save, got: {:?}",
        ops
    );

    if let Some(PlanResult::Op(SyncOp::UploadUpdate {
        local_id,
        remote_id,
        expected_local_md5,
        ..
    })) = upload_update
    {
        assert_eq!(*local_id, new_lid, "Should use the new inode");
        assert_eq!(remote_id.as_str(), "f1");
        assert_eq!(expected_local_md5, "new_hash");
    }
}

#[test]
fn test_atomic_save_same_content_produces_no_op() {
    // Atomic save where content didn't actually change (e.g. user opened and
    // saved without editing). Should produce no operations.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let old_lid = local_id(1, 100);
    let new_lid = local_id(1, 200);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    let synced = make_synced_with_location(
        old_lid.clone(),
        rid.clone(),
        "report.odt",
        Some("same_hash"),
        NodeType::File,
        "report.odt",
        Some(parent_lid.clone()),
        "report.odt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // New inode, same content hash
    let new_local = make_local_file(
        new_lid.clone(),
        Some(parent_lid.clone()),
        "report.odt",
        "same_hash",
    );
    store.insert_local_node(&new_local).unwrap();

    let remote = make_remote_file(
        rid.clone(),
        Some(parent_rid.clone()),
        "report.odt",
        "same_hash",
    );
    store.insert_remote_node(&remote).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    assert!(
        ops.is_empty(),
        "Atomic save with same content should be a no-op, got: {:?}",
        ops
    );
}

#[test]
fn test_atomic_save_does_not_rebind_when_name_differs() {
    // A synced file disappears and a *different* file appears in the same
    // parent. Should NOT rebind — treat as normal delete + create.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let old_lid = local_id(1, 100);
    let new_lid = local_id(1, 200);
    let rid = remote_id("f1");
    let parent_lid = local_id(1, 1);
    let parent_rid = remote_id("root");

    let synced_root = make_synced_with_location(
        parent_lid.clone(),
        parent_rid.clone(),
        "",
        None,
        NodeType::Directory,
        "",
        None,
        "",
        None,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(parent_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(parent_rid.clone(), None, ""))
        .unwrap();

    let synced = make_synced_with_location(
        old_lid.clone(),
        rid.clone(),
        "report.odt",
        Some("old_hash"),
        NodeType::File,
        "report.odt",
        Some(parent_lid.clone()),
        "report.odt",
        Some(parent_rid.clone()),
    );
    store.insert_synced(&synced).unwrap();

    // Different file name — this is NOT an atomic save
    let new_local = make_local_file(
        new_lid.clone(),
        Some(parent_lid.clone()),
        "other_file.txt",
        "new_hash",
    );
    store.insert_local_node(&new_local).unwrap();

    let remote = make_remote_file(
        rid.clone(),
        Some(parent_rid.clone()),
        "report.odt",
        "old_hash",
    );
    store.insert_remote_node(&remote).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should treat as separate delete + create, not rebind
    let has_delete = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DeleteRemote { .. })));
    let has_upload_new = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::UploadNew { .. })));

    assert!(
        has_delete,
        "Should delete remote when name differs, got: {:?}",
        ops
    );
    assert!(
        has_upload_new,
        "Should upload new when name differs, got: {:?}",
        ops
    );
}

// ==================== Overwrite detection tests ====================

#[test]
fn test_new_remote_and_local_same_path_same_content_emits_bind() {
    // When a new remote file and a new local file appear at the same path
    // with the same content, the planner should emit a BindExisting op
    // so that a SyncedRecord is created and the planner converges.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let root_lid = local_id(1, 1);
    let root_rid = remote_id("root");

    let synced_root = make_synced(
        root_lid.clone(),
        root_rid.clone(),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(root_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(root_rid.clone(), None, ""))
        .unwrap();

    // New remote file at "doc.txt" with hash "abc123"
    let remote_file =
        make_remote_file(remote_id("f1"), Some(root_rid.clone()), "doc.txt", "abc123");
    store.insert_remote_node(&remote_file).unwrap();

    // New local file at "doc.txt" with the SAME hash "abc123"
    let local_file = make_local_file(
        local_id(1, 100),
        Some(root_lid.clone()),
        "doc.txt",
        "abc123",
    );
    store.insert_local_node(&local_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should NOT have DownloadNew, UploadNew, or a conflict
    let has_download = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })));
    let has_upload = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::UploadNew { .. })));
    let has_conflict = ops.iter().any(|r| matches!(r, PlanResult::Conflict(_)));

    assert!(!has_download, "Should NOT download, got: {:?}", ops);
    assert!(!has_upload, "Should NOT upload, got: {:?}", ops);
    assert!(!has_conflict, "Should NOT conflict, got: {:?}", ops);

    // Should emit a BindExisting op to create the SyncedRecord
    let bind = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::BindExisting { .. })));
    assert!(
        bind.is_some(),
        "Should produce BindExisting to link local and remote, got: {:?}",
        ops
    );
    if let Some(PlanResult::Op(SyncOp::BindExisting {
        local_id: lid,
        remote_id: rid,
        ..
    })) = bind
    {
        assert_eq!(lid.inode, 100);
        assert_eq!(rid.as_str(), "f1");
    }
}

#[test]
fn test_new_remote_and_local_same_path_different_content_is_conflict() {
    // When a new remote file and a new local file appear at the same path
    // with different content, it's a genuine NameCollision conflict.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let root_lid = local_id(1, 1);
    let root_rid = remote_id("root");

    let synced_root = make_synced(
        root_lid.clone(),
        root_rid.clone(),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(root_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(root_rid.clone(), None, ""))
        .unwrap();

    // New remote file at "doc.txt" with hash "remote_hash"
    let remote_file = make_remote_file(
        remote_id("f1"),
        Some(root_rid.clone()),
        "doc.txt",
        "remote_hash",
    );
    store.insert_remote_node(&remote_file).unwrap();

    // New local file at "doc.txt" with DIFFERENT hash "local_hash"
    let local_file = make_local_file(
        local_id(1, 100),
        Some(root_lid.clone()),
        "doc.txt",
        "local_hash",
    );
    store.insert_local_node(&local_file).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should produce a NameCollision conflict
    let collision = ops.iter().find(|r| {
        matches!(
            r,
            PlanResult::Conflict(Conflict {
                kind: ConflictKind::NameCollision,
                ..
            })
        )
    });
    assert!(
        collision.is_some(),
        "Should produce NameCollision conflict for different content at same path, got: {:?}",
        ops
    );

    // Should NOT have both DownloadNew and UploadNew
    let has_download = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })));
    let has_upload = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::UploadNew { .. })));
    assert!(
        !(has_download && has_upload),
        "Should NOT produce both DownloadNew and UploadNew, got: {:?}",
        ops
    );
}

#[test]
fn test_new_remote_and_local_same_dir_same_path_emits_bind() {
    // When a new remote dir and a new local dir appear at the same path,
    // the planner should emit a BindExisting op to create a SyncedRecord.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let root_lid = local_id(1, 1);
    let root_rid = remote_id("root");

    let synced_root = make_synced(
        root_lid.clone(),
        root_rid.clone(),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(root_lid.clone(), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(root_rid.clone(), None, ""))
        .unwrap();

    // New remote dir "photos"
    let remote_dir = make_remote_dir(remote_id("d1"), Some(root_rid.clone()), "photos");
    store.insert_remote_node(&remote_dir).unwrap();

    // New local dir "photos"
    let local_dir = make_local_dir(local_id(1, 200), Some(root_lid.clone()), "photos");
    store.insert_local_node(&local_dir).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let has_create_local = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::CreateLocalDir { .. })));
    let has_create_remote = ops
        .iter()
        .any(|r| matches!(r, PlanResult::Op(SyncOp::CreateRemoteDir { .. })));

    assert!(
        !has_create_local,
        "Should NOT create local dir, got: {:?}",
        ops
    );
    assert!(
        !has_create_remote,
        "Should NOT create remote dir, got: {:?}",
        ops
    );

    let bind = ops
        .iter()
        .find(|r| matches!(r, PlanResult::Op(SyncOp::BindExisting { .. })));
    assert!(
        bind.is_some(),
        "Should produce BindExisting for matching dirs, got: {:?}",
        ops
    );
}

#[test]
fn test_both_deleted_generates_delete_synced() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // A synced record exists but neither the local nor remote node is present
    let synced = make_synced(
        local_id(1, 100),
        remote_id("f1"),
        "file.txt",
        Some("abc123"),
        NodeType::File,
    );
    store.insert_synced(&synced).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1, "Expected exactly one op, got: {:?}", ops);
    match &ops[0] {
        PlanResult::Op(SyncOp::DeleteSynced { local_id }) => {
            assert_eq!(local_id.inode, 100);
        }
        other => panic!("Expected DeleteSynced, got {:?}", other),
    }

    // The synced record should NOT have been deleted by the planner
    let still_exists = store.get_synced_by_local(&local_id(1, 100)).unwrap();
    assert!(
        still_exists.is_some(),
        "Planner should not modify the store; the synced record should still exist"
    );
}

#[test]
fn test_orphaned_remote_node_is_not_treated_as_cycle() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // Root
    let synced_root = make_synced(
        local_id(1, 1),
        remote_id("root"),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(local_id(1, 1), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(remote_id("root"), None, ""))
        .unwrap();

    // A remote file whose parent ("missing_parent") is NOT in the remote tree
    // (e.g., incomplete fetch). This should NOT be treated as a cycle.
    let orphan = make_remote_file(
        remote_id("orphan1"),
        Some(remote_id("missing_parent")),
        "orphan.txt",
        "abc",
    );
    store.insert_remote_node(&orphan).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let cycle_conflicts: Vec<_> = ops
        .iter()
        .filter(|r| matches!(r, PlanResult::Conflict(c) if c.kind == ConflictKind::CycleDetected))
        .collect();
    assert!(
        cycle_conflicts.is_empty(),
        "Orphaned nodes should not produce CycleDetected conflicts, got {cycle_conflicts:?}"
    );
}

#[test]
fn test_actual_cycle_is_detected() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // Root
    let synced_root = make_synced(
        local_id(1, 1),
        remote_id("root"),
        "",
        None,
        NodeType::Directory,
    );
    store.insert_synced(&synced_root).unwrap();
    store
        .insert_local_node(&make_local_dir(local_id(1, 1), None, ""))
        .unwrap();
    store
        .insert_remote_node(&make_remote_dir(remote_id("root"), None, ""))
        .unwrap();

    // Two directories forming a cycle: A→B→A
    let dir_a = make_remote_dir(remote_id("dir_a"), Some(remote_id("dir_b")), "dir_a");
    let dir_b = make_remote_dir(remote_id("dir_b"), Some(remote_id("dir_a")), "dir_b");
    store.insert_remote_node(&dir_a).unwrap();
    store.insert_remote_node(&dir_b).unwrap();

    let rules = IgnoreRules::none();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    let cycle_conflicts: Vec<_> = ops
        .iter()
        .filter(|r| matches!(r, PlanResult::Conflict(c) if c.kind == ConflictKind::CycleDetected))
        .collect();
    assert_eq!(
        cycle_conflicts.len(),
        2,
        "Both nodes in the cycle should produce CycleDetected conflicts"
    );
}

#[test]
fn test_synced_file_now_ignored_cleans_up_synced_record() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    // File is synced and still present on both sides
    let local_file = make_local_file(lid.clone(), None, "secret.tmp", "abc123");
    let remote_file = make_remote_file(rid.clone(), None, "secret.tmp", "abc123");
    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "secret.tmp",
        Some("abc123"),
        NodeType::File,
    );

    store.insert_local_node(&local_file).unwrap();
    store.insert_remote_node(&remote_file).unwrap();
    store.insert_synced(&synced).unwrap();

    // Use rules that ignore .tmp files (default rules do this)
    let rules = IgnoreRules::default_only();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should only produce a DeleteSynced to stop tracking, NOT delete local or remote
    assert_eq!(ops.len(), 1, "ops: {ops:?}");
    match &ops[0] {
        PlanResult::Op(SyncOp::DeleteSynced { local_id }) => {
            assert_eq!(*local_id, lid);
        }
        other => panic!("Expected DeleteSynced, got {:?}", other),
    }
}

#[test]
fn test_synced_file_now_ignored_with_remote_deleted_just_cleans_up() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    // File is synced, local still exists, remote was deleted
    let local_file = make_local_file(lid.clone(), None, "notes.bak", "abc123");
    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "notes.bak",
        Some("abc123"),
        NodeType::File,
    );

    store.insert_local_node(&local_file).unwrap();
    store.insert_synced(&synced).unwrap();

    let rules = IgnoreRules::default_only();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should NOT delete the local file — just clean up synced record
    assert_eq!(ops.len(), 1, "ops: {ops:?}");
    match &ops[0] {
        PlanResult::Op(SyncOp::DeleteSynced { local_id }) => {
            assert_eq!(*local_id, lid);
        }
        other => panic!("Expected DeleteSynced, got {:?}", other),
    }
}

#[test]
fn test_synced_file_now_ignored_with_local_deleted_just_cleans_up() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let lid = local_id(1, 100);
    let rid = remote_id("f1");

    // File is synced, remote still exists, local was deleted
    let remote_file = make_remote_file(rid.clone(), None, "cache.swp", "abc123");
    let synced = make_synced(
        lid.clone(),
        rid.clone(),
        "cache.swp",
        Some("abc123"),
        NodeType::File,
    );

    store.insert_remote_node(&remote_file).unwrap();
    store.insert_synced(&synced).unwrap();

    let rules = IgnoreRules::default_only();
    let planner = Planner::new(&store, PathBuf::from("/sync"), &rules);
    let ops = planner.plan().unwrap();

    // Should NOT delete the remote file — just clean up synced record
    assert_eq!(ops.len(), 1, "ops: {ops:?}");
    match &ops[0] {
        PlanResult::Op(SyncOp::DeleteSynced { local_id }) => {
            assert_eq!(*local_id, lid);
        }
        other => panic!("Expected DeleteSynced, got {:?}", other),
    }
}
