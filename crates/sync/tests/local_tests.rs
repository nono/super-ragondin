use std::fs;
use std::thread;
use std::time::Duration;
use super_ragondin_sync::ignore::IgnoreRules;
use super_ragondin_sync::local::scanner::Scanner;
use super_ragondin_sync::local::watcher::{WatchEvent, WatchEventKind, Watcher};
use super_ragondin_sync::model::{LocalNode, NodeType};
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
fn test_watcher_detects_file_create() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx, IgnoreRules::none()).unwrap();

    thread::spawn(move || {
        let _ = watcher.run();
    });

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
fn test_watcher_detects_file_delete() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Create file before starting watcher
    fs::write(root.join("to_delete.txt"), b"test").unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx, IgnoreRules::none()).unwrap();

    thread::spawn(move || {
        let _ = watcher.run();
    });

    // Delete the file
    fs::remove_file(root.join("to_delete.txt")).unwrap();

    // Wait for delete event
    let event: WatchEvent = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(event.kind, WatchEventKind::Delete));
}

#[test]
fn test_scan_ignores_hidden_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::write(root.join("visible.txt"), b"hello").unwrap();
    fs::write(root.join(".hidden"), b"secret").unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), b"data").unwrap();
    fs::write(root.join("file.swp"), b"vim temp").unwrap();

    let rules = IgnoreRules::default_only();
    let scanner = Scanner::new(root);
    let nodes = scanner.scan_with_ignore(&rules).unwrap();

    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "visible.txt");
}

#[test]
fn test_scan_ignores_nested_hidden_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    fs::create_dir(root.join("subdir")).unwrap();
    fs::write(root.join("subdir/normal.txt"), b"ok").unwrap();
    fs::write(root.join("subdir/.hidden"), b"secret").unwrap();

    let rules = IgnoreRules::default_only();
    let scanner = Scanner::new(root);
    let nodes = scanner.scan_with_ignore(&rules).unwrap();

    // subdir + normal.txt (but not .hidden)
    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().all(|n| n.name != ".hidden"));
}

#[test]
fn test_watcher_ignores_hidden_file_events() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let rules = IgnoreRules::default_only();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx, rules).unwrap();

    thread::spawn(move || {
        let _ = watcher.run();
    });

    // Create an ignored file — should NOT produce an event
    fs::write(root.join(".hidden"), b"secret").unwrap();
    // Create a normal file — SHOULD produce an event
    fs::write(root.join("visible.txt"), b"hello").unwrap();

    // Collect events for a short window
    let mut events = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_secs(2)) {
        events.push(event);
    }

    // Only visible.txt events should appear
    assert!(
        events
            .iter()
            .all(|e| !e.path.to_string_lossy().contains(".hidden"))
    );
    assert!(
        events
            .iter()
            .any(|e| e.path.to_string_lossy().contains("visible"))
    );
}
