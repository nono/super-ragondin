use std::path::{Component, Path};

/// Check that a path is relative and does not escape via `..` or root
/// components.
///
/// Rejects both absolute paths (those starting with `/`) and any path
/// containing `..` segments.
///
/// # Errors
/// Returns `Err` with a message if the path is absolute or contains a
/// `ParentDir` component.
pub(crate) fn check_relative_path(path: &str) -> Result<(), &'static str> {
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir | Component::RootDir => {
                return Err("path escapes sync directory");
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_parent_dir() {
        assert!(check_relative_path("../etc/passwd").is_err());
        assert!(check_relative_path("notes/../../../etc").is_err());
        assert!(check_relative_path("a/b/../../..").is_err());
    }

    #[test]
    fn test_rejects_absolute_path() {
        assert!(check_relative_path("/etc/passwd").is_err());
        assert!(check_relative_path("/tmp/file.txt").is_err());
    }

    #[test]
    fn test_accepts_normal_paths() {
        assert!(check_relative_path("notes/summary.md").is_ok());
        assert!(check_relative_path("./notes/file.txt").is_ok());
        assert!(check_relative_path("file.txt").is_ok());
        assert!(check_relative_path("a/b/c").is_ok());
    }
}
