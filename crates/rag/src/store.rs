use std::collections::{HashMap, HashSet};
use std::ops::Bound;
use std::path::Path;

use anyhow::Result;
use tantivy::collector::TopDocs;
use tantivy::query::{
    AllQuery, BooleanQuery, Occur, Query, QueryParser, RangeQuery, RegexQuery, TermQuery,
};
use tantivy::schema::{
    Field, IndexRecordOption, NumericOptions, OwnedValue, STORED, STRING, Schema, TEXT,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};

pub struct ChunkRecord {
    pub id: String,
    pub doc_id: String,
    pub mime_type: String,
    pub mtime: i64,
    pub chunk_index: u32,
    pub chunk_text: String,
    pub md5sum: String,
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

#[derive(Clone, Copy)]
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

fn escape_regex(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

const fn owned_value_as_str(v: &OwnedValue) -> &str {
    match v {
        OwnedValue::Str(s) => s.as_str(),
        _ => "",
    }
}

const fn owned_value_as_i64(v: &OwnedValue) -> i64 {
    match v {
        OwnedValue::I64(n) => *n,
        _ => 0,
    }
}

const fn owned_value_as_u64(v: &OwnedValue) -> u64 {
    match v {
        OwnedValue::U64(n) => *n,
        _ => 0,
    }
}

/// Filter for metadata-based queries. All fields are optional.
/// Constructed in Rust from validated inputs — never from raw user/JS strings.
pub struct MetadataFilter {
    pub mime_type: Option<String>,
    /// Matched as a prefix on `doc_id`. Trailing slash added if absent.
    pub path_prefix: Option<String>,
    /// Unix timestamp (seconds). Matched as `mtime > after`.
    pub after: Option<i64>,
    /// Unix timestamp (seconds). Matched as `mtime < before`.
    pub before: Option<i64>,
}

impl MetadataFilter {
    /// Build a Tantivy query from this filter.
    /// Returns `None` if no fields are set.
    #[must_use]
    pub(crate) fn to_tantivy_query(&self, fields: &Fields) -> Option<Box<dyn Query>> {
        let mut parts: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        if let Some(mime) = &self.mime_type {
            parts.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(fields.mime_type, mime),
                    IndexRecordOption::Basic,
                )),
            ));
        }

        if let Some(prefix) = &self.path_prefix {
            let prefix_with_slash = if prefix.ends_with('/') {
                prefix.clone()
            } else {
                format!("{prefix}/")
            };
            let escaped = escape_regex(&prefix_with_slash);
            let pattern = format!("{escaped}.*");
            if let Ok(rq) = RegexQuery::from_pattern(&pattern, fields.doc_id) {
                parts.push((Occur::Must, Box::new(rq)));
            }
        }

        if let Some(after) = self.after {
            parts.push((
                Occur::Must,
                Box::new(RangeQuery::new_i64_bounds(
                    "mtime".to_string(),
                    Bound::Excluded(after),
                    Bound::Unbounded,
                )),
            ));
        }

        if let Some(before) = self.before {
            parts.push((
                Occur::Must,
                Box::new(RangeQuery::new_i64_bounds(
                    "mtime".to_string(),
                    Bound::Unbounded,
                    Bound::Excluded(before),
                )),
            ));
        }

        if parts.is_empty() {
            None
        } else {
            Some(Box::new(BooleanQuery::new(parts)))
        }
    }
}

pub(crate) struct Fields {
    id: Field,
    doc_id: Field,
    mime_type: Field,
    mtime: Field,
    chunk_index: Field,
    chunk_text: Field,
    md5sum: Field,
}

pub struct RagStore {
    index: Index,
    reader: IndexReader,
    fields: Fields,
    skipped_path: std::path::PathBuf,
}

impl RagStore {
    /// # Errors
    /// Returns error if the index directory creation or Tantivy index operation fails.
    pub fn open(db_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(db_path)?;

        let mut schema_builder = Schema::builder();
        let id = schema_builder.add_text_field("id", STRING | STORED);
        let doc_id = schema_builder.add_text_field("doc_id", STRING | STORED);
        let mime_type = schema_builder.add_text_field("mime_type", STRING | STORED);
        let mtime = schema_builder.add_i64_field(
            "mtime",
            NumericOptions::default().set_indexed().set_stored(),
        );
        let chunk_index = schema_builder.add_u64_field(
            "chunk_index",
            NumericOptions::default().set_indexed().set_stored(),
        );
        let chunk_text = schema_builder.add_text_field("chunk_text", TEXT | STORED);
        let md5sum = schema_builder.add_text_field("md5sum", STORED);
        let schema = schema_builder.build();

        let dir = tantivy::directory::MmapDirectory::open(db_path)?;
        let index = Index::open_or_create(dir, schema)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let skipped_path = db_path.join("skipped_docs.json");

        Ok(Self {
            index,
            reader,
            fields: Fields {
                id,
                doc_id,
                mime_type,
                mtime,
                chunk_index,
                chunk_text,
                md5sum,
            },
            skipped_path,
        })
    }

