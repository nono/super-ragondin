use proptest::prelude::*;
use super_ragondin_sync::model::{LocalFileId, LocalNode, NodeType, RemoteId, RemoteNode};
use super_ragondin_sync::simulator::mock_fs::MockFs;
use super_ragondin_sync::simulator::mock_remote::MockRemote;
use super_ragondin_sync::simulator::runner::{ConcurrentRemoteOp, SimAction, SimulationRunner};
use super_ragondin_sync::store::TreeStore;
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

// ==================== Both-sides-deleted cleanup ====================

#[test]
fn both_sides_deleted_cleans_orphaned_synced_record() {
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

    // Create a dir on remote
    let dir_id = RemoteId::new("dir-both-deleted");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: dir_id.clone(),
            parent_id: Some(root_id),
            name: "shared-dir".to_string(),
        })
        .unwrap();

    // Sync - dir appears locally and a synced record is created
    runner.apply(SimAction::Sync).unwrap();

    // Delete locally (simulates user removing from filesystem)
    let local_dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "shared-dir")
        .collect();
    assert_eq!(local_dirs.len(), 1, "dir should exist locally after sync");
    let local_id = local_dirs[0].id.clone();

    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: local_id.clone(),
        })
        .unwrap();

    // Trash on remote (simulates remote user trashing)
    runner.apply(SimAction::RemoteTrash { id: dir_id }).unwrap();

    // Sync multiple rounds to let cleanup happen
    for _ in 0..3 {
        runner.apply(SimAction::Sync).unwrap();
    }

    // The synced record should be cleaned up - store must be consistent
    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
    runner.check_store_consistency().unwrap();
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

#[test]
fn check_content_integrity_detects_mismatch() {
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
            name: "data.txt".to_string(),
            content: b"original".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Integrity should pass after a clean sync
    runner.check_content_integrity().unwrap();

    // Tamper with remote content directly (bypassing sync)
    runner.remote.set_content(&file_id, b"tampered".to_vec());

    // check_content_integrity should now fail
    let result = runner.check_content_integrity();
    assert!(
        result.is_err(),
        "content integrity should fail when bytes differ"
    );
}


#[test]
fn check_no_orphaned_store_nodes_detects_orphan() {
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
            name: "orphan.txt".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Should pass after a clean sync
    runner.check_no_orphaned_store_nodes().unwrap();

    // Directly delete from MockRemote without going through sync
    runner.remote.delete_node(&file_id);

    // The store still has the remote node — orphan detected
    let result = runner.check_no_orphaned_store_nodes();
    assert!(result.is_err(), "should detect orphaned remote store node");
}


#[test]
fn check_no_duplicate_local_paths_detects_duplicate() {
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

    // Clean state should pass
    runner.check_no_duplicate_local_paths().unwrap();

    // Create two files with the same name under the same parent
    let id1 = LocalFileId::new(1, 40_000);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: id1,
            parent_local_id: Some(root_local_id.clone()),
            name: "dup.txt".to_string(),
            content: b"a".to_vec(),
        })
        .unwrap();

    let id2 = LocalFileId::new(1, 40_001);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: id2,
            parent_local_id: Some(root_local_id),
            name: "dup.txt".to_string(),
            content: b"b".to_vec(),
        })
        .unwrap();

    let result = runner.check_no_duplicate_local_paths();
    assert!(result.is_err(), "should detect duplicate local paths");
}

// ==================== Successive/Chained Move Tests ====================

/// Helper: set up a runner with root + initial sync, return (runner, root_id).
fn setup_runner_with_root(dir: &std::path::Path) -> (SimulationRunner, RemoteId) {
    let store = TreeStore::open(dir).unwrap();
    let mut runner = SimulationRunner::new(store, dir.join("sync"));

    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    (runner, root_id)
}

/// Find the local ID for a node by name.
fn find_local_id_by_name(runner: &SimulationRunner, name: &str) -> LocalFileId {
    runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == name)
        .map(|n| n.id.clone())
        .unwrap_or_else(|| panic!("local node '{name}' not found"))
}

// -- move_file_successive: src/file → dst1/file, sync, dst1/file → dst2/file --

