use gui_e2e::{
    app_binary_path, connect_driver, save_screenshot, screenshots_dir, start_tauri_driver,
    ConfigGuard, TauriDriverGuard,
};
use thirtyfour::prelude::*;

#[tokio::test]
#[ignore = "requires built GUI binary and tauri-driver"]
async fn setup_form_submits_and_transitions_to_auth() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui --no-default-features --features custom-protocol` first.",
        path = app_binary.display()
    );

    // Bind a TCP listener on a random port that never responds.
    // After form submission, the app transitions to Auth and tries OAuth registration
    // against the instance URL — this keeps the Auth screen in "waiting" state.
    let hang_listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| WebDriverError::FatalError(e.to_string()))?;
    let hang_port = hang_listener
        .local_addr()
        .map_err(|e| WebDriverError::FatalError(e.to_string()))?
        .port();

    // Ensure no config exists so the app starts in Unconfigured state.
    // ConfigGuard saves/restores any existing config on drop.
    let _config_guard = ConfigGuard::remove();

    let sync_dir = tempfile::tempdir().expect("failed to create temp sync dir");

    let _tauri_driver = TauriDriverGuard::new(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;
    driver.goto("tauri://localhost").await.ok();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for the Setup screen to appear.
    let heading = driver
        .query(By::Css("h1"))
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;
    assert_eq!(heading.text().await?, "Super Ragondin");

    // Fill in the form fields.
    let inputs = driver.find_all(By::Css("input")).await?;
    assert_eq!(
        inputs.len(),
        3,
        "Expected 3 input fields (URL, sync dir, API key)"
    );

    // Instance URL
    inputs[0]
        .send_keys(format!("http://127.0.0.1:{hang_port}"))
        .await?;
    // Sync directory
    inputs[1]
        .send_keys(sync_dir.path().to_string_lossy().to_string())
        .await?;
    // API key (optional, leave empty)

    // Click the submit button.
    let submit_btn = driver.find(By::Css("button[type='submit']")).await?;
    submit_btn.click().await?;

    // Wait for the Auth screen to appear (heading changes to "Connecting to Cozy").
    let auth_heading = driver
        .query(By::Css("h1"))
        .with_text("Connecting to Cozy")
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;
    assert_eq!(auth_heading.text().await?, "Connecting to Cozy");

    // Verify the "Waiting for authorization" message is visible.
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
        "expected 'Waiting for authorization' paragraph after form submission, got: {texts:?}"
    );

    // Take screenshot of the post-submission Auth screen.
    let screenshot_path = screenshots_dir().join("setup_form_interaction.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    driver.quit().await?;

    // Keep hang_listener alive until after quit.
    drop(hang_listener);

    Ok(())
}
