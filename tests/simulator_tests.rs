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

        // Check convergence
        runner.check_convergence().unwrap();
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

        // Check convergence
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

        // Check convergence
        runner.check_convergence().unwrap();
    }
}