#[test]
fn move_file_successive_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create dirs and file on remote, then sync to get local copies
    for (id, name) in [
        ("dir-src", "src"),
        ("dir-dst1", "dst1"),
        ("dir-dst2", "dst2"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let dst1_id = find_local_id_by_name(&runner, "dst1");
    let dst2_id = find_local_id_by_name(&runner, "dst2");

    // Move 1: src/file → dst1/file
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id.clone(),
            new_parent_local_id: Some(dst1_id),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move 2: dst1/file → dst2/file
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id,
            new_parent_local_id: Some(dst2_id),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    let remote_files: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(remote_files.len(), 1, "Exactly one 'file' on remote");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_file_successive_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    for (id, name) in [
        ("dir-src", "src"),
        ("dir-dst1", "dst1"),
        ("dir-dst2", "dst2"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let dst1_id = find_local_id_by_name(&runner, "dst1");
    let dst2_id = find_local_id_by_name(&runner, "dst2");

    // Stop client, perform both moves while stopped
    runner.apply(SimAction::StopClient).unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id.clone(),
            new_parent_local_id: Some(dst1_id),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id,
            new_parent_local_id: Some(dst2_id),
            new_name: "file".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_file_successive_remote: same pattern but changes happen on remote side --

#[test]
fn move_file_successive_remote() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    for (id, name) in [
        ("dir-src", "src"),
        ("dir-dst1", "dst1"),
        ("dir-dst2", "dst2"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move 1: src/file → dst1/file on remote
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-1"),
            new_parent_id: RemoteId::new("dir-dst1"),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move 2: dst1/file → dst2/file on remote
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-1"),
            new_parent_id: RemoteId::new("dir-dst2"),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1);

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_and_update_file: move src/file → dst/file AND update content in the same sync cycle --

#[test]
fn move_and_update_file_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create dirs and file on remote, then sync
    for (id, name) in [("dir-src", "src"), ("dir-dst", "dst")] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let dst_id = find_local_id_by_name(&runner, "dst");

    // Move src/file → dst/file AND update content in the same cycle
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id.clone(),
            new_parent_local_id: Some(dst_id),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalModifyFile {
            local_id: file_id.clone(),
            content: b"updated content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Verify file is at dst/file
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    // Verify content is updated locally
    let local_content = runner.local_fs.read_file(&file_id);
    assert_eq!(
        local_content,
        Some(&b"updated content".to_vec()),
        "Local file should have updated content"
    );

    // Verify content is updated on remote
    let remote_file: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(remote_file.len(), 1, "Exactly one 'file' on remote");
    let remote_content = runner.remote.get_content(&remote_file[0].id);
    assert_eq!(
        remote_content,
        Some(&b"updated content".to_vec()),
        "Remote file should have updated content"
    );

    // Verify dirs still exist
    let dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "src" || n.name == "dst")
        .collect();
    assert_eq!(dirs.len(), 2, "Both src/ and dst/ should exist");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_and_update_file_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    for (id, name) in [("dir-src", "src"), ("dir-dst", "dst")] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let dst_id = find_local_id_by_name(&runner, "dst");

    // Stop client, perform move + update while stopped
    runner.apply(SimAction::StopClient).unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id.clone(),
            new_parent_local_id: Some(dst_id),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalModifyFile {
            local_id: file_id.clone(),
            content: b"updated content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    let local_content = runner.local_fs.read_file(&file_id);
    assert_eq!(
        local_content,
        Some(&b"updated content".to_vec()),
        "Local file should have updated content"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_and_update_file_remote() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    for (id, name) in [("dir-src", "src"), ("dir-dst", "dst")] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move and update on remote in the same cycle
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-1"),
            new_parent_id: RemoteId::new("dir-dst"),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteModifyFile {
            id: RemoteId::new("file-1"),
            content: b"updated content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Verify file is at dst/file locally
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    // Verify content is updated locally
    let local_content = runner.local_fs.read_file(&local_files[0].id);
    assert_eq!(
        local_content,
        Some(&b"updated content".to_vec()),
        "Local file should have updated content"
    );

    // Verify dirs still exist
    let dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "src" || n.name == "dst")
        .collect();
    assert_eq!(dirs.len(), 2, "Both src/ and dst/ should exist");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_and_update_file_local_atomic_save() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    for (id, name) in [("dir-src", "src"), ("dir-dst", "dst")] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let dst_id = find_local_id_by_name(&runner, "dst");

    // Move + atomic save (simulates editors that write-to-temp then rename)
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id.clone(),
            new_parent_local_id: Some(dst_id),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalAtomicSave {
            local_id: file_id,
            content: b"updated content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    let remote_files: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(remote_files.len(), 1, "Exactly one 'file' on remote");
    let remote_content = runner.remote.get_content(&remote_files[0].id);
    assert_eq!(
        remote_content,
        Some(&b"updated content".to_vec()),
        "Remote file should have updated content"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_dir_a_to_b_to_c_to_b: cyclic rename A→B, sync, B→C, sync, C→B --

#[test]
fn move_dir_a_to_b_to_c_to_b_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create parent dir "src" and dir "A" inside it on remote
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-src"),
            parent_id: Some(root_id.clone()),
            name: "src".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-a"),
            parent_id: Some(RemoteId::new("dir-src")),
            name: "A".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let dir_a_id = find_local_id_by_name(&runner, "A");
    let src_id = find_local_id_by_name(&runner, "src");

    // Rename A → B
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_a_id.clone(),
            new_parent_local_id: Some(src_id.clone()),
            new_name: "B".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Rename B → C
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_a_id.clone(),
            new_parent_local_id: Some(src_id.clone()),
            new_name: "C".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Rename C → B (reuse name B)
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_a_id,
            new_parent_local_id: Some(src_id),
            new_name: "B".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_stale: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "A" || n.name == "C")
        .collect();
    assert!(local_stale.is_empty(), "A and C should not exist locally");

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "B")
        .collect();
    assert_eq!(local_b.len(), 1, "Exactly one B should exist locally");

    let remote_b: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "B")
        .collect();
    assert_eq!(remote_b.len(), 1, "Exactly one B should exist on remote");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_dir_a_to_b_to_c_to_b_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-src"),
            parent_id: Some(root_id.clone()),
            name: "src".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-a"),
            parent_id: Some(RemoteId::new("dir-src")),
            name: "A".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let dir_a_id = find_local_id_by_name(&runner, "A");
    let src_id = find_local_id_by_name(&runner, "src");

    // Stop, do all three renames, restart
    runner.apply(SimAction::StopClient).unwrap();
    for name in ["B", "C", "B"] {
        runner
            .apply(SimAction::LocalMove {
                local_id: dir_a_id.clone(),
                new_parent_local_id: Some(src_id.clone()),
                new_name: name.to_string(),
            })
            .unwrap();
    }

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "B")
        .collect();
    assert_eq!(local_b.len(), 1, "Exactly one B should exist locally");

    let remote_stale: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "A" || n.name == "C")
        .collect();
    assert!(
        remote_stale.is_empty(),
        "A and C should not exist on remote"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_file_a_to_b_to_c_to_b: same cyclic pattern but for files --

#[test]
fn move_file_a_to_b_to_c_to_b_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-src"),
            parent_id: Some(root_id.clone()),
            name: "src".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: RemoteId::new("dir-src"),
            name: "A".to_string(),
            content: b"foo".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_a_id = find_local_id_by_name(&runner, "A");
    let src_id = find_local_id_by_name(&runner, "src");

    // Rename A → B
    runner
        .apply(SimAction::LocalMove {
            local_id: file_a_id.clone(),
            new_parent_local_id: Some(src_id.clone()),
            new_name: "B".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Rename B → C
    runner
        .apply(SimAction::LocalMove {
            local_id: file_a_id.clone(),
            new_parent_local_id: Some(src_id.clone()),
            new_name: "C".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Rename C → B (reuse name B)
    runner
        .apply(SimAction::LocalMove {
            local_id: file_a_id,
            new_parent_local_id: Some(src_id),
            new_name: "B".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "B")
        .collect();
    assert_eq!(local_b.len(), 1, "Exactly one B should exist locally");

    let local_stale: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "A" || n.name == "C")
        .collect();
    assert!(local_stale.is_empty(), "A and C should not exist locally");

    let remote_b: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "B")
        .collect();
    assert_eq!(remote_b.len(), 1, "Exactly one B should exist on remote");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_file_a_to_b_to_c_to_b_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-src"),
            parent_id: Some(root_id.clone()),
            name: "src".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: RemoteId::new("dir-src"),
            name: "A".to_string(),
            content: b"foo".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_a_id = find_local_id_by_name(&runner, "A");
    let src_id = find_local_id_by_name(&runner, "src");

    // Stop, do all three renames, restart
    runner.apply(SimAction::StopClient).unwrap();
    for name in ["B", "C", "B"] {
        runner
            .apply(SimAction::LocalMove {
                local_id: file_a_id.clone(),
                new_parent_local_id: Some(src_id.clone()),
                new_name: name.to_string(),
            })
            .unwrap();
    }

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "B")
        .collect();
    assert_eq!(local_b.len(), 1, "Exactly one B should exist locally");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- file_swap: move a→c, sync, move b→a — tests identity tracking, not path-based matching --

#[test]
fn file_swap_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create two files on remote, then sync
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: root_id.clone(),
            name: "a".to_string(),
            content: b"content a".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-b"),
            parent_id: root_id,
            name: "b".to_string(),
            content: b"content b".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let root_local_id = find_local_id_by_name(&runner, "");
    let a_id = find_local_id_by_name(&runner, "a");
    let b_id = find_local_id_by_name(&runner, "b");

    // Move a → c, sync
    runner
        .apply(SimAction::LocalMove {
            local_id: a_id,
            new_parent_local_id: Some(root_local_id.clone()),
            new_name: "c".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move b → a
    runner
        .apply(SimAction::LocalMove {
            local_id: b_id,
            new_parent_local_id: Some(root_local_id),
            new_name: "a".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Verify: "a" has content b, "c" has content a, no "b" exists
    let local_a: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "a")
        .collect();
    assert_eq!(local_a.len(), 1, "Exactly one 'a' locally");

    let local_c: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "c")
        .collect();
    assert_eq!(local_c.len(), 1, "Exactly one 'c' locally");

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "b")
        .collect();
    assert!(local_b.is_empty(), "No 'b' should remain locally");

    // Content verification via remote (canonical source)
    let remote_a: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "a")
        .collect();
    assert_eq!(remote_a.len(), 1, "Exactly one 'a' on remote");

    let remote_c: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "c")
        .collect();
    assert_eq!(remote_c.len(), 1, "Exactly one 'c' on remote");

    let remote_b: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "b")
        .collect();
    assert!(remote_b.is_empty(), "No 'b' should remain on remote");

    // Verify content: "a" should have "content b", "c" should have "content a"
    let a_content = runner.remote.get_content(&remote_a[0].id);
    assert_eq!(
        a_content,
        Some(&b"content b".to_vec()),
        "'a' should have content b"
    );

    let c_content = runner.remote.get_content(&remote_c[0].id);
    assert_eq!(
        c_content,
        Some(&b"content a".to_vec()),
        "'c' should have content a"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn file_swap_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create two files on remote, then sync
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: root_id.clone(),
            name: "a".to_string(),
            content: b"content a".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-b"),
            parent_id: root_id,
            name: "b".to_string(),
            content: b"content b".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let root_local_id = find_local_id_by_name(&runner, "");
    let a_id = find_local_id_by_name(&runner, "a");
    let b_id = find_local_id_by_name(&runner, "b");

    // Stop client, perform both moves while stopped
    runner.apply(SimAction::StopClient).unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: a_id,
            new_parent_local_id: Some(root_local_id.clone()),
            new_name: "c".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: b_id,
            new_parent_local_id: Some(root_local_id),
            new_name: "a".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Verify: "a" has content b, "c" has content a, no "b" exists
    let local_a: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "a")
        .collect();
    assert_eq!(local_a.len(), 1, "Exactly one 'a' locally");

    let local_c: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "c")
        .collect();
    assert_eq!(local_c.len(), 1, "Exactly one 'c' locally");

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "b")
        .collect();
    assert!(local_b.is_empty(), "No 'b' should remain locally");

    // Content verification via remote
    let remote_a: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "a")
        .collect();
    assert_eq!(remote_a.len(), 1, "Exactly one 'a' on remote");

    let remote_c: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "c")
        .collect();
    assert_eq!(remote_c.len(), 1, "Exactly one 'c' on remote");

    let a_content = runner.remote.get_content(&remote_a[0].id);
    assert_eq!(
        a_content,
        Some(&b"content b".to_vec()),
        "'a' should have content b"
    );

    let c_content = runner.remote.get_content(&remote_c[0].id);
    assert_eq!(
        c_content,
        Some(&b"content a".to_vec()),
        "'c' should have content a"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn file_swap_remote() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create two files on remote, then sync
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: root_id.clone(),
            name: "a".to_string(),
            content: b"content a".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-b"),
            parent_id: root_id,
            name: "b".to_string(),
            content: b"content b".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move a → c on remote, sync
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-a"),
            new_parent_id: RemoteId::new("io.cozy.files.root-dir"),
            new_name: "c".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move b → a on remote
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-b"),
            new_parent_id: RemoteId::new("io.cozy.files.root-dir"),
            new_name: "a".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Verify locally
    let local_a: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "a")
        .collect();
    assert_eq!(local_a.len(), 1, "Exactly one 'a' locally");

    let local_c: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "c")
        .collect();
    assert_eq!(local_c.len(), 1, "Exactly one 'c' locally");

    let local_b: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "b")
        .collect();
    assert!(local_b.is_empty(), "No 'b' should remain locally");

    // Verify content via local fs
    let a_local_content = runner.local_fs.read_file(&local_a[0].id);
    assert_eq!(
        a_local_content,
        Some(&b"content b".to_vec()),
        "'a' should have content b"
    );

    let c_local_content = runner.local_fs.read_file(&local_c[0].id);
    assert_eq!(
        c_local_content,
        Some(&b"content a".to_vec()),
        "'c' should have content a"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Nested/Cascading Move Tests ====================

// -- move_file_inside_move: rename child, move parent dir, rename another child --
// Init: parent/, parent/dst/, parent/src/, parent/src/dir/, parent/src/dir/empty-subdir/,
//       parent/src/dir/subdir/, parent/src/dir/subdir/file, parent/src/dir/subdir/file2
// Actions: rename file→filerenamed, move src/dir→dst/dir, rename file2→filerenamed2
// Expected: parent/, parent/dst/, parent/dst/dir/, parent/dst/dir/empty-subdir/,
//           parent/dst/dir/subdir/, parent/dst/dir/subdir/filerenamed,
//           parent/dst/dir/subdir/filerenamed2, parent/src/

#[test]
fn move_file_inside_move_local() {
    let dir = tempdir().unwrap();
    let (mut runner, _root_id) = setup_runner_with_root(dir.path());

    // Build initial tree on remote
    for (id, parent, name) in [
        ("dir-parent", Some("io.cozy.files.root-dir"), "parent"),
        ("dir-dst", Some("dir-parent"), "dst"),
        ("dir-src", Some("dir-parent"), "src"),
        ("dir-dir", Some("dir-src"), "dir"),
        ("dir-empty-subdir", Some("dir-dir"), "empty-subdir"),
        ("dir-subdir", Some("dir-dir"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: parent.map(RemoteId::new),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content1".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-2"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file2".to_string(),
            content: b"content2".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let file2_id = find_local_id_by_name(&runner, "file2");
    let dir_id = find_local_id_by_name(&runner, "dir");
    let dst_id = find_local_id_by_name(&runner, "dst");
    let subdir_id = find_local_id_by_name(&runner, "subdir");

    // Action 1: rename file → filerenamed (keep same parent: subdir)
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id,
            new_parent_local_id: Some(subdir_id.clone()),
            new_name: "filerenamed".to_string(),
        })
        .unwrap();

    // Action 2: move src/dir → dst/dir
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_id,
            new_parent_local_id: Some(dst_id),
            new_name: "dir".to_string(),
        })
        .unwrap();

    // Action 3: rename file2 → filerenamed2 (keep same parent: subdir)
    runner
        .apply(SimAction::LocalMove {
            local_id: file2_id,
            new_parent_local_id: Some(subdir_id),
            new_name: "filerenamed2".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Verify expected tree
    let names: Vec<String> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(names.contains(&"parent".to_string()));
    assert!(names.contains(&"dst".to_string()));
    assert!(names.contains(&"dir".to_string()));
    assert!(names.contains(&"empty-subdir".to_string()));
    assert!(names.contains(&"subdir".to_string()));
    assert!(names.contains(&"filerenamed".to_string()));
    assert!(names.contains(&"filerenamed2".to_string()));
    assert!(names.contains(&"src".to_string()));
    // Old names should be gone
    assert!(
        !names.contains(&"file".to_string()),
        "'file' should be renamed"
    );
    assert!(
        !names.contains(&"file2".to_string()),
        "'file2' should be renamed"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_file_inside_move_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, _root_id) = setup_runner_with_root(dir.path());

    for (id, parent, name) in [
        ("dir-parent", Some("io.cozy.files.root-dir"), "parent"),
        ("dir-dst", Some("dir-parent"), "dst"),
        ("dir-src", Some("dir-parent"), "src"),
        ("dir-dir", Some("dir-src"), "dir"),
        ("dir-empty-subdir", Some("dir-dir"), "empty-subdir"),
        ("dir-subdir", Some("dir-dir"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: parent.map(RemoteId::new),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content1".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-2"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file2".to_string(),
            content: b"content2".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_id = find_local_id_by_name(&runner, "file");
    let file2_id = find_local_id_by_name(&runner, "file2");
    let dir_id = find_local_id_by_name(&runner, "dir");
    let dst_id = find_local_id_by_name(&runner, "dst");
    let subdir_id = find_local_id_by_name(&runner, "subdir");

    // All actions while stopped
    runner.apply(SimAction::StopClient).unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: file_id,
            new_parent_local_id: Some(subdir_id.clone()),
            new_name: "filerenamed".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_id,
            new_parent_local_id: Some(dst_id),
            new_name: "dir".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: file2_id,
            new_parent_local_id: Some(subdir_id),
            new_name: "filerenamed2".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let names: Vec<String> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(names.contains(&"filerenamed".to_string()));
    assert!(names.contains(&"filerenamed2".to_string()));
    assert!(!names.contains(&"file".to_string()));
    assert!(!names.contains(&"file2".to_string()));

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_from_inside_move: move parent dir, then extract a child out --
// Init: parent/, parent/dst/, parent/dst2/, parent/src/, parent/src/dir/,
//       parent/src/dir/empty-subdir/, parent/src/dir/subdir/, parent/src/dir/subdir/file
// Actions: move src/dir→dst/dir, move dst/dir/subdir→dst2/subdir
// Expected: parent/, parent/dst/, parent/dst/dir/, parent/dst/dir/empty-subdir/,
//           parent/dst2/, parent/dst2/subdir/, parent/dst2/subdir/file, parent/src/

#[test]
fn move_from_inside_move_local() {
    let dir = tempdir().unwrap();
    let (mut runner, _root_id) = setup_runner_with_root(dir.path());

    for (id, parent, name) in [
        ("dir-parent", Some("io.cozy.files.root-dir"), "parent"),
        ("dir-dst", Some("dir-parent"), "dst"),
        ("dir-dst2", Some("dir-parent"), "dst2"),
        ("dir-src", Some("dir-parent"), "src"),
        ("dir-dir", Some("dir-src"), "dir"),
        ("dir-empty-subdir", Some("dir-dir"), "empty-subdir"),
        ("dir-subdir", Some("dir-dir"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: parent.map(RemoteId::new),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let dir_id = find_local_id_by_name(&runner, "dir");
    let dst_id = find_local_id_by_name(&runner, "dst");
    let subdir_id = find_local_id_by_name(&runner, "subdir");
    let dst2_id = find_local_id_by_name(&runner, "dst2");

    // Action 1: move src/dir → dst/dir
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_id,
            new_parent_local_id: Some(dst_id),
            new_name: "dir".to_string(),
        })
        .unwrap();

    // Action 2: move dst/dir/subdir → dst2/subdir
    runner
        .apply(SimAction::LocalMove {
            local_id: subdir_id,
            new_parent_local_id: Some(dst2_id.clone()),
            new_name: "subdir".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Verify expected tree
    let names: Vec<String> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(names.contains(&"parent".to_string()));
    assert!(names.contains(&"dst".to_string()));
    assert!(names.contains(&"dir".to_string()));
    assert!(names.contains(&"empty-subdir".to_string()));
    assert!(names.contains(&"dst2".to_string()));
    assert!(names.contains(&"subdir".to_string()));
    assert!(names.contains(&"file".to_string()));
    assert!(names.contains(&"src".to_string()));

    // subdir should be under dst2, not under dir
    let subdir_node = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "subdir")
        .unwrap();
    assert_eq!(
        subdir_node.parent_id,
        Some(dst2_id),
        "subdir should be under dst2"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_from_inside_move_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, _root_id) = setup_runner_with_root(dir.path());

    for (id, parent, name) in [
        ("dir-parent", Some("io.cozy.files.root-dir"), "parent"),
        ("dir-dst", Some("dir-parent"), "dst"),
        ("dir-dst2", Some("dir-parent"), "dst2"),
        ("dir-src", Some("dir-parent"), "src"),
        ("dir-dir", Some("dir-src"), "dir"),
        ("dir-empty-subdir", Some("dir-dir"), "empty-subdir"),
        ("dir-subdir", Some("dir-dir"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: parent.map(RemoteId::new),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let dir_id = find_local_id_by_name(&runner, "dir");
    let dst_id = find_local_id_by_name(&runner, "dst");
    let subdir_id = find_local_id_by_name(&runner, "subdir");
    let dst2_id = find_local_id_by_name(&runner, "dst2");

    // All actions while stopped
    runner.apply(SimAction::StopClient).unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: dir_id,
            new_parent_local_id: Some(dst_id),
            new_name: "dir".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: subdir_id,
            new_parent_local_id: Some(dst2_id.clone()),
            new_name: "subdir".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let subdir_node = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "subdir")
        .unwrap();
    assert_eq!(
        subdir_node.parent_id,
        Some(dst2_id),
        "subdir should be under dst2"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_dir_parent_and_child: move parent + rename child dir + rename grandchild file --
// Init: parent/, parent/src/, parent/src/dir/, parent/src/dir/empty-subdir/,
//       parent/src/dir/subdir/, parent/src/dir/subdir/file
// Actions: move src→dst, rename dir→dir2, rename file→file2
// Expected: parent/, parent/dst/, parent/dst/dir2/, parent/dst/dir2/empty-subdir/,
//           parent/dst/dir2/subdir/, parent/dst/dir2/subdir/file2

#[test]
fn move_dir_parent_and_child_local() {
    let dir = tempdir().unwrap();
    let (mut runner, _root_id) = setup_runner_with_root(dir.path());

    for (id, parent, name) in [
        ("dir-parent", Some("io.cozy.files.root-dir"), "parent"),
        ("dir-src", Some("dir-parent"), "src"),
        ("dir-dir", Some("dir-src"), "dir"),
        ("dir-empty-subdir", Some("dir-dir"), "empty-subdir"),
        ("dir-subdir", Some("dir-dir"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: parent.map(RemoteId::new),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let src_id = find_local_id_by_name(&runner, "src");
    let parent_id = find_local_id_by_name(&runner, "parent");
    let dir_local_id = find_local_id_by_name(&runner, "dir");
    let file_id = find_local_id_by_name(&runner, "file");
    let subdir_id = find_local_id_by_name(&runner, "subdir");

    // Action 1: rename src → dst (move under same parent)
    runner
        .apply(SimAction::LocalMove {
            local_id: src_id.clone(),
            new_parent_local_id: Some(parent_id),
            new_name: "dst".to_string(),
        })
        .unwrap();

    // Action 2: rename dir → dir2 (keep same parent: src, now named dst)
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_local_id,
            new_parent_local_id: Some(src_id),
            new_name: "dir2".to_string(),
        })
        .unwrap();

    // Action 3: rename file → file2 (keep same parent: subdir)
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id,
            new_parent_local_id: Some(subdir_id),
            new_name: "file2".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Verify expected tree
    let names: Vec<String> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(names.contains(&"parent".to_string()));
    assert!(names.contains(&"dst".to_string()));
    assert!(names.contains(&"dir2".to_string()));
    assert!(names.contains(&"empty-subdir".to_string()));
    assert!(names.contains(&"subdir".to_string()));
    assert!(names.contains(&"file2".to_string()));
    // Old names should be gone
    assert!(
        !names.contains(&"src".to_string()),
        "'src' should be renamed to 'dst'"
    );
    assert!(
        !names.contains(&"dir".to_string()),
        "'dir' should be renamed to 'dir2'"
    );
    assert!(
        !names.contains(&"file".to_string()),
        "'file' should be renamed to 'file2'"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_dir_parent_and_child_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, _root_id) = setup_runner_with_root(dir.path());

    for (id, parent, name) in [
        ("dir-parent", Some("io.cozy.files.root-dir"), "parent"),
        ("dir-src", Some("dir-parent"), "src"),
        ("dir-dir", Some("dir-src"), "dir"),
        ("dir-empty-subdir", Some("dir-dir"), "empty-subdir"),
        ("dir-subdir", Some("dir-dir"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: parent.map(RemoteId::new),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let src_id = find_local_id_by_name(&runner, "src");
    let parent_id = find_local_id_by_name(&runner, "parent");
    let dir_local_id = find_local_id_by_name(&runner, "dir");
    let file_id = find_local_id_by_name(&runner, "file");
    let subdir_id = find_local_id_by_name(&runner, "subdir");

    // All actions while stopped
    runner.apply(SimAction::StopClient).unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: src_id.clone(),
            new_parent_local_id: Some(parent_id),
            new_name: "dst".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: dir_local_id,
            new_parent_local_id: Some(src_id),
            new_name: "dir2".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::LocalMove {
            local_id: file_id,
            new_parent_local_id: Some(subdir_id),
            new_name: "file2".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let names: Vec<String> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(names.contains(&"dst".to_string()));
    assert!(names.contains(&"dir2".to_string()));
    assert!(names.contains(&"file2".to_string()));
    assert!(!names.contains(&"src".to_string()));
    assert!(!names.contains(&"dir".to_string()));
    assert!(!names.contains(&"file".to_string()));

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Property-Based Tests ====================

fn arbitrary_file_name() -> impl Strategy<Value = String> {
    // 50% chance: pick from a fixed pool (collision pressure + unicode coverage)
    // 50% chance: generate a fresh short name (diverse inputs)
    let pool = prop::sample::select(vec![
        "notes.txt".to_string(),
        "café.txt".to_string(),
        "résumé.doc".to_string(),
        "📊 report.pdf".to_string(),
        "données.csv".to_string(),
        "photo 🌅.jpg".to_string(),
        "naïve.md".to_string(),
        "über.log".to_string(),
    ]);
    let fresh = "[a-z]{1,4}\\.[a-z]{2,3}";
    prop_oneof![pool, fresh]
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

#[test]
fn check_all_invariants_passes_on_clean_state() {
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
    runner.check_all_invariants().unwrap();
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

        // Check all invariants
        runner.check_all_invariants().unwrap();
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

        // Check all invariants
        runner.check_all_invariants().unwrap();
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

        runner.check_all_invariants().unwrap();
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

        // Check all invariants
        runner.check_all_invariants().unwrap();
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

        // Ensure client is running for final syncs
        runner.apply(SimAction::RestartClient).unwrap();
        // Multiple sync rounds to handle concurrent remote changes injected mid-sync
        for _ in 0..3 {
            runner.apply(SimAction::Sync).unwrap();
        }

        // After sync, check all invariants
        runner.check_all_invariants().unwrap();
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
            // Extra sync to pick up any concurrent remote changes that fired mid-sync
            runner.apply(SimAction::Sync).unwrap();

            // Refresh sim_state to match the actual local filesystem after sync.
            // Sync can create new local dirs (from synced remote dirs or collision
            // rename) or remove dirs — IDs in sim_state may have become stale.
            let tracked_ids: std::collections::HashSet<_> = sim_state
                .local_dir_ids
                .iter()
                .chain(sim_state.local_file_ids.iter())
                .cloned()
                .collect();
            for node in runner.local_fs.list_all() {
                if !tracked_ids.contains(&node.id) {
                    match node.node_type {
                        NodeType::Directory => sim_state.local_dir_ids.push(node.id.clone()),
                        NodeType::File => sim_state.local_file_ids.push(node.id.clone()),
                    }
                }
                if let Some(ref parent_id) = node.parent_id {
                    sim_state.local_parents.insert(node.id.clone(), parent_id.clone());
                }
            }
            sim_state.local_dir_ids.retain(|id| runner.local_fs.exists(id));
            sim_state.local_file_ids.retain(|id| runner.local_fs.exists(id));
            // Prune remote IDs deleted during sync (e.g., when a local delete
            // was propagated to remote). Without this, later actions may try
            // to modify remote nodes that no longer exist.
            sim_state.remote_file_ids.retain(|id| runner.remote.get_node(id).is_some());
            sim_state.remote_dir_ids.retain(|id| runner.remote.get_node(id).is_some());

            // Check convergence after every round. Allow up to 3 extra syncs
            // for concurrent remote changes (trashes, deletes) that arrived
            // mid-sync and need an additional round to fully propagate.
            for _ in 0..3 {
                if runner.check_convergence().is_ok() {
                    break;
                }
                runner.apply(SimAction::Sync).unwrap();
            }
            runner.check_convergence().unwrap();
        }

        // Extra sync rounds to handle any concurrent remote changes from the last round
        for _ in 0..3 {
            runner.apply(SimAction::Sync).unwrap();
        }

        // Final invariant checks
        runner.check_all_invariants().unwrap();
    }
}

// ==================== Concurrent Remote Changes During Sync ====================

#[test]
fn concurrent_remote_create_during_sync_converges() {
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

    // Create a local file that will be uploaded during sync
    let file_local_id = LocalFileId::new(1, 9999);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id,
            parent_local_id: Some(root_local_id),
            name: "local.txt".to_string(),
            content: b"local content".to_vec(),
        })
        .unwrap();

    // Queue a concurrent remote file creation that fires during sync execution
    runner
        .apply(SimAction::ConcurrentRemoteChange(
            ConcurrentRemoteOp::CreateFile {
                id: RemoteId::new("concurrent-file-1"),
                parent_id: root_id.clone(),
                name: "concurrent.txt".to_string(),
                content: b"concurrent content".to_vec(),
            },
        ))
        .unwrap();

    // First sync: uploads local file, concurrent change fires mid-execution
    runner.apply(SimAction::Sync).unwrap();

    // concurrent.txt is on MockRemote but not yet synced locally
    assert!(
        runner
            .remote
            .get_node(&RemoteId::new("concurrent-file-1"))
            .is_some(),
        "Concurrent file should exist on remote"
    );

    // Second sync picks up the concurrent change
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn concurrent_remote_modify_during_sync_converges() {
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

    // Create and sync a file
    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: root_id.clone(),
            name: "data.txt".to_string(),
            content: b"original".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Create a new remote file (triggers sync work)
    let file2_id = RemoteId::new("file-2");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file2_id,
            parent_id: root_id.clone(),
            name: "other.txt".to_string(),
            content: b"other".to_vec(),
        })
        .unwrap();

    // Queue a concurrent modification that fires during the next sync
    runner
        .apply(SimAction::ConcurrentRemoteChange(
            ConcurrentRemoteOp::ModifyFile {
                id: file_id,
                content: b"modified concurrently".to_vec(),
            },
        ))
        .unwrap();

    // First sync: downloads file-2, concurrent modify happens mid-execution
    runner.apply(SimAction::Sync).unwrap();

    // Second sync picks up the modification
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn concurrent_remote_delete_during_sync_converges() {
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

    // Create and sync two files
    let file1_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file1_id.clone(),
            parent_id: root_id.clone(),
            name: "keep.txt".to_string(),
            content: b"keep me".to_vec(),
        })
        .unwrap();

    let file2_id = RemoteId::new("file-2");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file2_id.clone(),
            parent_id: root_id.clone(),
            name: "delete-me.txt".to_string(),
            content: b"will be deleted".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Modify keep.txt on remote (triggers sync work)
    runner
        .apply(SimAction::RemoteModifyFile {
            id: file1_id,
            content: b"updated keep".to_vec(),
        })
        .unwrap();

    // Queue concurrent deletion of file-2 during sync
    runner
        .apply(SimAction::ConcurrentRemoteChange(
            ConcurrentRemoteOp::DeleteFile { id: file2_id },
        ))
        .unwrap();

    // First sync + second sync to converge
    runner.apply(SimAction::Sync).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== State Snapshot & Rollback Tests ====================

#[test]
fn snapshot_and_rollback_restores_state() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Setup: create root and a file, sync them
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
            name: "original.txt".to_string(),
            content: b"original content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Take a snapshot
    runner.apply(SimAction::SnapshotState).unwrap();

    // Make changes: create a new file on remote and sync
    let file2_id = RemoteId::new("file-2");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file2_id,
            parent_id: root_id,
            name: "new.txt".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Verify new file exists
    assert_eq!(
        runner
            .local_fs
            .list_all()
            .iter()
            .filter(|n| n.name == "new.txt")
            .count(),
        1
    );

    // Rollback to snapshot
    runner.apply(SimAction::RollbackToSnapshot).unwrap();

    // new.txt should be gone, original.txt should still be there
    assert_eq!(
        runner
            .local_fs
            .list_all()
            .iter()
            .filter(|n| n.name == "new.txt")
            .count(),
        0,
        "new.txt should be gone after rollback"
    );
    assert_eq!(
        runner
            .local_fs
            .list_all()
            .iter()
            .filter(|n| n.name == "original.txt")
            .count(),
        1,
        "original.txt should still exist after rollback"
    );

    // After a new sync, should converge
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();
}

#[test]
fn rollback_without_snapshot_is_noop() {
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

    // Rollback without snapshot should succeed (no-op)
    runner.apply(SimAction::RollbackToSnapshot).unwrap();
    runner.check_convergence().unwrap();
}

#[test]
fn snapshot_rollback_with_local_changes() {
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

    // Snapshot after initial sync
    runner.apply(SimAction::SnapshotState).unwrap();

    // Create local file and sync
    let file_local_id = LocalFileId::new(1, 9999);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "local.txt".to_string(),
            content: b"local content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Rollback — local file and its remote counterpart should be gone
    runner.apply(SimAction::RollbackToSnapshot).unwrap();

    assert!(
        !runner.local_fs.exists(&file_local_id),
        "Local file should be gone after rollback"
    );

    // After sync, should converge (empty state)
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();
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

// ==================== Trash Folder Tests ====================

#[test]
fn remote_trash_file_removes_locally_on_sync() {
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
            content: b"hello world".to_vec(),
        })
        .unwrap();

    // Sync — file appears locally
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "test.txt")
        .collect();
    assert_eq!(local_files.len(), 1, "File should exist locally after sync");

    // Trash the file on remote
    runner
        .apply(SimAction::RemoteTrash { id: file_id })
        .unwrap();

    // Sync — trash should propagate as a deletion
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "test.txt")
        .collect();
    assert_eq!(
        local_files.len(),
        0,
        "File should be removed locally after trash"
    );

    runner.check_convergence().unwrap();
}

#[test]
fn remote_trash_dir_removes_children_locally() {
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

    let dir_id = RemoteId::new("dir-1");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: dir_id.clone(),
            parent_id: Some(root_id),
            name: "photos".to_string(),
        })
        .unwrap();

    let file_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_id.clone(),
            parent_id: dir_id.clone(),
            name: "vacation.jpg".to_string(),
            content: b"image data".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Trash the directory (which contains the file)
    runner.apply(SimAction::RemoteTrash { id: dir_id }).unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let local_dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "photos")
        .collect();
    assert_eq!(local_dirs.len(), 0, "Trashed dir should be removed locally");

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "vacation.jpg")
        .collect();
    assert_eq!(
        local_files.len(),
        0,
        "File in trashed dir should be removed locally"
    );

    runner.check_convergence().unwrap();
}

// ==================== Move + Trash Tests ====================

/// Move src/file → dst/file, then trash dst/file.
/// Expected: file is trashed (absent locally), tree is just src/ and dst/.
#[test]
fn move_and_trash_file() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create src/ and dst/ dirs, and src/file on remote
    for (id, name) in [("dir-src", "src"), ("dir-dst", "dst")] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(root_id.clone()),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-src"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move file from src/ to dst/ on remote
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-1"),
            new_parent_id: RemoteId::new("dir-dst"),
            new_name: "file".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Trash dst/file on remote
    runner
        .apply(SimAction::RemoteTrash {
            id: RemoteId::new("file-1"),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // File should be gone locally; only src/ and dst/ remain (plus root)
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 0, "file should be trashed");

    let local_names: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(local_names.len(), 2, "Only src/ and dst/ should remain");
    assert!(local_names.contains(&"src".to_string()));
    assert!(local_names.contains(&"dst".to_string()));

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

/// Move dir/file out to root as "file", then trash the now-empty dir.
/// Expected: file survives at root, dir is trashed.
#[test]
fn move_dir_content_and_trash_dir() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create dir/ with a file inside on remote
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-1"),
            parent_id: Some(root_id.clone()),
            name: "dir".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-1"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move file out of dir/ to root
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-1"),
            new_parent_id: root_id,
            new_name: "file".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Trash the now-empty dir
    runner
        .apply(SimAction::RemoteTrash {
            id: RemoteId::new("dir-1"),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // File should survive at root, dir should be gone
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "file should survive at root");

    let local_dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "dir")
        .collect();
    assert_eq!(local_dirs.len(), 0, "dir should be trashed");

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

/// Trash src/subdir/file, then move src/subdir → dst/subdir.
/// Tests dependency ordering: child trash before parent move.
/// Expected: file is trashed, subdir ends up under dst/.
#[test]
fn trash_file_and_move_parent() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create src/, dst/, src/subdir/, and src/subdir/file
    for (id, parent, name) in [
        ("dir-src", root_id.clone(), "src"),
        ("dir-dst", root_id, "dst"),
        ("dir-subdir", RemoteId::new("dir-src"), "subdir"),
    ] {
        runner
            .apply(SimAction::RemoteCreateDir {
                id: RemoteId::new(id),
                parent_id: Some(parent),
                name: name.to_string(),
            })
            .unwrap();
    }
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-1"),
            parent_id: RemoteId::new("dir-subdir"),
            name: "file".to_string(),
            content: b"content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Trash the file inside subdir
    runner
        .apply(SimAction::RemoteTrash {
            id: RemoteId::new("file-1"),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move subdir from src/ to dst/
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("dir-subdir"),
            new_parent_id: RemoteId::new("dir-dst"),
            new_name: "subdir".to_string(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // File should be gone
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 0, "file should be trashed");

    // subdir should be under dst/
    let subdir_nodes: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "subdir")
        .collect();
    assert_eq!(subdir_nodes.len(), 1, "subdir should exist");

    // Verify subdir's parent is dst/
    let dst_local_id = find_local_id_by_name(&runner, "dst");
    assert_eq!(
        subdir_nodes[0].parent_id,
        Some(dst_local_id),
        "subdir should be under dst/"
    );

    runner.check_convergence().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Atomic save (write-to-temp, delete, rename) ====================

#[test]
fn atomic_save_preserves_remote_identity() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Setup root + remote file
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
            name: "report.odt".to_string(),
            content: b"original content".to_vec(),
        })
        .unwrap();

    // Sync so the file is downloaded locally
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Find the local ID for the synced file
    let local_id = runner.remote_to_local.get(&file_id).cloned().unwrap();

    // Atomic save: same path, new inode, new content
    runner
        .apply(SimAction::LocalAtomicSave {
            local_id,
            content: b"updated content".to_vec(),
        })
        .unwrap();

    // Sync — should upload update, NOT delete + re-create
    runner.apply(SimAction::Sync).unwrap();

    // Remote file must still have the same ID (preserving sharing, etc.)
    assert!(
        runner.remote.get_node(&file_id).is_some(),
        "Remote file must keep its original ID"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn atomic_save_same_content_converges() {
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
            name: "report.odt".to_string(),
            content: b"same content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let local_id = runner.remote_to_local.get(&file_id).cloned().unwrap();

    // Atomic save with identical content (e.g. user saved without editing)
    runner
        .apply(SimAction::LocalAtomicSave {
            local_id,
            content: b"same content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    assert!(
        runner.remote.get_node(&file_id).is_some(),
        "Remote file must keep its original ID"
    );
    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Replace file with file (delete + create at same path) ====================

/// Local side: delete 'file' then create a new 'file' with different content.
/// The new file has a different inode so it must not be confused with an update.
#[test]
fn replace_file_with_file_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create file on remote and sync
    let file_remote_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id,
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let old_local_id = runner
        .remote_to_local
        .get(&file_remote_id)
        .cloned()
        .unwrap();
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Delete the old file locally
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: old_local_id,
        })
        .unwrap();

    // Create a new file at the same path with different content (new inode)
    let new_local_id = LocalFileId::new(1, 99_001);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_local_id,
            parent_local_id: Some(root_local_id),
            name: "file".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // Exactly one 'file' locally
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    // Exactly one 'file' on remote (not trashed)
    let remote_files: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(remote_files.len(), 1, "Exactly one 'file' on remote");

    // Content should be the new content
    let remote_file = remote_files[0];
    let remote_content = runner.remote.get_content(&remote_file.id);
    assert_eq!(
        remote_content,
        Some(&b"new content".to_vec()),
        "Remote should have the new content"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

/// Local side while stopped: delete + create at same path happen before sync resumes.
#[test]
fn replace_file_with_file_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    let file_remote_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id,
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let old_local_id = runner
        .remote_to_local
        .get(&file_remote_id)
        .cloned()
        .unwrap();
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Stop client, perform delete + create while stopped
    runner.apply(SimAction::StopClient).unwrap();

    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: old_local_id,
        })
        .unwrap();

    let new_local_id = LocalFileId::new(1, 99_002);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_local_id,
            parent_local_id: Some(root_local_id),
            name: "file".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

/// Remote side: delete file on remote and create a new one at the same path.
#[test]
fn replace_file_with_file_remote() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    let file_remote_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id.clone(),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Delete old file on remote
    runner
        .apply(SimAction::RemoteDeleteFile { id: file_remote_id })
        .unwrap();

    // Create a new file at the same path with different content (new remote ID)
    let new_remote_id = RemoteId::new("file-2");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: new_remote_id,
            parent_id: root_id,
            name: "file".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    // Verify content is updated
    let local_file = local_files[0];
    let local_content = runner.local_fs.read_file(&local_file.id);
    assert_eq!(
        local_content,
        Some(&b"new content".to_vec()),
        "Local should have the new content"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

/// Remote side with trash: trash file on remote then create a new one at the same path.
#[test]
fn replace_file_with_file_remote_trash() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    let file_remote_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id.clone(),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Trash old file on remote (like a user would via the Cozy UI)
    runner
        .apply(SimAction::RemoteTrash { id: file_remote_id })
        .unwrap();

    // Create a new file at the same path
    let new_remote_id = RemoteId::new("file-2");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: new_remote_id,
            parent_id: root_id,
            name: "file".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

/// Both sides: local replaces file AND remote replaces file concurrently.
/// Both create new identities at the same path with different content.
/// This should be detected as a conflict.
#[test]
fn replace_file_with_file_both_sides() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    let file_remote_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id.clone(),
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let old_local_id = runner
        .remote_to_local
        .get(&file_remote_id)
        .cloned()
        .unwrap();
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Stop client so both sides change independently
    runner.apply(SimAction::StopClient).unwrap();

    // Local: delete + create with "local new content"
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: old_local_id,
        })
        .unwrap();
    let new_local_id = LocalFileId::new(1, 99_003);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_local_id,
            parent_local_id: Some(root_local_id),
            name: "file".to_string(),
            content: b"local new content".to_vec(),
        })
        .unwrap();

    // Remote: delete + create with "remote new content"
    runner
        .apply(SimAction::RemoteDeleteFile { id: file_remote_id })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-2"),
            parent_id: root_id,
            name: "file".to_string(),
            content: b"remote new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Both versions should be present (one possibly with a conflict name)
    // or resolved in some way — at minimum, convergence must hold
    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

/// Replace file in a subdirectory (not root) — ensures parent resolution works.
#[test]
fn replace_file_with_file_in_subdir_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Create a subdirectory
    let subdir_remote_id = RemoteId::new("dir-sub");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: subdir_remote_id.clone(),
            parent_id: Some(root_id),
            name: "subdir".to_string(),
        })
        .unwrap();

    // Create a file inside the subdirectory
    let file_remote_id = RemoteId::new("file-1");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: subdir_remote_id,
            name: "file".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let old_local_id = runner
        .remote_to_local
        .get(&file_remote_id)
        .cloned()
        .unwrap();
    let subdir_local_id = find_local_id_by_name(&runner, "subdir");

    // Delete the old file locally
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: old_local_id,
        })
        .unwrap();

    // Create a new file at the same path
    let new_local_id = LocalFileId::new(1, 99_004);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_local_id,
            parent_local_id: Some(subdir_local_id),
            name: "file".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file")
        .collect();
    assert_eq!(local_files.len(), 1, "Exactly one 'file' locally");

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Application save patterns (save-through-temp-file) ====================

