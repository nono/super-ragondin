use cozy_desktop::local::scanner::Scanner;
use cozy_desktop::local::watcher::{WatchEvent, WatchEventKind, Watcher};
use cozy_desktop::model::{LocalNode, NodeType};
use std::fs;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_scan_directory() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create test structure
    fs::create_dir(root.join("subdir")).unwrap();
    fs::write(root.join("file.txt"), b"hello").unwrap();
    fs::write(root.join("subdir/nested.txt"), b"world").unwrap();

    let scanner = Scanner::new(root);
    let nodes = scanner.scan().unwrap();

    assert_eq!(nodes.len(), 3);

    let file: &LocalNode = nodes.iter().find(|n| n.name == "file.txt").unwrap();
    assert_eq!(file.node_type, NodeType::File);
    assert_eq!(file.size, Some(5));

    let subdir: &LocalNode = nodes.iter().find(|n| n.name == "subdir").unwrap();
    assert_eq!(subdir.node_type, NodeType::Directory);
}

#[test]
#[ignore] // May be flaky in CI due to timing
fn test_watcher_detects_file_create() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx).unwrap();

    thread::spawn(move || {
        let _ = watcher.run();
    });

    // Give watcher time to start
    thread::sleep(Duration::from_millis(100));

    // Create a file
    fs::write(root.join("new_file.txt"), b"test").unwrap();

    // Wait for event
    let event: WatchEvent = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        event.kind,
        WatchEventKind::Create | WatchEventKind::Modify
    ));
}

#[test]
#[ignore] // May be flaky in CI due to timing
fn test_watcher_detects_file_delete() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Create file before starting watcher
    fs::write(root.join("to_delete.txt"), b"test").unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx).unwrap();

    thread::spawn(move || {
        let _ = watcher.run();
    });

    // Give watcher time to start
    thread::sleep(Duration::from_millis(100));

    // Delete the file
    fs::remove_file(root.join("to_delete.txt")).unwrap();

    // Wait for delete event
    let event: WatchEvent = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(event.kind, WatchEventKind::Delete));
}
