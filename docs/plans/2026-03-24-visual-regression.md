# Visual Regression Testing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add pixel-diff visual regression to the GUI E2E test, with committed baseline images and a diff PNG saved on failure.

**Architecture:** A new `compare_or_create_baseline()` helper in `crates/gui-e2e/src/lib.rs` handles all comparison logic. The test calls it after `driver.quit()`. Baselines live in `crates/gui-e2e/references/` (committed). Generated screenshots and diff images live in `crates/gui-e2e/screenshots/` (gitignored).

**Tech Stack:** Rust, `image = "0.25"` (PNG load/save/pixel ops), `tempfile` (unit test fixtures), existing `thirtyfour` WebDriver test harness.

**Spec:** `docs/superpowers/specs/2026-03-24-visual-regression-design.md`

---

### Task 1: Add dependencies

**Files:**
- Modify: `crates/gui-e2e/Cargo.toml`

- [ ] **Step 1: Add `image` and `tempfile` via cargo add**

```bash
cargo add image@0.25 --package gui-e2e
cargo add tempfile --package gui-e2e --dev
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build -p gui-e2e
```

Expected: `Finished` with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/gui-e2e/Cargo.toml Cargo.lock
git commit -m "chore(gui-e2e): add image and tempfile dependencies"
```

---

### Task 2: Write unit tests for `compare_or_create_baseline` (red phase)

**Files:**
- Modify: `crates/gui-e2e/src/lib.rs`

These tests will fail to compile until Task 3 implements the function. That is expected.

- [ ] **Step 1: Add unit tests to the bottom of `lib.rs`**

Append the following module after the last line of `crates/gui-e2e/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: Add `temp-env` dev dependency (required for env var manipulation without unsafe)**

```bash
cargo add temp-env --package gui-e2e --dev
```

- [ ] **Step 3: Run tests to confirm they fail to compile (expected)**

```bash
cargo test -p gui-e2e --lib 2>&1 | head -20
```

Expected: compile error — `compare_or_create_baseline` not found.

- [ ] **Step 4: Commit the failing tests**

```bash
git add crates/gui-e2e/src/lib.rs crates/gui-e2e/Cargo.toml Cargo.lock
git commit -m "test(gui-e2e): add unit tests for compare_or_create_baseline (red)"
```

---

### Task 3: Implement `references_dir()` and `compare_or_create_baseline()` (green phase)

**Files:**
- Modify: `crates/gui-e2e/src/lib.rs`

- [ ] **Step 1: Add `references_dir()` after `screenshots_dir()` in `lib.rs`**

Insert after the closing brace of `screenshots_dir()` (after line 80):

```rust
/// Returns the references directory for this crate (committed baseline PNGs).
#[must_use]
pub fn references_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("references")
}
```

- [ ] **Step 2: Add `diff_image_path()` private helper after `references_dir()`**

```rust
/// Derives the diff image path from a screenshot path by inserting `.diff` before the extension.
/// E.g. `screenshots/setup_screen.png` → `screenshots/setup_screen.diff.png`.
fn diff_image_path(screenshot: &Path) -> PathBuf {
    let stem = screenshot
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let diff_name = format!("{stem}.diff.png");
    screenshot.with_file_name(diff_name)
}
```

- [ ] **Step 3: Add `compare_or_create_baseline()` after `diff_image_path()`**

```rust
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
        if let Some(parent) = reference.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
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

    Ok(())
}
```

- [ ] **Step 4: Run `cargo fmt --all`**

```bash
cargo fmt --all
```

- [ ] **Step 5: Run unit tests to confirm they pass**

```bash
cargo test -p gui-e2e --lib
```

Expected: all 8 tests pass.

- [ ] **Step 6: Run `cargo clippy --all-features` and fix any warnings**

```bash
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/gui-e2e/src/lib.rs
git commit -m "feat(gui-e2e): implement references_dir and compare_or_create_baseline"
```

---