/// LibreOffice save pattern: rename file.ods → file.ods.osl-tmp, create new
/// file.ods, update file.ods with new content, delete file.ods.osl-tmp.
/// The remote identity of file.ods must be preserved (update, not delete+create).
#[test]
fn ods_update_through_osl_tmp_file() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create file.ods on remote and sync
    let file_remote_id = RemoteId::new("file-ods");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id,
            name: "file.ods".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_local_id = find_local_id_by_name(&runner, "file.ods");

    // Stop client — all changes happen while stopped (single scan after restart)
    runner.apply(SimAction::StopClient).unwrap();

    // Step 1: rename file.ods → file.ods.osl-tmp
    runner
        .apply(SimAction::LocalMove {
            local_id: file_local_id.clone(),
            new_parent_local_id: None, // keep same parent
            new_name: "file.ods.osl-tmp".to_string(),
        })
        .unwrap();

    // Step 2: create new file.ods (new inode)
    let new_file_local_id = LocalFileId::new(1, 50_000);
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .expect("root dir must exist");
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "file.ods".to_string(),
            content: b"temporary content".to_vec(),
        })
        .unwrap();

    // Step 3: update file.ods with final content
    runner
        .apply(SimAction::LocalModifyFile {
            local_id: new_file_local_id.clone(),
            content: b"updated content #1".to_vec(),
        })
        .unwrap();

    // Step 4: delete the temp file (file.ods.osl-tmp)
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: file_local_id,
        })
        .unwrap();

    // Restart and sync
    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // file.ods must exist locally with updated content
    let local_ods: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file.ods")
        .collect();
    assert_eq!(local_ods.len(), 1, "Exactly one file.ods locally");
    let ods_content = runner.local_fs.read_file(&new_file_local_id);
    assert_eq!(
        ods_content,
        Some(&b"updated content #1".to_vec()),
        "file.ods must have updated content"
    );

    // No temp file should remain locally
    let tmp_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name.contains("osl-tmp"))
        .collect();
    assert!(
        tmp_files.is_empty(),
        "No .osl-tmp file should remain locally"
    );

    // No temp file on remote either (nothing in trash)
    let remote_tmp: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name.contains("osl-tmp"))
        .collect();
    assert!(
        remote_tmp.is_empty(),
        "No .osl-tmp file should exist on remote"
    );

    // Remote must have exactly one file.ods
    let remote_ods: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "file.ods")
        .collect();
    assert_eq!(remote_ods.len(), 1, "Exactly one file.ods on remote");

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

