use anyhow::Result;
use std::path::Path;

/// # Errors
/// Returns error if the file cannot be read.
pub fn read_as_base64(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}
