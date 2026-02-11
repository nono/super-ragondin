use cozy_desktop::model::{LocalFileId, LocalNode, NodeType, RemoteId, RemoteNode};
use cozy_desktop::simulator::mock_fs::MockFs;
use cozy_desktop::simulator::mock_remote::MockRemote;
use cozy_desktop::simulator::runner::{SimAction, SimulationRunner};
use cozy_desktop::store::TreeStore;
use proptest::prelude::*;
use tempfile::tempdir;

#[test]
fn mock_fs_create_and_read_file() {
    let mut fs = MockFs::new();

    let id = LocalFileId::new(1, 100);
    let node = LocalNode {
        id: id.clone(),
        parent_id: None,
        name: "test.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        mtime: 1000,
    };
    let content = b"hello world".to_vec();

    fs.create_file(id.clone(), node.clone(), content.clone());

    assert!(fs.exists(&id));
    assert_eq!(fs.read_file(&id), Some(&content));
    assert_eq!(
        fs.get_node(&id).map(|n| &n.name),
        Some(&"test.txt".to_string())
    );
}

#[test]
fn mock_fs_delete_removes_file() {
    let mut fs = MockFs::new();

    let id = LocalFileId::new(1, 100);
    let node = LocalNode {
        id: id.clone(),
        parent_id: None,
        name: "test.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        mtime: 1000,
    };

    fs.create_file(id.clone(), node, b"content".to_vec());
    assert!(fs.exists(&id));

    fs.delete(&id);
    assert!(!fs.exists(&id));
    assert!(fs.read_file(&id).is_none());
}

#[test]
fn mock_remote_add_and_get_node() {
    let mut remote = MockRemote::new();

    let id = RemoteId::new("remote-123");
    let node = RemoteNode {
        id: id.clone(),
        parent_id: None,
        name: "test.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc123".to_string()),
        size: Some(100),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    };
    let content = b"hello world".to_vec();

    remote.add_node(node.clone(), Some(content.clone()));

    assert!(remote.get_node(&id).is_some());
    assert_eq!(remote.get_content(&id), Some(&content));
    assert_eq!(remote.current_seq(), 1);
}

#[test]
fn mock_remote_delete_node() {
    let mut remote = MockRemote::new();

    let id = RemoteId::new("remote-123");
    let node = RemoteNode {
        id: id.clone(),
        parent_id: None,
        name: "test.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc123".to_string()),
        size: Some(100),
        updated_at: 1000,
        rev: "1-abc".to_string(),
    };

    remote.add_node(node, Some(b"content".to_vec()));
    assert!(remote.get_node(&id).is_some());

    remote.delete_node(&id);
    assert!(remote.get_node(&id).is_none());
    assert_eq!(remote.current_seq(), 2);
}

#[test]
fn mock_remote_changes_since() {
    let mut remote = MockRemote::new();

    let id1 = RemoteId::new("file-1");
    let node1 = RemoteNode {
        id: id1.clone(),
        parent_id: None,
        name: "first.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-a".to_string(),
    };

    let id2 = RemoteId::new("file-2");
    let node2 = RemoteNode {
        id: id2.clone(),
        parent_id: None,
        name: "second.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-b".to_string(),
    };

    remote.add_node(node1, None);
    let after_first = remote.current_seq();
    remote.add_node(node2, None);

    let changes = remote.get_changes_since(after_first);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].1.id, id2);
}

// ==================== SimulationRunner Tests ====================

#[test]
fn simulation_runner_remote_create_then_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root directory on remote
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    // Create a file on remote
    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id,
            name: "test.txt".to_string(),
            content: b"hello world".to_vec(),
        })
        .unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // File should now exist locally (plus root dir)
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .collect();
    assert_eq!(local_files.len(), 1);
    assert_eq!(local_files[0].name, "test.txt");
}

#[test]
fn simulation_runner_local_create_then_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root directory on remote (needed for parent reference)
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    // Sync to get root
    runner.apply(SimAction::Sync).unwrap();

    // Get the local ID for root
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Create a file locally
    let file_local_id = LocalFileId::new(1, 9999);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "local.txt".to_string(),
            content: b"local content".to_vec(),
        })
        .unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // File should now exist on remote
    let remote_files: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "local.txt")
        .collect();
    assert_eq!(remote_files.len(), 1);
}

