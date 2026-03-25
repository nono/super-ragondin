use gui_e2e::{
    app_binary_path, compare_or_create_baseline, connect_driver, references_dir, save_screenshot,
    screenshots_dir, start_tauri_driver, ConfigGuard,
};
use thirtyfour::prelude::*;

/// RAII guard that kills and waits on the `tauri-driver` child process.
struct TauriDriverGuard(std::process::Child);

impl Drop for TauriDriverGuard {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

#[tokio::test]
#[ignore = "requires built GUI binary and tauri-driver"]
async fn main_layout_screen_renders_correctly() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui --no-default-features --features custom-protocol` first.",
        path = app_binary.display()
    );

    // Use unique temp directories per test run to avoid state leakage between runs.
    // TempDir cleans up on drop; keep the handles alive for the duration of the test.
    let sync_dir = tempfile::tempdir().expect("failed to create temp sync dir");
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");

    // Config JSON for the Ready state: fake OAuth access token + fake API key.
    //
    // - `app_state_from_config` returns `Ready` (access_token is set).
    // - `getSuggestions()` opens an empty LanceDB store → returns "NoFilesIndexed" →
    //   AskPanel shows idle state with "No files indexed yet" hint and the ask input.
    // - `startSync()` fails on the first network request but handles the error gracefully.
    // - `getRecentFiles()` returns an empty list (empty store) → SyncPanel shows "No files yet".
    let ready_config = format!(
        r#"{{
  "instance_url": "https://alice.mycozy.cloud",
  "sync_dir": "{}",
  "data_dir": "{}",
  "api_key": "fake-openrouter-key",
  "oauth_client": {{
    "instance_url": "https://alice.mycozy.cloud",
    "client_id": "fake-client-id",
    "client_secret": "fake-client-secret",
    "registration_access_token": "fake-reg-token",
    "access_token": "fake-access-token",
    "refresh_token": null
  }},
  "last_seq": null
}}"#,
        sync_dir.path().display(),
        data_dir.path().display()
    );

    // ConfigGuard restores the original config on drop.
    // Declare before TauriDriverGuard so it is dropped after the app is killed.
    let _config_guard = ConfigGuard::install(&ready_config);

    let _tauri_driver = TauriDriverGuard(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;

    // Navigate to the Tauri app URL to ensure a clean load with Tauri's JS bridge injected.
    driver.goto("tauri://localhost").await.ok();

    // Give Tauri time to inject its JS bridge and let Svelte render.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for the Ask panel header to confirm the MainLayout has rendered.
    let ask_title = driver
        .query(By::Css(".panel-header .title"))
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;
    let ask_title_text = ask_title.text().await?;
    assert_eq!(ask_title_text, "Ask");

    // Wait for the ask input row: getSuggestions returns NoFilesIndexed (empty LanceDB store),
    // so AskPanel is in idle state with the input visible.
    driver
        .query(By::Css(".input-row input"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;

    // Verify the idle hint text (no suggestions, no error).
    let hint = driver
        .query(By::Css(".panel-body .hint"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;
    let hint_text = hint.text().await?;
    assert!(
        hint_text.contains("No files indexed"),
        "expected no-files-indexed hint, got: {hint_text:?}"
    );

    // Wait for the sync status badge to show "Up to date" (sync cycle fails fast on fake token).
    let status_label = driver
        .query(By::Css(".status-badge .label"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;
    let status_text = status_label.text().await?;
    assert_eq!(
        status_text, "Up to date",
        "sync status badge should show 'Up to date' after sync cycle fails"
    );

    // Verify the SyncPanel shows "No files yet" (empty store, no synced files).
    let empty_msg = driver
        .query(By::Css(".empty"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;
    let empty_text = empty_msg.text().await?;
    assert_eq!(empty_text, "No files yet");

    // Take screenshot.
    let screenshot_path = screenshots_dir().join("main_layout_screen.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    // Quit driver before comparison so the session closes even if comparison fails.
    driver.quit().await?;

    // Visual regression: compare against committed baseline.
    compare_or_create_baseline(
        &screenshot_path,
        &references_dir().join("main_layout_screen.png"),
        1.0, // 1% pixel-diff tolerance
    )
    .map_err(WebDriverError::FatalError)?;

    Ok(())
}
