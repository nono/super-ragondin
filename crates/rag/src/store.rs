use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow_array::{
    FixedSizeListArray, Float32Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::arrow::SendableRecordBatchStream;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::Table;

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
    pub async fn open(db_path: &Path) -> Result<Self> {
        let db: Connection = lancedb::connect(db_path.to_str().unwrap())
            .execute()
            .await?;
        let table_names = db.table_names().execute().await?;
        let table = if table_names.contains(&TABLE_NAME.to_string()) {
            db.open_table(TABLE_NAME).execute().await?
        } else {
            let schema = chunks_schema();
            db.create_empty_table(TABLE_NAME, schema)
                .execute()
                .await?
        };
        Ok(Self { table })
    }

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

    pub async fn delete_doc(&self, doc_id: &str) -> Result<()> {
        let safe = doc_id.replace('\'', "\\'");
        self.table
            .delete(&format!("doc_id = '{safe}'"))
            .await?;
        Ok(())
    }

    /// Return one entry per unique doc_id.
    ///
    /// NOTE: All chunks for a given doc_id share the same md5sum (invariant maintained by caller
    /// who always deletes before upserting). This deduplication relies on that invariant.
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

    pub async fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let mut stream: SendableRecordBatchStream = self
            .table
            .vector_search(query_embedding)?
            .limit(limit)
            .execute()
            .await?;

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
}
