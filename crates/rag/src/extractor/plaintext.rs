use anyhow::Result;
use std::path::Path;

/// # Errors
/// Returns error if the file cannot be read.
pub fn extract_plaintext(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_utf8() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "Hello, world!\nSecond line.").unwrap();
        let text = extract_plaintext(f.path()).unwrap();
        assert_eq!(text, "Hello, world!\nSecond line.");
    }

    #[test]
    fn test_extract_empty() {
        let f = NamedTempFile::new().unwrap();
        let text = extract_plaintext(f.path()).unwrap();
        assert!(text.is_empty());
    }
}
