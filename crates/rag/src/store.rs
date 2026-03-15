use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::Table;
use lancedb::arrow::SendableRecordBatchStream;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

const TABLE_NAME: &str = "chunks";
const EMBED_DIM: i32 = 3072;

pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_index: u32,
    pub chunk_text: String,
    pub md5sum: String,
    pub embedding: Vec<f32>,
}

pub struct IndexedDoc {
    pub doc_id: String,
    pub md5sum: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_text: String,
}

pub enum DocSort {
    Recent,
    Oldest,
}

/// One entry per document returned by `list_docs()`.
#[derive(Debug, Clone)]
pub struct DocInfo {
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
}

/// One chunk entry returned by `get_chunks()`.
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub chunk_index: u32,
    pub chunk_text: String,
}

/// Filter for metadata-based queries. All fields are optional.
/// Constructed in Rust from validated inputs — never from raw user/JS strings.
pub struct MetadataFilter {
    pub mime_type: Option<String>,
    /// Matched as `doc_id LIKE 'prefix/%'`. Trailing slash added if absent.
    pub path_prefix: Option<String>,
    /// Unix timestamp (seconds). Matched as `mtime > after`.
    pub after: Option<i64>,
    /// Unix timestamp (seconds). Matched as `mtime < before`.
    pub before: Option<i64>,
}

impl MetadataFilter {
    /// Build a `LanceDB` SQL WHERE clause from this filter.
    /// Returns `None` if no fields are set.
    #[must_use]
    pub fn to_where_clause(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();

        if let Some(mime) = &self.mime_type {
            let safe = mime.replace('\'', "\\'");
            parts.push(format!("mime_type = '{safe}'"));
        }
        if let Some(prefix) = &self.path_prefix {
            let prefix_with_slash = if prefix.ends_with('/') {
                prefix.clone()
            } else {
                format!("{prefix}/")
            };
            // Escape SQL special chars and LIKE wildcard chars
            let safe = prefix_with_slash
                .replace('\'', "\\'")
                .replace('%', "\\%")
                .replace('_', "\\_");
            parts.push(format!("doc_id LIKE '{safe}%' ESCAPE '\\'"));
        }
        if let Some(after) = self.after {
            parts.push(format!("mtime > {after}"));
        }
        if let Some(before) = self.before {
            parts.push(format!("mtime < {before}"));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }
}

fn chunks_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("mtime", DataType::Int64, false),
        Field::new("chunk_index", DataType::UInt32, false),
        Field::new("chunk_text", DataType::Utf8, false),
        Field::new("md5sum", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBED_DIM,
            ),
            false,
        ),
    ]))
}

pub struct RagStore {
    table: Table,
}

impl RagStore {
    /// # Errors
    /// Returns error if the database connection or table operation fails.
    ///
    /// # Panics
    /// Panics if `db_path` contains non-UTF-8 characters.
    pub async fn open(db_path: &Path) -> Result<Self> {
        let db: Connection = lancedb::connect(db_path.to_str().unwrap())
            .execute()
            .await?;
        let table_names = db.table_names().execute().await?;
        let table = if table_names.contains(&TABLE_NAME.to_string()) {
            db.open_table(TABLE_NAME).execute().await?
        } else {
            let schema = chunks_schema();
            db.create_empty_table(TABLE_NAME, schema).execute().await?
        };
        Ok(Self { table })
    }