    /// Insert chunks into the store.
    ///
    /// **Callers must call [`delete_doc`] before upserting** to avoid duplicate chunks.
    ///
    /// # Errors
    /// Returns error if the index write or commit fails.
    pub fn upsert_chunks(&self, chunks: &[ChunkRecord]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let mut writer: IndexWriter = self.index.writer(50_000_000)?;
        for c in chunks {
            writer.add_document(doc!(
                self.fields.id => c.id.as_str(),
                self.fields.doc_id => c.doc_id.as_str(),
                self.fields.mime_type => c.mime_type.as_str(),
                self.fields.mtime => c.mtime,
                self.fields.chunk_index => u64::from(c.chunk_index),
                self.fields.chunk_text => c.chunk_text.as_str(),
                self.fields.md5sum => c.md5sum.as_str(),
            ))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// # Errors
    /// Returns error if the index delete or commit fails.
    pub fn delete_doc(&self, doc_id: &str) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(50_000_000)?;
        let term = Term::from_field_text(self.fields.doc_id, doc_id);
        writer.delete_term(term);
        writer.commit()?;
        self.reader.reload()?;
        self.remove_skipped(doc_id)?;
        Ok(())
    }

    /// Record a file that produced no indexable content.
    ///
    /// # Errors
    /// Returns error if the JSON file operation fails.
    pub fn upsert_skipped(&self, doc_id: &str, md5sum: &str) -> Result<()> {
        let mut map = self.load_skipped()?;
        map.insert(doc_id.to_string(), md5sum.to_string());
        self.save_skipped(&map)
    }

    /// Return one entry per unique `doc_id` — both properly indexed docs and skipped docs.
    ///
    /// # Errors
    /// Returns error if the index read or JSON file operation fails.
    pub fn list_indexed(&self) -> Result<Vec<IndexedDoc>> {
        let searcher = self.reader.searcher();
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for segment_reader in searcher.segment_readers() {
            let store_reader = segment_reader.get_store_reader(1)?;
            for doc_id_ordinal in 0..segment_reader.num_docs() {
                let doc: TantivyDocument = store_reader.get(doc_id_ordinal)?;
                let doc_id_val = doc
                    .get_first(self.fields.doc_id)
                    .map(owned_value_as_str)
                    .unwrap_or_default()
                    .to_string();
                if seen.insert(doc_id_val.clone()) {
                    let md5sum_val = doc
                        .get_first(self.fields.md5sum)
                        .map(owned_value_as_str)
                        .unwrap_or_default()
                        .to_string();
                    result.push(IndexedDoc {
                        doc_id: doc_id_val,
                        md5sum: md5sum_val,
                    });
                }
            }
        }

        // Also include skipped docs
        let skipped = self.load_skipped()?;
        for (doc_id, md5sum) in skipped {
            if seen.insert(doc_id.clone()) {
                result.push(IndexedDoc { doc_id, md5sum });
            }
        }

        Ok(result)
    }

    /// # Errors
    /// Returns error if the search query fails.
    #[allow(clippy::similar_names)]
    pub fn search(
        &self,
        query_str: &str,
        limit: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchResult>> {
        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.fields.chunk_text]);
        let text_query = query_parser.parse_query(query_str)?;

        let final_query: Box<dyn Query> = if let Some(f) = filter
            && let Some(filter_query) = f.to_tantivy_query(&self.fields)
        {
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, text_query),
                (Occur::Must, filter_query),
            ]))
        } else {
            text_query
        };

        let top_docs = searcher.search(&*final_query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let doc_id = doc
                .get_first(self.fields.doc_id)
                .map(owned_value_as_str)
                .unwrap_or_default()
                .to_string();
            let mime_type = doc
                .get_first(self.fields.mime_type)
                .map(owned_value_as_str)
                .unwrap_or_default()
                .to_string();
            let mtime = doc
                .get_first(self.fields.mtime)
                .map(owned_value_as_i64)
                .unwrap_or_default();
            let chunk_text = doc
                .get_first(self.fields.chunk_text)
                .map(owned_value_as_str)
                .unwrap_or_default()
                .to_string();
            results.push(SearchResult {
                doc_id,
                mime_type,
                mtime,
                chunk_text,
            });
        }
        Ok(results)
    }

    /// Return one entry per unique `doc_id`, sorted by `mtime`.
    ///
    /// De-duplication is client-side (keeps the row with the highest mtime per `doc_id`).
    /// `limit` is applied after de-duplication.
    ///
    /// # Errors
    /// Returns error if the search query fails.
    #[allow(clippy::similar_names)]
    pub fn list_docs(
        &self,
        filter: Option<&MetadataFilter>,
        sort: DocSort,
        limit: Option<usize>,
    ) -> Result<Vec<DocInfo>> {
        let searcher = self.reader.searcher();

        let query: Box<dyn Query> = if let Some(f) = filter
            && let Some(fq) = f.to_tantivy_query(&self.fields)
        {
            fq
        } else {
            Box::new(AllQuery)
        };

        let top_docs = searcher.search(&*query, &TopDocs::with_limit(100_000))?;

        let mut map: HashMap<String, DocInfo> = HashMap::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let doc_id = doc
                .get_first(self.fields.doc_id)
                .map(owned_value_as_str)
                .unwrap_or_default()
                .to_string();
            let mime_type = doc
                .get_first(self.fields.mime_type)
                .map(owned_value_as_str)
                .unwrap_or_default()
                .to_string();
            let mtime = doc
                .get_first(self.fields.mtime)
                .map(owned_value_as_i64)
                .unwrap_or_default();
            map.entry(doc_id.clone())
                .and_modify(|e| {
                    if mtime > e.mtime {
                        e.mtime = mtime;
                    }
                })
                .or_insert_with(|| DocInfo {
                    doc_id,
                    mime_type,
                    mtime,
                });
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
    /// # Errors
    /// Returns error if the search query fails.
    pub fn list_recent(&self, since: std::time::SystemTime) -> Result<Vec<String>> {
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
        let docs = self.list_docs(Some(&filter), DocSort::Recent, Some(20))?;
        Ok(docs.into_iter().map(|d| d.doc_id).collect())
    }

    /// Return all chunks for a document, ordered by `chunk_index`.
    ///
    /// # Errors
    /// Returns error if the search query fails.
    pub fn get_chunks(&self, doc_id: &str) -> Result<Vec<ChunkInfo>> {
        let searcher = self.reader.searcher();
        let query = TermQuery::new(
            Term::from_field_text(self.fields.doc_id, doc_id),
            IndexRecordOption::Basic,
        );
        let top_docs = searcher.search(&query, &TopDocs::with_limit(100_000))?;

        let mut chunks = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            #[allow(clippy::cast_possible_truncation)]
            let chunk_index = doc
                .get_first(self.fields.chunk_index)
                .map(owned_value_as_u64)
                .unwrap_or_default() as u32;
            let chunk_text = doc
                .get_first(self.fields.chunk_text)
                .map(owned_value_as_str)
                .unwrap_or_default()
                .to_string();
            chunks.push(ChunkInfo {
                chunk_index,
                chunk_text,
            });
        }
        chunks.sort_by_key(|c| c.chunk_index);
        Ok(chunks)
    }

    fn load_skipped(&self) -> Result<HashMap<String, String>> {
        if self.skipped_path.exists() {
            let data = std::fs::read_to_string(&self.skipped_path)?;
            let map: HashMap<String, String> = serde_json::from_str(&data)?;
            Ok(map)
        } else {
            Ok(HashMap::new())
        }
    }

    fn save_skipped(&self, map: &HashMap<String, String>) -> Result<()> {
        let data = serde_json::to_string(map)?;
        std::fs::write(&self.skipped_path, data)?;
        Ok(())
    }

    fn remove_skipped(&self, doc_id: &str) -> Result<()> {
        let mut map = self.load_skipped()?;
        if map.remove(doc_id).is_some() {
            self.save_skipped(&map)?;
        }
        Ok(())
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
        }
    }

    #[test]
    fn test_upsert_and_list_indexed() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        let chunk = make_chunk("notes/test.md", 0, "hello world");
        store.upsert_chunks(&[chunk]).unwrap();

        let indexed = store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/test.md");
        assert_eq!(indexed[0].md5sum, "abc123");
    }

    #[test]
    fn test_delete_by_doc() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/a.md", 0, "aaa")])
            .unwrap();
        store
            .upsert_chunks(&[make_chunk("notes/b.md", 0, "bbb")])
            .unwrap();
        store.delete_doc("notes/a.md").unwrap();

        let indexed = store.list_indexed().unwrap();
        assert_eq!(indexed.len(), 1);
        assert_eq!(indexed[0].doc_id, "notes/b.md");
    }

    #[test]
    fn test_search_returns_results() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[make_chunk(
                "docs/policy.md",
                0,
                "our remote work policy allows flexibility",
            )])
            .unwrap();
        store
            .upsert_chunks(&[make_chunk("docs/other.md", 0, "unrelated content here")])
            .unwrap();

        let results = store.search("remote work policy", 5, None).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].doc_id, "docs/policy.md");
    }

    #[test]
    fn test_search_with_mime_filter() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();

        let mut pdf_chunk = make_chunk("docs/report.pdf", 0, "quarterly results");
        pdf_chunk.mime_type = "application/pdf".to_string();
        let txt_chunk = make_chunk("notes/a.md", 0, "quarterly results");

        store.upsert_chunks(&[pdf_chunk, txt_chunk]).unwrap();

        let filter = MetadataFilter {
            mime_type: Some("application/pdf".to_string()),
            path_prefix: None,
            after: None,
            before: None,
        };
        let results = store.search("quarterly results", 5, Some(&filter)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "docs/report.pdf");
    }

    #[test]
    fn test_get_chunks_ordered() {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        store
            .upsert_chunks(&[
                make_chunk("notes/a.md", 1, "second"),
                make_chunk("notes/a.md", 0, "first"),
            ])
            .unwrap();

        let chunks = store.get_chunks("notes/a.md").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].chunk_text, "first");
        assert_eq!(chunks[1].chunk_index, 1);
        assert_eq!(chunks[1].chunk_text, "second");

        let empty = store.get_chunks("nonexistent.md").unwrap();
        assert!(empty.is_empty());
    }

    fn unix_secs(t: std::time::SystemTime) -> i64 {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .cast_signed()
    }

    fn make_store() -> (RagStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RagStore::open(dir.path()).expect("open");
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
        }
    }

    #[test]
    fn test_list_recent_returns_only_recent_docs() {
        let (store, _dir) = make_store();
        let now = std::time::SystemTime::now();
        let recent = now - std::time::Duration::from_secs(60);
        let old = now - std::time::Duration::from_secs(3600);

        store
            .upsert_chunks(&[dummy_chunk("docs/new.md", recent)])
            .unwrap();
        store
            .upsert_chunks(&[dummy_chunk("docs/old.md", old)])
            .unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).unwrap();

        assert_eq!(result, vec!["docs/new.md".to_string()]);
    }

    #[test]
    fn test_list_recent_empty_when_nothing_recent() {
        let (store, _dir) = make_store();
        let now = std::time::SystemTime::now();
        let old = now - std::time::Duration::from_secs(3600);

        store
            .upsert_chunks(&[dummy_chunk("docs/old.md", old)])
            .unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_recent_deduplicates_doc_ids() {
        let (store, _dir) = make_store();
        let now = std::time::SystemTime::now();
        let recent = now - std::time::Duration::from_secs(60);

        let mut c0 = dummy_chunk("docs/multi.md", recent);
        let mut c1 = dummy_chunk("docs/multi.md", recent);
        c0.id = "docs/multi.md-0".to_string();
        c0.chunk_index = 0;
        c1.id = "docs/multi.md-1".to_string();
        c1.chunk_index = 1;
        store.upsert_chunks(&[c0, c1]).unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "docs/multi.md");
    }

    #[test]
    fn test_list_recent_caps_at_20() {
        let (store, _dir) = make_store();
        let now = std::time::SystemTime::now();
        let recent = now - std::time::Duration::from_secs(60);

        let chunks: Vec<ChunkRecord> = (0..25_u32)
            .map(|i| dummy_chunk(&format!("docs/file{i}.md"), recent))
            .collect();
        store.upsert_chunks(&chunks).unwrap();

        let since = now - std::time::Duration::from_secs(900);
        let result = store.list_recent(since).unwrap();
        assert_eq!(
            result.len(),
            20,
            "expected exactly 20 results (cap), got {}",
            result.len()
        );
    }
}