/// Generic save-through-rename pattern: rename file.ods → other-file.ods,
/// create new file.ods, update file.ods, delete other-file.ods, update file.ods
/// again. Expected: file.ods with final content, nothing else.
#[test]
fn update_through_unignored_tmp_file() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create file.ods on remote and sync
    let file_remote_id = RemoteId::new("file-ods");
    runner
        .apply(SimAction::RemoteCreateFile {
            id: file_remote_id.clone(),
            parent_id: root_id,
            name: "file.ods".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_local_id = find_local_id_by_name(&runner, "file.ods");

    // Stop client — all changes happen while stopped
    runner.apply(SimAction::StopClient).unwrap();

    // Step 1: rename file.ods → other-file.ods
    runner
        .apply(SimAction::LocalMove {
            local_id: file_local_id.clone(),
            new_parent_local_id: None,
            new_name: "other-file.ods".to_string(),
        })
        .unwrap();

    // Step 2: create new file.ods (new inode)
    let new_file_local_id = LocalFileId::new(1, 50_001);
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .expect("root dir must exist");
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_file_local_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "file.ods".to_string(),
            content: b"temporary content".to_vec(),
        })
        .unwrap();

    // Step 3: update file.ods with content #1
    runner
        .apply(SimAction::LocalModifyFile {
            local_id: new_file_local_id.clone(),
            content: b"updated content #1".to_vec(),
        })
        .unwrap();

    // Step 4: delete other-file.ods (the original renamed away)
    runner
        .apply(SimAction::LocalDeleteFile {
            local_id: file_local_id,
        })
        .unwrap();

    // Step 5: update file.ods again with content #2
    runner
        .apply(SimAction::LocalModifyFile {
            local_id: new_file_local_id.clone(),
            content: b"updated content #2".to_vec(),
        })
        .unwrap();

    // Restart and sync
    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // file.ods must exist locally with final content
    let local_ods: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "file.ods")
        .collect();
    assert_eq!(local_ods.len(), 1, "Exactly one file.ods locally");
    let ods_content = runner.local_fs.read_file(&new_file_local_id);
    assert_eq!(
        ods_content,
        Some(&b"updated content #2".to_vec()),
        "file.ods must have final content"
    );

    // No other-file.ods should remain
    let other_local: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "other-file.ods")
        .collect();
    assert!(
        other_local.is_empty(),
        "No other-file.ods should remain locally"
    );

    let other_remote: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "other-file.ods")
        .collect();
    assert!(
        other_remote.is_empty(),
        "No other-file.ods should exist on remote"
    );

    // Remote must have exactly one file.ods
    let remote_ods: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| n.name == "file.ods")
        .collect();
    assert_eq!(remote_ods.len(), 1, "Exactly one file.ods on remote");

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

