use chrono::Utc;
use std::path::Path;

/// Maximum length for the filename stem before appending the conflict suffix.
/// This prevents exceeding filesystem name limits (255 bytes on most systems).
const MAX_STEM_LEN: usize = 180;

/// Suffix used to identify conflict copies
const CONFLICT_MARKER: &str = "-conflict-";

/// Generate a conflict copy filename for the given path.
///
/// Pattern: `stem-conflict-2024-01-15T12_30_45.123Z.ext`
/// - Colons in the ISO timestamp are replaced with underscores (Windows compat)
/// - The stem is truncated to 180 chars before appending the suffix
/// - If the filename already has a conflict suffix, it is replaced (no accumulation)
/// - The extension is preserved after the suffix
#[must_use]
pub fn generate_conflict_name(path: &Path) -> String {
    let file_name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().to_string());

    let (stem, ext) = split_stem_extension(&file_name);
    let stem = strip_existing_conflict_suffix(&stem);

    let truncated = truncate_to_max(&stem, MAX_STEM_LEN);

    let timestamp = Utc::now().format("%Y-%m-%dT%H_%M_%S%.3fZ").to_string();

    if ext.is_empty() {
        format!("{truncated}{CONFLICT_MARKER}{timestamp}")
    } else {
        format!("{truncated}{CONFLICT_MARKER}{timestamp}.{ext}")
    }
}

/// Split a filename into stem and extension.
/// Handles files with multiple dots (e.g., "archive.tar.gz" → ("archive.tar", "gz")).
/// Hidden files starting with dot are handled (e.g., ".gitignore" → (".gitignore", "")).
fn split_stem_extension(filename: &str) -> (String, String) {
    if filename.starts_with('.') && !filename[1..].contains('.') {
        return (filename.to_string(), String::new());
    }

    match filename.rfind('.') {
        Some(0) | None => (filename.to_string(), String::new()),
        Some(pos) => (filename[..pos].to_string(), filename[pos + 1..].to_string()),
    }
}

/// Remove an existing conflict suffix to prevent accumulation.
/// E.g., "file-conflict-2024-01-15T12_30_45.123Z" → "file"
fn strip_existing_conflict_suffix(stem: &str) -> String {
    stem.rfind(CONFLICT_MARKER)
        .map_or_else(|| stem.to_string(), |pos| stem[..pos].to_string())
}

/// Truncate a string to at most `max` bytes, ensuring we don't split a UTF-8 char.
fn truncate_to_max(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn conflict_name_for_simple_file() {
        let path = PathBuf::from("/sync/docs/report.txt");
        let name = generate_conflict_name(&path);
        assert!(name.starts_with("report-conflict-"));
        assert!(name.ends_with(".txt"));
        assert!(!name.contains(':'));
    }

    #[test]
    fn conflict_name_preserves_extension() {
        let path = PathBuf::from("/sync/photo.jpg");
        let name = generate_conflict_name(&path);
        assert!(name.ends_with(".jpg"));
    }

    #[test]
    fn conflict_name_no_extension() {
        let path = PathBuf::from("/sync/Makefile");
        let name = generate_conflict_name(&path);
        assert!(name.starts_with("Makefile-conflict-"));
        // Should end with 'Z' (the timestamp), not with a file extension
        assert!(name.ends_with('Z'));
    }

    #[test]
    fn conflict_name_replaces_existing_suffix() {
        let path = PathBuf::from("/sync/file-conflict-2024-01-15T12_30_45.123Z.txt");
        let name = generate_conflict_name(&path);
        // Should not accumulate conflict suffixes
        let conflict_count = name.matches("-conflict-").count();
        assert_eq!(conflict_count, 1, "should have exactly one conflict suffix");
        assert!(name.starts_with("file-conflict-"));
    }

    #[test]
    fn conflict_name_truncates_long_stem() {
        let long_name = "a".repeat(250);
        let path = PathBuf::from(format!("/sync/{long_name}.txt"));
        let name = generate_conflict_name(&path);
        assert!(
            name.len() < 256,
            "conflict name should fit in 255-byte limit, got {}",
            name.len()
        );
        assert!(name.ends_with(".txt"));
    }

    #[test]
    fn conflict_name_handles_dotfiles() {
        let path = PathBuf::from("/sync/.gitignore");
        let name = generate_conflict_name(&path);
        assert!(name.starts_with(".gitignore-conflict-"));
    }

    #[test]
    fn conflict_name_handles_multi_dot_extension() {
        let path = PathBuf::from("/sync/archive.tar.gz");
        let name = generate_conflict_name(&path);
        assert!(name.ends_with(".gz"));
        assert!(name.starts_with("archive.tar-conflict-"));
    }

    #[test]
    fn conflict_name_timestamp_format() {
        let path = PathBuf::from("/sync/file.txt");
        let name = generate_conflict_name(&path);
        // Pattern: file-conflict-YYYY-MM-DDTHH_MM_SS.mmmZ.txt
        let mid = name
            .strip_prefix("file-conflict-")
            .unwrap()
            .strip_suffix(".txt")
            .unwrap();
        assert!(mid.ends_with('Z'), "timestamp should end with Z: {mid}");
        assert!(
            mid.contains('T'),
            "timestamp should contain T separator: {mid}"
        );
        assert!(
            !mid.contains(':'),
            "timestamp should not contain colons: {mid}"
        );
    }

    #[test]
    fn split_stem_extension_basic() {
        assert_eq!(
            split_stem_extension("file.txt"),
            ("file".to_string(), "txt".to_string())
        );
    }

    #[test]
    fn split_stem_extension_no_ext() {
        assert_eq!(
            split_stem_extension("Makefile"),
            ("Makefile".to_string(), String::new())
        );
    }

    #[test]
    fn split_stem_extension_dotfile() {
        assert_eq!(
            split_stem_extension(".gitignore"),
            (".gitignore".to_string(), String::new())
        );
    }

    #[test]
    fn strip_existing_conflict_no_suffix() {
        assert_eq!(strip_existing_conflict_suffix("myfile"), "myfile");
    }

    #[test]
    fn strip_existing_conflict_with_suffix() {
        assert_eq!(
            strip_existing_conflict_suffix("myfile-conflict-2024-01-15T12_30_45.123Z"),
            "myfile"
        );
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_to_max("hello", 3), "hel");
    }

    #[test]
    fn truncate_noop_if_short() {
        assert_eq!(truncate_to_max("hi", 10), "hi");
    }

    #[test]
    fn truncate_utf8_boundary() {
        // "héllo" — 'é' is 2 bytes in UTF-8
        let s = "héllo";
        let t = truncate_to_max(s, 3);
        // Should truncate at a char boundary: "hé" (3 bytes) fits
        assert_eq!(t, "hé");
    }
}