#[test]
fn simulation_runner_bidirectional_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root on remote
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    // Create a file on remote
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("remote-file"),
            parent_id: root_id.clone(),
            name: "remote.txt".to_string(),
            content: b"remote content".to_vec(),
        })
        .unwrap();

    // Sync to get root and remote file locally
    runner.apply(SimAction::Sync).unwrap();

    // Get root local ID
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Create a different file locally
    let local_file_id = LocalFileId::new(1, 8888);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: local_file_id,
            parent_local_id: Some(root_local_id),
            name: "local.txt".to_string(),
            content: b"local content".to_vec(),
        })
        .unwrap();

    // Sync again
    runner.apply(SimAction::Sync).unwrap();

    // Check convergence - both files should exist on both sides
    runner.check_convergence().unwrap();

    // Verify we have 2 files locally
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .collect();
    assert_eq!(local_files.len(), 2);

    // Verify we have 2 files remotely
    let remote_files: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| !n.name.is_empty())
        .collect();
    assert_eq!(remote_files.len(), 2);
}

#[test]
fn simulation_runner_remote_delete_propagates() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root on remote
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    // Create a file on remote
    let file_id = RemoteId::new("file-to-delete");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id,
            name: "deleteme.txt".to_string(),
            content: b"will be deleted".to_vec(),
        })
        .unwrap();

    // Sync - file should appear locally
    runner.apply(SimAction::Sync).unwrap();

    // Verify file exists locally
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "deleteme.txt")
        .collect();
    assert_eq!(local_files.len(), 1);

    // Delete the file on remote
    runner
        .apply(SimAction::RemoteDeleteFile { id: file_id })
        .unwrap();

    // Sync again - deletion should propagate
    runner.apply(SimAction::Sync).unwrap();

    // Verify file no longer exists locally
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "deleteme.txt")
        .collect();
    assert_eq!(local_files.len(), 0);
}

// ==================== Convergence Path Tests ====================

#[test]
fn check_convergence_verifies_file_paths() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: "hello.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Now rename locally only (without going through sync) to create a path mismatch
    let file_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "hello.txt")
        .map(|n| n.id.clone())
        .unwrap();

    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    runner.local_fs.move_node(
        &file_local_id,
        Some(root_local_id),
        "renamed.txt".to_string(),
    );

    // MD5 still matches, but paths differ — convergence should fail
    let result = runner.check_convergence();
    assert!(result.is_err(), "Convergence should fail when paths differ");
    let err = result.unwrap_err();
    assert!(
        err.contains("renamed.txt") && err.contains("hello.txt"),
        "Error should show differing paths: {err}"
    );
}

// ==================== Stop/Restart Tests ====================

#[test]
fn simulation_stop_prevents_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: "test.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    // Stop the client
    runner.apply(SimAction::StopClient).unwrap();

    // Sync should be a no-op while stopped
    runner.apply(SimAction::Sync).unwrap();

    // File should NOT have been synced locally
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .collect();
    assert_eq!(local_files.len(), 0, "Sync should be no-op while stopped");
}

#[test]
fn simulation_local_changes_while_stopped_skip_store() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Stop the client
    runner.apply(SimAction::StopClient).unwrap();

    // Create a file locally while stopped
    let file_local_id = LocalFileId::new(1, 8888);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "offline.txt".to_string(),
            content: b"created while stopped".to_vec(),
        })
        .unwrap();

    // File should exist in MockFs
    assert!(
        runner.local_fs.exists(&file_local_id),
        "File should be in MockFs"
    );

    // But NOT in the store's local tree
    let store_node = runner.store.get_local_node(&file_local_id);
    assert!(
        store_node.is_err() || store_node.unwrap().is_none(),
        "File should NOT be in the store while stopped"
    );
}

#[test]
fn simulation_restart_reconciles_local_changes() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Stop the client
    runner.apply(SimAction::StopClient).unwrap();

    // Create a file locally while stopped
    let file_local_id = LocalFileId::new(1, 7777);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "offline.txt".to_string(),
            content: b"created while stopped".to_vec(),
        })
        .unwrap();

    // Restart the client — should reconcile MockFs into the store
    runner.apply(SimAction::RestartClient).unwrap();

    // Now the file should be in the store's local tree
    let store_node = runner.store.get_local_node(&file_local_id).unwrap();
    assert!(
        store_node.is_some(),
        "File should be in the store after restart"
    );
    assert_eq!(store_node.unwrap().name, "offline.txt");
}

