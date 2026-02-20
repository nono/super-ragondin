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
fn simulation_runner_nested_local_dirs_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root on remote and sync
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id,
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

    // Create nested dirs locally: root -> photos -> vacation
    let photos_id = LocalFileId::new(1, 20_000);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: photos_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "photos".to_string(),
        })
        .unwrap();

    let vacation_id = LocalFileId::new(1, 20_001);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: vacation_id,
            parent_local_id: Some(photos_id),
            name: "vacation".to_string(),
        })
        .unwrap();

    // Sync should upload both dirs to remote
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();
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

#[test]
fn simulation_runner_local_move_dir_then_sync() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root on remote and sync
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id,
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

    // Create two dirs locally: root -> docs, root -> archive
    let docs_id = LocalFileId::new(1, 30_000);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: docs_id.clone(),
            parent_local_id: Some(root_local_id.clone()),
            name: "docs".to_string(),
        })
        .unwrap();

    let archive_id = LocalFileId::new(1, 30_001);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: archive_id.clone(),
            parent_local_id: Some(root_local_id.clone()),
            name: "archive".to_string(),
        })
        .unwrap();

    // Create a file inside docs
    let file_id = LocalFileId::new(1, 30_002);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_id,
            parent_local_id: Some(docs_id.clone()),
            name: "readme.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    // Sync everything
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move docs dir into archive (rename to "old-docs")
    runner
        .apply(SimAction::LocalMove {
            local_id: docs_id,
            new_parent_local_id: Some(archive_id),
            new_name: "old-docs".to_string(),
        })
        .unwrap();

    // Sync the move
    runner.apply(SimAction::Sync).unwrap();

    // Remote should reflect the moved directory
    let remote_dirs: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "old-docs")
        .collect();
    assert_eq!(remote_dirs.len(), 1, "Moved dir should exist on remote");

    let old_dirs: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "docs")
        .collect();
    assert_eq!(old_dirs.len(), 0, "Old dir name should be gone on remote");

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
        choices in prop::collection::vec(arbitrary_action_choice(), 5..50)
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

        let mut sim_state = SimState::new(root_local_id);
        let actions = resolve_action_choices(&choices, &mut sim_state);

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
        runner.check_store_consistency().unwrap();
    }

    #[test]
    fn prop_multi_sync_round_converges(
        rounds in prop::collection::vec(
            prop::collection::vec(arbitrary_action_choice(), 2..15),
            3..8
        )
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

        // Run multiple rounds: resolve actions, apply them, then sync
        let mut sim_state = SimState::new(root_local_id);
        for round_choices in &rounds {
            let actions = resolve_action_choices(round_choices, &mut sim_state);

            for action in &actions {
                runner.apply(action.clone()).unwrap();
            }

            // Ensure client is running, then sync at the end of each round
            runner.apply(SimAction::RestartClient).unwrap();
            runner.apply(SimAction::Sync).unwrap();

            // Check convergence after every round to catch state accumulation bugs
            runner.check_convergence().unwrap();
        }

        // Final invariant checks
        runner.check_idempotency().unwrap();
        runner.check_store_consistency().unwrap();
    }
}

// ==================== Regression: directory delete must cascade ====================

#[test]
fn local_delete_dir_removes_children() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    let root_id = RemoteId::new("root");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id,
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

    // Create a local dir, then a file inside it
    let dir_id = LocalFileId::new(1, 50000);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "subdir".to_string(),
        })
        .unwrap();

    let file_id = LocalFileId::new(1, 50001);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_id.clone(),
            parent_local_id: Some(dir_id.clone()),
            name: "child.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    assert!(runner.local_fs.exists(&dir_id));
    assert!(runner.local_fs.exists(&file_id));

    // Delete the directory — child should also be removed
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: dir_id.clone(),
        })
        .unwrap();

    assert!(!runner.local_fs.exists(&dir_id));
    assert!(
        !runner.local_fs.exists(&file_id),
        "Child file should be removed when parent dir is deleted"
    );
}

