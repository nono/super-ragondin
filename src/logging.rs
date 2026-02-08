use std::path::PathBuf;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Returns the XDG-compliant log directory.
///
/// Uses `$XDG_STATE_HOME/cozy-desktop/` if set,
/// otherwise falls back to `$HOME/.local/state/cozy-desktop/`.
#[must_use]
pub fn log_dir() -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("cozy-desktop")
}

/// Initialize the dual-output logging system.
///
/// - stderr: human-readable, colored, controlled by `RUST_LOG` (default: `cozy_desktop=info`)
/// - file: JSONL format, daily rotation, always at DEBUG level
///
/// Log files are written to `log_dir()` with the prefix `cozy-desktop`
/// and suffix `.jsonl`, e.g. `cozy-desktop.2026-02-08.jsonl`.
///
/// # Panics
///
/// Panics if the tracing subscriber cannot be initialized (e.g. called twice).
pub fn init() {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).ok();

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("cozy-desktop")
        .filename_suffix("jsonl")
        .build(&dir)
        .expect("failed to create log file appender");

    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .flatten_event(true);

    tracing_subscriber::registry()
        .with(
            stderr_layer.with_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "cozy_desktop=info".into()),
            ),
        )
        .with(file_layer.with_filter(tracing_subscriber::EnvFilter::new("cozy_desktop=debug")))
        .init();

    tracing::info!(log_dir = %dir.display(), "⚙️ Logging initialized");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_ends_with_cozy_desktop() {
        let dir = log_dir();
        assert!(
            dir.ends_with("cozy-desktop"),
            "expected log dir to end with 'cozy-desktop', got: {dir:?}"
        );
    }
}