#[test]
fn simulation_stop_restart_full_cycle_converges() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Stop the client
    runner.apply(SimAction::StopClient).unwrap();

    // Create a local file while stopped
    let file_local_id = LocalFileId::new(1, 6666);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "local_offline.txt".to_string(),
            content: b"local offline".to_vec(),
        })
        .unwrap();

    // Create a remote file while stopped
    let remote_file_id = RemoteId::new("remote-offline-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: remote_file_id.clone(),
            parent_id: root_id.clone(),
            name: "remote_offline.txt".to_string(),
            content: b"remote offline".to_vec(),
        })
        .unwrap();

    // Restart the client
    runner.apply(SimAction::RestartClient).unwrap();

    // Now sync — both files should converge
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
}

#[test]
fn simulation_local_delete_while_stopped_reconciled_on_restart() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: "existing.txt".to_string(),
            content: b"exists".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Find the local id of the synced file
    let file_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "existing.txt")
        .map(|n| n.id.clone())
        .unwrap();

    // Stop the client
    runner.apply(SimAction::StopClient).unwrap();

    // Delete the file locally while stopped
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: file_local_id.clone(),
        })
        .unwrap();

    // File gone from MockFs but still in store
    assert!(!runner.local_fs.exists(&file_local_id));
    assert!(
        runner
            .store
            .get_local_node(&file_local_id)
            .unwrap()
            .is_some()
    );

    // Restart — should remove from store
    runner.apply(SimAction::RestartClient).unwrap();

    assert!(
        runner
            .store
            .get_local_node(&file_local_id)
            .unwrap()
            .is_none()
    );
}

// ==================== Idempotency Tests ====================

#[test]
fn sync_is_idempotent_after_remote_create() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id,
            parent_id: root_id,
            name: "hello.txt".to_string(),
            content: b"world".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn sync_is_idempotent_after_local_create() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    let file_local_id = LocalFileId::new(1, 11111);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id,
            parent_local_id: Some(root_local_id),
            name: "local.txt".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Property-Based Tests ====================

fn arbitrary_file_name() -> impl Strategy<Value = String> {
    "[a-z]{1,8}\\.[a-z]{2,3}".prop_map(|s| s)
}

fn arbitrary_content() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 1..100)
}

fn arbitrary_remote_id() -> impl Strategy<Value = RemoteId> {
    "[a-f0-9]{8}-[a-f0-9]{4}".prop_map(RemoteId::new)
}

