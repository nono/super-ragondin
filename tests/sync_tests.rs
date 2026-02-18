use cozy_desktop::model::{LocalFileId, NodeType, RemoteId, RemoteNode, SyncedRecord};
use cozy_desktop::planner::Planner;
use cozy_desktop::store::TreeStore;
use cozy_desktop::sync::engine::SyncEngine;
use tempfile::tempdir;

fn insert_root_synced(store: &TreeStore) {
    let synced = SyncedRecord {
        local_id: LocalFileId::new(1, 1),
        remote_id: RemoteId::new("io.cozy.files.root-dir"),
        rel_path: String::new(),
        md5sum: None,
        size: None,
        rev: "1-root".to_string(),
        node_type: NodeType::Directory,
        local_name: Some(String::new()),
        local_parent_id: None,
        remote_name: Some(String::new()),
        remote_parent_id: None,
    };
    store.insert_synced(&synced).unwrap();
}

#[test]
fn test_sync_engine_plans_download_for_new_remote_file() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Add a file to remote tree that doesn't exist locally or in synced
    let remote_file = RemoteNode {
        id: RemoteId::new("remote-file-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "document.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("098f6bcd4621d373cade4e832627b4f6".to_string()),
        size: Some(4),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    };
    store.insert_remote_node(&remote_file).unwrap();

    // Add root dir to remote (so we have a valid parent)
    let root = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: "".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root).unwrap();
    insert_root_synced(&store);

    store.flush().unwrap();

    // Plan should generate a download operation
    let planner = Planner::new(&store, sync_dir.path().to_path_buf());
    let results = planner.plan().unwrap();

    assert!(!results.is_empty(), "Should have planned operations");

    // Find the download operation for our file
    let has_download = results.iter().any(|r| {
        matches!(r, cozy_desktop::model::PlanResult::Op(
            cozy_desktop::model::SyncOp::DownloadNew { remote_id, .. }
        ) if remote_id.as_str() == "remote-file-1")
    });
    assert!(has_download, "Should plan to download the new remote file");
}

#[test]
fn test_sync_engine_creation() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // Engine should be creatable
    assert!(engine.sync_dir().exists());
}

#[test]
fn test_sync_engine_initial_scan() {
    use std::fs;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    // Create some files in the sync dir
    fs::write(sync_dir.path().join("file1.txt"), "hello").unwrap();
    fs::create_dir(sync_dir.path().join("subdir")).unwrap();
    fs::write(sync_dir.path().join("subdir/file2.txt"), "world").unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // Scan the local directory
    engine.initial_scan().unwrap();

    // Verify nodes were added to local tree
    let local_nodes = engine.store().list_all_local().unwrap();
    assert!(!local_nodes.is_empty(), "Should have scanned local nodes");

    // Should have at least: file1.txt, subdir, subdir/file2.txt
    assert!(local_nodes.len() >= 3, "Should have at least 3 nodes");
}

#[test]
fn test_sync_engine_plan() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Add a remote file
    let remote_file = RemoteNode {
        id: RemoteId::new("file-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "test.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc123".to_string()),
        size: Some(100),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    };
    store.insert_remote_node(&remote_file).unwrap();
    insert_root_synced(&store);
    store.flush().unwrap();

    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // Engine should be able to plan
    let results = engine.plan().unwrap();
    assert!(!results.is_empty(), "Should have planned operations");
}