// ==================== Move + create at old path ====================

// -- move_a_to_b_and_create_a (file version): move file a→b, create new file "a" --

#[test]
fn move_a_to_b_and_create_a_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create file "a" on remote and sync
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: root_id,
            name: "a".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Local actions: move a→b, then create new file "a"
    let file_a_local_id = find_local_id_by_name(&runner, "a");
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: file_a_local_id,
            new_parent_local_id: Some(root_local_id.clone()),
            new_name: "b".to_string(),
        })
        .unwrap();

    let new_a_local_id = LocalFileId::new(1, 9990);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_a_local_id,
            parent_local_id: Some(root_local_id),
            name: "a".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Both "a" and "b" should exist locally
    let local_names: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(
        local_names.contains(&"a".to_string()),
        "File 'a' must exist locally"
    );
    assert!(
        local_names.contains(&"b".to_string()),
        "File 'b' must exist locally"
    );

    // Both should exist on remote
    let remote_names: Vec<_> = runner
        .remote
        .nodes
        .values()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(
        remote_names.contains(&"a".to_string()),
        "File 'a' must exist on remote"
    );
    assert!(
        remote_names.contains(&"b".to_string()),
        "File 'b' must exist on remote"
    );

    // Check contents: "b" has initial content, "a" has new content
    let remote_b = runner
        .remote
        .nodes
        .values()
        .find(|n| n.name == "b")
        .unwrap();
    let remote_a = runner
        .remote
        .nodes
        .values()
        .find(|n| n.name == "a")
        .unwrap();
    let content_b = runner.remote.get_content(&remote_b.id).unwrap();
    let content_a = runner.remote.get_content(&remote_a.id).unwrap();
    assert_eq!(
        content_b, b"initial content",
        "'b' should have original content"
    );
    assert_eq!(content_a, b"new content", "'a' should have new content");

    // Original remote ID must be preserved for "b" (the moved file)
    assert!(
        runner.remote.get_node(&RemoteId::new("file-a")).is_some(),
        "Original remote node must still exist (as 'b')"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_a_to_b_and_create_a_local_while_stopped() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: root_id,
            name: "a".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    let file_a_local_id = find_local_id_by_name(&runner, "a");
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    // Stop client, perform both operations while stopped
    runner.apply(SimAction::StopClient).unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: file_a_local_id,
            new_parent_local_id: Some(root_local_id.clone()),
            new_name: "b".to_string(),
        })
        .unwrap();

    let new_a_local_id = LocalFileId::new(1, 9991);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_a_local_id,
            parent_local_id: Some(root_local_id),
            name: "a".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::RestartClient).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    let local_names: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(
        local_names.contains(&"a".to_string()),
        "File 'a' must exist locally"
    );
    assert!(
        local_names.contains(&"b".to_string()),
        "File 'b' must exist locally"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_a_to_b_and_create_a_remote() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create file "a" on remote and sync
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a"),
            parent_id: root_id.clone(),
            name: "a".to_string(),
            content: b"initial content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Remote actions: move a→b, then create new file "a"
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("file-a"),
            new_parent_id: root_id.clone(),
            new_name: "b".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-new-a"),
            parent_id: root_id,
            name: "a".to_string(),
            content: b"new content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Both "a" and "b" should exist locally
    let local_names: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(
        local_names.contains(&"a".to_string()),
        "File 'a' must exist locally"
    );
    assert!(
        local_names.contains(&"b".to_string()),
        "File 'b' must exist locally"
    );

    // Check local contents via md5
    let local_b = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "b")
        .unwrap();
    let local_a = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name == "a")
        .unwrap();
    let content_b = runner.local_fs.read_file(&local_b.id).unwrap();
    let content_a = runner.local_fs.read_file(&local_a.id).unwrap();
    assert_eq!(
        content_b.as_slice(),
        b"initial content",
        "'b' should have original content"
    );
    assert_eq!(
        content_a.as_slice(),
        b"new content",
        "'a' should have new content"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

// -- move_dir_a_to_b_and_create_a (directory version): move dir a→b, create new dir "a" with contents --

// TODO: the planner incorrectly triggers a NameCollision on the new local dir "a"
// because the remote still has the old dir "a" (not yet moved to "b"). The collision
// resolver deletes the new local "a" tree, losing the newly created files. The fix
// needs the planner to recognize that the old remote "a" is the same identity as the
// locally-moved "b" (tracked by LocalFileId), so the new local "a" is truly new.
#[test]
#[ignore = "planner confuses local move+create with name collision (dir version)"]
fn move_dir_a_to_b_and_create_a_local() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create dir "a" with file "a/file.txt" and subdir "a/subdir/child.txt" on remote
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-a"),
            parent_id: Some(root_id.clone()),
            name: "a".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a-txt"),
            parent_id: RemoteId::new("dir-a"),
            name: "file.txt".to_string(),
            content: b"initial file content".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-a-subdir"),
            parent_id: Some(RemoteId::new("dir-a")),
            name: "subdir".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a-child"),
            parent_id: RemoteId::new("dir-a-subdir"),
            name: "child.txt".to_string(),
            content: b"initial child content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Local actions: move dir a→b
    let dir_a_local_id = find_local_id_by_name(&runner, "a");
    let root_local_id = runner
        .local_fs
        .list_all()
        .into_iter()
        .find(|n| n.name.is_empty())
        .map(|n| n.id.clone())
        .unwrap();

    runner
        .apply(SimAction::LocalMove {
            local_id: dir_a_local_id,
            new_parent_local_id: Some(root_local_id.clone()),
            new_name: "b".to_string(),
        })
        .unwrap();

    // Create new dir "a" with new contents
    let new_dir_a_id = LocalFileId::new(1, 9980);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: new_dir_a_id.clone(),
            parent_local_id: Some(root_local_id.clone()),
            name: "a".to_string(),
        })
        .unwrap();

    let new_file_a_txt_id = LocalFileId::new(1, 9981);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_file_a_txt_id,
            parent_local_id: Some(new_dir_a_id.clone()),
            name: "file.txt".to_string(),
            content: b"new file content".to_vec(),
        })
        .unwrap();

    let new_subdir_id = LocalFileId::new(1, 9982);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: new_subdir_id.clone(),
            parent_local_id: Some(new_dir_a_id),
            name: "subdir".to_string(),
        })
        .unwrap();

    let new_child_id = LocalFileId::new(1, 9983);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: new_child_id,
            parent_local_id: Some(new_subdir_id),
            name: "child.txt".to_string(),
            content: b"new child content".to_vec(),
        })
        .unwrap();

    // May need multiple sync rounds: move is processed first, then new dirs,
    // then files inside new dirs need parent dirs to exist on remote.
    runner.apply(SimAction::Sync).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    // Both trees should exist
    let local_dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.node_type == NodeType::Directory && !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(
        local_dirs.contains(&"a".to_string()),
        "Dir 'a' must exist locally"
    );
    assert!(
        local_dirs.contains(&"b".to_string()),
        "Dir 'b' must exist locally"
    );

    // Check file counts: should have file.txt and child.txt in both a/ and b/
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.node_type == NodeType::File)
        .map(|n| n.name.clone())
        .collect();
    let file_txt_count = local_files.iter().filter(|n| *n == "file.txt").count();
    let child_txt_count = local_files.iter().filter(|n| *n == "child.txt").count();
    assert_eq!(
        file_txt_count, 2,
        "Should have two 'file.txt' (one in a/, one in b/)"
    );
    assert_eq!(
        child_txt_count, 2,
        "Should have two 'child.txt' (one in a/subdir/, one in b/subdir/)"
    );

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
}

