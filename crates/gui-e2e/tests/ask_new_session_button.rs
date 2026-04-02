use gui_e2e::{
    app_binary_path, compare_or_create_baseline, connect_driver, references_dir, save_screenshot,
    screenshots_dir, start_tauri_driver, ConfigGuard, TauriDriverGuard,
};
use thirtyfour::prelude::*;

#[tokio::test]
#[ignore = "requires built GUI binary and tauri-driver"]
async fn new_session_button_is_visible_and_clickable() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui --no-default-features --features custom-protocol` first.",
        path = app_binary.display()
    );

    let sync_dir = tempfile::tempdir().expect("failed to create temp sync dir");
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");

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

    let _config_guard = ConfigGuard::install(&ready_config);

    let _tauri_driver = TauriDriverGuard::new(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;
    driver.goto("tauri://localhost").await.ok();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for the Ask panel to render.
    driver
        .query(By::Css(".panel-header .title"))
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;

    // Find the New conversation button.
    let new_btn = driver
        .query(By::Css(".new-session-btn"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;

    // Verify it's not disabled.
    let disabled = new_btn.attr("disabled").await?;
    assert!(disabled.is_none(), "button should not be disabled");

    // Click it.
    new_btn.click().await?;

    // Take screenshot.
    let screenshot_path = screenshots_dir().join("ask_new_session_button.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    // Compare against baseline.
    let reference_path = references_dir().join("ask_new_session_button.png");
    if let Err(msg) = compare_or_create_baseline(&screenshot_path, &reference_path, 5.0) {
        if msg.contains("Baseline created") {
            eprintln!("{msg}");
        } else {
            panic!("{msg}");
        }
    }

    driver.quit().await?;
    Ok(())
}