#[test]
fn test_sync_engine_execute_create_local_dir() {
    use cozy_desktop::model::SyncOp;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Add a remote directory
    let remote_dir = RemoteNode {
        id: RemoteId::new("dir-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "documents".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-abc".to_string(),
    };
    store.insert_remote_node(&remote_dir).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // Create the operation
    let op = SyncOp::CreateLocalDir {
        remote_id: RemoteId::new("dir-1"),
        local_path: sync_dir.path().join("documents"),
    };

    // Execute it
    engine.execute_op(&op).unwrap();

    // Verify directory was created
    assert!(
        sync_dir.path().join("documents").is_dir(),
        "Directory should have been created"
    );

    // Verify SyncedRecord was created linking remote and local
    let synced = engine
        .store()
        .get_synced_by_remote(&RemoteId::new("dir-1"))
        .unwrap();
    assert!(synced.is_some(), "SyncedRecord should exist for remote_id");

    let synced = synced.unwrap();
    assert_eq!(synced.remote_id.as_str(), "dir-1");
    assert_eq!(synced.rel_path, "documents");
    assert_eq!(synced.node_type, NodeType::Directory);
}

#[test]
fn test_sync_engine_execute_delete_local() {
    use cozy_desktop::model::{LocalFileId, LocalNode, SyncOp};
    use md5::{Digest, Md5};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    // Create a file to delete
    let file_path = sync_dir.path().join("to_delete.txt");
    let content = b"content";
    fs::write(&file_path, content).unwrap();

    let mut hasher = Md5::new();
    hasher.update(content);
    let md5 = hex::encode(hasher.finalize());

    let metadata = fs::metadata(&file_path).unwrap();
    let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Add to local tree
    let local_node = LocalNode {
        id: local_id.clone(),
        parent_id: None,
        name: "to_delete.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.clone()),
        size: Some(content.len() as u64),
        mtime: 1000,
    };
    store.insert_local_node(&local_node).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // Create delete operation with correct md5
    let op = SyncOp::DeleteLocal {
        local_id: local_id.clone(),
        local_path: file_path.clone(),
        expected_md5: Some(md5),
    };

    // Execute
    engine.execute_op(&op).unwrap();

    // Verify file was deleted
    assert!(!file_path.exists(), "File should have been deleted");

    // Verify local node was removed from store
    let node = engine.store().get_local_node(&local_id).unwrap();
    assert!(
        node.is_none(),
        "Local node should have been removed from store"
    );
}

#[test]
fn test_planner_computes_nested_paths() {
    use cozy_desktop::model::PlanResult;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Create remote hierarchy: root -> docs -> file.txt
    let root = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: "".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root).unwrap();
    insert_root_synced(&store);

    let docs_dir = RemoteNode {
        id: RemoteId::new("docs-dir"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "docs".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-docs".to_string(),
    };
    store.insert_remote_node(&docs_dir).unwrap();

    // Synced record for docs dir so planner can plan file.txt download
    let docs_synced = SyncedRecord {
        local_id: LocalFileId::new(1, 2),
        remote_id: RemoteId::new("docs-dir"),
        rel_path: "docs".to_string(),
        md5sum: None,
        size: None,
        rev: "1-docs".to_string(),
        node_type: NodeType::Directory,
        local_name: Some("docs".to_string()),
        local_parent_id: Some(LocalFileId::new(1, 1)),
        remote_name: Some("docs".to_string()),
        remote_parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
    };
    store.insert_synced(&docs_synced).unwrap();

    let file = RemoteNode {
        id: RemoteId::new("file-1"),
        parent_id: Some(RemoteId::new("docs-dir")),
        name: "file.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc123".to_string()),
        size: Some(100),
        updated_at: 2000,
        rev: "1-file".to_string(),
    };
    store.insert_remote_node(&file).unwrap();
    store.flush().unwrap();

    let planner = Planner::new(&store, sync_dir.path().to_path_buf());
    let results = planner.plan().unwrap();

    // Find the download operation for file.txt
    let download_op = results.iter().find(|r| {
        matches!(r, PlanResult::Op(cozy_desktop::model::SyncOp::DownloadNew { remote_id, .. })
            if remote_id.as_str() == "file-1")
    });

    assert!(download_op.is_some(), "Should plan to download file.txt");

    // Verify the path includes the parent directory
    if let Some(PlanResult::Op(cozy_desktop::model::SyncOp::DownloadNew { local_path, .. })) =
        download_op
    {
        let expected_path = sync_dir.path().join("docs").join("file.txt");
        assert_eq!(
            *local_path, expected_path,
            "Path should be nested: {:?} != {:?}",
            local_path, expected_path
        );
    }

    // Root should NOT be planned as CreateLocalDir
    let root_create = results.iter().find(|r| {
        matches!(r, PlanResult::Op(cozy_desktop::model::SyncOp::CreateLocalDir { remote_id, .. })
            if remote_id.as_str() == "io.cozy.files.root-dir")
    });
    assert!(
        root_create.is_none(),
        "Root directory should not be planned as CreateLocalDir"
    );
}

#[test]
fn test_sync_engine_execute_move_local() {
    use cozy_desktop::model::{LocalFileId, LocalNode, SyncOp};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let from_path = sync_dir.path().join("old.txt");
    fs::write(&from_path, "content").unwrap();

    let metadata = fs::metadata(&from_path).unwrap();
    let local_id = LocalFileId::new(metadata.dev(), metadata.ino());
    let parent_id = {
        let m = fs::metadata(sync_dir.path()).unwrap();
        LocalFileId::new(m.dev(), m.ino())
    };

    let store = TreeStore::open(store_dir.path()).unwrap();

    let local_node = LocalNode {
        id: local_id.clone(),
        parent_id: Some(parent_id.clone()),
        name: "old.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(7),
        mtime: 1000,
    };
    store.insert_local_node(&local_node).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    let to_path = sync_dir.path().join("new.txt");
    let op = SyncOp::MoveLocal {
        local_id: local_id.clone(),
        from_path: from_path.clone(),
        to_path: to_path.clone(),
        expected_parent_id: Some(parent_id.clone()),
        expected_name: "old.txt".to_string(),
    };

    engine.execute_op(&op).unwrap();

    assert!(!from_path.exists(), "Old path should not exist");
    assert!(to_path.exists(), "New path should exist");
    assert_eq!(fs::read_to_string(&to_path).unwrap(), "content");
}

