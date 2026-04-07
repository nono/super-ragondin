use crate::store::{MetadataFilter, RagStore, SearchResult};
use anyhow::Result;

/// # Errors
/// Returns error if the database search fails.
pub fn search(
    question: &str,
    rag_store: &RagStore,
    limit: usize,
    filter: Option<&MetadataFilter>,
) -> Result<Vec<SearchResult>> {
    rag_store.search(question, limit, filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ChunkRecord, RagStore};
    use tempfile::tempdir;

    #[test]
    fn test_search_returns_results() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[ChunkRecord {
                id: "notes/a.md:0".to_string(),
                doc_id: "notes/a.md".to_string(),
                mime_type: "text/plain".to_string(),
                mtime: 1_700_000_000,
                chunk_index: 0,
                chunk_text: "Remote work policy details here.".to_string(),
                md5sum: "abc".to_string(),
            }])
            .unwrap();

        let results = search("remote work policy", &store, 5, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "notes/a.md");
    }
}