#[test]
fn move_dir_a_to_b_and_create_a_remote() {
    let dir = tempdir().unwrap();
    let (mut runner, root_id) = setup_runner_with_root(dir.path());

    // Init: create dir "a" with file and subdir on remote, sync
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-a"),
            parent_id: Some(root_id.clone()),
            name: "a".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a-txt"),
            parent_id: RemoteId::new("dir-a"),
            name: "file.txt".to_string(),
            content: b"initial file content".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-a-subdir"),
            parent_id: Some(RemoteId::new("dir-a")),
            name: "subdir".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-a-child"),
            parent_id: RemoteId::new("dir-a-subdir"),
            name: "child.txt".to_string(),
            content: b"initial child content".to_vec(),
        })
        .unwrap();
    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Remote actions: move dir a→b, create new dir "a" with new contents
    runner
        .apply(SimAction::RemoteMove {
            id: RemoteId::new("dir-a"),
            new_parent_id: root_id.clone(),
            new_name: "b".to_string(),
        })
        .unwrap();

    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-new-a"),
            parent_id: Some(root_id.clone()),
            name: "a".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-new-a-txt"),
            parent_id: RemoteId::new("dir-new-a"),
            name: "file.txt".to_string(),
            content: b"new file content".to_vec(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateDir {
            id: RemoteId::new("dir-new-a-subdir"),
            parent_id: Some(RemoteId::new("dir-new-a")),
            name: "subdir".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteCreateFile {
            id: RemoteId::new("file-new-a-child"),
            parent_id: RemoteId::new("dir-new-a-subdir"),
            name: "child.txt".to_string(),
            content: b"new child content".to_vec(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Both trees should exist locally
    let local_dirs: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.node_type == NodeType::Directory && !n.name.is_empty())
        .map(|n| n.name.clone())
        .collect();
    assert!(
        local_dirs.contains(&"a".to_string()),
        "Dir 'a' must exist locally"
    );
    assert!(
        local_dirs.contains(&"b".to_string()),
        "Dir 'b' must exist locally"
    );

    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.node_type == NodeType::File)
        .map(|n| n.name.clone())
        .collect();
    let file_txt_count = local_files.iter().filter(|n| *n == "file.txt").count();
    let child_txt_count = local_files.iter().filter(|n| *n == "child.txt").count();
    assert_eq!(file_txt_count, 2, "Should have two 'file.txt'");
    assert_eq!(child_txt_count, 2, "Should have two 'child.txt'");

    runner.check_convergence().unwrap();
    runner.check_store_consistency().unwrap();
    runner.check_idempotency().unwrap();
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

// ==================== Network/IO Error Simulation Tests ====================

#[test]
fn single_download_failure_recovers_within_sync() {
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
            content: b"hello world".to_vec(),
        })
        .unwrap();

    // A single failure is recovered by the sync loop's internal retry
    runner.apply(SimAction::FailNextDownload).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
}

