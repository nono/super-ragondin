use std::cell::RefCell;
use std::sync::Arc;

use boa_engine::{Context, JsError, JsValue, Source, js_string};
use serde_json::Value as SerdeValue;
use super_ragondin_rag::{config::RagConfig, embedder::OpenRouterEmbedder, store::RagStore};

use crate::tools;

/// Shared Rust state available to all JS native functions during a single execution.
/// Set via thread-local before each Boa evaluation; cleared after.
///
/// All tool functions (`search`, `listFiles`, etc.) access this to reach the store,
/// embedder, and Tokio runtime handle.
#[allow(dead_code)]
pub struct SandboxContext {
    pub store: Arc<RagStore>,
    pub embedder: Arc<OpenRouterEmbedder>,
    pub config: RagConfig,
    pub handle: tokio::runtime::Handle,
    pub sync_dir: std::path::PathBuf,
}

thread_local! {
    /// Active sandbox context for the current Boa execution.
    /// None outside of a `Sandbox::execute()` call.
    #[allow(dead_code)]
    pub static SANDBOX_CTX: RefCell<Option<SandboxContext>> = const { RefCell::new(None) };
}

/// Convert a `JsValue` to a `serde_json::Value` via `JSON.stringify` inside Boa.
///
/// Uses a temporary global variable (`__sr_tmp__`) as an intermediary.
/// Returns `Value::Null` on failure.
#[allow(dead_code)]
pub fn jsvalue_to_serde(val: JsValue, ctx: &mut Context) -> SerdeValue {
    let _ = ctx
        .global_object()
        .set(js_string!("__sr_tmp__"), val, false, ctx);
    match ctx.eval(Source::from_bytes(b"JSON.stringify(__sr_tmp__)")) {
        Ok(JsValue::String(s)) => {
            serde_json::from_str(&s.to_std_string_escaped()).unwrap_or(SerdeValue::Null)
        }
        _ => SerdeValue::Null,
    }
}

/// Convert a `serde_json::Value` to a `JsValue` by evaluating the JSON as JS code.
///
/// Uses the `(JSON)` eval trick, which is safe for any valid JSON value.
#[allow(dead_code)]
pub fn serde_to_jsvalue(val: &SerdeValue, ctx: &mut Context) -> Result<JsValue, JsError> {
    let json_str = serde_json::to_string(val).unwrap_or_else(|_| "null".to_string());
    ctx.eval(Source::from_bytes(format!("({json_str})").as_bytes()))
}

/// Execution wrapper: creates a fresh Boa context per call.
///
/// Must be called from a `spawn_blocking` thread — tool functions call
/// `Handle::current().block_on(...)` internally.
#[allow(dead_code)]
pub struct Sandbox {
    store: Arc<RagStore>,
    config: RagConfig,
    sync_dir: std::path::PathBuf,
}

