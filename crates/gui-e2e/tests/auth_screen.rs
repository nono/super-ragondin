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
async fn auth_screen_renders_correctly() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui --no-default-features --features custom-protocol` first.",
        path = app_binary.display()
    );

    // Bind a TCP listener on a random port and never respond to it.
    // The app's OAuth registration request will connect but receive no HTTP response,
    // keeping the Auth screen in the "waiting for authorization" state throughout the test.
    let hang_listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| WebDriverError::FatalError(e.to_string()))?;
    let hang_port = hang_listener
        .local_addr()
        .map_err(|e| WebDriverError::FatalError(e.to_string()))?
        .port();

    let config_json = format!(
        r#"{{
  "instance_url": "http://127.0.0.1:{hang_port}",
  "sync_dir": "/tmp/sr-e2e-sync",
  "data_dir": "/tmp/sr-e2e-data",
  "oauth_client": null,
  "last_seq": null
}}"#
    );

    // ConfigGuard restores the original config on drop.
    // Declare before TauriDriverGuard so it is dropped after the app is killed.
    let _config_guard = ConfigGuard::install(&config_json);

    let _tauri_driver = TauriDriverGuard(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;

    // Navigate to the Tauri app URL to ensure a clean load with Tauri's JS bridge injected.
    driver.goto("tauri://localhost").await.ok();

    // Give Tauri time to inject its JS bridge and let Svelte render.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for the Auth screen heading.
    let heading = driver
        .query(By::Css("h1"))
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;
    let heading_text = heading.text().await?;
    assert_eq!(heading_text, "Connecting to Cozy");

    // Verify the waiting-state paragraphs are visible (not the error state).
    let paragraphs = driver.find_all(By::Css("p")).await?;
    let texts: Vec<String> = {
        let mut v = Vec::new();
        for p in &paragraphs {
            v.push(p.text().await?);
        }
        v
    };
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Waiting for authorization")),
        "expected 'Waiting for authorization' paragraph, got: {texts:?}"
    );
    assert!(
        texts.iter().all(|t| !t.contains("Retry")),
        "error state (Retry) should not be visible yet, got: {texts:?}"
    );

    // Take screenshot.
    let screenshot_path = screenshots_dir().join("auth_screen.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    // Quit driver before comparison so the session closes even if comparison fails.
    driver.quit().await?;

    // Keep hang_listener alive until after quit so the auth flow stays suspended.
    drop(hang_listener);

    // Visual regression: compare against committed baseline.
    compare_or_create_baseline(
        &screenshot_path,
        &references_dir().join("auth_screen.png"),
        1.0, // 1% pixel-diff tolerance
    )
    .map_err(WebDriverError::FatalError)?;

    Ok(())
}
