//! Integration tests that run against a real cozy-stack instance.
//!
//! These tests require a running `cozy-stack serve` and are gated behind
//! `#[ignore = "requires running cozy-stack"]`. Run them with:
//!
//! ```bash
//! cargo test --test integration_tests -- --ignored
//! ```

use std::process::Command;
use super_ragondin_sync::ignore::IgnoreRules;
use super_ragondin_sync::model::{PlanResult, RemoteId, SyncOp};
use super_ragondin_sync::remote::client::CozyClient;
use super_ragondin_sync::store::TreeStore;
use super_ragondin_sync::sync::engine::SyncEngine;
use super_ragondin_sync::util::compute_md5_from_bytes;
use tempfile::TempDir;

/// Test fixture that manages a disposable Cozy instance.
///
/// Creates a fresh instance on construction, tears it down on drop.
/// Uses `cozy-stack` CLI commands for instance and token management.
struct TestCozy {
    domain: String,
    access_token: String,
    sync_dir: TempDir,
    store_dir: TempDir,
    staging_dir: TempDir,
}

impl TestCozy {
    fn setup() -> Self {
        let id = &uuid::Uuid::new_v4().to_string()[..8];
        let domain = format!("test-{id}.localhost:8080");

        // Create instance
        let output = Command::new("cozy-stack")
            .args([
                "instances",
                "add",
                &domain,
                "--passphrase",
                "cozy",
                "--apps",
                "home,drive",
                "--email",
                "test@cozy.localhost",
                "--public-name",
                "Test",
            ])
            .output()
            .expect("Failed to run cozy-stack");
        assert!(
            output.status.success(),
            "Failed to create instance: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Register OAuth client
        let output = Command::new("cozy-stack")
            .args([
                "instances",
                "client-oauth",
                &domain,
                "http://localhost/",
                "integration-test",
                "github.com/nono/cozy-desktop-ng",
            ])
            .output()
            .expect("Failed to run cozy-stack");
        assert!(
            output.status.success(),
            "Failed to create OAuth client: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let client_id = String::from_utf8(output.stdout)
            .expect("Invalid UTF-8 in client_id")
            .trim()
            .to_string();

        // Get access token
        let output = Command::new("cozy-stack")
            .args([
                "instances",
                "token-oauth",
                &domain,
                &client_id,
                "io.cozy.files",
            ])
            .output()
            .expect("Failed to run cozy-stack");
        assert!(
            output.status.success(),
            "Failed to get token: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let access_token = String::from_utf8(output.stdout)
            .expect("Invalid UTF-8 in token")
            .trim()
            .to_string();

        Self {
            domain,
            access_token,
            sync_dir: TempDir::new().expect("Failed to create sync dir"),
            store_dir: TempDir::new().expect("Failed to create store dir"),
            staging_dir: TempDir::new().expect("Failed to create staging dir"),
        }
    }

    fn instance_url(&self) -> String {
        format!("http://{}", self.domain)
    }

    fn client(&self) -> CozyClient {
        CozyClient::new(&self.instance_url(), &self.access_token)
    }

    fn engine(&self) -> SyncEngine {
        let store = TreeStore::open(self.store_dir.path()).expect("Failed to open store");
        SyncEngine::new(
            store,
            self.sync_dir.path().to_path_buf(),
            self.staging_dir.path().to_path_buf(),
            IgnoreRules::default_only(),
        )
    }
}

impl Drop for TestCozy {
    fn drop(&mut self) {
        match Command::new("cozy-stack")
            .args(["instances", "rm", "--force", &self.domain])
            .output()
        {
            Ok(output) if !output.status.success() => {
                eprintln!(
                    "Warning: failed to clean up cozy-stack instance `{}`:\n{}",
                    self.domain,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                eprintln!(
                    "Warning: failed to run cozy-stack to clean up instance `{}`: {}",
                    self.domain, e
                );
            }
            _ => {}
        }
    }
}

fn cozy_stack_available() -> bool {
    Command::new("cozy-stack")
        .args(["instances", "ls"])
        .output()
        .is_ok_and(|o| o.status.success())
}

// ==================== Tests ====================

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn test_upload_local_file_to_remote() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    let cozy = TestCozy::setup();
    let client = cozy.client();
    let mut engine = cozy.engine();

    // Create a local file
    std::fs::write(cozy.sync_dir.path().join("hello.txt"), "Hello, Cozy!").unwrap();

    // Fetch remote state, then sync
    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    let results = engine.run_cycle_async(&client).await.unwrap();

    // Should have uploaded the file
    let has_upload = results.iter().any(
        |r| matches!(r, PlanResult::Op(SyncOp::UploadNew { name, .. }) if name == "hello.txt"),
    );
    assert!(has_upload, "Should have uploaded hello.txt");

    // Verify file exists on remote
    let changes = client.fetch_changes(None).await.unwrap();
    let found = changes
        .results
        .iter()
        .any(|r| !r.deleted && r.node.name == "hello.txt");
    assert!(found, "hello.txt should exist on remote after sync");
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn test_download_remote_file_to_local() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    let cozy = TestCozy::setup();
    let client = cozy.client();
    let mut engine = cozy.engine();

    // Upload a file to remote directly
    let content = b"Remote content";
    let md5 = compute_md5_from_bytes(content);
    client
        .upload_file(
            &RemoteId::new("io.cozy.files.root-dir"),
            "from-remote.txt",
            content.to_vec(),
            &md5,
        )
        .await
        .unwrap();

    // Fetch remote state, then sync
    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    let results = engine.run_cycle_async(&client).await.unwrap();

    // Should have downloaded the file
    let has_download = results.iter().any(|r| {
        matches!(r, PlanResult::Op(SyncOp::DownloadNew { local_path, .. })
            if local_path.file_name().and_then(|n| n.to_str()) == Some("from-remote.txt"))
    });
    assert!(has_download, "Should have downloaded from-remote.txt");

    // Verify file exists locally
    let local_path = cozy.sync_dir.path().join("from-remote.txt");
    assert!(local_path.exists(), "from-remote.txt should exist locally");
    assert_eq!(
        std::fs::read_to_string(&local_path).unwrap(),
        "Remote content"
    );
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn test_bidirectional_sync() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    let cozy = TestCozy::setup();
    let client = cozy.client();
    let mut engine = cozy.engine();

    // Create a local file
    std::fs::write(cozy.sync_dir.path().join("local.txt"), "from local").unwrap();

    // Upload a file to remote
    let content = b"from remote";
    let md5 = compute_md5_from_bytes(content);
    client
        .upload_file(
            &RemoteId::new("io.cozy.files.root-dir"),
            "remote.txt",
            content.to_vec(),
            &md5,
        )
        .await
        .unwrap();

    // Sync
    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    let results = engine.run_cycle_async(&client).await.unwrap();

    // Should have both upload and download
    let has_upload = results.iter().any(
        |r| matches!(r, PlanResult::Op(SyncOp::UploadNew { name, .. }) if name == "local.txt"),
    );
    let has_download = results.iter().any(|r| {
        matches!(r, PlanResult::Op(SyncOp::DownloadNew { local_path, .. })
            if local_path.file_name().and_then(|n| n.to_str()) == Some("remote.txt"))
    });
    assert!(has_upload, "Should have uploaded local.txt");
    assert!(has_download, "Should have downloaded remote.txt");

    // Verify both files exist locally
    assert!(cozy.sync_dir.path().join("local.txt").exists());
    assert_eq!(
        std::fs::read_to_string(cozy.sync_dir.path().join("remote.txt")).unwrap(),
        "from remote"
    );
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn test_second_sync_is_noop() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    let cozy = TestCozy::setup();
    let client = cozy.client();
    let mut engine = cozy.engine();

    // Create a local file and sync
    std::fs::write(cozy.sync_dir.path().join("stable.txt"), "no changes").unwrap();

    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    engine.run_cycle_async(&client).await.unwrap();

    // Re-fetch remote and run second cycle
    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    let results = engine.run_cycle_async(&client).await.unwrap();

    let ops: Vec<_> = results
        .iter()
        .filter(|r| matches!(r, PlanResult::Op(_)))
        .collect();
    assert!(
        ops.is_empty(),
        "Second sync should be a no-op, but got: {ops:?}"
    );
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn test_download_default_directories() -> Result<(), Box<dyn std::error::Error>> {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return Ok(());
    }

    let cozy = TestCozy::setup();
    let client = cozy.client();
    let mut engine = cozy.engine();

    // Fetch remote state (instance comes with default dirs)
    engine.fetch_and_apply_remote_changes(&client, None).await?;
    let results = engine.run_cycle_async(&client).await?;

    // Should create the default directories locally
    let created_dirs: Vec<_> = results
        .iter()
        .filter_map(|r| match r {
            PlanResult::Op(SyncOp::CreateLocalDir { local_path, .. }) => local_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from),
            _ => None,
        })
        .collect();

    assert!(
        !created_dirs.is_empty(),
        "Should have created default directories, got ops: {results:?}"
    );

    // Verify at least one default dir exists on disk
    for name in &created_dirs {
        assert!(
            cozy.sync_dir.path().join(name).is_dir(),
            "Directory {name} should exist on disk"
        );
    }

    Ok(())
}

#[tokio::test]
#[ignore = "requires running cozy-stack"]
async fn test_upload_local_directory_and_nested_file() {
    if !cozy_stack_available() {
        eprintln!("Skipping: cozy-stack not available");
        return;
    }

    let cozy = TestCozy::setup();
    let client = cozy.client();
    let mut engine = cozy.engine();

    // Create a local directory with a file inside
    let dir_path = cozy.sync_dir.path().join("docs");
    std::fs::create_dir(&dir_path).unwrap();
    std::fs::write(dir_path.join("note.txt"), "nested content").unwrap();

    // First sync: should create the directory on remote
    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    let results = engine.run_cycle_async(&client).await.unwrap();

    let has_create_dir = results.iter().any(
        |r| matches!(r, PlanResult::Op(SyncOp::CreateRemoteDir { name, .. }) if name == "docs"),
    );
    assert!(has_create_dir, "Should have created 'docs' dir on remote");

    // The nested file may need a second sync if the dir wasn't synced yet
    // when the planner saw it. Re-fetch and re-sync.
    engine
        .fetch_and_apply_remote_changes(&client, None)
        .await
        .unwrap();
    engine.run_cycle_async(&client).await.unwrap();

    // After at most 2 cycles, the nested file should be uploaded
    let all_synced: Vec<_> = engine
        .store()
        .list_all_synced()
        .unwrap()
        .iter()
        .map(|s| s.rel_path.clone())
        .collect();
    assert!(
        all_synced.iter().any(|p| p.contains("note.txt")),
        "note.txt should be synced, synced records: {all_synced:?}"
    );
}
