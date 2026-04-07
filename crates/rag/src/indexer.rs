use crate::chunker;
use crate::config::RagConfig;
use crate::embedder::{OpenRouterVision, VisionDescriber};
use crate::extractor;
use crate::store::{ChunkRecord, RagStore};
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use super_ragondin_sync::model::{NodeType, SyncedRecord};

/// Reconcile Tantivy index against the current set of synced records.
/// - Files in synced but not indexed (or with different md5sum) → index.
/// - Doc IDs in index not present in synced → delete.
///
/// # Errors
/// Returns error if any database or vision operation fails.
#[tracing::instrument(skip(synced, rag_store, describer), fields(synced_count = synced.len(), sync_dir = %sync_dir.display()))]
pub async fn reconcile(
    synced: &[SyncedRecord],
    sync_dir: &Path,
    rag_store: &RagStore,
    describer: &dyn VisionDescriber,
) -> Result<()> {
    let synced_map: HashMap<&str, &str> = synced
        .iter()
        .filter(|r| r.node_type == NodeType::File)
        .filter_map(|r| r.md5sum.as_deref().map(|md5| (r.rel_path.as_str(), md5)))
        .collect();

    let indexed = rag_store.list_indexed()?;
    let indexed_map: HashMap<String, String> =
        indexed.into_iter().map(|d| (d.doc_id, d.md5sum)).collect();

    let delete_count = indexed_map
        .keys()
        .filter(|k| !synced_map.contains_key(k.as_str()))
        .count();
    let reindex_count = synced_map
        .iter()
        .filter(|(rel_path, md5sum)| {
            indexed_map.get(**rel_path).map(String::as_str) != Some(md5sum)
        })
        .count();
    tracing::debug!(
        synced_files = synced_map.len(),
        already_indexed = indexed_map.len(),
        to_delete = delete_count,
        to_index = reindex_count,
        "RAG reconcile plan"
    );

    for doc_id in indexed_map.keys() {
        if !synced_map.contains_key(doc_id.as_str()) {
            tracing::debug!(doc_id, "Removing deleted file from index");
            rag_store.delete_doc(doc_id)?;
        }
    }

    for (rel_path, md5sum) in &synced_map {
        if indexed_map.get(*rel_path).map(String::as_str) == Some(md5sum) {
            tracing::debug!(rel_path, "md5 unchanged, skipping");
            continue;
        }
        let file_path = sync_dir.join(rel_path);
        if !file_path.exists() {
            tracing::warn!(rel_path, "Synced file not found on disk, skipping");
            continue;
        }
        let mtime = file_path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs().cast_signed());

        let mime_type = detect_mime(&file_path);

        rag_store.delete_doc(rel_path)?;

        match index_file(rel_path, &file_path, &mime_type, mtime, md5sum, describer).await {
            Ok(chunks) if chunks.is_empty() => {
                rag_store.upsert_skipped(rel_path, md5sum)?;
                tracing::debug!(
                    rel_path,
                    "File produced no indexable content, marked as skipped"
                );
            }
            Ok(chunks) => {
                rag_store.upsert_chunks(&chunks)?;
                tracing::info!(rel_path, chunks = chunks.len(), "Indexed file");
            }
            Err(e) => {
                tracing::warn!(rel_path, error = %e, "Failed to index file, skipping");
            }
        }
    }

    Ok(())
}

struct NoOpDescriber;

#[async_trait::async_trait]
impl VisionDescriber for NoOpDescriber {
    async fn describe_image(&self, _image_b64: &str, _mime: &str) -> Result<String> {
        anyhow::bail!("no API key configured — image description unavailable")
    }
}

