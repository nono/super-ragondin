use crate::error::Result;
use md5::{Digest, Md5};
use std::fs;
use std::io::Read;
use std::path::Path;

/// Compute the MD5 hash of a file at the given path.
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn compute_md5_from_path(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Compute the MD5 hash of a byte slice.
#[must_use]
pub fn compute_md5_from_bytes(content: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}
