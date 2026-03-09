use cozy_desktop::model::{
    ConflictKind, LocalFileId, NodeType, RemoteId, RemoteNode, SyncedRecord,
};
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

#[test]
fn test_sync_engine_run_cycle() {
    use cozy_desktop::model::{PlanResult, RemoteNode, SyncOp};

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Add root remote node so the planner doesn't think it was remotely deleted
    let root_remote = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root_remote).unwrap();

    // Add a remote directory that should be created locally
    let remote_dir = RemoteNode {
        id: RemoteId::new("dir-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "photos".to_string(),
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

    // First cycle: should create the directory
    let results = engine.run_cycle().unwrap();
    let create_ops: Vec<_> = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(SyncOp::CreateLocalDir { .. })))
        .collect();
    assert_eq!(create_ops.len(), 1, "First cycle should create one dir");
    assert!(
        sync_dir.path().join("photos").is_dir(),
        "Directory should exist after first cycle"
    );

    // Second cycle: no new ops since everything is synced
    let results = engine.run_cycle().unwrap();
    let ops: Vec<_> = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(_)))
        .collect();
    assert!(ops.is_empty(), "Second cycle should have no ops");
}

#[tokio::test]
async fn test_sync_engine_download_new_via_async() -> Result<(), Box<dyn std::error::Error>> {
    use cozy_desktop::model::{PlanResult, SyncOp};
    use cozy_desktop::remote::client::CozyClient;
    use std::os::unix::fs::MetadataExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // Mock the download endpoint
    Mock::given(method("GET"))
        .and(path("/files/download/file-1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world"))
        .mount(&mock_server)
        .await;

    let store_dir = tempdir()?;
    let sync_dir = tempdir()?;
    let staging_dir = tempdir()?;

    let store = TreeStore::open(store_dir.path())?;

    // Set up root with actual sync dir inode so initial_scan matches
    let sync_meta = std::fs::metadata(sync_dir.path())?;
    let root_local_id = LocalFileId::new(sync_meta.dev(), sync_meta.ino());
    let root_synced = SyncedRecord {
        local_id: root_local_id.clone(),
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
    store.insert_synced(&root_synced)?;

    // Add root remote node
    let root = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root)?;

    // Also insert root local node so planner doesn't think it was deleted
    let root_local = cozy_desktop::model::LocalNode {
        id: root_local_id,
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 0,
    };
    store.insert_local_node(&root_local)?;

    // Add a remote file to download
    let md5sum = "5eb63bbbe01eeed093cb22bb8f5acdc3"; // md5 of "hello world"
    let remote_file = RemoteNode {
        id: RemoteId::new("file-1"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "hello.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5sum.to_string()),
        size: Some(11),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    };
    store.insert_remote_node(&remote_file)?;
    store.flush()?;

    let client = CozyClient::new(&mock_server.uri(), "fake-token");
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let results = engine.run_cycle_async(&client).await?;

    // Should have planned and executed a DownloadNew
    let has_download = results.iter().any(|r| {
        matches!(
            r,
            PlanResult::Op(SyncOp::DownloadNew { remote_id, .. })
            if remote_id.as_str() == "file-1"
        )
    });
    assert!(has_download, "Should have planned DownloadNew");

    // File should now exist on disk
    let file_path = sync_dir.path().join("hello.txt");
    assert!(file_path.exists(), "Downloaded file should exist");
    assert_eq!(std::fs::read_to_string(&file_path)?, "hello world");

    // Synced record should exist
    let synced = engine
        .store()
        .get_synced_by_remote(&RemoteId::new("file-1"))?;
    assert!(synced.is_some(), "SyncedRecord should exist after download");
    let synced = synced.unwrap();
    assert_eq!(synced.md5sum.as_deref(), Some(md5sum));

    Ok(())
}

#[tokio::test]
async fn test_sync_engine_upload_new_via_async() -> Result<(), Box<dyn std::error::Error>> {
    use cozy_desktop::model::{PlanResult, SyncOp};
    use cozy_desktop::remote::client::CozyClient;
    use std::os::unix::fs::MetadataExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // Mock the upload endpoint (POST /files/:parent-id?Type=file&Name=...)
    Mock::given(method("POST"))
        .and(path("/files/io.cozy.files.root-dir"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "data": {
                "id": "new-remote-file-1",
                "attributes": {
                    "type": "file",
                    "name": "local.txt",
                    "dir_id": "io.cozy.files.root-dir",
                    "md5sum": "fc3ff98e8c6a0d3087d515c0473f8677",
                    "size": 12,
                    "updated_at": "2026-01-01T00:00:00Z"
                },
                "meta": {
                    "rev": "1-newrev"
                }
            }
        })))
        .mount(&mock_server)
        .await;

    let store_dir = tempdir()?;
    let sync_dir = tempdir()?;
    let staging_dir = tempdir()?;

    // Create a local file to upload
    std::fs::write(sync_dir.path().join("local.txt"), "hello world!")?;

    let store = TreeStore::open(store_dir.path())?;

    // Set up root
    let sync_meta = std::fs::metadata(sync_dir.path())?;
    let root_local_id = LocalFileId::new(sync_meta.dev(), sync_meta.ino());
    let root_synced = SyncedRecord {
        local_id: root_local_id.clone(),
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
    store.insert_synced(&root_synced)?;

    let root = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root)?;

    let root_local = cozy_desktop::model::LocalNode {
        id: root_local_id,
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 0,
    };
    store.insert_local_node(&root_local)?;
    store.flush()?;

    let client = CozyClient::new(&mock_server.uri(), "fake-token");
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let results = engine.run_cycle_async(&client).await?;

    // Should have planned and executed an UploadNew
    let has_upload = results.iter().any(
        |r| matches!(r, PlanResult::Op(SyncOp::UploadNew { name, .. }) if name == "local.txt"),
    );
    assert!(has_upload, "Should have planned UploadNew");

    // Synced record should exist linking to the new remote id
    let synced = engine
        .store()
        .get_synced_by_remote(&RemoteId::new("new-remote-file-1"))?;
    assert!(synced.is_some(), "SyncedRecord should exist after upload");

    Ok(())
}

