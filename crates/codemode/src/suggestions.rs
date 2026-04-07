use std::path::PathBuf;

use anyhow::{Result, bail};
use super_ragondin_rag::{
    config::RagConfig,
    store::{DocInfo, DocSort, MetadataFilter, RagStore},
};

/// Returned when `generate` is called but no files have been indexed yet.
#[derive(Debug, thiserror::Error)]
#[error("no files indexed")]
pub struct NoFilesIndexed;

/// Per-file context collected during Phase 1.
#[derive(Debug, serde::Serialize)]
pub struct FileContext {
    pub path: String,
    pub mime_type: String,
    pub summary: Option<String>,
}

/// Intermediate: path, mime, and pre-fetched text for a document.
pub(crate) struct DocText {
    pub(crate) path: String,
    pub(crate) mime_type: String,
    pub(crate) text: String,
}

pub struct SuggestionEngine {
    config: RagConfig,
    sync_dir: PathBuf,
    store: RagStore,
}

impl SuggestionEngine {
    /// Open the RAG store and construct the engine.
    ///
    /// # Errors
    /// Returns an error if the RAG store cannot be opened.
    pub fn new(config: RagConfig, sync_dir: PathBuf) -> Result<Self> {
        let store = RagStore::open(&config.db_path)?;
        Ok(Self {
            config,
            sync_dir,
            store,
        })
    }

