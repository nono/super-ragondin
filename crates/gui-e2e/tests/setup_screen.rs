use gui_e2e::{
    app_binary_path, compare_or_create_baseline, connect_driver, references_dir, save_screenshot,
    screenshots_dir, start_tauri_driver,
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
async fn setup_screen_renders_correctly() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui` first.",
        path = app_binary.display()
    );

    let _tauri_driver = TauriDriverGuard(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;

    // Wait for the app to load and the Setup screen to appear
    let heading = driver
        .query(By::Css("h1"))
        .wait(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;
    let heading_text = heading.text().await?;
    assert_eq!(heading_text, "Super Ragondin");

    // Verify the 3 input fields are present
    let inputs = driver.find_all(By::Css("input")).await?;
    assert_eq!(
        inputs.len(),
        3,
        "Expected 3 input fields (URL, sync dir, API key)"
    );

    // Verify input placeholders
    let url_placeholder = inputs[0].attr("placeholder").await?;
    assert_eq!(
        url_placeholder.as_deref(),
        Some("https://alice.mycozy.cloud")
    );

    let dir_placeholder = inputs[1].attr("placeholder").await?;
    assert_eq!(dir_placeholder.as_deref(), Some("/home/user/Cozy"));

    let key_placeholder = inputs[2].attr("placeholder").await?;
    assert_eq!(key_placeholder.as_deref(), Some("sk-or-…"));

    // Verify the submit button
    let button = driver.find(By::Css("button[type='submit']")).await?;
    let button_text = button.text().await?;
    assert_eq!(button_text, "Connect to Cozy →");

    // Take screenshot
    let screenshot_path = screenshots_dir().join("setup_screen.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "Screenshot was not saved");

    // Quit driver before comparison so the session closes even when comparison fails
    driver.quit().await?;

    // Visual regression: compare against committed baseline
    compare_or_create_baseline(
        &screenshot_path,
        &references_dir().join("setup_screen.png"),
        1.0, // 1% pixel-diff tolerance
    )
    .map_err(WebDriverError::CustomError)?;

    Ok(())
}
