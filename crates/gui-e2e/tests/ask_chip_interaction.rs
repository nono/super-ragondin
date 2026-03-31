use gui_e2e::{
    app_binary_path, connect_driver, save_screenshot, screenshots_dir, start_tauri_driver,
    ConfigGuard, TauriDriverGuard,
};
use thirtyfour::prelude::*;

#[tokio::test]
#[ignore = "requires built GUI binary and tauri-driver"]
async fn clicking_clarification_chip_transitions_to_done() -> WebDriverResult<()> {
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

    let _tauri_driver = TauriDriverGuard::new(start_tauri_driver().expect(
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

    // Emit the askUserEvent to put AskPanel into 'clarifying' state.
    driver
        .execute(
            r#"
            window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
                event: 'ask-user-event',
                payload: {
                    question: 'Which format do you prefer?',
                    choices: ['PDF', 'Markdown', 'HTML']
                }
            });
            "#,
            vec![],
        )
        .await?;

    // Wait for the clarification UI to appear.
    driver
        .query(By::Css(".clarify-question"))
        .wait(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(300),
        )
        .first()
        .await?;

    // Verify the chips are rendered before clicking.
    let chips = driver.find_all(By::Css(".clarify-box .chip")).await?;
    assert_eq!(chips.len(), 3, "expected 3 choice chips");

    // Click the first chip ("PDF").
    chips[0].click().await?;

    // After clicking, sendClarification sets panelState to 'asking' (Thinking…),
    // then answerUser returns Ok (no pending sender → no-op) → panelState goes to 'done'.
    // The 'done' state shows the assistant message area (empty since answer is "").
    // Wait for the clarification UI to disappear (chips are gone).
    driver
        .query(By::Css(".clarify-box"))
        .wait(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(300),
        )
        .not_exists()
        .await?;

    // Verify we are no longer in 'clarifying' state — the clarify-question is gone.
    let clarify_elements = driver.find_all(By::Css(".clarify-question")).await?;
    assert!(
        clarify_elements.is_empty(),
        "clarification UI should be gone after clicking a chip"
    );

    // Take screenshot of the post-click state.
    let screenshot_path = screenshots_dir().join("ask_chip_interaction.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "screenshot was not saved");

    driver.quit().await?;

    Ok(())
}