/// Run RAG reconciliation, falling back to a no-op describer when `api_key` is `None`.
///
/// Text files are indexed regardless of API key; image description is skipped.
/// Failures are logged as warnings and do not propagate — reconciliation is non-fatal.
pub async fn reconcile_if_configured(
    api_key: Option<String>,
    db_path: PathBuf,
    synced: &[SyncedRecord],
    sync_dir: &Path,
) {
    let mut rag_config = RagConfig::from_env_with_db_path(db_path);
    if let Some(key) = api_key {
        rag_config.api_key = key;
    }
    let rag_store = match RagStore::open(&rag_config.db_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to open RAG store (non-fatal)");
            return;
        }
    };
    let result = if rag_config.api_key.is_empty() {
        let describer = NoOpDescriber;
        reconcile(synced, sync_dir, &rag_store, &describer).await
    } else {
        let describer = OpenRouterVision::new(rag_config);
        reconcile(synced, sync_dir, &rag_store, &describer).await
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "RAG reconciliation failed (non-fatal)");
    }
}

fn detect_mime(path: &Path) -> String {
    // Try binary magic-byte detection first
    if let Some(mime) = infer::get_from_path(path).ok().flatten() {
        return mime.mime_type().to_string();
    }
    // Fall back to extension-based detection for text formats that have no magic bytes
    match path.extension().and_then(|e| e.to_str()) {
        Some("txt") => "text/plain".to_string(),
        Some("md" | "markdown") => "text/markdown".to_string(),
        Some("csv") => "text/csv".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[tracing::instrument(skip(file_path, mtime, md5sum, describer), fields(rel_path = rel_path, mime_type = mime_type))]
async fn index_file(
    rel_path: &str,
    file_path: &Path,
    mime_type: &str,
    mtime: i64,
    md5sum: &str,
    describer: &dyn VisionDescriber,
) -> Result<Vec<ChunkRecord>> {
    let texts: Vec<String> = match mime_type {
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" => {
            let b64 = crate::extractor::image::read_as_base64(file_path)?;
            let description = describer.describe_image(&b64, mime_type).await?;
            chunker::chunk_text_single(&description)
        }
        _ => {
            let raw = extractor::extract(file_path, mime_type)?;
            match raw {
                None => return Ok(Vec::new()),
                Some(text) if text.is_empty() => {
                    if mime_type == "application/pdf" {
                        match crate::extractor::pdf::render_first_page_as_base64(file_path) {
                            Ok(b64) => {
                                let description =
                                    describer.describe_image(&b64, "image/png").await?;
                                chunker::chunk_text_single(&description)
                            }
                            Err(e) => {
                                tracing::warn!(path = %file_path.display(), error = %e, "Could not render scanned PDF");
                                return Ok(Vec::new());
                            }
                        }
                    } else {
                        return Ok(Vec::new());
                    }
                }
                Some(text) => {
                    tracing::debug!(text_len = text.len(), "extracted text");
                    let chunks = chunker::chunk_text(&text, mime_type)?;
                    tracing::debug!(chunk_count = chunks.len(), "chunked text");
                    chunks
                }
            }
        }
    };

    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let chunk_count = texts.len();
    tracing::debug!(chunk_count, "chunked text");
    let chunks = texts
        .into_iter()
        .enumerate()
        .map(|(i, text)| ChunkRecord {
            id: format!("{rel_path}:{i}"),
            doc_id: rel_path.to_string(),
            mime_type: mime_type.to_string(),
            mtime,
            chunk_index: u32::try_from(i).expect("chunk index fits u32"),
            chunk_text: text,
            md5sum: md5sum.to_string(),
        })
        .collect();

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::VisionDescriber;
    use crate::store::RagStore;
    use async_trait::async_trait;
    use tempfile::tempdir;

    struct StubDescriber;

    #[async_trait]
    impl VisionDescriber for StubDescriber {
        async fn describe_image(&self, _b64: &str, _mime: &str) -> anyhow::Result<String> {
            Ok("stub image description".to_string())
        }
    }

    /// Build a minimal SyncedRecord for a file path.
    fn synced_record(rel_path: &str, md5sum: &str) -> super_ragondin_sync::model::SyncedRecord {
        use super_ragondin_sync::model::{LocalFileId, NodeType, RemoteId, SyncedRecord};
        SyncedRecord {
            local_id: LocalFileId::new(1, 1),
            remote_id: RemoteId(rel_path.to_string()),
            rel_path: rel_path.to_string(),
            md5sum: Some(md5sum.to_string()),
            size: None,
            rev: "1".to_string(),
            node_type: NodeType::File,
            local_name: None,
            local_parent_id: None,
            remote_name: None,
            remote_parent_id: None,
        }
    }

    #[tokio::test]
    async fn test_reconcile_indexes_new_file() {
        let db_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let file_path = sync_dir.path().join("notes").join("hello.txt");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(
            &file_path,
            "Hello, this is a test note with enough content.",
        )
        .unwrap();

        let rag_store = RagStore::open(db_dir.path()).unwrap();
        let describer = StubDescriber;
        let records = vec![synced_record("notes/hello.txt", "abc123")];

        reconcile(&records, sync_dir.path(), &rag_store, &describer)
            .await
            .unwrap();

        let indexed = rag_store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/hello.txt");
    }

    #[tokio::test]
    async fn test_reconcile_skips_unindexable_file_on_second_run() {
        let db_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let file_path = sync_dir.path().join("binary.bin");
        std::fs::write(&file_path, b"\x00\x01\x02\x03\xff\xfe binary junk").unwrap();

        let rag_store = RagStore::open(db_dir.path()).unwrap();
        let describer = StubDescriber;
        let records = vec![synced_record("binary.bin", "deadbeef")];

        reconcile(&records, sync_dir.path(), &rag_store, &describer)
            .await
            .unwrap();

        let indexed = rag_store.list_indexed().unwrap();
        assert_eq!(
            indexed.len(),
            1,
            "skipped file should appear in list_indexed"
        );
        assert_eq!(indexed[0].doc_id, "binary.bin");
        assert_eq!(indexed[0].md5sum, "deadbeef");

        reconcile(&records, sync_dir.path(), &rag_store, &describer)
            .await
            .unwrap();

        let indexed = rag_store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1, "no duplicate entry after second run");
    }

    #[tokio::test]
    async fn test_reconcile_reindexes_previously_skipped_file_when_md5_changes() {
        let db_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();

        let file_path = sync_dir.path().join("doc.bin");
        std::fs::write(&file_path, b"\x00binary").unwrap();
        let rag_store = RagStore::open(db_dir.path()).unwrap();
        let describer = StubDescriber;

        reconcile(
            &[synced_record("doc.bin", "md5_v1")],
            sync_dir.path(),
            &rag_store,
            &describer,
        )
        .await
        .unwrap();

        let indexed = rag_store.list_indexed().unwrap();
        assert_eq!(indexed[0].md5sum, "md5_v1");

        std::fs::write(
            &file_path,
            "Now this is indexable text content for the RAG.",
        )
        .unwrap();

        reconcile(
            &[synced_record("doc.bin", "md5_v2")],
            sync_dir.path(),
            &rag_store,
            &describer,
        )
        .await
        .unwrap();

        let indexed = rag_store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].md5sum, "md5_v2");
    }

    #[tokio::test]
    async fn test_reconcile_if_configured_is_noop_when_api_key_absent() {
        let sync_dir = tempdir().unwrap();
        let db_path = sync_dir.path().join("rag_db");
        reconcile_if_configured(None, db_path, &[], sync_dir.path()).await;
    }

    #[tokio::test]
    async fn test_reconcile_removes_deleted_file() {
        let db_dir = tempdir().unwrap();
        let sync_dir = tempdir().unwrap();
        let rag_store = RagStore::open(db_dir.path()).unwrap();
        let describer = StubDescriber;

        rag_store
            .upsert_chunks(&[crate::store::ChunkRecord {
                id: "old/file.txt:0".to_string(),
                doc_id: "old/file.txt".to_string(),
                mime_type: "text/plain".to_string(),
                mtime: 0,
                chunk_index: 0,
                chunk_text: "old content".to_string(),
                md5sum: "deadbeef".to_string(),
            }])
            .unwrap();

        reconcile(&[], sync_dir.path(), &rag_store, &describer)
            .await
            .unwrap();

        let indexed = rag_store.list_indexed().unwrap();
        assert!(indexed.is_empty());
    }
}