    /// Insert chunks into the store.
    ///
    /// **Callers must call [`delete_doc`] before upserting** to avoid duplicate chunks.
    /// This is an append-only operation; `LanceDB` does not deduplicate on insert.
    ///
    /// # Errors
    /// Returns error if the Arrow data construction or database insert fails.
    #[allow(clippy::similar_names)]
    pub async fn upsert_chunks(&self, chunks: &[ChunkRecord]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let schema = chunks_schema();

        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        let doc_ids: Vec<&str> = chunks.iter().map(|c| c.doc_id.as_str()).collect();
        let mimes: Vec<&str> = chunks.iter().map(|c| c.mime_type.as_str()).collect();
        let mtimes: Vec<i64> = chunks.iter().map(|c| c.mtime).collect();
        let indices: Vec<u32> = chunks.iter().map(|c| c.chunk_index).collect();
        let texts: Vec<&str> = chunks.iter().map(|c| c.chunk_text.as_str()).collect();
        let md5sums: Vec<&str> = chunks.iter().map(|c| c.md5sum.as_str()).collect();

        let flat_embeddings: Vec<f32> = chunks
            .iter()
            .flat_map(|c| c.embedding.iter().copied())
            .collect();
        let embedding_values = Arc::new(Float32Array::from(flat_embeddings));
        let embedding_col = Arc::new(FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            EMBED_DIM,
            embedding_values,
            None,
        )?);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(StringArray::from(doc_ids)),
                Arc::new(StringArray::from(mimes)),
                Arc::new(Int64Array::from(mtimes)),
                Arc::new(UInt32Array::from(indices)),
                Arc::new(StringArray::from(texts)),
                Arc::new(StringArray::from(md5sums)),
                embedding_col,
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        self.table.add(batches).execute().await?;
        Ok(())
    }

    /// # Errors
    /// Returns error if the database delete operation fails.
    pub async fn delete_doc(&self, doc_id: &str) -> Result<()> {
        let safe = doc_id.replace('\'', "\\'");
        self.table.delete(&format!("doc_id = '{safe}'")).await?;
        Ok(())
    }

    /// Return one entry per unique `doc_id`.
    ///
    /// NOTE: All chunks for a given `doc_id` share the same md5sum (invariant maintained by caller
    /// who always deletes before upserting). This deduplication relies on that invariant.
    ///
    /// # Errors
    /// Returns error if the database query fails.
    ///
    /// # Panics
    /// Panics if the expected columns are not present in the result batch.
    pub async fn list_indexed(&self) -> Result<Vec<IndexedDoc>> {
        let mut stream: SendableRecordBatchStream = self
            .table
            .query()
            .select(Select::Columns(vec![
                "doc_id".to_string(),
                "md5sum".to_string(),
            ]))
            .execute()
            .await?;

        let mut seen: HashSet<String> = HashSet::new();
        let mut result = Vec::new();
        while let Some(batch) = stream.try_next().await? {
            let doc_ids = batch
                .column_by_name("doc_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let md5sums = batch
                .column_by_name("md5sum")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..batch.num_rows() {
                let doc_id = doc_ids.value(i).to_string();
                if seen.insert(doc_id.clone()) {
                    result.push(IndexedDoc {
                        doc_id,
                        md5sum: md5sums.value(i).to_string(),
                    });
                }
            }
        }
        Ok(result)
    }

    /// # Errors
    /// Returns error if the vector search query fails.
    ///
    /// # Panics
    /// Panics if the expected columns are not present in the result batch.
    #[allow(clippy::similar_names)]
    pub async fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        let mut query = self.table.vector_search(query_embedding)?.limit(limit);
        if let Some(f) = filter
            && let Some(clause) = f.to_where_clause()
        {
            query = query.only_if(clause);
        }
        let mut stream: SendableRecordBatchStream = query.execute().await?;

        let mut results = Vec::new();
        while let Some(batch) = stream.try_next().await? {
            let doc_ids = batch
                .column_by_name("doc_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let mimes = batch
                .column_by_name("mime_type")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let mtimes = batch
                .column_by_name("mtime")
                .unwrap()
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            let texts = batch
                .column_by_name("chunk_text")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..batch.num_rows() {
                results.push(SearchResult {
                    doc_id: doc_ids.value(i).to_string(),
                    mime_type: mimes.value(i).to_string(),
                    mtime: mtimes.value(i),
                    chunk_text: texts.value(i).to_string(),
                });
            }
        }
        Ok(results)
    }

    /// Return one entry per unique `doc_id`, sorted by `mtime`.
    ///
    /// De-duplication is client-side (keeps the row with the highest mtime per `doc_id`).
    /// `limit` is applied after de-duplication.
    ///
    /// # Errors
    /// Returns error if the database query fails.
    ///
    /// # Panics
    /// Panics if expected columns are absent from the result batch.
    #[allow(clippy::similar_names)]
    pub async fn list_docs(
        &self,
        filter: Option<&MetadataFilter>,
        sort: DocSort,
        limit: Option<usize>,
    ) -> Result<Vec<DocInfo>> {
        let mut q = self.table.query().select(Select::Columns(vec![
            "doc_id".to_string(),
            "mime_type".to_string(),
            "mtime".to_string(),
        ]));
        if let Some(f) = filter
            && let Some(clause) = f.to_where_clause()
        {
            q = q.only_if(clause);
        }
        let mut stream: SendableRecordBatchStream = q.execute().await?;

        let mut map: HashMap<String, DocInfo> = HashMap::new();
        while let Some(batch) = stream.try_next().await? {
            let doc_ids = batch
                .column_by_name("doc_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let mimes = batch
                .column_by_name("mime_type")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let mtimes = batch
                .column_by_name("mtime")
                .unwrap()
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            for i in 0..batch.num_rows() {
                let doc_id = doc_ids.value(i).to_string();
                let mtime = mtimes.value(i);
                map.entry(doc_id.clone())
                    .and_modify(|e| {
                        if mtime > e.mtime {
                            e.mtime = mtime;
                        }
                    })
                    .or_insert_with(|| DocInfo {
                        doc_id,
                        mime_type: mimes.value(i).to_string(),
                        mtime,
                    });
            }
        }

        let mut docs: Vec<DocInfo> = map.into_values().collect();
        docs.sort_by(|a, b| match sort {
            DocSort::Recent => b.mtime.cmp(&a.mtime),
            DocSort::Oldest => a.mtime.cmp(&b.mtime),
        });
        if let Some(n) = limit {
            docs.truncate(n);
        }
        Ok(docs)
    }

    /// Return the `doc_id`s of documents modified after `since`, most recent first.
    ///
    /// Uses the existing [`MetadataFilter`] `after` field (Unix timestamp seconds).
    /// De-duplicates across chunks and caps results at 20.
    ///
    /// # Errors
    /// Returns error if the database query fails.
    pub async fn list_recent(&self, since: std::time::SystemTime) -> Result<Vec<String>> {
        let since_secs = since
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .cast_signed();
        let filter = MetadataFilter {
            mime_type: None,
            path_prefix: None,
            after: Some(since_secs),
            before: None,
        };
        let docs = self
            .list_docs(Some(&filter), DocSort::Recent, Some(20))
            .await?;
        Ok(docs.into_iter().map(|d| d.doc_id).collect())
    }

    /// Return all chunks for a document, ordered by `chunk_index`.
    ///
    /// # Errors
    /// Returns error if the database query fails.
    ///
    /// # Panics
    /// Panics if expected columns are absent from the result batch.
    pub async fn get_chunks(&self, doc_id: &str) -> Result<Vec<ChunkInfo>> {
        let safe = doc_id.replace('\'', "\\'");
        let mut stream: SendableRecordBatchStream = self
            .table
            .query()
            .only_if(format!("doc_id = '{safe}'"))
            .select(Select::Columns(vec![
                "chunk_index".to_string(),
                "chunk_text".to_string(),
            ]))
            .execute()
            .await?;

        let mut chunks = Vec::new();
        while let Some(batch) = stream.try_next().await? {
            let indices = batch
                .column_by_name("chunk_index")
                .unwrap()
                .as_any()
                .downcast_ref::<UInt32Array>()
                .unwrap();
            let texts = batch
                .column_by_name("chunk_text")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..batch.num_rows() {
                chunks.push(ChunkInfo {
                    chunk_index: indices.value(i),
                    chunk_text: texts.value(i).to_string(),
                });
            }
        }
        chunks.sort_by_key(|c| c.chunk_index);
        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_chunk(doc_id: &str, chunk_index: u32, text: &str) -> ChunkRecord {
        ChunkRecord {
            id: format!("{doc_id}:{chunk_index}"),
            doc_id: doc_id.to_string(),
            mime_type: "text/plain".to_string(),
            mtime: 1_700_000_000,
            chunk_index,
            chunk_text: text.to_string(),
            md5sum: "abc123".to_string(),
            embedding: vec![0.0_f32; 3072],
        }
    }

    #[tokio::test]
    async fn test_upsert_and_list_indexed() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        let chunk = make_chunk("notes/test.md", 0, "hello world");
        store.upsert_chunks(&[chunk]).await.unwrap();

        let indexed = store.list_indexed().await.unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/test.md");
        assert_eq!(indexed[0].md5sum, "abc123");
    }

    #[test]
    fn test_metadata_filter_where_clause() {
        let filter = MetadataFilter {
            mime_type: Some("application/pdf".to_string()),
            path_prefix: Some("work".to_string()), // no trailing slash
            after: Some(1_700_000_000),
            before: None,
        };
        let clause = filter.to_where_clause().unwrap();
        assert!(clause.contains("mime_type = 'application/pdf'"));
        assert!(clause.contains("doc_id LIKE 'work/%' ESCAPE '\\'")); // trailing slash added
        assert!(clause.contains("mtime > 1700000000"));

        let empty = MetadataFilter {
            mime_type: None,
            path_prefix: None,
            after: None,
            before: None,
        };
        assert!(empty.to_where_clause().is_none());
    }

    #[test]
    fn test_metadata_filter_escapes_single_quotes() {
        let filter = MetadataFilter {
            mime_type: Some("text/plain".to_string()),
            path_prefix: Some("O'Brien/notes".to_string()),
            after: None,
            before: None,
        };
        let clause = filter.to_where_clause().unwrap();
        // single quotes in path_prefix must be escaped
        assert!(clause.contains("O\\'Brien"));
    }

    #[test]
    fn test_metadata_filter_escapes_like_wildcards() {
        let filter = MetadataFilter {
            mime_type: None,
            path_prefix: Some("my_notes%data".to_string()),
            after: None,
            before: None,
        };
        let clause = filter.to_where_clause().unwrap();
        assert!(clause.contains("\\_"));
        assert!(clause.contains("\\%"));
        assert!(clause.contains("ESCAPE"));
    }

    #[tokio::test]
    async fn test_list_docs_dedup_and_sort() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();

        // Two chunks for doc a (mtime 200), one for doc b (mtime 100)
        let mut c0 = make_chunk("notes/a.md", 0, "chunk0");
        c0.mtime = 200;
        let mut c1 = make_chunk("notes/a.md", 1, "chunk1");
        c1.mtime = 200;
        let mut c2 = make_chunk("work/b.md", 0, "chunk2");
        c2.mtime = 100;
        store.upsert_chunks(&[c0, c1, c2]).await.unwrap();

        // Recent first — should be deduplicated to 2 docs
        let docs = store.list_docs(None, DocSort::Recent, None).await.unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].doc_id, "notes/a.md"); // higher mtime first
        assert_eq!(docs[1].doc_id, "work/b.md");

        // Filter by path prefix
        let filtered = store
            .list_docs(
                Some(&MetadataFilter {
                    path_prefix: Some("work".to_string()),
                    mime_type: None,
                    after: None,
                    before: None,
                }),
                DocSort::Recent,
                None,
            )
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].doc_id, "work/b.md");

        // Limit
        let limited = store
            .list_docs(None, DocSort::Recent, Some(1))
            .await
            .unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[tokio::test]
    async fn test_get_chunks_ordered() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        store
            .upsert_chunks(&[
                make_chunk("notes/a.md", 1, "second"),
                make_chunk("notes/a.md", 0, "first"),
            ])
            .await
            .unwrap();

        let chunks = store.get_chunks("notes/a.md").await.unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].chunk_text, "first");
        assert_eq!(chunks[1].chunk_index, 1);
        assert_eq!(chunks[1].chunk_text, "second");

        let empty = store.get_chunks("nonexistent.md").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_search_with_mime_filter() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();

        let mut pdf_chunk = make_chunk("docs/report.pdf", 0, "quarterly results");
        pdf_chunk.mime_type = "application/pdf".to_string();
        let txt_chunk = make_chunk("notes/a.md", 0, "quarterly results");
        // txt_chunk mime_type defaults to "text/plain" from make_chunk

        store.upsert_chunks(&[pdf_chunk, txt_chunk]).await.unwrap();

        let filter = MetadataFilter {
            mime_type: Some("application/pdf".to_string()),
            path_prefix: None,
            after: None,
            before: None,
        };
        let results = store
            .search(&vec![0.0_f32; 3072], 5, Some(&filter))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "docs/report.pdf");
    }

    #[tokio::test]
    async fn test_delete_by_doc() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).await.unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/a.md", 0, "aaa")])
            .await
            .unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/b.md", 0, "bbb")])
            .await
            .unwrap();
        store.delete_doc("notes/a.md").await.unwrap();

        let indexed = store.list_indexed().await.unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/b.md");
    }

    fn unix_secs(t: std::time::SystemTime) -> i64 {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    async fn make_store() -> (RagStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RagStore::open(dir.path()).await.expect("open");
        (store, dir)
    }

    fn dummy_chunk(doc_id: &str, mtime: std::time::SystemTime) -> ChunkRecord {
        ChunkRecord {
            id: format!("{doc_id}-0"),
            doc_id: doc_id.to_string(),
            mime_type: "text/plain".to_string(),
            mtime: unix_secs(mtime),
            chunk_index: 0,
            chunk_text: "hello".to_string(),
            md5sum: "abc".to_string(),
            embedding: vec![0.0_f32; 3072],
        }
    }

    #[tokio::test]
    async fn test_list_recent_returns_only_recent_docs() {
        let (store, _dir) = make_store().await;
        let now = std::time::SystemTime::now();
        let recent = now - std::time::Duration::from_secs(60);
        let old = now - std::time::Duration::from_secs(3600);

        store
            .upsert_chunks(&[dummy_chunk("docs/new.md", recent)])
            .await
            .unwrap();
        store
            .upsert_chunks(&[dummy_chunk("docs/old.md", old)])
            .await
            .unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();

        assert_eq!(result, vec!["docs/new.md".to_string()]);
    }

    #[tokio::test]
    async fn test_list_recent_empty_when_nothing_recent() {
        let (store, _dir) = make_store().await;
        let now = std::time::SystemTime::now();
        let old = now - std::time::Duration::from_secs(3600);

        store
            .upsert_chunks(&[dummy_chunk("docs/old.md", old)])
            .await
            .unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_list_recent_deduplicates_doc_ids() {
        let (store, _dir) = make_store().await;
        let now = std::time::SystemTime::now();
        let recent = now - std::time::Duration::from_secs(60);

        let mut c0 = dummy_chunk("docs/multi.md", recent);
        let mut c1 = dummy_chunk("docs/multi.md", recent);
        c0.id = "docs/multi.md-0".to_string();
        c0.chunk_index = 0;
        c1.id = "docs/multi.md-1".to_string();
        c1.chunk_index = 1;
        store.upsert_chunks(&[c0, c1]).await.unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "docs/multi.md");
    }

    #[tokio::test]
    async fn test_list_recent_caps_at_20() {
        let (store, _dir) = make_store().await;
        let now = std::time::SystemTime::now();
        let recent = now - std::time::Duration::from_secs(60);

        let chunks: Vec<ChunkRecord> = (0..25_u32)
            .map(|i| dummy_chunk(&format!("docs/file{i}.md"), recent))
            .collect();
        store.upsert_chunks(&chunks).await.unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).await.unwrap();
        assert!(
            result.len() <= 20,
            "got {} results, expected <= 20",
            result.len()
        );
    }
}