#[test]
fn test_delete_local_refuses_when_md5_changed() {
    use cozy_desktop::model::{LocalFileId, LocalNode, SyncOp};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let file_path = sync_dir.path().join("important.txt");
    fs::write(&file_path, "original content").unwrap();

    let metadata = fs::metadata(&file_path).unwrap();
    let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

    let store = TreeStore::open(store_dir.path()).unwrap();
    let local_node = LocalNode {
        id: local_id.clone(),
        parent_id: None,
        name: "important.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("old_md5".to_string()),
        size: Some(16),
        mtime: 1000,
    };
    store.insert_local_node(&local_node).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // User modifies the file between planning and execution
    fs::write(&file_path, "modified content!!!").unwrap();

    let op = SyncOp::DeleteLocal {
        local_id: local_id.clone(),
        local_path: file_path.clone(),
        expected_md5: Some("old_md5".to_string()),
    };

    // Should refuse to delete since md5 no longer matches
    let result = engine.execute_op(&op);
    assert!(result.is_err(), "Should refuse to delete modified file");
    assert!(
        file_path.exists(),
        "File must still exist after refused delete"
    );
}

#[test]
fn test_delete_local_succeeds_when_md5_matches() {
    use cozy_desktop::model::{LocalFileId, LocalNode, SyncOp};
    use md5::{Digest, Md5};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let file_path = sync_dir.path().join("to_delete.txt");
    let content = b"delete me";
    fs::write(&file_path, content).unwrap();

    let mut hasher = Md5::new();
    hasher.update(content);
    let md5 = hex::encode(hasher.finalize());

    let metadata = fs::metadata(&file_path).unwrap();
    let local_id = LocalFileId::new(metadata.dev(), metadata.ino());

    let store = TreeStore::open(store_dir.path()).unwrap();
    let local_node = LocalNode {
        id: local_id.clone(),
        parent_id: None,
        name: "to_delete.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.clone()),
        size: Some(content.len() as u64),
        mtime: 1000,
    };
    store.insert_local_node(&local_node).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    let op = SyncOp::DeleteLocal {
        local_id,
        local_path: file_path.clone(),
        expected_md5: Some(md5),
    };

    engine.execute_op(&op).unwrap();
    assert!(
        !file_path.exists(),
        "File should be deleted when md5 matches"
    );
}

#[test]
fn test_move_local_refuses_when_inode_mismatches() {
    use cozy_desktop::model::{LocalFileId, LocalNode, SyncOp};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let from_path = sync_dir.path().join("old.txt");
    fs::write(&from_path, "content").unwrap();

    let metadata = fs::metadata(&from_path).unwrap();
    let real_id = LocalFileId::new(metadata.dev(), metadata.ino());
    let parent_id = {
        let m = fs::metadata(sync_dir.path()).unwrap();
        LocalFileId::new(m.dev(), m.ino())
    };

    let store = TreeStore::open(store_dir.path()).unwrap();
    let local_node = LocalNode {
        id: real_id.clone(),
        parent_id: Some(parent_id.clone()),
        name: "old.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(7),
        mtime: 1000,
    };
    store.insert_local_node(&local_node).unwrap();
    store.flush().unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        sync_dir.path().join(".staging"),
    );

    // Use a wrong local_id (different inode) - simulates the file being
    // replaced between planning and execution
    let wrong_id = LocalFileId::new(metadata.dev(), metadata.ino() + 999);

    let to_path = sync_dir.path().join("new.txt");
    let op = SyncOp::MoveLocal {
        local_id: wrong_id,
        from_path: from_path.clone(),
        to_path: to_path.clone(),
        expected_parent_id: Some(parent_id),
        expected_name: "old.txt".to_string(),
    };

    let result = engine.execute_op(&op);
    assert!(
        result.is_err(),
        "Should refuse to move when inode mismatches"
    );
    assert!(from_path.exists(), "Original file must still exist");
    assert!(!to_path.exists(), "Destination must not be created");
}
