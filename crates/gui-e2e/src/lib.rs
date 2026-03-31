use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::json;
use thirtyfour::prelude::*;

const TAURI_DRIVER_PORT: u16 = 4444;

/// RAII guard that kills and waits on the `tauri-driver` child process.
pub struct TauriDriverGuard(Child);

impl TauriDriverGuard {
    /// Wraps a `tauri-driver` child process for automatic cleanup.
    #[must_use]
    pub const fn new(child: Child) -> Self {
        Self(child)
    }
}

impl Drop for TauriDriverGuard {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

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

/// Returns the references directory for this crate (committed baseline PNGs).
#[must_use]
pub fn references_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("references")
}

/// Derives the diff image path from a screenshot path by inserting `.diff` before the extension.
/// E.g. `screenshots/setup_screen.png` → `screenshots/setup_screen.diff.png`.
fn diff_image_path(screenshot: &Path) -> PathBuf {
    let stem = screenshot.file_stem().unwrap_or_default().to_string_lossy();
    let diff_name = format!("{stem}.diff.png");
    screenshot.with_file_name(diff_name)
}

/// Compares a screenshot against a committed baseline PNG using fuzzy pixel comparison.
///
/// # Behaviour
///
/// - If the reference does not exist: copies the screenshot to the reference path (creating
///   parent directories) and returns `Err` asking the developer to review and commit it.
/// - If `UPDATE_SNAPSHOTS=1`: overwrites the reference, deletes any stale diff image, prints
///   a notice to stderr, and returns `Ok(())`. "Reference does not exist" takes precedence:
///   if the reference is missing, the create-baseline path runs instead.
/// - Otherwise: loads both images (as RGB, alpha ignored), checks dimensions match, counts
///   pixels where the max RGB channel delta exceeds 8/255, and fails with a diff percentage
///   and a saved diff image if `diff_pct > threshold_pct`.
///
/// # Errors
///
/// Returns `Err(String)` on baseline creation, visual regression, or I/O failure.
pub fn compare_or_create_baseline(
    screenshot: &Path,
    reference: &Path,
    threshold_pct: f64,
) -> Result<(), String> {
    // "reference does not exist" takes precedence over UPDATE_SNAPSHOTS
    if !reference.exists() {
        if let Some(parent) = reference.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(screenshot, reference).map_err(|e| e.to_string())?;
        return Err(format!(
            "Baseline created at {} — review and commit it, then re-run",
            reference.display()
        ));
    }

    if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        std::fs::copy(screenshot, reference).map_err(|e| e.to_string())?;
        let diff = diff_image_path(screenshot);
        if diff.exists() {
            std::fs::remove_file(&diff).ok();
        }
        eprintln!("[gui-e2e] baseline updated: {}", reference.display());
        return Ok(());
    }

    let ref_img = image::open(reference)
        .map_err(|e| format!("failed to open reference {}: {e}", reference.display()))?
        .into_rgb8();
    let new_img = image::open(screenshot)
        .map_err(|e| format!("failed to open screenshot {}: {e}", screenshot.display()))?
        .into_rgb8();

    let (ref_w, ref_h) = ref_img.dimensions();
    let (new_w, new_h) = new_img.dimensions();
    if ref_w != new_w || ref_h != new_h {
        return Err(format!(
            "dimension mismatch: reference is {ref_w}x{ref_h}, screenshot is {new_w}x{new_h}"
        ));
    }

    let total = u64::from(ref_w) * u64::from(ref_h);
    let mut differing: u64 = 0;
    let mut diff_img = image::RgbImage::new(ref_w, ref_h);

    for (x, y, ref_pixel) in ref_img.enumerate_pixels() {
        let new_pixel = new_img.get_pixel(x, y);
        let max_delta = ref_pixel
            .0
            .iter()
            .zip(new_pixel.0.iter())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);

        if max_delta > 8 {
            differing += 1;
            diff_img.put_pixel(x, y, image::Rgb([255, 0, 0]));
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let faded = image::Rgb(ref_pixel.0.map(|c| (f64::from(c) * 0.3) as u8));
            diff_img.put_pixel(x, y, faded);
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let diff_pct = differing as f64 / total as f64 * 100.0;
    if diff_pct > threshold_pct {
        let diff_path = diff_image_path(screenshot);
        diff_img
            .save(&diff_path)
            .map_err(|e| format!("failed to save diff image: {e}"))?;
        return Err(format!(
            "pixel diff {diff_pct:.2}% exceeds threshold {threshold_pct:.1}% — see {}",
            diff_path.display()
        ));
    }

    // Clean up any stale diff image from a previous failing run
    let diff = diff_image_path(screenshot);
    if diff.exists() {
        std::fs::remove_file(&diff).ok();
    }

    Ok(())
}