#[allow(dead_code)]
impl Sandbox {
    /// Create a new sandbox with the given store, config, and sync directory.
    #[must_use]
    pub const fn new(
        store: Arc<RagStore>,
        config: RagConfig,
        sync_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            store,
            config,
            sync_dir,
        }
    }

    /// Execute JS code in a fresh Boa context.
    ///
    /// Returns the JSON-serialized value of the last expression.
    ///
    /// # Errors
    /// Returns `Err(String)` with a human-readable message on JS or Rust error.
    pub fn execute(&self, code: &str) -> Result<String, String> {
        let handle = tokio::runtime::Handle::current();
        let embedder = Arc::new(OpenRouterEmbedder::new(self.config.clone()));

        SANDBOX_CTX.with(|cell| {
            *cell.borrow_mut() = Some(SandboxContext {
                store: Arc::clone(&self.store),
                embedder,
                config: self.config.clone(),
                handle,
                sync_dir: self.sync_dir.clone(),
            });
        });

        let result = self.run_boa(code);

        SANDBOX_CTX.with(|cell| {
            *cell.borrow_mut() = None;
        });

        result
    }

    #[allow(clippy::unused_self)]
    fn run_boa(&self, code: &str) -> Result<String, String> {
        let mut ctx = Context::default();

        tools::search::register(&mut ctx).map_err(|e| format!("JS error: register search: {e}"))?;
        tools::list_files::register(&mut ctx)
            .map_err(|e| format!("JS error: register listFiles: {e}"))?;
        tools::get_document::register(&mut ctx)
            .map_err(|e| format!("JS error: register getDocument: {e}"))?;
        tools::sub_agent::register(&mut ctx)
            .map_err(|e| format!("JS error: register subAgent: {e}"))?;
        tools::save_file::register(&mut ctx)
            .map_err(|e| format!("JS error: register saveFile: {e}"))?;
        tools::list_dirs::register(&mut ctx)
            .map_err(|e| format!("JS error: register listDirs: {e}"))?;
        tools::generate_image::register(&mut ctx)
            .map_err(|e| format!("JS error: register generateImage: {e}"))?;

        let val = ctx
            .eval(Source::from_bytes(code.as_bytes()))
            .map_err(|e| format!("JS error: {e}"))?;

        let serde_val = jsvalue_to_serde(val, &mut ctx);
        serde_json::to_string(&serde_val).map_err(|e| format!("JS error: serialize result: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use boa_engine::Context;
    use std::sync::Arc;
    use super_ragondin_rag::{config::RagConfig, store::RagStore};
    use tempfile::tempdir;

    async fn make_sandbox() -> (Sandbox, tempfile::TempDir, tempfile::TempDir) {
        let db_dir = tempdir().expect("failed to create temp db dir");
        let sync_dir = tempdir().expect("failed to create temp sync dir");
        let store = Arc::new(
            RagStore::open(db_dir.path())
                .await
                .expect("failed to open RagStore"),
        );
        let config = RagConfig::from_env_with_db_path(db_dir.path().to_path_buf());
        let sandbox = Sandbox::new(store, config, sync_dir.path().to_path_buf());
        (sandbox, db_dir, sync_dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_arithmetic() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute("1 + 2").unwrap();
        assert_eq!(result, "3");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_returns_last_expression() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute("const x = 10; x * 2").unwrap();
        assert_eq!(result, "20");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_js_error_returns_err_string() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute("undeclaredFunction()");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("JS error"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_object_result() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute("({ a: 1, b: 'hello' })").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], "hello");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_fresh_context_per_call() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        // Set a variable in one call
        sandbox.execute("var x = 42;").ok();
        // It must NOT be visible in the next call
        let result = sandbox.execute("typeof x === 'undefined'").unwrap();
        assert_eq!(result, "true");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sandbox_globals_registered() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        for fn_name in &[
            "search",
            "listFiles",
            "getDocument",
            "subAgent",
            "saveFile",
            "listDirs",
            "generateImage",
        ] {
            let result = sandbox.execute(&format!("typeof {fn_name}")).unwrap();
            assert_eq!(
                result,
                format!("\"function\""),
                "{fn_name} should be a function"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_save_file_creates_file() {
        let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
        let result = sandbox
            .execute(r#"saveFile("hello.txt", "world")"#)
            .unwrap();
        assert_eq!(result, "null");
        let content = std::fs::read_to_string(sync_dir.path().join("hello.txt"))
            .expect("file should exist after saveFile");
        assert_eq!(content, "world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_dirs_returns_directories() {
        let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
        // Create some directories in the sync dir
        std::fs::create_dir(sync_dir.path().join("alpha")).expect("create alpha");
        std::fs::create_dir(sync_dir.path().join("beta")).expect("create beta");
        std::fs::File::create(sync_dir.path().join("file.txt")).expect("create file");
        let result = sandbox.execute("listDirs()").unwrap();
        let parsed: Vec<String> = serde_json::from_str(&result).expect("should be a JSON array");
        assert_eq!(parsed, vec!["alpha", "beta"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_generate_image_rejects_path_traversal() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute(r#"generateImage("test", { path: "../escape.png" })"#);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("path") || msg.contains("escapes"),
            "got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_generate_image_rejects_reference_traversal() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute(r#"generateImage("test", { reference: "../secret.png" })"#);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("path") || msg.contains("escapes"),
            "got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_generate_image_nonexistent_reference_returns_io_error() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox.execute(r#"generateImage("test", { reference: "nonexistent.png" })"#);
        assert!(result.is_err());
        // Should be an IO error about the missing file, not a path error
        let msg = result.unwrap_err();
        assert!(
            !msg.contains("escapes") && !msg.contains("relative"),
            "expected IO error, got: {msg}"
        );
    }

    #[test]
    fn test_jsvalue_to_serde_string() {
        let mut ctx = Context::default();
        let val = ctx
            .eval(boa_engine::Source::from_bytes(b"'hello'"))
            .unwrap();
        let serde = jsvalue_to_serde(val, &mut ctx);
        assert_eq!(serde, serde_json::json!("hello"));
    }

    #[test]
    fn test_jsvalue_to_serde_array() {
        let mut ctx = Context::default();
        let val = ctx
            .eval(boa_engine::Source::from_bytes(b"[1, 2, 3]"))
            .unwrap();
        let serde = jsvalue_to_serde(val, &mut ctx);
        assert_eq!(serde, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_serde_to_jsvalue_roundtrip() {
        let mut ctx = Context::default();
        let original =
            serde_json::json!({ "doc_id": "notes/a.md", "mtime": "2024-01-01T00:00:00Z" });
        let js = serde_to_jsvalue(&original, &mut ctx).unwrap();
        let back = jsvalue_to_serde(js, &mut ctx);
        assert_eq!(original, back);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires OPENROUTER_API_KEY"]
    async fn test_generate_image_basic() {
        let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
        let result = sandbox
            .execute(
                r#"generateImage("a simple red circle on white background", { size: "0.5K" })"#,
            )
            .expect("generateImage should succeed");
        // Result is a JSON string (quoted base64)
        let b64: String = serde_json::from_str(&result).expect("result should be a JSON string");
        assert!(!b64.is_empty(), "base64 result should not be empty");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("result should be valid base64");
        // Check for PNG (\x89PNG) or JPEG (\xFF\xD8) magic bytes
        assert!(
            bytes.starts_with(b"\x89PNG") || bytes.starts_with(b"\xFF\xD8"),
            "decoded bytes should start with PNG or JPEG magic, got: {:?}",
            &bytes[..4.min(bytes.len())]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires OPENROUTER_API_KEY"]
    async fn test_generate_image_saves_file() {
        let (sandbox, _db_dir, sync_dir) = make_sandbox().await;
        sandbox
            .execute(r#"generateImage("a simple blue square", { path: "generated/out.png", size: "0.5K" })"#)
            .expect("generateImage with path should succeed");
        let file_path = sync_dir.path().join("generated/out.png");
        assert!(file_path.exists(), "file should have been written");
        let bytes = std::fs::read(&file_path).unwrap();
        assert!(!bytes.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires OPENROUTER_API_KEY"]
    async fn test_generate_image_with_reference() {
        let (sandbox, _db_dir, sync_dir) = make_sandbox().await;

        // Write a minimal valid 1x1 PNG as the reference image
        let minimal_png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + type
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // bit depth etc.
            0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT length + type
            0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, // IDAT data
            0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, // IDAT data
            0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND length + type
            0x44, 0xAE, 0x42, 0x60, 0x82, // IEND data
        ];
        std::fs::write(sync_dir.path().join("ref.png"), minimal_png).unwrap();

        let result = sandbox
            .execute(r#"generateImage("enhance this image with warm colors", { reference: "ref.png", size: "0.5K" })"#)
            .expect("generateImage with reference should succeed");
        let b64: String = serde_json::from_str(&result).expect("result should be a JSON string");
        assert!(!b64.is_empty(), "should return non-empty base64");
    }
}
