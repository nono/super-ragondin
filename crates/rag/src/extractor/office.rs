use anyhow::Result;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::Read;
use std::path::Path;

/// Extract text from a .docx file (ZIP containing word/document.xml).
///
/// # Errors
/// Returns error if the file cannot be opened or parsed.
pub fn extract_docx(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut xml_content = String::new();
    zip.by_name("word/document.xml")?
        .read_to_string(&mut xml_content)?;
    Ok(xml_text_content(&xml_content))
}

/// Extract text from an .odt file (ZIP containing content.xml).
///
/// # Errors
/// Returns error if the file cannot be opened or parsed.
pub fn extract_odt(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut xml_content = String::new();
    zip.by_name("content.xml")?
        .read_to_string(&mut xml_content)?;
    Ok(xml_text_content(&xml_content))
}

/// Walk XML events and collect all text content, inserting spaces at element boundaries.
fn xml_text_content(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut parts = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.unescape() {
                    let s = text.trim().to_string();
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!("XML parse error: {e}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    parts.join(" ")
}

/// Extract text from an .xlsx file using calamine.
///
/// # Errors
/// Returns error if the file cannot be opened or parsed.
pub fn extract_xlsx(path: &Path) -> Result<String> {
    use calamine::{Data, Reader as CalaReader, open_workbook_auto};
    let mut workbook = open_workbook_auto(path)?;
    let mut lines = Vec::new();
    for sheet_name in workbook.sheet_names() {
        if let Ok(range) = workbook.worksheet_range(&sheet_name) {
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .filter_map(|cell| match cell {
                        Data::String(s) => Some(s.clone()),
                        Data::Float(f) => Some(f.to_string()),
                        Data::Int(i) => Some(i.to_string()),
                        Data::Bool(b) => Some(b.to_string()),
                        Data::DateTime(f) => Some(f.to_string()),
                        _ => None,
                    })
                    .collect();
                if !cells.is_empty() {
                    lines.push(cells.join("\t"));
                }
            }
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_docx() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.docx"
        ));
        if !path.exists() {
            return;
        }
        let text = extract_docx(path).unwrap();
        assert!(!text.is_empty());
    }

    #[test]
    fn test_extract_xlsx() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.xlsx"
        ));
        if !path.exists() {
            return;
        }
        let text = extract_xlsx(path).unwrap();
        assert!(!text.is_empty());
    }

    #[test]
    fn test_extract_odt() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.odt"
        ));
        if !path.exists() {
            return;
        }
        let text = extract_odt(path).unwrap();
        assert!(!text.is_empty());
    }
}
