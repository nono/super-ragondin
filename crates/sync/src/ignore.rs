use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

const DEFAULT_RULES: &str = include_str!("config/syncignore");

pub struct IgnoreRules {
    gitignore: Gitignore,
}

impl IgnoreRules {
    /// Create rules that ignore nothing (for testing/simulation).
    ///
    /// # Panics
    ///
    /// Panics if building an empty gitignore fails (should never happen).
    #[must_use]
    pub fn none() -> Self {
        let builder = GitignoreBuilder::new("");
        let gitignore = builder.build().expect("failed to build empty gitignore");
        Self { gitignore }
    }

    /// Create rules from the embedded default syncignore file only.
    #[must_use]
    pub fn default_only() -> Self {
        Self::build(DEFAULT_RULES, "")
    }

    /// Create rules from embedded defaults plus user-provided rules.
    ///
    /// User rules are appended after defaults, so they can override
    /// with negation patterns (`!pattern`).
    #[must_use]
    pub fn with_user_rules(user_content: &str) -> Self {
        Self::build(DEFAULT_RULES, user_content)
    }

    /// Load rules from the default file and an optional user rules file.
    ///
    /// If the user file does not exist or cannot be read, only defaults apply.
    #[must_use]
    pub fn load(user_rules_path: Option<&Path>) -> Self {
        let user_content = user_rules_path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Self::build(DEFAULT_RULES, &user_content)
    }

    fn build(default_content: &str, user_content: &str) -> Self {
        let mut builder = GitignoreBuilder::new("");
        for line in default_content.lines() {
            builder.add_line(None, line).ok();
        }
        for line in user_content.lines() {
            builder.add_line(None, line).ok();
        }
        let gitignore = builder.build().expect("failed to build gitignore rules");
        Self { gitignore }
    }

    /// Returns `true` if the given relative path should be ignored.
    ///
    /// `rel_path` is relative to the sync root (e.g. `"subdir/file.txt"`).
    /// `is_dir` indicates whether the path is a directory.
    #[must_use]
    pub fn is_ignored(&self, rel_path: &str, is_dir: bool) -> bool {
        self.gitignore
            .matched_path_or_any_parents(rel_path, is_dir)
            .is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_ignore_hidden_files() {
        let rules = IgnoreRules::default_only();
        assert!(rules.is_ignored(".hidden", false));
        assert!(rules.is_ignored(".git", true));
    }

    #[test]
    fn default_rules_ignore_editor_temp_files() {
        let rules = IgnoreRules::default_only();
        assert!(rules.is_ignored("file.tmp", false));
        assert!(rules.is_ignored("file.bak", false));
        assert!(rules.is_ignored("file~", false));
        assert!(rules.is_ignored("file.swp", false));
        assert!(rules.is_ignored("file.swx", false));
        assert!(rules.is_ignored("~$document.docx", false));
    }

    #[test]
    fn default_rules_allow_normal_files() {
        let rules = IgnoreRules::default_only();
        assert!(!rules.is_ignored("document.pdf", false));
        assert!(!rules.is_ignored("photo.jpg", false));
        assert!(!rules.is_ignored("notes.txt", false));
        assert!(!rules.is_ignored("project", true));
    }

    #[test]
    fn nested_paths_are_checked() {
        let rules = IgnoreRules::default_only();
        assert!(rules.is_ignored("subdir/.hidden", false));
        assert!(rules.is_ignored("a/b/file.tmp", false));
        assert!(!rules.is_ignored("subdir/normal.txt", false));
    }

    #[test]
    fn user_rules_override_defaults() {
        let user_content = "*.log\n!important.log\n";
        let rules = IgnoreRules::with_user_rules(user_content);
        assert!(rules.is_ignored("debug.log", false));
        assert!(!rules.is_ignored("important.log", false));
        assert!(!rules.is_ignored("data.csv", false));
    }

    #[test]
    fn folder_only_patterns() {
        let user_content = "build/\n";
        let rules = IgnoreRules::with_user_rules(user_content);
        assert!(rules.is_ignored("build", true));
        assert!(!rules.is_ignored("build", false));
    }

    #[test]
    fn empty_user_rules_only_uses_defaults() {
        let rules = IgnoreRules::with_user_rules("");
        assert!(rules.is_ignored(".hidden", false));
        assert!(!rules.is_ignored("normal.txt", false));
    }
}
