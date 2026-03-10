use anyhow::Result;
use std::path::Path;

pub fn read_as_base64(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    ))
}
