use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::json;
use thirtyfour::prelude::*;

const TAURI_DRIVER_PORT: u16 = 4444;

/// Spawns `tauri-driver` and returns the child process handle.
///
/// # Errors
///
/// Returns an error if `tauri-driver` cannot be started.
pub fn start_tauri_driver() -> std::io::Result<Child> {
    let binary = find_tauri_driver();
    let child = Command::new(&binary)
        .arg("--port")
        .arg(TAURI_DRIVER_PORT.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // Give tauri-driver time to bind the port
    std::thread::sleep(Duration::from_secs(2));

    Ok(child)
}

/// Connects to the running `tauri-driver` and launches the app binary.
///
/// # Errors
///
/// Returns an error if the `WebDriver` session cannot be created.
pub async fn connect_driver(app_binary: &Path) -> WebDriverResult<WebDriver> {
    let mut caps: Capabilities = Capabilities::new();
    caps.insert(
        "tauri:options".to_string(),
        json!({ "application": app_binary.to_string_lossy() }),
    );
    caps.insert("browserName".to_string(), json!("wry"));

    let server_url = format!("http://127.0.0.1:{TAURI_DRIVER_PORT}/");
    WebDriver::new(&server_url, caps).await
}

/// Takes a screenshot and saves it as a PNG file.
///
/// # Errors
///
/// Returns an error if the screenshot cannot be taken or saved.
pub async fn save_screenshot(driver: &WebDriver, path: &Path) -> WebDriverResult<()> {
    let screenshot = driver.screenshot_as_png().await?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, screenshot).map_err(WebDriverError::IoError)?;
    Ok(())
}

/// Returns the path to the debug binary for `super-ragondin-gui`.
///
/// # Panics
///
/// Panics if the workspace root cannot be determined from `CARGO_MANIFEST_DIR`.
#[must_use]
pub fn app_binary_path() -> PathBuf {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("cannot find workspace root")
        .to_path_buf();
    workspace_root.join("target/debug/super-ragondin-gui")
}

/// Returns the screenshots directory for this crate.
#[must_use]
pub fn screenshots_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("screenshots")
}

/// Finds the `tauri-driver` binary, checking `~/.cargo/bin` as a fallback.
fn find_tauri_driver() -> PathBuf {
    if let Ok(path) = which("tauri-driver") {
        return path;
    }
    let cargo_bin = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cargo/bin/tauri-driver"))
        .filter(|p| p.exists());
    cargo_bin.unwrap_or_else(|| PathBuf::from("tauri-driver"))
}

fn which(name: &str) -> Result<PathBuf, ()> {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(name))
                .find(|p| p.exists())
        })
        .ok_or(())
}
