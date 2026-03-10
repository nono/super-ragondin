pub mod image;
pub mod office;
pub mod pdf;
pub mod plaintext;

use anyhow::Result;
use std::path::Path;

/// Extract text content from `path`. Returns `None` if the MIME type is unsupported.
/// Returns `Ok(Some(""))` for PDFs with no extractable text (scanned) — indexer handles fallback.
pub fn extract(path: &Path, mime_type: &str) -> Result<Option<String>> {
    match mime_type {
        "text/plain" | "text/markdown" | "text/csv" | "text/x-markdown" => {
            Ok(Some(plaintext::extract_plaintext(path)?))
        }
        "application/pdf" => Ok(Some(pdf::extract_pdf(path)?)),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Ok(Some(office::extract_docx(path)?))
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Ok(Some(office::extract_xlsx(path)?))
        }
        "application/vnd.oasis.opendocument.text" => Ok(Some(office::extract_odt(path)?)),
        // Images handled separately (need async embedder for description)
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" => Ok(None),
        other => {
            tracing::debug!(mime_type = other, path = %path.display(), "Skipping unsupported MIME type");
            Ok(None)
        }
    }
}