    /// Generate 6 prompt suggestions based on recently modified files.
    ///
    /// # Errors
    /// Returns an error if no files are indexed or suggestion generation fails.
    pub async fn generate(&self, cwd: Option<PathBuf>) -> Result<Vec<String>> {
        // Phase 1a: store queries (fast, no timeout)
        let docs = query_docs(&self.store, &self.sync_dir, cwd.as_deref())?;

        // Phase 1b: fetch chunk text for up to 5 docs
        let mut doc_texts = Vec::new();
        for doc in docs.iter().take(5) {
            let chunks = self
                .store
                .get_chunks(&doc.doc_id)
                .unwrap_or_else(|_| vec![]);
            let text: String = chunks
                .iter()
                .map(|c| c.chunk_text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let text = if text.len() > 2000 {
                let end = (0..=2000)
                    .rev()
                    .find(|&i| text.is_char_boundary(i))
                    .unwrap_or(0);
                text[..end].to_string()
            } else {
                text
            };
            doc_texts.push(DocText {
                path: doc.doc_id.clone(),
                mime_type: doc.mime_type.clone(),
                text,
            });
        }
        // Remaining docs beyond 5 get no summary
        let rest: Vec<FileContext> = docs[doc_texts.len()..]
            .iter()
            .map(|d| FileContext {
                path: d.doc_id.clone(),
                mime_type: d.mime_type.clone(),
                summary: None,
            })
            .collect();

        // Phase 1c: parallel summaries with 3s deadline
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        let api_key = self.config.api_key.clone();
        let model = self.config.subagent_model.clone();
        let mut contexts = collect_summaries(doc_texts, deadline, move |text: String| {
            let api_key = api_key.clone();
            let model = model.clone();
            async move { summarize(&api_key, &model, &text).await }
        })
        .await;
        contexts.extend(rest);

        // Phase 2: generate suggestions with single 4s timeout (covers initial call + retry)
        let api_key = self.config.api_key.clone();
        let model = self.config.chat_model.clone();
        tokio::time::timeout(
            std::time::Duration::from_secs(4),
            generate_suggestions_with_fn(contexts, move |messages| {
                let api_key = api_key.clone();
                let model = model.clone();
                async move { crate::llm::call_llm(&api_key, &model, messages).await }
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("suggestion generation timed out"))?
    }
}

/// Phase 1, steps 1-4: determine which docs to use.
/// Returns up to 10 `DocInfo` sorted by most-recently-modified.
///
/// # Errors
/// Returns a [`NoFilesIndexed`] error if the store is empty.
pub(crate) fn query_docs(
    store: &RagStore,
    sync_dir: &std::path::Path,
    cwd: Option<&std::path::Path>,
) -> Result<Vec<DocInfo>> {
    // Step 1-2: try prefix query if cwd is inside sync_dir and prefix is non-empty
    let prefix = cwd
        .and_then(|c| c.strip_prefix(sync_dir).ok())
        .and_then(|rel| {
            let s = rel.to_string_lossy().into_owned();
            if s.is_empty() { None } else { Some(s) }
        });

    let docs = if let Some(prefix) = prefix {
        let filter = MetadataFilter {
            path_prefix: Some(prefix),
            mime_type: None,
            after: None,
            before: None,
        };
        let docs = store.list_docs(Some(&filter), DocSort::Recent, Some(10))?;
        if docs.is_empty() {
            // Fallback to whole-store query
            store.list_docs(None, DocSort::Recent, Some(10))?
        } else {
            docs
        }
    } else {
        // No usable prefix — whole-store query
        store.list_docs(None, DocSort::Recent, Some(10))?
    };

    if docs.is_empty() {
        return Err(NoFilesIndexed.into());
    }
    Ok(docs)
}

/// Phase 1, step 5: run summaries in parallel, retaining partial results if deadline passes.
///
/// `summarize_fn` is injected for testability (unit tests pass fast/slow closures).
pub(crate) async fn collect_summaries<F, Fut>(
    doc_texts: Vec<DocText>,
    deadline: tokio::time::Instant,
    summarize_fn: F,
) -> Vec<FileContext>
where
    F: Fn(String) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let mut handles: Vec<(String, String, tokio::task::JoinHandle<Result<String>>)> = Vec::new();
    for dt in doc_texts {
        let fn_clone = summarize_fn.clone();
        let text = dt.text.clone();
        let handle = tokio::spawn(async move { fn_clone(text).await });
        handles.push((dt.path, dt.mime_type, handle));
    }

    let mut contexts = Vec::new();
    for (path, mime_type, handle) in handles {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let summary = if remaining.is_zero() {
            handle.abort();
            None
        } else {
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(Ok(Ok(s))) => Some(s),
                _ => None,
            }
        };
        contexts.push(FileContext {
            path,
            mime_type,
            summary,
        });
    }
    contexts
}

async fn summarize(api_key: &str, model: &str, text: &str) -> Result<String> {
    let messages = vec![
        serde_json::json!({"role": "system", "content": "You are a helpful assistant. Summarize the following document in one sentence of at most 20 words."}),
        serde_json::json!({"role": "user", "content": text}),
    ];
    crate::llm::call_llm(api_key, model, messages).await
}

const PHASE2_SYSTEM_PROMPT: &str = "\
You are a creative assistant helping a user discover what they can ask their personal document AI.\n\
\n\
Given the following list of recently modified files (with optional summaries), generate exactly 6 prompt suggestions.\n\
- 2 suggestions must be practical (obvious, immediately useful)\n\
- 4 suggestions must be creative, surprising, or delightful — things the user would not think to ask on their own\n\
- Each suggestion must be specific to the actual content provided, not generic\n\
- Each suggestion must be under 80 characters\n\
- Return ONLY a JSON array of 6 strings, with no other text or explanation";

/// Phase 2: generate suggestions via LLM with one corrective retry on bad JSON.
///
/// The caller is responsible for wrapping this in a timeout.
/// `llm_fn` is injected for testability.
pub(crate) async fn generate_suggestions_with_fn<F, Fut>(
    contexts: Vec<FileContext>,
    llm_fn: F,
) -> Result<Vec<String>>
where
    F: Fn(Vec<serde_json::Value>) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let user_prompt = serde_json::to_string(&contexts)?;
    let mut messages = vec![
        serde_json::json!({"role": "system", "content": PHASE2_SYSTEM_PROMPT}),
        serde_json::json!({"role": "user", "content": user_prompt}),
    ];

    let response = llm_fn(messages.clone()).await?;
    if let Ok(suggestions) = parse_suggestions(&response) {
        return Ok(suggestions);
    }

    // Corrective retry (same timeout window — managed by caller)
    messages.push(serde_json::json!({"role": "assistant", "content": response}));
    messages.push(serde_json::json!({
        "role": "user",
        "content": "Your response was not valid JSON. Return ONLY a JSON array of 6 strings."
    }));
    let response2 = llm_fn(messages).await?;
    parse_suggestions(&response2)
        .map_err(|_| anyhow::anyhow!("failed to parse suggestions after retry"))
}