#[test]
fn remote_delete_dir_removes_children() {
    let mut remote = MockRemote::new();

    let root_id = RemoteId::new("root");
    let root_node = RemoteNode {
        id: root_id.clone(),
        parent_id: None,
        name: String::new(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-root".to_string(),
    };
    remote.add_node(root_node, None);

    let dir_id = RemoteId::new("dir-1");
    let dir_node = RemoteNode {
        id: dir_id.clone(),
        parent_id: Some(root_id),
        name: "subdir".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-dir".to_string(),
    };
    remote.add_node(dir_node, None);

    let file_id = RemoteId::new("file-1");
    let file_node = RemoteNode {
        id: file_id.clone(),
        parent_id: Some(dir_id.clone()),
        name: "child.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc".to_string()),
        size: Some(5),
        updated_at: 1000,
        rev: "1-file".to_string(),
    };
    remote.add_node(file_node, Some(b"hello".to_vec()));

    assert!(remote.get_node(&dir_id).is_some());
    assert!(remote.get_node(&file_id).is_some());

    // Delete the directory — child should also be removed
    remote.delete_node(&dir_id);

    assert!(remote.get_node(&dir_id).is_none());
    assert!(
        remote.get_node(&file_id).is_none(),
        "Child file should be removed when parent dir is deleted"
    );
}

// ==================== Store Consistency Tests ====================

#[test]
fn check_store_consistency_passes_after_sync() {
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
            name: "test.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_store_consistency().unwrap();
}

#[test]
fn check_store_consistency_detects_orphaned_synced_record() {
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
            parent_id: root_id,
            name: "test.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Delete the remote node from the store directly, leaving an orphaned synced record
    runner.store.delete_remote_node(&file_id).unwrap();

    let result = runner.check_store_consistency();
    assert!(result.is_err(), "Expected error for orphaned synced record");
    let err = result.unwrap_err();
    assert!(
        err.contains("remote node missing"),
        "Error should mention missing remote node, got: {err}"
    );
}

#[test]
fn check_store_consistency_detects_missing_local_node() {
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
            name: "test.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Find the local id of the synced file and delete it from the store
    let synced_records = runner.store.list_all_synced().unwrap();
    let file_synced = synced_records
        .iter()
        .find(|s| s.rel_path == "test.txt")
        .unwrap();
    runner
        .store
        .delete_local_node(&file_synced.local_id)
        .unwrap();

    let result = runner.check_store_consistency();
    assert!(result.is_err(), "Expected error for missing local node");
    let err = result.unwrap_err();
    assert!(
        err.contains("local node missing"),
        "Error should mention missing local node, got: {err}"
    );
}

#[test]
fn check_store_consistency_detects_remote_to_local_mismatch() {
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
            parent_id: root_id,
            name: "test.txt".to_string(),
            content: b"hello".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Corrupt remote_to_local by inserting a wrong mapping
    runner
        .remote_to_local
        .insert(file_id, LocalFileId::new(999, 999));

    let result = runner.check_store_consistency();
    assert!(
        result.is_err(),
        "Expected error for remote_to_local mismatch"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("remote_to_local"),
        "Error should mention remote_to_local mismatch, got: {err}"
    );
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

// ==================== Regression: deeply nested local dirs must sync ====================

#[test]
fn nested_local_dirs_with_file_converge() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Setup: create root on remote and sync
    let root_id = RemoteId::new("root");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id,
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

    // Create: root -> dir_a ("n.txt") -> dir_b ("uv.txt") -> file ("lmnop.txt")
    let dir_a = LocalFileId::new(1, 60000);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_a.clone(),
            parent_local_id: Some(root_local_id),
            name: "n.txt".to_string(),
        })
        .unwrap();

    let dir_b = LocalFileId::new(1, 60001);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_b.clone(),
            parent_local_id: Some(dir_a),
            name: "uv.txt".to_string(),
        })
        .unwrap();

    let file_c = LocalFileId::new(1, 60002);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_c,
            parent_local_id: Some(dir_b),
            name: "lmnop.txt".to_string(),
            content: b"some content here".to_vec(),
        })
        .unwrap();

    // Sync should upload the entire tree
    runner.apply(SimAction::Sync).unwrap();

    // After sync, local and remote must converge
    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn nested_local_dirs_created_while_stopped_converge() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Setup: create root on remote and sync
    let root_id = RemoteId::new("root");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id,
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

    // Create nested dirs while stopped
    let dir_a = LocalFileId::new(1, 60000);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_a.clone(),
            parent_local_id: Some(root_local_id),
            name: "n.txt".to_string(),
        })
        .unwrap();

    let dir_b = LocalFileId::new(1, 60001);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_b.clone(),
            parent_local_id: Some(dir_a),
            name: "uv.txt".to_string(),
        })
        .unwrap();

    let file_c = LocalFileId::new(1, 60002);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_c,
            parent_local_id: Some(dir_b),
            name: "lmnop.txt".to_string(),
            content: b"some content here".to_vec(),
        })
        .unwrap();

    // Restart and sync
    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Regression: file moved out of directory before directory deleted ====================

