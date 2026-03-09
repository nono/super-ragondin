use crate::error::Result;
use md5::{Digest, Md5};
use serde::{Deserialize, Deserializer};
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

/// Deserialize a JSON value that may be a string or a number as `Option<u64>`.
///
/// The Cozy API sometimes returns `size` as a JSON string instead of a number.
/// This deserializer handles both representations.
///
/// # Errors
/// Returns an error if the value is a string that cannot be parsed as `u64`.
pub fn deserialize_string_or_u64<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrU64 {
        U64(u64),
        Str(String),
    }

    let opt: Option<StringOrU64> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(StringOrU64::U64(n)) => Ok(Some(n)),
        Some(StringOrU64::Str(s)) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
    }
}
