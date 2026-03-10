use anyhow::Result;
use chonkie::{
    CharacterTokenizer, RecursiveChunker, SentenceChunker, TokenChunker,
    types::{RecursiveRules},
};

const PROSE_CHUNK_SIZE: usize = 512;
const PROSE_OVERLAP: usize = 50;
const TABLE_CHUNK_SIZE: usize = 256;

/// Split text into chunks appropriate for the given MIME type.
pub fn chunk_text(text: &str, mime_type: &str) -> Result<Vec<String>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    match mime_type {
        "text/plain" | "text/markdown" | "text/x-markdown" => chunk_prose_sentence(text),
        "text/csv"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            chunk_tabular(text)
        }
        // PDF, DOCX, ODT — prose with recursive strategy
        _ => chunk_prose_recursive(text),
    }
}

/// For image descriptions and scanned PDF fallbacks — always one chunk.
pub fn chunk_text_single(text: &str) -> Vec<String> {
    vec![text.to_string()]
}

fn chunk_prose_sentence(text: &str) -> Result<Vec<String>> {
    let tokenizer = CharacterTokenizer::new();
    let chunker = SentenceChunker::new(
        tokenizer,
        PROSE_CHUNK_SIZE,
        PROSE_OVERLAP,
        1,
        12,
        true,
        vec![".", "!", "?", "\n"],
        chonkie::chunker::sentence::DelimiterHandling::Previous,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create SentenceChunker: {e}"))?;
    let chunks = chunker.chunk(text);
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

fn chunk_prose_recursive(text: &str) -> Result<Vec<String>> {
    let tokenizer = CharacterTokenizer::new();
    let chunker = RecursiveChunker::new(tokenizer, PROSE_CHUNK_SIZE, RecursiveRules::default());
    let owned = text.to_string();
    let chunks = chunker.chunk(&owned);
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

fn chunk_tabular(text: &str) -> Result<Vec<String>> {
    let tokenizer = CharacterTokenizer::new();
    let chunker = TokenChunker::new(tokenizer, TABLE_CHUNK_SIZE, 0);
    let chunks = chunker.chunk(text);
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_plaintext_returns_nonempty() {
        let text = "This is the first sentence. This is the second sentence. \
                    And here comes a third one that is a bit longer than the others.";
        let chunks = chunk_text(text, "text/plain").unwrap();
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(!c.is_empty());
        }
    }

    #[test]
    fn test_chunk_spreadsheet_uses_token_chunker() {
        let rows: Vec<String> = (0..20).map(|i| format!("row{i}\tvalue{i}")).collect();
        let text = rows.join("\n");
        let chunks = chunk_text(&text, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet").unwrap();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_image_description_single_chunk() {
        let text = "A photograph of a mountain landscape at sunset.";
        let chunks = chunk_text_single(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }
}