#[test]
fn file_moved_out_of_dir_then_dir_deleted_converges() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Setup: root dir synced
    let root_id = RemoteId::new("root");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id,
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

    // Create a dir with a file, sync them
    let dir_a = LocalFileId::new(1, 70000);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_a.clone(),
            parent_local_id: Some(root_local_id.clone()),
            name: "parent-dir".to_string(),
        })
        .unwrap();

    let file_a = LocalFileId::new(1, 70001);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_a.clone(),
            parent_local_id: Some(dir_a.clone()),
            name: "child.txt".to_string(),
            content: b"original".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Stop client, move file to a new dir, delete old dir, modify file
    runner.apply(SimAction::StopClient).unwrap();

    let dir_b = LocalFileId::new(1, 70002);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_b.clone(),
            parent_local_id: Some(root_local_id),
            name: "new-parent".to_string(),
        })
        .unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: file_a.clone(),
            new_parent_local_id: Some(dir_b),
            new_name: "moved.txt".to_string(),
        })
        .unwrap();

    runner
        .apply(SimAction::LocalDeleteFile { local_id: dir_a })
        .unwrap();

    runner
        .apply(SimAction::LocalModifyFile {
            local_id: file_a,
            content: b"modified content".to_vec(),
        })
        .unwrap();

    // Restart and sync: file should appear on remote under new-parent
    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[derive(Debug, Clone)]