#[test]
fn single_upload_failure_recovers_within_sync() {
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

    let file_local_id = LocalFileId::new(1, 9999);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_local_id,
            parent_local_id: Some(root_local_id),
            name: "local.txt".to_string(),
            content: b"local content".to_vec(),
        })
        .unwrap();

    // A single failure is recovered by the sync loop's internal retry
    runner.apply(SimAction::FailNextUpload).unwrap();
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
}

#[test]
fn many_failures_exhaust_sync_loop_then_next_sync_recovers() {
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

    // Inject more failures than the sync loop's max_rounds (10), so the
    // first sync cannot complete — it exhausts all retry rounds.
    for _ in 0..12 {
        runner.apply(SimAction::FailNextDownload).unwrap();
    }

    runner.apply(SimAction::Sync).unwrap();

    // File should NOT exist locally (all rounds failed)
    let local_files: Vec<_> = runner
        .local_fs
        .list_all()
        .into_iter()
        .filter(|n| n.name == "test.txt")
        .collect();
    assert_eq!(
        local_files.len(),
        0,
        "File should not be downloaded when all rounds fail"
    );

    // Second sync — remaining 2 failures are consumed, then it succeeds
    runner.apply(SimAction::Sync).unwrap();

    runner.check_convergence().unwrap();
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

// ==================== Cycle Detection in Moves ====================

#[test]
fn remote_cycle_two_dirs_does_not_infinite_loop() {
    // Simulate a CouchDB change feed that creates a parent cycle:
    // dir_a's parent is dir_b, and dir_b's parent is dir_a.
    // The planner must detect the cycle and not infinite-loop.
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store, dir.path().join("sync"));

    // Create root and two dirs on remote, sync them
    let root_id = RemoteId::new("io.cozy.files.root-dir");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: root_id.clone(),
            parent_id: None,
            name: String::new(),
        })
        .unwrap();

    let dir_a = RemoteId::new("dir-a");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: dir_a.clone(),
            parent_id: Some(root_id.clone()),
            name: "alpha".to_string(),
        })
        .unwrap();

    let dir_b = RemoteId::new("dir-b");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: dir_b.clone(),
            parent_id: Some(root_id.clone()),
            name: "beta".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Now create a cycle: move dir_a under dir_b, then dir_b under dir_a
    runner
        .apply(SimAction::RemoteMove {
            id: dir_a.clone(),
            new_parent_id: dir_b.clone(),
            new_name: "alpha".to_string(),
        })
        .unwrap();
    runner
        .apply(SimAction::RemoteMove {
            id: dir_b.clone(),
            new_parent_id: dir_a.clone(),
            new_name: "beta".to_string(),
        })
        .unwrap();

    // Sync must not infinite-loop — it should complete
    runner.apply(SimAction::Sync).unwrap();
}

#[test]
fn remote_cycle_self_parent_does_not_infinite_loop() {
    // A directory whose parent_id points to itself
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

    let dir_a = RemoteId::new("dir-a");
    runner
        .apply(SimAction::RemoteCreateDir {
            id: dir_a.clone(),
            parent_id: Some(root_id.clone()),
            name: "alpha".to_string(),
        })
        .unwrap();

    runner.apply(SimAction::Sync).unwrap();
    runner.check_convergence().unwrap();

    // Move dir_a to be its own parent (self-cycle)
    runner
        .apply(SimAction::RemoteMove {
            id: dir_a.clone(),
            new_parent_id: dir_a.clone(),
            new_name: "alpha".to_string(),
        })
        .unwrap();

    // Sync must not infinite-loop
    runner.apply(SimAction::Sync).unwrap();
}