fn parse_suggestions(text: &str) -> Result<Vec<String>> {
    let text = text.trim();
    // Strip optional markdown code fence
    let text = text.strip_prefix("```json").unwrap_or(text);
    let text = text.strip_prefix("```").unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text).trim();

    let suggestions: Vec<String> = serde_json::from_str(text)?;
    if suggestions.len() != 6 {
        bail!("expected 6 suggestions, got {}", suggestions.len());
    }
    Ok(suggestions)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use tempfile::tempdir;

    use super_ragondin_rag::store::RagStore;

    fn empty_store() -> (RagStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = RagStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[tokio::test]
    async fn test_query_docs_cwd_outside_sync_dir_uses_whole_store() {
        let (store, _dir) = empty_store();
        let sync_dir = PathBuf::from("/tmp/sync");
        let cwd = Some(PathBuf::from("/home/user/other"));
        let result = super::query_docs(&store, sync_dir.as_path(), cwd.as_deref());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<super::NoFilesIndexed>()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_query_docs_cwd_none_uses_whole_store() {
        let (store, _dir) = empty_store();
        let sync_dir = PathBuf::from("/tmp/sync");
        let result = super::query_docs(&store, sync_dir.as_path(), None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<super::NoFilesIndexed>()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_query_docs_cwd_at_sync_dir_root_uses_whole_store() {
        // cwd == sync_dir → strip_prefix yields "" → no prefix → whole-store
        let (store, _dir) = empty_store();
        let sync_dir = PathBuf::from("/tmp/sync");
        let cwd = Some(PathBuf::from("/tmp/sync"));
        let result = super::query_docs(&store, sync_dir.as_path(), cwd.as_deref());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .downcast_ref::<super::NoFilesIndexed>()
                .is_some()
        );
    }

    /// Spec row 1: "cwd inside sync_dir → prefix query used"
    /// Spec row 3: "Prefix query returns 0 → whole-store fallback"
    ///
    /// These require seeding the LanceDB store with real document data, which in turn
    /// requires calling the embedder (needs an API key). They are integration tests.
    #[tokio::test]
    #[ignore = "requires indexed files (run with --ignored after super-ragondin sync)"]
    async fn test_query_docs_cwd_inside_sync_dir_uses_prefix_query() {
        todo!("seed store, then call query_docs with cwd=/tmp/sync/work and assert prefix results")
    }

    #[tokio::test]
    #[ignore = "requires indexed files (run with --ignored after super-ragondin sync)"]
    async fn test_query_docs_prefix_empty_falls_back_to_whole_store_with_results() {
        todo!("seed store without data under prefix, verify fallback returns whole-store results")
    }

    #[tokio::test]
    async fn test_collect_summaries_partial_timeout_retains_completed() {
        use std::time::Duration;

        let doc_texts = vec![
            super::DocText {
                path: "fast.md".to_string(),
                mime_type: "text/plain".to_string(),
                text: "fast".to_string(),
            },
            super::DocText {
                path: "slow.md".to_string(),
                mime_type: "text/plain".to_string(),
                text: "slow".to_string(),
            },
        ];
        // Deadline: 100ms — fast doc completes immediately, slow doc takes 500ms
        let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
        let contexts = super::collect_summaries(doc_texts, deadline, |text: String| async move {
            if text == "slow" {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Ok(format!("summary of {text}"))
        })
        .await;

        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0].summary.as_deref(), Some("summary of fast"));
        assert!(
            contexts[1].summary.is_none(),
            "slow doc should have no summary"
        );
    }

    #[tokio::test]
    async fn test_phase2_bad_json_then_corrective_retry_succeeds() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let count = Arc::new(AtomicUsize::new(0));
        let count2 = count.clone();
        let result = super::generate_suggestions_with_fn(vec![], move |_messages| {
            let n = count2.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Ok("not valid json".to_string())
                } else {
                    Ok(r#"["a","b","c","d","e","f"]"#.to_string())
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), vec!["a", "b", "c", "d", "e", "f"]);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_phase2_bad_json_twice_returns_err() {
        let result = super::generate_suggestions_with_fn(vec![], |_| async {
            Ok("not valid json".to_string())
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_phase2_wrong_length_returns_err() {
        let result = super::generate_suggestions_with_fn(vec![], |_| async {
            Ok(r#"["a","b","c"]"#.to_string())
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_phase2_markdown_fenced_json_parses_ok() {
        let json = "```json\n[\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]\n```";
        let result = super::generate_suggestions_with_fn(vec![], |_| {
            let json = json.to_string();
            async move { Ok(json) }
        })
        .await;
        assert_eq!(result.unwrap(), vec!["a", "b", "c", "d", "e", "f"]);
    }
}
