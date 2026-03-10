use crate::embedder::Embedder;
use crate::store::{RagStore, SearchResult};
use anyhow::Result;

pub async fn search(
    question: &str,
    rag_store: &RagStore,
    embedder: &dyn Embedder,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let embeddings = embedder.embed_texts(&[question.to_string()]).await?;
    let query_vec = embeddings.into_iter().next().unwrap_or_default();
    rag_store.search(&query_vec, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::Embedder;
    use crate::store::{ChunkRecord, RagStore};
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct StubEmbedder;

    #[async_trait]
    impl Embedder for StubEmbedder {
        async fn embed_texts(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0_f32; 3072]).collect())
        }
        async fn describe_image(&self, _b64: &str, _mime: &str) -> anyhow::Result<String> {
            Ok("stub".to_string())
        }
    }

    #[tokio::test]
    async fn test_search_returns_results() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        store
            .upsert_chunks(&[ChunkRecord {
                id: "notes/a.md:0".to_string(),
                doc_id: "notes/a.md".to_string(),
                mime_type: "text/plain".to_string(),
                mtime: 1_700_000_000,
                chunk_index: 0,
                chunk_text: "Remote work policy details here.".to_string(),
                md5sum: "abc".to_string(),
                embedding: vec![0.0_f32; 3072],
            }])
            .await
            .unwrap();

        let embedder = StubEmbedder;
        let results = search("remote work policy", &store, &embedder, 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "notes/a.md");
    }
}