#[test]
fn local_file_and_dir_same_name_converges() {
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

    // Create a local file "📊 report.pdf"
    let file_id = LocalFileId::new(1, 50_001);
    runner
        .apply(SimAction::LocalCreateFile {
            local_id: file_id.clone(),
            parent_local_id: Some(root_local_id.clone()),
            name: "📊 report.pdf".to_string(),
            content: vec![0],
        })
        .unwrap();

    // Create a local dir with the SAME name "📊 report.pdf"
    let dir_id = LocalFileId::new(1, 50_002);
    runner
        .apply(SimAction::LocalCreateDir {
            local_id: dir_id.clone(),
            parent_local_id: Some(root_local_id),
            name: "📊 report.pdf".to_string(),
        })
        .unwrap();

    // Sync uploads both (file + dir) to remote
    runner.apply(SimAction::Sync).unwrap();

    // Stop client
    runner.apply(SimAction::StopClient).unwrap();

    // Atomic save on the file (changes inode) — not recorded in store (stopped)
    runner
        .apply(SimAction::LocalAtomicSave {
            local_id: file_id,
            content: vec![105, 213, 174],
        })
        .unwrap();

    // Delete the dir locally — not recorded in store (stopped)
    runner
        .apply(SimAction::LocalDeleteFile { local_id: dir_id })
        .unwrap();

    // Restart reconciles local_fs with store, then sync
    runner.apply(SimAction::RestartClient).unwrap();
    for _ in 0..3 {
        runner.apply(SimAction::Sync).unwrap();
    }

    runner.check_convergence().unwrap();
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
    /// Saved snapshot for rollback
    snapshot: Option<Box<SimState>>,
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
            snapshot: None,
        }
    }

    fn take_snapshot(&mut self) {
        let mut snap = self.clone();
        snap.snapshot = None;
        self.snapshot = Some(Box::new(snap));
    }

    fn rollback(&mut self) {
        if let Some(snap) = self.snapshot.take() {
            *self = *snap;
        }
    }

    /// Remove a remote directory and all its descendants from tracking
    fn remove_remote_tree(&mut self, dir_id: &RemoteId) {
        let mut to_remove = Vec::new();
        let mut stack = vec![dir_id.clone()];
        let mut visited = std::collections::HashSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for id in &self.remote_file_ids {
                if self.remote_parents.get(id) == Some(&current) {
                    to_remove.push(id.clone());
                }
            }
            for id in &self.remote_dir_ids {
                if self.remote_parents.get(id) == Some(&current) {
                    stack.push(id.clone());
                }
            }
            to_remove.push(current);
        }
        for id in &to_remove {
            self.remote_file_ids.retain(|x| x != id);
            self.remote_dir_ids.retain(|x| x != id);
            self.remote_parents.remove(id);
        }
    }

    /// Remove a local directory and all its descendants from tracking
    fn remove_local_tree(&mut self, dir_id: &LocalFileId) {
        let mut to_remove = Vec::new();
        let mut stack = vec![dir_id.clone()];
        let mut visited = std::collections::HashSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for id in &self.local_file_ids {
                if self.local_parents.get(id) == Some(&current) {
                    to_remove.push(id.clone());
                }
            }
            for id in &self.local_dir_ids {
                if self.local_parents.get(id) == Some(&current) {
                    stack.push(id.clone());
                }
            }
            to_remove.push(current);
        }
        for id in &to_remove {
            self.local_file_ids.retain(|x| x != id);
            self.local_dir_ids.retain(|x| x != id);
            self.local_parents.remove(id);
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

    /// Check if moving `dir_id` under `new_parent` would create a cycle
    /// in the local parent chain.
    fn would_create_local_cycle(&self, dir_id: &LocalFileId, new_parent: &LocalFileId) -> bool {
        let mut current = new_parent.clone();
        let mut visited = std::collections::HashSet::new();
        loop {
            if &current == dir_id {
                return true;
            }
            if !visited.insert(current.clone()) {
                return false;
            }
            if let Some(parent) = self.local_parents.get(&current) {
                current = parent.clone();
            } else {
                return false;
            }
        }
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
        parent_idx: usize,
        new_name: String,
    },
    LocalMoveDir {
        idx: usize,
        parent_idx: usize,
        new_name: String,
    },
    LocalDeleteDir {
        idx: usize,
    },
    RemoteDeleteDir {
        idx: usize,
    },
    FailNextDownload,
    FailNextUpload,
    ConcurrentRemoteCreate {
        parent_idx: usize,
        name: String,
        content: Vec<u8>,
    },
    ConcurrentRemoteModify {
        idx: usize,
        content: Vec<u8>,
    },
    ConcurrentRemoteDelete {
        idx: usize,
    },
    ConcurrentRemoteTrash {
        idx: usize,
    },
    RemoteTrashFile {
        idx: usize,
    },
    RemoteTrashDir {
        idx: usize,
    },
    LocalAtomicSave {
        idx: usize,
        content: Vec<u8>,
    },
    SnapshotState,
    RollbackToSnapshot,
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
        (any::<usize>(), any::<usize>(), arbitrary_file_name()).prop_map(
            |(idx, parent_idx, new_name)| ActionChoice::RemoteMoveDir {
                idx,
                parent_idx,
                new_name,
            },
        ),
        (any::<usize>(), any::<usize>(), arbitrary_file_name()).prop_map(
            |(idx, parent_idx, new_name)| ActionChoice::LocalMoveDir {
                idx,
                parent_idx,
                new_name,
            },
        ),
        any::<usize>().prop_map(|idx| ActionChoice::LocalDeleteDir { idx }),
        any::<usize>().prop_map(|idx| ActionChoice::RemoteDeleteDir { idx }),
        Just(ActionChoice::FailNextDownload),
        Just(ActionChoice::FailNextUpload),
        (any::<usize>(), arbitrary_file_name(), arbitrary_content()).prop_map(
            |(parent_idx, name, content)| ActionChoice::ConcurrentRemoteCreate {
                parent_idx,
                name,
                content,
            }
        ),
        (any::<usize>(), arbitrary_content())
            .prop_map(|(idx, content)| ActionChoice::ConcurrentRemoteModify { idx, content }),
        any::<usize>().prop_map(|idx| ActionChoice::ConcurrentRemoteDelete { idx }),
        any::<usize>().prop_map(|idx| ActionChoice::ConcurrentRemoteTrash { idx }),
        any::<usize>().prop_map(|idx| ActionChoice::RemoteTrashFile { idx }),
        any::<usize>().prop_map(|idx| ActionChoice::RemoteTrashDir { idx }),
        (any::<usize>(), arbitrary_content())
            .prop_map(|(idx, content)| ActionChoice::LocalAtomicSave { idx, content }),
        Just(ActionChoice::SnapshotState),
        Just(ActionChoice::RollbackToSnapshot),
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
        ActionChoice::RemoteMoveDir {
            idx,
            parent_idx,
            new_name,
        } => {
            if state.remote_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.remote_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            let new_parent = state.remote_dir_ids[parent_idx % state.remote_dir_ids.len()].clone();
            // Allow cyclic moves — the planner must handle them gracefully
            state.remote_parents.insert(id.clone(), new_parent.clone());
            SimAction::RemoteMove {
                id,
                new_parent_id: new_parent,
                new_name: new_name.clone(),
            }
        }
        ActionChoice::LocalMoveDir {
            idx,
            parent_idx,
            new_name,
        } => {
            if state.local_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.local_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            let new_parent = state.local_dir_ids[parent_idx % state.local_dir_ids.len()].clone();
            // Skip cyclic local moves — the local FS can't have cycles
            if state.would_create_local_cycle(&id, &new_parent) {
                return SimAction::Sync;
            }
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
        ActionChoice::FailNextDownload => SimAction::FailNextDownload,
        ActionChoice::FailNextUpload => SimAction::FailNextUpload,
        ActionChoice::ConcurrentRemoteCreate {
            parent_idx,
            name,
            content,
        } => {
            if state.remote_dir_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.next_remote_id();
            let parent = state.remote_dir_ids[parent_idx % state.remote_dir_ids.len()].clone();
            // Don't track in remote_file_ids — the file won't exist on MockRemote
            // until the concurrent change fires mid-sync.
            SimAction::ConcurrentRemoteChange(ConcurrentRemoteOp::CreateFile {
                id,
                parent_id: parent,
                name: name.clone(),
                content: content.clone(),
            })
        }
        ActionChoice::ConcurrentRemoteModify { idx, content } => {
            if state.remote_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            SimAction::ConcurrentRemoteChange(ConcurrentRemoteOp::ModifyFile {
                id,
                content: content.clone(),
            })
        }
        ActionChoice::ConcurrentRemoteDelete { idx } => {
            if state.remote_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            state.remote_file_ids.retain(|x| x != &id);
            SimAction::ConcurrentRemoteChange(ConcurrentRemoteOp::DeleteFile { id })
        }
        ActionChoice::ConcurrentRemoteTrash { idx } => {
            if state.remote_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            state.remote_file_ids.retain(|x| x != &id);
            SimAction::ConcurrentRemoteChange(ConcurrentRemoteOp::TrashFile { id })
        }
        ActionChoice::RemoteTrashFile { idx } => {
            if state.remote_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.remote_file_ids[idx % state.remote_file_ids.len()].clone();
            state.remote_file_ids.retain(|x| x != &id);
            SimAction::RemoteTrash { id }
        }
        ActionChoice::RemoteTrashDir { idx } => {
            if state.remote_dir_ids.len() <= 1 {
                return SimAction::Sync;
            }
            let non_root: Vec<_> = state.remote_dir_ids[1..].to_vec();
            let id = non_root[idx % non_root.len()].clone();
            state.remove_remote_tree(&id);
            SimAction::RemoteTrash { id }
        }
        ActionChoice::LocalAtomicSave { idx, content } => {
            if state.local_file_ids.is_empty() {
                return SimAction::Sync;
            }
            let id = state.local_file_ids[idx % state.local_file_ids.len()].clone();
            SimAction::LocalAtomicSave {
                local_id: id,
                content: content.clone(),
            }
        }
        ActionChoice::SnapshotState => {
            state.take_snapshot();
            SimAction::SnapshotState
        }
        ActionChoice::RollbackToSnapshot => {
            state.rollback();
            SimAction::RollbackToSnapshot
        }
    }
}
