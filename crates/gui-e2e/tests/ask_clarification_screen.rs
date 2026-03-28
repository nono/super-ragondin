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
async fn ask_clarification_screen_renders_correctly() -> WebDriverResult<()> {
    let app_binary = app_binary_path();
    assert!(
        app_binary.exists(),
        "App binary not found at {path}. Run `cargo build -p super-ragondin-gui --no-default-features --features custom-protocol` first.",
        path = app_binary.display()
    );

    let sync_dir = tempfile::tempdir().expect("failed to create temp sync dir");
    let data_dir = tempfile::tempdir().expect("failed to create temp data dir");

    // Config for Ready state with fake tokens (same pattern as main_layout_screen).
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

    let _tauri_driver = TauriDriverGuard(start_tauri_driver().expect(
        "Failed to start tauri-driver. Is it installed? (`cargo install tauri-driver --locked`)",
    ));

    let driver = connect_driver(&app_binary).await?;
    driver.goto("tauri://localhost").await.ok();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Wait for the Ask panel to confirm MainLayout has rendered.
    driver
        .query(By::Css(".panel-header .title"))
        .wait(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_millis(500),
        )
        .first()
        .await?;

    // Wait for the ask input to be ready.
    driver
        .query(By::Css(".input-row input"))
        .wait(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;

    // Emit the askUserEvent via Tauri's internal IPC to put AskPanel into 'clarifying' state.
    // This simulates the backend emitting the event during an askUser() call.
    // We use __TAURI_INTERNALS__.invoke to call the event plugin directly.
    driver
        .execute(
            r#"
            window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
                event: 'ask-user-event',
                payload: {
                    question: 'Which format do you prefer for the report?',
                    choices: ['PDF', 'Markdown', 'HTML']
                }
            });
            "#,
            vec![],
        )
        .await?;

    // Wait for the clarification UI to appear.
    let clarify_question = driver
        .query(By::Css(".clarify-question"))
        .wait(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;
    let question_text = clarify_question.text().await?;
    assert_eq!(question_text, "Which format do you prefer for the report?");

    // Verify the choice chips are rendered.
    let chips = driver.find_all(By::Css(".clarify-box .chip")).await?;
    assert_eq!(chips.len(), 3, "expected 3 choice chips");

    let mut chip_texts = Vec::new();
    for chip in &chips {
        chip_texts.push(chip.text().await?);
    }
    assert!(
        chip_texts.iter().any(|t| t.contains("PDF")),
        "expected PDF choice, got: {chip_texts:?}"
    );
    assert!(
        chip_texts.iter().any(|t| t.contains("Markdown")),
        "expected Markdown choice, got: {chip_texts:?}"
    );
    assert!(
        chip_texts.iter().any(|t| t.contains("HTML")),
        "expected HTML choice, got: {chip_texts:?}"
    );

    // Verify the custom answer input is present.
    let custom_input = driver.find(By::Css(".clarify-input-row input")).await?;
    let placeholder = custom_input.attr("placeholder").await?;
    assert_eq!(placeholder.as_deref(), Some("Or type a custom answer…"));

    // Take screenshot.
    let screenshot_path = screenshots_dir().join("ask_clarification_screen.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    // Quit driver before comparison.
    driver.quit().await?;

    // Visual regression: compare against committed baseline.
    compare_or_create_baseline(
        &screenshot_path,
        &references_dir().join("ask_clarification_screen.png"),
        1.0,
    )
    .map_err(WebDriverError::FatalError)?;

    Ok(())
}