#[test]
fn simulation_runner_remote_rename_then_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: "old.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "old.txt")
        .collect();
    assert_eq!(local_files.len(), 1);

    runner
        .apply(SimAction::RemoteMove {
            id: file_id.clone(),
            new_parent_id: root_id.clone(),
            new_name: "new.txt".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let old_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "old.txt")
        .collect();
    assert_eq!(old_files.len(), 0, "Old name should be gone");

    let new_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "new.txt")
        .collect();
    assert_eq!(new_files.len(), 1, "New name should exist");

    runner.check_convergence().unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn prop_remote_create_then_sync_converges(
        file_id in arbitrary_remote_id(),
        name in arbitrary_file_name(),
        content in arbitrary_content()
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

        // Create root on remote
        let root_id = RemoteId::new("root");
        runner.apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        }).unwrap();

        // Create file on remote
        runner.apply(SimAction::RemoteCreateFile {
            id: file_id,
            parent_id: root_id,
            name,
            content,
        }).unwrap();

        // Sync
        runner.apply(SimAction::Sync).unwrap();

        // Check convergence and idempotency
        runner.check_convergence().unwrap();
        runner.check_idempotency().unwrap();
    }

    #[test]
    fn prop_local_create_then_sync_converges(
        name in arbitrary_file_name(),
        content in arbitrary_content()
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

        // Create root on remote and sync
        let root_id = RemoteId::new("root");
        runner.apply(SimAction::RemoteCreateDir {
            id: root_id,
            parent_id: None,
            name: String::new(),
        }).unwrap();
        runner.apply(SimAction::Sync).unwrap();

        // Get root local ID
        let root_local_id = runner.local_fs.list_all()
            .into_iter()
            .find(|n| n.name.is_empty())
            .map(|n| n.id.clone())
            .unwrap();

        // Create file locally
        let file_local_id = LocalFileId::new(1, 99999);
        runner.apply(SimAction::LocalCreateFile {
            local_id: file_local_id,
            parent_local_id: Some(root_local_id),
            name,
            content,
        }).unwrap();

        // Sync
        runner.apply(SimAction::Sync).unwrap();

        // Check convergence and idempotency
        runner.check_convergence().unwrap();
        runner.check_idempotency().unwrap();
    }

    #[test]
    fn prop_remote_rename_then_sync_converges(
        original_name in arbitrary_file_name(),
        new_name in arbitrary_file_name(),
        content in arbitrary_content()
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

        let root_id = RemoteId::new("root");
        runner.apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        }).unwrap();

        let file_id = RemoteId::new("file-1");
        runner.apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: original_name,
            content,
        }).unwrap();

        runner.apply(SimAction::Sync).unwrap();

        runner.apply(SimAction::RemoteMove {
            id: file_id,
            new_parent_id: root_id,
            new_name,
        }).unwrap();

        runner.apply(SimAction::Sync).unwrap();

        runner.check_convergence().unwrap();
    }

    #[test]
    fn prop_multiple_files_converge(
        files in prop::collection::vec(
            (arbitrary_file_name(), arbitrary_content()),
            1..5
        )
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

        // Create root
        let root_id = RemoteId::new("root");
        runner.apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        }).unwrap();

        // Create multiple files on remote
        for (i, (name, content)) in files.into_iter().enumerate() {
            let file_id = RemoteId::new(format!("file-{i}"));
            runner.apply(SimAction::RemoteCreateFile {
                id: file_id,
                parent_id: root_id.clone(),
                name,
                content,
            }).unwrap();
        }

        // Sync
        runner.apply(SimAction::Sync).unwrap();

        // Check convergence and idempotency
        runner.check_convergence().unwrap();
        runner.check_idempotency().unwrap();
    }

    #[test]
    fn prop_arbitrary_action_sequence_converges(
        seed in prop::collection::vec(any::<u8>(), 50..200)
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

        // Setup: create root on remote and sync it
        let root_id = RemoteId::new("root");
        runner.apply(SimAction::RemoteCreateDir {
            id: root_id,
            parent_id: None,
            name: String::new(),
        }).unwrap();
        runner.apply(SimAction::Sync).unwrap();

        // Get the root local ID assigned by sync
        let root_local_id = runner.local_fs.list_all()
            .into_iter()
            .find(|n| n.name.is_empty())
            .map(|n| n.id.clone())
            .unwrap();

        let actions = generate_valid_action_sequence(&seed, 20, root_local_id);

        // Apply the generated action sequence
        for action in &actions {
            runner.apply(action.clone()).unwrap();
        }

        // Ensure client is running for final sync
        runner.apply(SimAction::RestartClient).unwrap();
        runner.apply(SimAction::Sync).unwrap();

        // After sync, local and remote must converge and be idempotent
        runner.check_convergence().unwrap();
        runner.check_idempotency().unwrap();
    }
}

#[test]
fn check_convergence_detects_directory_mismatch() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root on remote
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    // Sync to establish root
    runner.apply(SimAction::Sync).unwrap();

    // Now add a directory only on remote (without syncing)
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("orphan-dir"),
            parent_id: Some(root_id),
            name: "photos".to_string(),
        })
        .unwrap();

    // check_convergence should detect the directory mismatch
    let result = runner.check_convergence();
    assert!(
        result.is_err(),
        "Expected convergence error for directory mismatch"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("photos"),
        "Error should mention the mismatched directory name, got: {err}"
    );
}

#[derive(Debug, Clone)]
struct SimState {
    remote_file_ids: Vec<RemoteId>,
    remote_dir_ids: Vec<RemoteId>,
    local_file_ids: Vec<LocalFileId>,
    local_dir_ids: Vec<LocalFileId>,
    next_remote_counter: usize,
    next_local_inode: u64,
    stopped: bool,
}

impl SimState {
    fn new(root_local_id: LocalFileId) -> Self {
        Self {
            remote_file_ids: Vec::new(),
            remote_dir_ids: vec![RemoteId::new("root")],
            local_file_ids: Vec::new(),
            local_dir_ids: vec![root_local_id],
            next_remote_counter: 0,
            next_local_inode: 50_000,
            stopped: false,
        }
    }

    fn next_remote_id(&mut self) -> RemoteId {
        let id = RemoteId::new(format!("gen-remote-{}", self.next_remote_counter));
        self.next_remote_counter += 1;
        id
    }