struct SimState {
    remote_file_ids: Vec<RemoteId>,
    remote_dir_ids: Vec<RemoteId>,
    local_file_ids: Vec<LocalFileId>,
    local_dir_ids: Vec<LocalFileId>,
    /// Tracks parent for each remote file/dir (for cascade deletes)
    remote_parents: std::collections::HashMap<RemoteId, RemoteId>,
    /// Tracks parent for each local file/dir (for cascade deletes)
    local_parents: std::collections::HashMap<LocalFileId, LocalFileId>,
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
            remote_parents: std::collections::HashMap::new(),
            local_parents: std::collections::HashMap::new(),
            next_remote_counter: 0,
            next_local_inode: 50_000,
            stopped: false,
        }
    }

    /// Remove a remote directory and all its descendants from tracking
    fn remove_remote_tree(&mut self, dir_id: &RemoteId) {
        let children_files: Vec<RemoteId> = self
            .remote_file_ids
            .iter()
            .filter(|id| self.remote_parents.get(*id) == Some(dir_id))
            .cloned()
            .collect();
        let children_dirs: Vec<RemoteId> = self
            .remote_dir_ids
            .iter()
            .filter(|id| self.remote_parents.get(*id) == Some(dir_id))
            .cloned()
            .collect();
        for child in &children_dirs {
            self.remove_remote_tree(child);
        }
        for child in &children_files {
            self.remote_file_ids.retain(|x| x != child);
            self.remote_parents.remove(child);
        }
        for child in &children_dirs {
            self.remote_dir_ids.retain(|x| x != child);
            self.remote_parents.remove(child);
        }
        self.remote_dir_ids.retain(|x| x != dir_id);
        self.remote_parents.remove(dir_id);
    }

    /// Remove a local directory and all its descendants from tracking
    fn remove_local_tree(&mut self, dir_id: &LocalFileId) {
        let children_files: Vec<LocalFileId> = self
            .local_file_ids
            .iter()
            .filter(|id| self.local_parents.get(*id) == Some(dir_id))
            .cloned()
            .collect();
        let children_dirs: Vec<LocalFileId> = self
            .local_dir_ids
            .iter()
            .filter(|id| self.local_parents.get(*id) == Some(dir_id))
            .cloned()
            .collect();
        for child in &children_dirs {
            self.remove_local_tree(child);
        }
        for child in &children_files {
            self.local_file_ids.retain(|x| x != child);
            self.local_parents.remove(child);
        }
        for child in &children_dirs {
            self.local_dir_ids.retain(|x| x != child);
            self.local_parents.remove(child);
        }
        self.local_dir_ids.retain(|x| x != dir_id);
        self.local_parents.remove(dir_id);
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

#[derive(Debug, Clone)]
enum ActionChoice {
    RemoteCreateFile {
        parent_idx: usize,
        name: String,
        content: Vec<u8>,
    },
    RemoteDeleteFile {
        idx: usize,
    },
    RemoteModifyFile {
        idx: usize,
        content: Vec<u8>,
    },
    RemoteMoveFile {
        idx: usize,
        parent_idx: usize,
        new_name: String,
    },
    LocalCreateFile {
        parent_idx: usize,
        name: String,
        content: Vec<u8>,
    },
    LocalDeleteFile {
        idx: usize,
    },
    LocalModifyFile {
        idx: usize,
        content: Vec<u8>,
    },
    Sync,
    StopClient,
    RestartClient,
    RemoteCreateDir {
        parent_idx: usize,
        name: String,
    },
    LocalCreateDir {
        parent_idx: usize,
        name: String,
    },
    LocalMoveFile {
        idx: usize,
        parent_idx: usize,
        new_name: String,
    },
    RemoteMoveDir {
        idx: usize,
        new_name: String,
    },
    LocalMoveDir {
        idx: usize,
        new_name: String,
    },
    LocalDeleteDir {
        idx: usize,
    },
    RemoteDeleteDir {
        idx: usize,
    },
}

fn arbitrary_action_choice() -> impl Strategy<Value = ActionChoice> {
    prop_oneof![
        (any::<usize>(), arbitrary_file_name(), arbitrary_content()).prop_map(
            |(parent_idx, name, content)| ActionChoice::RemoteCreateFile {
                parent_idx,
                name,
                content,
            }
        ),
        any::<usize>().prop_map(|idx| ActionChoice::RemoteDeleteFile { idx }),
        (any::<usize>(), arbitrary_content())
            .prop_map(|(idx, content)| ActionChoice::RemoteModifyFile { idx, content }),
        (any::<usize>(), any::<usize>(), arbitrary_file_name()).prop_map(
            |(idx, parent_idx, new_name)| ActionChoice::RemoteMoveFile {
                idx,
                parent_idx,
                new_name,
            }
        ),
        (any::<usize>(), arbitrary_file_name(), arbitrary_content()).prop_map(
            |(parent_idx, name, content)| ActionChoice::LocalCreateFile {
                parent_idx,
                name,
                content,
            }
        ),
        any::<usize>().prop_map(|idx| ActionChoice::LocalDeleteFile { idx }),
        (any::<usize>(), arbitrary_content())
            .prop_map(|(idx, content)| ActionChoice::LocalModifyFile { idx, content }),
        Just(ActionChoice::Sync),
        Just(ActionChoice::StopClient),
        Just(ActionChoice::RestartClient),
        (any::<usize>(), arbitrary_file_name())
            .prop_map(|(parent_idx, name)| ActionChoice::RemoteCreateDir { parent_idx, name }),
        (any::<usize>(), arbitrary_file_name())
            .prop_map(|(parent_idx, name)| ActionChoice::LocalCreateDir { parent_idx, name }),
        (any::<usize>(), any::<usize>(), arbitrary_file_name()).prop_map(
            |(idx, parent_idx, new_name)| ActionChoice::LocalMoveFile {
                idx,
                parent_idx,
                new_name,
            }
        ),
        (any::<usize>(), arbitrary_file_name())
            .prop_map(|(idx, new_name)| ActionChoice::RemoteMoveDir { idx, new_name }),
        (any::<usize>(), arbitrary_file_name())
            .prop_map(|(idx, new_name)| ActionChoice::LocalMoveDir { idx, new_name }),
        any::<usize>().prop_map(|idx| ActionChoice::LocalDeleteDir { idx }),
        any::<usize>().prop_map(|idx| ActionChoice::RemoteDeleteDir { idx }),
    ]
}

fn resolve_action_choices(choices: &[ActionChoice], state: &mut SimState) -> Vec<SimAction> {
    choices
        .iter()
        .map(|c| resolve_single_action(c, state))
        .collect()
}

fn resolve_single_action(choice: &ActionChoice, state: &mut SimState) -> SimAction {
    match choice {
        ActionChoice::RemoteCreateFile {
            parent_idx,
            name,
            content,
        } => {
            if state.remote_dir_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.next_remote_id();
            let parent = state.remote_dir_ids[parent_idx % state.remote_dir_ids.len()].clone();
            state.remote_file_ids.push(id.clone());
            state.remote_parents.insert(id.clone(), parent.clone());
            SimAction::RemoteCreateFile {
                id,
                parent_id: parent,
                name: name.clone(),
                content: content.clone(),
            }
        }
        ActionChoice::RemoteDeleteFile { idx } => {
            if state.remote_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            state.remote_file_ids.retain(|x| x != &id);
            SimAction::RemoteDeleteFile { id }
        }
        ActionChoice::RemoteModifyFile { idx, content } => {
            if state.remote_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            SimAction::RemoteModifyFile {
                id,
                content: content.clone(),
            }
        }
        ActionChoice::RemoteMoveFile {
            idx,
            parent_idx,
            new_name,
        } => {
            if state.remote_file_ids.is_empty() || state.remote_dir_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            let new_parent = state.remote_dir_ids[parent_idx % state.remote_dir_ids.len()].clone();
            state.remote_parents.insert(id.clone(), new_parent.clone());
            SimAction::RemoteMove {
                id,
                new_parent_id: new_parent,
                new_name: new_name.clone(),
            }
        }
        ActionChoice::LocalCreateFile {
            parent_idx,
            name,
            content,
        } => {
            let local_id = state.next_local_file_id();
            let parent = if state.local_dir_ids.is_empty() {
                None
            } else {
                Some(state.local_dir_ids[parent_idx % state.local_dir_ids.len()].clone())
            };
            state.local_file_ids.push(local_id.clone());
            if let Some(ref p) = parent {
                state.local_parents.insert(local_id.clone(), p.clone());
            }
            SimAction::LocalCreateFile {
                local_id,
                parent_local_id: parent,
                name: name.clone(),
                content: content.clone(),
            }
        }
        ActionChoice::LocalDeleteFile { idx } => {
            if state.local_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.local_file_ids[idx % state.local_file_ids.len()].clone();
            state.local_file_ids.retain(|x| x != &id);
            SimAction::LocalDeleteFile { local_id: id }
        }
        ActionChoice::LocalModifyFile { idx, content } => {
            if state.local_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.local_file_ids[idx % state.local_file_ids.len()].clone();
            SimAction::LocalModifyFile {
                local_id: id,
                content: content.clone(),
            }
        }
        ActionChoice::Sync => SimAction::Sync,
        ActionChoice::StopClient => {
            if state.stopped {
                return SimAction::Sync;
            }
            state.stopped = true;
            SimAction::StopClient
        }
        ActionChoice::RestartClient => {
            if !state.stopped {
                return SimAction::Sync;
            }
            state.stopped = false;
            SimAction::RestartClient
        }
        ActionChoice::RemoteCreateDir { parent_idx, name } => {
            if state.remote_dir_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.next_remote_id();
            let parent = state.remote_dir_ids[parent_idx % state.remote_dir_ids.len()].clone();
            state.remote_dir_ids.push(id.clone());
            state.remote_parents.insert(id.clone(), parent.clone());
            SimAction::RemoteCreateDir {
                id,
                parent_id: Some(parent),
                name: name.clone(),
            }
        }
        ActionChoice::LocalCreateDir { parent_idx, name } => {
            let local_id = state.next_local_file_id();
            let parent = if state.local_dir_ids.is_empty() {
                None
            } else {
                Some(state.local_dir_ids[parent_idx % state.local_dir_ids.len()].clone())
            };
            state.local_dir_ids.push(local_id.clone());
            if let Some(ref p) = parent {
                state.local_parents.insert(local_id.clone(), p.clone());
            }
            SimAction::LocalCreateDir {
                local_id,
                parent_local_id: parent,
                name: name.clone(),
            }
        }
        ActionChoice::LocalMoveFile {
            idx,
            parent_idx,
            new_name,
        } => {
            if state.local_file_ids.is_empty() || state.local_dir_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.local_file_ids[idx % state.local_file_ids.len()].clone();
            let new_parent = state.local_dir_ids[parent_idx % state.local_dir_ids.len()].clone();
            state.local_parents.insert(id.clone(), new_parent.clone());
            SimAction::LocalMove {
                local_id: id,
                new_parent_local_id: Some(new_parent),
                new_name: new_name.clone(),
            }
        }
        ActionChoice::RemoteMoveDir { idx, new_name } => {
            if state.remote_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.remote_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            let new_parent = state.remote_dir_ids[0].clone();
            state.remote_parents.insert(id.clone(), new_parent.clone());
            SimAction::RemoteMove {
                id,
                new_parent_id: new_parent,
                new_name: new_name.clone(),
            }
        }
        ActionChoice::LocalMoveDir { idx, new_name } => {
            if state.local_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.local_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            let new_parent = state.local_dir_ids[0].clone();
            state.local_parents.insert(id.clone(), new_parent.clone());
            SimAction::LocalMove {
                local_id: id,
                new_parent_local_id: Some(new_parent),
                new_name: new_name.clone(),
            }
        }
        ActionChoice::LocalDeleteDir { idx } => {
            if state.local_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.local_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            state.remove_local_tree(&id);
            SimAction::LocalDeleteFile { local_id: id }
        }
        ActionChoice::RemoteDeleteDir { idx } => {
            if state.remote_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.remote_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            state.remove_remote_tree(&id);
            SimAction::RemoteDeleteFile { id }
        }
    }
}
