use anyhow::Result;
use chonkie::{RecursiveChunker, SentenceChunker, TokenChunker, types::RecursiveRules};

const PROSE_CHUNK_SIZE: usize = 2000;
const PROSE_OVERLAP: usize = 200;
const TABLE_CHUNK_SIZE: usize = 1000;

/// Minimum sentence length for `SentenceChunker` (in tokens).
const SENTENCE_MIN_CHARS: usize = 1;
/// Maximum sentences per chunk before forced split.
const SENTENCE_MAX_PER_CHUNK: usize = 48;
/// Delimiters used to detect sentence boundaries.
const SENTENCE_DELIMITERS: &[&str] = &[".", "!", "?", "\n"];

/// A tokenizer backed by tiktoken's `cl100k_base` encoding (GPT-4 / text-embedding-3).
///
/// Implements chonkie's `Tokenizer` trait so that chunk sizes are measured in
/// real BPE tokens rather than raw character counts.
struct TiktokenTokenizer {
    bpe: tiktoken_rs::CoreBPE,
}

impl TiktokenTokenizer {
    fn new() -> Result<Self> {
        let bpe = tiktoken_rs::cl100k_base()
            .map_err(|e| anyhow::anyhow!("Failed to load cl100k_base tokenizer: {e}"))?;
        Ok(Self { bpe })
    }
}

impl chonkie::Tokenizer for TiktokenTokenizer {
    fn encode(&self, text: &str) -> Vec<usize> {
        self.bpe
            .encode_with_special_tokens(text)
            .into_iter()
            .map(|t| t as usize)
            .collect()
    }

    fn decode(&self, tokens: &[usize]) -> String {
        // Token IDs come from our own `encode` and are valid u32 values.
        #[allow(clippy::cast_possible_truncation)]
        let ranks: Vec<tiktoken_rs::Rank> =
            tokens.iter().map(|&t| t as tiktoken_rs::Rank).collect();
        self.bpe.decode(ranks).unwrap_or_default()
    }
}

/// Split text into chunks appropriate for the given MIME type.
///
/// # Errors
///
/// Returns an error if the tokenizer or chunker cannot be initialized.
pub fn chunk_text(text: &str, mime_type: &str) -> Result<Vec<String>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    match mime_type {
        "text/plain" | "text/markdown" | "text/x-markdown" => chunk_prose_sentence(text),
        "text/csv" | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            chunk_tabular(text)
        }
        // PDF, DOCX, ODT — prose with recursive strategy
        _ => chunk_prose_recursive(text),
    }
}

/// For image descriptions and scanned PDF fallbacks — always one chunk.
#[must_use]
pub fn chunk_text_single(text: &str) -> Vec<String> {
    vec![text.to_string()]
}

fn chunk_prose_sentence(text: &str) -> Result<Vec<String>> {
    let tokenizer = TiktokenTokenizer::new()?;
    let delimiters: Vec<String> = SENTENCE_DELIMITERS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let chunker = SentenceChunker::new(
        tokenizer,
        PROSE_CHUNK_SIZE,
        PROSE_OVERLAP,
        SENTENCE_MIN_CHARS,
        SENTENCE_MAX_PER_CHUNK,
        true,
        delimiters,
        chonkie::chunker::sentence::DelimiterHandling::Previous,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create SentenceChunker: {e}"))?;
    let chunks = chunker.chunk(text);
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

fn chunk_prose_recursive(text: &str) -> Result<Vec<String>> {
    let tokenizer = TiktokenTokenizer::new()?;
    let chunker = RecursiveChunker::new(tokenizer, PROSE_CHUNK_SIZE, RecursiveRules::default());
    let owned = text.to_string();
    let chunks = chunker.chunk(&owned);
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

fn chunk_tabular(text: &str) -> Result<Vec<String>> {
    let tokenizer = TiktokenTokenizer::new()?;
    let chunker = TokenChunker::new(tokenizer, TABLE_CHUNK_SIZE, 0);
    let chunks = chunker.chunk(text);
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_plaintext_returns_nonempty() -> Result<()> {
        let text = "This is the first sentence. This is the second sentence. \
                    And here comes a third one that is a bit longer than the others.";
        let chunks = chunk_text(text, "text/plain")?;
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(!c.is_empty());
        }
        Ok(())
    }

    #[test]
    fn test_chunk_spreadsheet_uses_token_chunker() -> Result<()> {
        let rows: Vec<String> = (0..20).map(|i| format!("row{i}\tvalue{i}")).collect();
        let text = rows.join("\n");
        let chunks = chunk_text(
            &text,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )?;
        assert!(!chunks.is_empty());
        Ok(())
    }

    #[test]
    fn test_chunk_image_description_single_chunk() {
        let text = "A photograph of a mountain landscape at sunset.";
        let chunks = chunk_text_single(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }
}
