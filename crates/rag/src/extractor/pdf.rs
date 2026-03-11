use anyhow::Result;
use std::path::Path;

const SCANNED_THRESHOLD: usize = 50;

/// Extract text from a PDF file.
/// If extracted text is shorter than `SCANNED_THRESHOLD` chars, the PDF is likely
/// scanned. Returns empty string in that case — the indexer will route to vision LLM.
///
/// # Errors
/// Returns error if the file cannot be read.
pub fn extract_pdf(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    match pdf_extract::extract_text_from_mem(&bytes) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.len() < SCANNED_THRESHOLD {
                tracing::debug!(
                    path = %path.display(),
                    chars = trimmed.len(),
                    "PDF appears to be scanned (too little text extracted)"
                );
                return Ok(String::new()); // Caller checks empty → vision fallback
            }
            Ok(trimmed)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "pdf-extract failed");
            Ok(String::new())
        }
    }
}

/// Render the first page of a PDF to a PNG and return as base64.
/// Used by the indexer when `extract_pdf` returns empty (scanned PDF).
/// NOTE: Requires pdfium shared library at runtime. Not tested in CI.
///
/// # Errors
/// Returns error if the PDF cannot be loaded or rendered.
pub fn render_first_page_as_base64(path: &Path) -> Result<String> {
    use image::ImageEncoder;
    use pdfium_render::prelude::*;

    let pdfium = Pdfium::new(Pdfium::bind_to_system_library()?);
    let doc = pdfium.load_pdf_from_file(path, None)?;
    let page = doc.pages().get(0)?;
    let bitmap = page.render_with_config(
        &PdfRenderConfig::new()
            .set_target_width(1200)
            .set_maximum_height(1600),
    )?;
    let img = bitmap.as_image();
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut std::io::Cursor::new(&mut buf)).write_image(
        img.as_bytes(),
        img.width(),
        img.height(),
        img.color().into(),
    )?;
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &buf,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_pdf() -> Result<()> {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.pdf"
        ));
        if !path.exists() {
            eprintln!("Skipping: sample.pdf not present");
            return Ok(());
        }
        let result = extract_pdf(path)?;
        assert!(result.len() > 50, "Expected meaningful text from PDF");
        Ok(())
    }
}
