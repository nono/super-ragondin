use cozy_desktop::model::{LocalFileId, LocalNode, NodeType, RemoteId, RemoteNode, SyncedRecord};
use cozy_desktop::store::TreeStore;
use tempfile::tempdir;

#[test]
fn test_remote_node_crud() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let node = RemoteNode {
        id: RemoteId::new("remote-123"),
        parent_id: None,
        name: "docs".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1706886400,
        rev: "1-abc".to_string(),
    };

    store.insert_remote_node(&node).unwrap();
    let retrieved = store.get_remote_node(&node.id).unwrap().unwrap();
    assert_eq!(retrieved.name, "docs");
    assert_eq!(retrieved.rev, "1-abc");

    store.delete_remote_node(&node.id).unwrap();
    assert!(store.get_remote_node(&node.id).unwrap().is_none());
}

#[test]
fn test_local_node_crud() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let node = LocalNode {
        id: LocalFileId::new(1, 100),
        parent_id: None,
        name: "myfile.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc123".to_string()),
        size: Some(1024),
        mtime: 1706886400,
    };

    store.insert_local_node(&node).unwrap();
    let retrieved = store.get_local_node(&node.id).unwrap().unwrap();
    assert_eq!(retrieved.name, "myfile.txt");
    assert_eq!(retrieved.id.device_id, 1);
    assert_eq!(retrieved.id.inode, 100);

    store.delete_local_node(&node.id).unwrap();
    assert!(store.get_local_node(&node.id).unwrap().is_none());
}

#[test]
fn test_synced_record_crud() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let record = SyncedRecord {
        local_id: LocalFileId::new(1, 200),
        remote_id: RemoteId::new("remote-456"),
        rel_path: "docs/report.pdf".to_string(),
        md5sum: Some("def789".to_string()),
        size: Some(2048),
        rev: "2-xyz".to_string(),
        node_type: NodeType::File,
    };

    store.insert_synced(&record).unwrap();

    let by_local = store
        .get_synced_by_local(&record.local_id)
        .unwrap()
        .unwrap();
    assert_eq!(by_local.rel_path, "docs/report.pdf");

    let by_remote = store
        .get_synced_by_remote(&record.remote_id)
        .unwrap()
        .unwrap();
    assert_eq!(by_remote.local_id, record.local_id);

    store.delete_synced(&record.local_id).unwrap();
    assert!(
        store
            .get_synced_by_local(&record.local_id)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_synced_by_remote(&record.remote_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_remote_children_index() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let parent = RemoteNode {
        id: RemoteId::new("parent-dir"),
        parent_id: None,
        name: "docs".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-a".to_string(),
    };

    let child1 = RemoteNode {
        id: RemoteId::new("child-1"),
        parent_id: Some(RemoteId::new("parent-dir")),
        name: "file1.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("hash1".to_string()),
        size: Some(100),
        updated_at: 1001,
        rev: "1-b".to_string(),
    };

    let child2 = RemoteNode {
        id: RemoteId::new("child-2"),
        parent_id: Some(RemoteId::new("parent-dir")),
        name: "file2.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("hash2".to_string()),
        size: Some(200),
        updated_at: 1002,
        rev: "1-c".to_string(),
    };

    store.insert_remote_node(&parent).unwrap();
    store.insert_remote_node(&child1).unwrap();
    store.insert_remote_node(&child2).unwrap();

    let children = store
        .list_remote_children(&RemoteId::new("parent-dir"))
        .unwrap();
    assert_eq!(children.len(), 2);
}

#[test]
fn test_local_children_index() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let parent_id = LocalFileId::new(1, 50);
    let parent = LocalNode {
        id: parent_id.clone(),
        parent_id: None,
        name: "projects".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        mtime: 1000,
    };

    let child1 = LocalNode {
        id: LocalFileId::new(1, 100),
        parent_id: Some(parent_id.clone()),
        name: "readme.md".to_string(),
        node_type: NodeType::File,
        md5sum: Some("aaa".to_string()),
        size: Some(500),
        mtime: 1001,
    };

    let child2 = LocalNode {
        id: LocalFileId::new(1, 101),
        parent_id: Some(parent_id.clone()),
        name: "main.rs".to_string(),
        node_type: NodeType::File,
        md5sum: Some("bbb".to_string()),
        size: Some(1000),
        mtime: 1002,
    };

    store.insert_local_node(&parent).unwrap();
    store.insert_local_node(&child1).unwrap();
    store.insert_local_node(&child2).unwrap();

    let children = store.list_local_children(&parent_id).unwrap();
    assert_eq!(children.len(), 2);
}

#[test]
fn test_list_all_remote() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let node1 = RemoteNode {
        id: RemoteId::new("node1"),
        parent_id: None,
        name: "a.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: "1-a".to_string(),
    };

    let node2 = RemoteNode {
        id: RemoteId::new("node2"),
        parent_id: None,
        name: "b.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        updated_at: 1001,
        rev: "1-b".to_string(),
    };

    store.insert_remote_node(&node1).unwrap();
    store.insert_remote_node(&node2).unwrap();

    let all = store.list_all_remote().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_list_all_local() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let node1 = LocalNode {
        id: LocalFileId::new(1, 100),
        parent_id: None,
        name: "a.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        mtime: 1000,
    };

    let node2 = LocalNode {
        id: LocalFileId::new(1, 101),
        parent_id: None,
        name: "b.txt".to_string(),
        node_type: NodeType::File,
        md5sum: None,
        size: None,
        mtime: 1001,
    };

    store.insert_local_node(&node1).unwrap();
    store.insert_local_node(&node2).unwrap();

    let all = store.list_all_local().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_list_all_synced() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let record1 = SyncedRecord {
        local_id: LocalFileId::new(1, 100),
        remote_id: RemoteId::new("r1"),
        rel_path: "a.txt".to_string(),
        md5sum: None,
        size: None,
        rev: "1-a".to_string(),
        node_type: NodeType::File,
    };

    let record2 = SyncedRecord {
        local_id: LocalFileId::new(1, 101),
        remote_id: RemoteId::new("r2"),
        rel_path: "b.txt".to_string(),
        md5sum: None,
        size: None,
        rev: "1-b".to_string(),
        node_type: NodeType::File,
    };

    store.insert_synced(&record1).unwrap();
    store.insert_synced(&record2).unwrap();

    let all = store.list_all_synced().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_trees_are_independent() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let remote_node = RemoteNode {
        id: RemoteId::new("shared-id"),
        parent_id: None,
        name: "test".to_string(),
        node_type: NodeType::File,
        md5sum: Some("hash".to_string()),
        size: Some(50),
        updated_at: 1000,
        rev: "1-a".to_string(),
    };

    store.insert_remote_node(&remote_node).unwrap();

    assert!(store.get_remote_node(&remote_node.id).unwrap().is_some());
    assert!(store.list_all_local().unwrap().is_empty());
    assert!(store.list_all_synced().unwrap().is_empty());
}