### Task 4: Integrate comparison into `setup_screen.rs`

**Files:**
- Modify: `crates/gui-e2e/tests/setup_screen.rs`

- [ ] **Step 1: Update the import line at the top of `setup_screen.rs`**

Replace:
```rust
use gui_e2e::{
    app_binary_path, connect_driver, save_screenshot, screenshots_dir, start_tauri_driver,
};
```
With:
```rust
use gui_e2e::{
    app_binary_path, compare_or_create_baseline, connect_driver, references_dir, save_screenshot,
    screenshots_dir, start_tauri_driver,
};
```

- [ ] **Step 2: Replace the screenshot + quit block at the end of the test**

Replace:
```rust
    // Take screenshot
    let screenshot_path = screenshots_dir().join("setup_screen.png");
    save_screenshot(&driver, &screenshot_path).await?;
    assert!(screenshot_path.exists(), "Screenshot was not saved");

    driver.quit().await?;

    Ok(())
```
With:
```rust
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
        1.0,
    )
    .map_err(WebDriverError::CustomError)?;

    Ok(())
```

- [ ] **Step 3: Run `cargo fmt --all`**

```bash
cargo fmt --all
```

- [ ] **Step 4: Verify it compiles**

```bash
cargo build -p gui-e2e
```

Expected: `Finished` with no errors.

- [ ] **Step 5: Run `cargo clippy --all-features`**

```bash
cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/gui-e2e/tests/setup_screen.rs
git commit -m "feat(gui-e2e): add visual regression comparison to setup_screen test"
```

---

### Task 5: Create `references/` directory and update docs

**Files:**
- Create: `crates/gui-e2e/references/.gitkeep`
- Modify: `.gitignore`
- Modify: `README.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: Create the `references/` directory with a `.gitkeep`**

```bash
mkdir -p crates/gui-e2e/references
touch crates/gui-e2e/references/.gitkeep
```

- [ ] **Step 2: Verify `.gitignore` already covers screenshots correctly**

The existing line `crates/gui-e2e/screenshots/*.png` in `.gitignore` already covers both `setup_screen.png` and `setup_screen.diff.png`. No change needed. Confirm with:

```bash
grep "gui-e2e" .gitignore
```

Expected output includes: `crates/gui-e2e/screenshots/*.png`

- [ ] **Step 3: Update README.md E2E section**

Find the existing E2E block in `README.md` and replace it with:

```markdown
To run GUI end-to-end tests (requires `tauri-driver`, `WebKitWebDriver`, and `xvfb`):

```bash
cargo install tauri-driver --locked
sudo apt install webkit2gtk-driver xvfb
cargo build -p super-ragondin-gui --no-default-features
xvfb-run cargo test -p gui-e2e -- --ignored
```

To create or update visual regression baselines:

```bash
UPDATE_SNAPSHOTS=1 xvfb-run cargo test -p gui-e2e -- --ignored
# Review crates/gui-e2e/references/*.png, then commit them
```
```

- [ ] **Step 4: Update AGENTS.md Commands section and Environment Variables table**

Add after the existing `xvfb-run` line in the Commands bash block:

```
UPDATE_SNAPSHOTS=1 xvfb-run cargo test -p gui-e2e -- --ignored  # Update visual regression baselines
```

Also add a row to the Environment Variables table (after the `PROPTEST_CASES` row):

```
| `UPDATE_SNAPSHOTS` | unset | Set to `1` to overwrite visual regression baselines instead of comparing |
```

- [ ] **Step 5: Run `cargo test -q` to confirm nothing broken**

```bash
cargo test -q 2>&1 | grep -E "FAILED|error\[" | head -10
```

Expected: no output (all tests pass).

- [ ] **Step 6: Commit everything**

```bash
git add crates/gui-e2e/references/.gitkeep README.md AGENTS.md
git commit -m "feat(gui-e2e): add references/ directory and update docs for visual regression workflow"
```