    fn next_local_file_id(&mut self) -> LocalFileId {
        let id = LocalFileId::new(1, self.next_local_inode);
        self.next_local_inode += 1;
        id
    }
}

fn pick<T: Clone>(items: &[T], byte: u8) -> T {
    items[byte as usize % items.len()].clone()
}

fn name_from_bytes(b1: u8, b2: u8) -> String {
    let len = (b1 % 6) as usize + 1;
    let chars: String = (0..len)
        .map(|i| {
            let v = b2.wrapping_add(i as u8) % 26;
            (b'a' + v) as char
        })
        .collect();
    format!("{chars}.txt")
}

fn generate_valid_action_sequence(
    seed: &[u8],
    max_actions: usize,
    root_local_id: LocalFileId,
) -> Vec<SimAction> {
    let mut state = SimState::new(root_local_id);
    let mut actions = Vec::new();
    let mut cursor = 0;

    let read = |cursor: &mut usize| -> u8 {
        if *cursor < seed.len() {
            let v = seed[*cursor];
            *cursor += 1;
            v
        } else {
            0
        }
    };

    for _ in 0..max_actions {
        if cursor >= seed.len() {
            break;
        }

        let action_type = read(&mut cursor);

        let has_remote_files = !state.remote_file_ids.is_empty();
        let has_local_files = !state.local_file_ids.is_empty();
        let has_remote_dirs = !state.remote_dir_ids.is_empty();

        let action = match action_type % 10 {
            0 if has_remote_dirs => {
                let id = state.next_remote_id();
                let parent = pick(&state.remote_dir_ids, read(&mut cursor));
                let name = name_from_bytes(read(&mut cursor), read(&mut cursor));
                let content_len = (read(&mut cursor) % 20) as usize + 1;
                let content: Vec<u8> = (0..content_len).map(|_| read(&mut cursor)).collect();
                state.remote_file_ids.push(id.clone());
                SimAction::RemoteCreateFile {
                    id,
                    parent_id: parent,
                    name,
                    content,
                }
            }
            1 if has_remote_files => {
                let id = pick(&state.remote_file_ids, read(&mut cursor));
                state.remote_file_ids.retain(|x| x != &id);
                SimAction::RemoteDeleteFile { id }
            }
            2 if has_remote_files => {
                let id = pick(&state.remote_file_ids, read(&mut cursor));
                let content_len = (read(&mut cursor) % 20) as usize + 1;
                let content: Vec<u8> = (0..content_len).map(|_| read(&mut cursor)).collect();
                SimAction::RemoteModifyFile { id, content }
            }
            3 if has_remote_files && has_remote_dirs => {
                let id = pick(&state.remote_file_ids, read(&mut cursor));
                let new_parent = pick(&state.remote_dir_ids, read(&mut cursor));
                let new_name = name_from_bytes(read(&mut cursor), read(&mut cursor));
                SimAction::RemoteMove {
                    id,
                    new_parent_id: new_parent,
                    new_name,
                }
            }
            4 => {
                let local_id = state.next_local_file_id();
                let parent = if state.local_dir_ids.is_empty() {
                    None
                } else {
                    Some(pick(&state.local_dir_ids, read(&mut cursor)))
                };
                let name = name_from_bytes(read(&mut cursor), read(&mut cursor));
                let content_len = (read(&mut cursor) % 20) as usize + 1;
                let content: Vec<u8> = (0..content_len).map(|_| read(&mut cursor)).collect();
                state.local_file_ids.push(local_id.clone());
                SimAction::LocalCreateFile {
                    local_id,
                    parent_local_id: parent,
                    name,
                    content,
                }
            }
            5 if has_local_files => {
                let id = pick(&state.local_file_ids, read(&mut cursor));
                state.local_file_ids.retain(|x| x != &id);
                SimAction::LocalDeleteFile { local_id: id }
            }
            6 if has_local_files => {
                let id = pick(&state.local_file_ids, read(&mut cursor));
                let content_len = (read(&mut cursor) % 20) as usize + 1;
                let content: Vec<u8> = (0..content_len).map(|_| read(&mut cursor)).collect();
                SimAction::LocalModifyFile {
                    local_id: id,
                    content,
                }
            }
            7 => SimAction::Sync,
            8 if !state.stopped => {
                state.stopped = true;
                SimAction::StopClient
            }
            9 if state.stopped => {
                state.stopped = false;
                SimAction::RestartClient
            }
            _ => SimAction::Sync,
        };

        actions.push(action);
    }

    actions
}