#[tokio::test]
async fn test_sync_engine_parallel_downloads() -> Result<(), Box<dyn std::error::Error>> {
    use cozy_desktop::model::{PlanResult, SyncOp};
    use cozy_desktop::remote::client::CozyClient;
    use md5::Digest;
    use std::os::unix::fs::MetadataExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // Mock two download endpoints
    let content_a = b"content of file A";
    let content_b = b"content of file B";

    let md5_a = format!("{:x}", md5::Md5::digest(content_a));
    let md5_b = format!("{:x}", md5::Md5::digest(content_b));

    Mock::given(method("GET"))
        .and(path("/files/download/file-a"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(content_a.as_slice()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("GET"))
        .and(path("/files/download/file-b"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(content_b.as_slice()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store_dir = tempdir()?;
    let sync_dir = tempdir()?;
    let staging_dir = tempdir()?;

    let store = TreeStore::open(store_dir.path())?;

    // Set up root
    let sync_meta = std::fs::metadata(sync_dir.path())?;
    let root_local_id = LocalFileId::new(sync_meta.dev(), sync_meta.ino());
    let root_synced = SyncedRecord {
        local_id: root_local_id.clone(),
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
    store.insert_synced(&root_synced)?;

    let root = RemoteNode {
        id: RemoteId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-root".to_string(),
    };
    store.insert_remote_node(&root)?;

    let root_local = cozy_desktop::model::LocalNode {
        id: root_local_id,
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 0,
    };
    store.insert_local_node(&root_local)?;

    // Add two remote files
    let file_a = RemoteNode {
        id: RemoteId::new("file-a"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "a.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5_a.clone()),
        size: Some(content_a.len() as u64),
        updated_at: 1000,
        rev: "1-a".to_string(),
    };
    let file_b = RemoteNode {
        id: RemoteId::new("file-b"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "b.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5_b.clone()),
        size: Some(content_b.len() as u64),
        updated_at: 1000,
        rev: "1-b".to_string(),
    };
    store.insert_remote_node(&file_a)?;
    store.insert_remote_node(&file_b)?;
    store.flush()?;

    let client = CozyClient::new(&mock_server.uri(), "fake-token");
    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let results = engine.run_cycle_async(&client).await?;

    // Both downloads should have been planned
    let download_count = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(SyncOp::DownloadNew { .. })))
        .count();
    assert_eq!(download_count, 2, "Should have planned 2 downloads");

    // Both files should exist on disk
    assert_eq!(
        std::fs::read_to_string(sync_dir.path().join("a.txt"))?,
        "content of file A"
    );
    assert_eq!(
        std::fs::read_to_string(sync_dir.path().join("b.txt"))?,
        "content of file B"
    );

    // Both synced records should exist
    assert!(
        engine
            .store()
            .get_synced_by_remote(&RemoteId::new("file-a"))?
            .is_some()
    );
    assert!(
        engine
            .store()
            .get_synced_by_remote(&RemoteId::new("file-b"))?
            .is_some()
    );

    Ok(())
}

#[test]
fn test_initial_scan_bootstraps_root() {
    use std::os::unix::fs::MetadataExt;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    let mut engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    engine.initial_scan().unwrap();

    // Root synced record should be bootstrapped automatically
    let sync_meta = std::fs::metadata(sync_dir.path()).unwrap();
    let root_local_id = LocalFileId::new(sync_meta.dev(), sync_meta.ino());

    let root_synced = engine.store().get_synced_by_local(&root_local_id).unwrap();
    assert!(
        root_synced.is_some(),
        "Root synced record should be bootstrapped by initial_scan"
    );
    let root_synced = root_synced.unwrap();
    assert_eq!(
        root_synced.remote_id,
        RemoteId::new("io.cozy.files.root-dir")
    );
    assert_eq!(root_synced.node_type, NodeType::Directory);
    assert!(root_synced.local_parent_id.is_none());
    assert!(root_synced.remote_parent_id.is_none());

    // Root local node should also be inserted
    let root_local = engine.store().get_local_node(&root_local_id).unwrap();
    assert!(
        root_local.is_some(),
        "Root local node should be inserted by initial_scan"
    );
    let root_local = root_local.unwrap();
    assert_eq!(root_local.name, "");
    assert!(root_local.parent_id.is_none());
    assert_eq!(root_local.node_type, NodeType::Directory);
}

#[tokio::test]
async fn test_fetch_and_apply_remote_changes_populates_remote_tree()
-> Result<(), Box<dyn std::error::Error>> {
    use cozy_desktop::remote::client::CozyClient;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // Mock the changes endpoint
    Mock::given(method("GET"))
        .and(path("/files/_changes"))
        .and(query_param("include_docs", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "last_seq": "5-abc",
            "results": [
                {
                    "id": "file-1",
                    "seq": "3-aaa",
                    "doc": {
                        "_id": "file-1",
                        "_rev": "1-rev1",
                        "type": "file",
                        "name": "notes.txt",
                        "dir_id": "io.cozy.files.root-dir",
                        "md5sum": "d41d8cd98f00b204e9800998ecf8427e",
                        "size": 0,
                        "updated_at": "2026-01-01T00:00:00Z"
                    }
                },
                {
                    "id": "dir-1",
                    "seq": "4-bbb",
                    "doc": {
                        "_id": "dir-1",
                        "_rev": "1-rev2",
                        "type": "directory",
                        "name": "photos",
                        "dir_id": "io.cozy.files.root-dir",
                        "size": null,
                        "updated_at": "2026-01-01T00:00:00Z"
                    }
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let store_dir = tempdir()?;
    let sync_dir = tempdir()?;
    let staging_dir = tempdir()?;

    let store = TreeStore::open(store_dir.path())?;
    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let client = CozyClient::new(&mock_server.uri(), "fake-token");

    // Remote tree should be empty before
    assert!(engine.store().list_all_remote()?.is_empty());

    let last_seq = engine.fetch_and_apply_remote_changes(&client, None).await?;

    assert_eq!(last_seq, "5-abc");

    // Remote tree should now have 3 nodes (2 children + implicitly created root)
    let remote_nodes = engine.store().list_all_remote()?;
    assert_eq!(
        remote_nodes.len(),
        3,
        "The root node should be created along with its children"
    );

    let root_exists = remote_nodes
        .iter()
        .any(|n| n.id == RemoteId::new("io.cozy.files.root-dir"));
    assert!(root_exists, "The root node must exist in the remote tree");

    if let Some(file_node) = engine.store().get_remote_node(&RemoteId::new("file-1"))? {
        assert_eq!(file_node.name, "notes.txt");
        assert_eq!(file_node.node_type, NodeType::File);
    } else {
        panic!("File node 'file-1' should exist");
    }

    if let Some(dir_node) = engine.store().get_remote_node(&RemoteId::new("dir-1"))? {
        assert_eq!(dir_node.name, "photos");
        assert_eq!(dir_node.node_type, NodeType::Directory);
    } else {
        panic!("Directory node 'dir-1' should exist");
    }

    Ok(())
}

#[tokio::test]
async fn test_fetch_and_apply_remote_changes_handles_deletions() {
    use cozy_desktop::remote::client::CozyClient;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/files/_changes"))
        .and(query_param("include_docs", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "last_seq": "10-xyz",
            "results": [
                {
                    "id": "file-to-delete",
                    "seq": "10-xyz",
                    "deleted": true
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Pre-populate a remote node that will be deleted
    let existing = RemoteNode {
        id: RemoteId::new("file-to-delete"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "old.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-old".to_string(),
    };
    store.insert_remote_node(&existing).unwrap();
    store.flush().unwrap();

    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let client = CozyClient::new(&mock_server.uri(), "fake-token");

    // Confirm the node exists
    assert!(
        engine
            .store()
            .get_remote_node(&RemoteId::new("file-to-delete"))
            .unwrap()
            .is_some()
    );

    let last_seq = engine
        .fetch_and_apply_remote_changes(&client, Some("5-prev"))
        .await
        .unwrap();

    assert_eq!(last_seq, "10-xyz");

    // Node should be deleted from remote tree
    assert!(
        engine
            .store()
            .get_remote_node(&RemoteId::new("file-to-delete"))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_fetch_and_apply_remote_changes_skips_trash_dir() {
    use cozy_desktop::remote::client::CozyClient;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/files/_changes"))
        .and(query_param("include_docs", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "last_seq": "3-abc",
            "results": [
                {
                    "id": "io.cozy.files.trash-dir",
                    "seq": "1-aaa",
                    "doc": {
                        "_id": "io.cozy.files.trash-dir",
                        "_rev": "1-trash",
                        "type": "directory",
                        "name": ".cozy_trash",
                        "dir_id": "io.cozy.files.root-dir",
                        "size": null,
                        "updated_at": "2026-01-01T00:00:00Z"
                    }
                },
                {
                    "id": "file-1",
                    "seq": "2-bbb",
                    "doc": {
                        "_id": "file-1",
                        "_rev": "1-rev1",
                        "type": "file",
                        "name": "hello.txt",
                        "dir_id": "io.cozy.files.root-dir",
                        "md5sum": "d41d8cd98f00b204e9800998ecf8427e",
                        "size": 0,
                        "updated_at": "2026-01-01T00:00:00Z"
                    }
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();
    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let client = CozyClient::new(&mock_server.uri(), "fake-token");

    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();

    // The trash dir should NOT be in the remote tree
    assert!(
        engine
            .store()
            .get_remote_node(&RemoteId::new("io.cozy.files.trash-dir"))
            .unwrap()
            .is_none(),
        "Trash directory should not be synced"
    );

    // The regular file should still be present
    assert!(
        engine
            .store()
            .get_remote_node(&RemoteId::new("file-1"))
            .unwrap()
            .is_some(),
        "Regular files should still be synced"
    );
}

#[tokio::test]
async fn test_fetch_and_apply_remote_changes_treats_trashed_node_as_deletion() {
    use cozy_desktop::remote::client::CozyClient;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // A file that was moved to trash (its dir_id is now the trash dir)
    Mock::given(method("GET"))
        .and(path("/files/_changes"))
        .and(query_param("include_docs", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "last_seq": "10-xyz",
            "results": [
                {
                    "id": "file-trashed",
                    "seq": "10-xyz",
                    "doc": {
                        "_id": "file-trashed",
                        "_rev": "2-trashed",
                        "type": "file",
                        "name": "old.txt",
                        "dir_id": "io.cozy.files.trash-dir",
                        "md5sum": "d41d8cd98f00b204e9800998ecf8427e",
                        "size": 0,
                        "updated_at": "2026-01-01T00:00:00Z"
                    }
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Pre-populate the node (it existed before being trashed)
    let existing = RemoteNode {
        id: RemoteId::new("file-trashed"),
        parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
        name: "old.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: "1-old".to_string(),
    };
    store.insert_remote_node(&existing).unwrap();
    store.flush().unwrap();

    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    let client = CozyClient::new(&mock_server.uri(), "fake-token");

    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();

    // The trashed node should be removed from the remote tree
    assert!(
        engine
            .store()
            .get_remote_node(&RemoteId::new("file-trashed"))
            .unwrap()
            .is_none(),
        "Trashed file should be removed from the remote tree"
    );
}

#[test]
fn test_resolve_both_modified_renames_local_file() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    // Create a local file at sync_dir/report.txt with "local content"
    let local_path = sync_dir.path().join("report.txt");
    std::fs::write(&local_path, b"local modified content").unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Set up the synced record as if this file was previously synced
    let local_id = LocalFileId::new(1, 100);
    let remote_id = RemoteId::new("remote-report");

    let synced = SyncedRecord {
        local_id: local_id.clone(),
        remote_id: remote_id.clone(),
        rel_path: "report.txt".to_string(),
        md5sum: Some("old-md5".to_string()),
        size: Some(10),
        rev: "1-old".to_string(),
        node_type: NodeType::File,
        local_name: Some("report.txt".to_string()),
        local_parent_id: Some(LocalFileId::new(1, 1)),
        remote_name: Some("report.txt".to_string()),
        remote_parent_id: Some(RemoteId::new("io.cozy.files.root-dir")),
    };
    store.insert_synced(&synced).unwrap();

    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    // Simulate a BothModified conflict
    let conflict = cozy_desktop::model::Conflict {
        local_id: Some(local_id),
        remote_id: Some(remote_id),
        local_path: Some(local_path.clone()),
        reason: "Modified on both sides".to_string(),
        kind: ConflictKind::BothModified,
    };

    // Resolve the conflict
    let result = engine.resolve_conflict(&conflict);
    assert!(
        result.is_ok(),
        "resolve_conflict should succeed: {result:?}"
    );

    // The original file should still exist (unchanged for now)
    // A conflict copy should have been created
    let entries: Vec<_> = std::fs::read_dir(sync_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    let conflict_file = entries
        .iter()
        .find(|name| name.contains("-conflict-"))
        .expect("Should have created a conflict copy");

    assert!(
        conflict_file.starts_with("report-conflict-"),
        "Conflict file should start with 'report-conflict-': {conflict_file}"
    );
    assert!(
        conflict_file.ends_with(".txt"),
        "Conflict file should preserve extension: {conflict_file}"
    );

    // Conflict copy should contain the local content
    let conflict_path = sync_dir.path().join(conflict_file);
    let conflict_content = std::fs::read_to_string(&conflict_path).unwrap();
    assert_eq!(conflict_content, "local modified content");
}

#[test]
fn test_resolve_conflict_no_local_path_is_noop() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();
    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    // Conflict without local_path should be a no-op (just logged)
    let conflict = cozy_desktop::model::Conflict {
        local_id: None,
        remote_id: Some(RemoteId::new("r1")),
        local_path: None,
        reason: "test".to_string(),
        kind: ConflictKind::CycleDetected,
    };

    let result = engine.resolve_conflict(&conflict);
    assert!(result.is_ok());
}

#[test]
fn test_resolve_conflict_missing_local_file_is_noop() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();
    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    // Conflict pointing to a non-existent file should succeed (no-op)
    let conflict = cozy_desktop::model::Conflict {
        local_id: Some(LocalFileId::new(1, 100)),
        remote_id: Some(RemoteId::new("r1")),
        local_path: Some(sync_dir.path().join("nonexistent.txt")),
        reason: "Modified on both sides".to_string(),
        kind: ConflictKind::BothModified,
    };

    let result = engine.resolve_conflict(&conflict);
    assert!(result.is_ok());
}

#[test]
fn test_resolve_conflict_parent_missing_is_noop() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    // Create a local file inside a subdirectory
    let sub_dir = sync_dir.path().join("docs");
    std::fs::create_dir(&sub_dir).unwrap();
    let local_path = sub_dir.join("note.txt");
    std::fs::write(&local_path, "some content").unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();
    let engine = SyncEngine::new(
        store,
        sync_dir.path().to_path_buf(),
        staging_dir.path().to_path_buf(),
    );

    // ParentMissing conflict should NOT rename the file — it's a transient
    // condition that resolves on the next cycle when the parent is synced.
    let conflict = cozy_desktop::model::Conflict {
        local_id: Some(LocalFileId::new(1, 200)),
        remote_id: None,
        local_path: Some(local_path.clone()),
        reason: "Parent directory not synced".to_string(),
        kind: ConflictKind::ParentMissing,
    };

    let result = engine.resolve_conflict(&conflict);
    assert!(result.is_ok());

    // The original file should still exist at its original path
    assert!(
        local_path.exists(),
        "ParentMissing conflict should not rename the file"
    );

    // No conflict copy should have been created
    let entries: Vec<_> = std::fs::read_dir(&sub_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !entries.iter().any(|name| name.contains("-conflict-")),
        "ParentMissing should not create conflict copies, found: {entries:?}"
    );
}
