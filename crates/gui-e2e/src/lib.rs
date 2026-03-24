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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn solid_png(path: &std::path::Path, color: Rgb<u8>, w: u32, h: u32) {
        let mut img = RgbImage::new(w, h);
        for p in img.pixels_mut() {
            *p = color;
        }
        img.save(path).unwrap();
    }

    #[test]
    fn no_baseline_creates_it_and_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("refs/shot.png");
        solid_png(&shot, Rgb([100, 100, 100]), 10, 10);

        let result = compare_or_create_baseline(&shot, &reference, 1.0);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("Baseline created"), "got: {msg}");
        assert!(msg.contains(reference.to_str().unwrap()), "got: {msg}");
        assert!(reference.exists(), "reference was not created");
    }

    #[test]
    fn identical_images_passes() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("shot_ref.png");
        solid_png(&shot, Rgb([80, 160, 200]), 20, 20);
        std::fs::copy(&shot, &reference).unwrap();

        assert!(compare_or_create_baseline(&shot, &reference, 1.0).is_ok());
    }

    #[test]
    fn small_diff_below_threshold_passes() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("ref.png");
        // 10×10 = 100 pixels. One pixel has delta 9 (> 8) so it is counted as differing.
        // 1 / 100 = 1.0% which equals the threshold — not above it — so the test must pass.
        solid_png(&reference, Rgb([100, 100, 100]), 10, 10);
        let mut img = RgbImage::new(10, 10);
        for p in img.pixels_mut() {
            *p = Rgb([100, 100, 100]);
        }
        img.put_pixel(0, 0, Rgb([109, 100, 100])); // delta 9 > 8 → counted; 1/100 = 1.0% == threshold
        img.save(&shot).unwrap();

        assert!(compare_or_create_baseline(&shot, &reference, 1.0).is_ok());
    }

    #[test]
    fn large_diff_above_threshold_fails_and_saves_diff_image() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("ref.png");
        // All pixels differ by 100 in the red channel → 100 % diff
        solid_png(&reference, Rgb([0, 0, 0]), 10, 10);
        solid_png(&shot, Rgb([100, 0, 0]), 10, 10);

        let result = compare_or_create_baseline(&shot, &reference, 1.0);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("pixel diff"), "got: {msg}");
        assert!(msg.contains("exceeds threshold"), "got: {msg}");

        // Diff image should be saved next to the screenshot as shot.diff.png
        let diff_path = dir.path().join("shot.diff.png");
        assert!(diff_path.exists(), "diff image was not saved");
    }

    #[test]
    fn dimension_mismatch_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("ref.png");
        solid_png(&shot, Rgb([0, 0, 0]), 10, 10);
        solid_png(&reference, Rgb([0, 0, 0]), 20, 20);

        let result = compare_or_create_baseline(&shot, &reference, 1.0);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("dimension mismatch"), "got: {msg}");
    }

    #[test]
    fn update_snapshots_overwrites_reference_and_returns_ok() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("ref.png");
        solid_png(&shot, Rgb([200, 0, 0]), 10, 10);
        solid_png(&reference, Rgb([0, 0, 0]), 10, 10); // existing baseline

        temp_env::with_var("UPDATE_SNAPSHOTS", Some("1"), || {
            let result = compare_or_create_baseline(&shot, &reference, 1.0);
            assert!(result.is_ok(), "got: {:?}", result);
        });

        // Reference must now match the screenshot
        let updated = image::open(&reference).unwrap().into_rgb8();
        assert_eq!(updated.get_pixel(0, 0), &Rgb([200, 0, 0]));
    }

    #[test]
    fn update_snapshots_deletes_stale_diff_image() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("ref.png");
        let diff = dir.path().join("shot.diff.png");
        solid_png(&shot, Rgb([200, 0, 0]), 10, 10);
        solid_png(&reference, Rgb([0, 0, 0]), 10, 10);
        solid_png(&diff, Rgb([255, 0, 0]), 10, 10); // stale diff

        temp_env::with_var("UPDATE_SNAPSHOTS", Some("1"), || {
            compare_or_create_baseline(&shot, &reference, 1.0).unwrap();
        });

        assert!(!diff.exists(), "stale diff image should have been deleted");
    }

    #[test]
    fn update_snapshots_with_no_baseline_creates_it_and_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("refs/shot.png");
        solid_png(&shot, Rgb([50, 50, 50]), 10, 10);

        temp_env::with_var("UPDATE_SNAPSHOTS", Some("1"), || {
            let result = compare_or_create_baseline(&shot, &reference, 1.0);
            // "reference does not exist" takes precedence over UPDATE_SNAPSHOTS
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("Baseline created"));
        });

        assert!(reference.exists());
    }
}