/// Returns the path to `super-ragondin`'s config file.
///
/// Uses the same logic as `crates/gui/src/commands.rs::config_path()`.
#[must_use]
pub fn app_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin")
        .join("config.json")
}

/// RAII guard that writes a test config on construction and restores the original on drop.
enum OriginalConfig {
    /// No file existed at the path before installation.
    Absent,
    /// A file existed and was successfully read.
    Present(Vec<u8>),
    /// A file existed but could not be read (e.g. permission denied).
    Unreadable,
}

pub struct ConfigGuard {
    path: PathBuf,
    original: OriginalConfig,
}

impl ConfigGuard {
    /// Write `config_json` to the app config path, saving any existing file for restoration.
    ///
    /// # Panics
    ///
    /// Panics if the config directory cannot be created or the file cannot be written.
    #[must_use]
    pub fn install(config_json: &str) -> Self {
        let path = app_config_path();
        let original = match std::fs::read(&path) {
            Ok(bytes) => OriginalConfig::Present(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => OriginalConfig::Absent,
            Err(_) => OriginalConfig::Unreadable,
        };
        std::fs::create_dir_all(path.parent().expect("config path has no parent"))
            .expect("failed to create config dir");
        std::fs::write(&path, config_json).expect("failed to write test config");
        Self { path, original }
    }

    /// Remove the config file so the app starts in Unconfigured state.
    /// Restores the original file (if any) on drop.
    ///
    /// # Panics
    ///
    /// Panics if the file exists but cannot be removed.
    #[must_use]
    pub fn remove() -> Self {
        let path = app_config_path();
        let original = match std::fs::read(&path) {
            Ok(bytes) => OriginalConfig::Present(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => OriginalConfig::Absent,
            Err(_) => OriginalConfig::Unreadable,
        };
        if path.exists() {
            std::fs::remove_file(&path).expect("failed to remove config file");
        }
        Self { path, original }
    }
}

impl Drop for ConfigGuard {
    fn drop(&mut self) {
        match &self.original {
            OriginalConfig::Present(bytes) => {
                std::fs::write(&self.path, bytes).ok();
            }
            OriginalConfig::Absent => {
                std::fs::remove_file(&self.path).ok();
            }
            OriginalConfig::Unreadable => {
                // The file existed but we couldn't read it — leave it as-is to avoid
                // destroying a config file we never had permission to inspect.
            }
        }
    }
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
    use serial_test::serial;

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
    fn passing_comparison_deletes_stale_diff_image() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        let reference = dir.path().join("ref.png");
        let diff = dir.path().join("shot.diff.png");
        solid_png(&shot, Rgb([80, 80, 80]), 10, 10);
        std::fs::copy(&shot, &reference).unwrap(); // identical → will pass
        solid_png(&diff, Rgb([255, 0, 0]), 10, 10); // stale diff from previous run

        assert!(compare_or_create_baseline(&shot, &reference, 1.0).is_ok());
        assert!(
            !diff.exists(),
            "stale diff should be deleted on passing comparison"
        );
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
    #[serial]
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
