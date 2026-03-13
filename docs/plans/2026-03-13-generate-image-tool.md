# `generateImage` JS Tool Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `generateImage(prompt, options?)` JavaScript global to the codemode sandbox that calls the OpenRouter image generation API and returns a base64-encoded image string.

**Architecture:** New `generate_image.rs` tool module following the exact pattern of `sub_agent.rs` — parse Boa args, validate paths early, read `SANDBOX_CTX`, then delegate to an async function that calls OpenRouter. A new `path_utils.rs` module extracts the shared `check_relative_path` helper from `save_file.rs`.

**Tech Stack:** Rust, Boa JS engine, reqwest (already present), base64 (already present), infer (already present in `rag` crate; must be added to `codemode`), OpenRouter API (`google/gemini-3.1-flash-image-preview`)

---

## Chunk 1: Infrastructure — RagConfig + path_utils

### Task 1: Add `infer` dependency to `codemode`

**Files:**
- Modify: `crates/codemode/Cargo.toml` (via `cargo add`)

- [ ] **Step 1: Add the dependency**

```bash
cargo add infer --package super-ragondin-codemode
```

Expected: `Cargo.toml` for `super-ragondin-codemode` now lists `infer`.

- [ ] **Step 2: Verify it builds**

```bash
cargo build -p super-ragondin-codemode
```

Expected: compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/codemode/Cargo.toml Cargo.lock
git commit -m "chore(codemode): add infer dependency for MIME detection"
```

---

### Task 2: Add `image_model` to `RagConfig` — TDD

**Files:**
- Modify: `crates/rag/src/config.rs`

- [ ] **Step 1: Write the failing tests**

Open `crates/rag/src/config.rs`. In the `#[cfg(test)]` block, make two changes:

1. Update `test_config_defaults` — add `"OPENROUTER_IMAGE_MODEL"` to the `with_vars_unset` list and assert the default:

```rust
#[test]
fn test_config_defaults() {
    temp_env::with_vars_unset(
        [
            "OPENROUTER_API_KEY",
            "OPENROUTER_EMBED_MODEL",
            "OPENROUTER_VISION_MODEL",
            "OPENROUTER_CHAT_MODEL",
            "OPENROUTER_SUBAGENT_MODEL",
            "OPENROUTER_IMAGE_MODEL",
        ],
        || {
            let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
            assert_eq!(config.embed_model, "openai/text-embedding-3-large");
            assert_eq!(config.vision_model, "google/gemini-2.5-flash");
            assert_eq!(
                config.chat_model,
                "mistralai/mistral-small-3.2-24b-instruct"
            );
            assert_eq!(config.subagent_model, "google/gemini-2.5-flash");
            assert_eq!(
                config.image_model,
                "google/gemini-3.1-flash-image-preview"
            );
            assert!(config.api_key.is_empty());
        },
    );
}
```

2. Add a new test at the bottom of the `tests` block:

```rust
#[test]
fn test_image_model_from_env() {
    temp_env::with_vars(
        [("OPENROUTER_IMAGE_MODEL", Some("custom/img-model"))],
        || {
            let config = RagConfig::from_env_with_db_path(PathBuf::from("/tmp/test.db"));
            assert_eq!(config.image_model, "custom/img-model");
        },
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-ragondin-rag -q
```

Expected: compilation error — `image_model` field does not exist yet.

- [ ] **Step 3: Implement the `image_model` field**

In `crates/rag/src/config.rs`, make the following changes:

Add the constant after the existing ones at the top:

```rust
pub const OPENROUTER_IMAGE_MODEL_DEFAULT: &str = "google/gemini-3.1-flash-image-preview";
```

Add the field to the `RagConfig` struct (after `subagent_model`):

```rust
pub image_model: String,
```

Add to the `Debug` impl (after the `subagent_model` line):

```rust
.field("image_model", &self.image_model)
```

Add to `from_env_with_db_path` (after the `subagent_model` line):

```rust
image_model: std::env::var("OPENROUTER_IMAGE_MODEL")
    .unwrap_or_else(|_| OPENROUTER_IMAGE_MODEL_DEFAULT.to_string()),
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p super-ragondin-rag -q
```

Expected: all tests pass.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rag/src/config.rs
git commit -m "feat(rag): add OPENROUTER_IMAGE_MODEL config field"
```

---

### Task 3: Create `path_utils.rs` and refactor `save_file.rs`

**Files:**
- Create: `crates/codemode/src/tools/path_utils.rs`
- Modify: `crates/codemode/src/tools/save_file.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

- [ ] **Step 1: Write the failing tests first (TDD RED)**

Create `crates/codemode/src/tools/path_utils.rs` with only the test module (no implementation yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_parent_dir() {
        assert!(check_relative_path("../etc/passwd").is_err());
        assert!(check_relative_path("notes/../../../etc").is_err());
        assert!(check_relative_path("a/b/../../..").is_err());
    }

    #[test]
    fn test_accepts_normal_paths() {
        assert!(check_relative_path("notes/summary.md").is_ok());
        assert!(check_relative_path("./notes/file.txt").is_ok());
        assert!(check_relative_path("file.txt").is_ok());
        assert!(check_relative_path("a/b/c").is_ok());
    }
}
```

Also add `pub(crate) mod path_utils;` to `crates/codemode/src/tools/mod.rs` now (needed for the tests to compile at all).

Run to confirm it fails:

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: compilation error — `check_relative_path` not defined.

- [ ] **Step 2: Implement `path_utils.rs` (TDD GREEN)**

Replace the content of `crates/codemode/src/tools/path_utils.rs` with the full implementation + tests:

```rust
use std::path::{Component, Path};

/// Check that a relative path does not escape via `..` or root components.
///
/// Does not check whether the path is absolute — callers must do that
/// separately with `Path::is_absolute()`.
///
/// # Errors
/// Returns `Err` with a message if the path contains a `ParentDir` or `RootDir` component.
pub(crate) fn check_relative_path(path: &str) -> Result<(), &'static str> {
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir | Component::RootDir => {
                return Err("path escapes sync directory");
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_parent_dir() {
        assert!(check_relative_path("../etc/passwd").is_err());
        assert!(check_relative_path("notes/../../../etc").is_err());
        assert!(check_relative_path("a/b/../../..").is_err());
    }

    #[test]
    fn test_accepts_normal_paths() {
        assert!(check_relative_path("notes/summary.md").is_ok());
        assert!(check_relative_path("./notes/file.txt").is_ok());
        assert!(check_relative_path("file.txt").is_ok());
        assert!(check_relative_path("a/b/c").is_ok());
    }
}
```

- [ ] **Step 3: Update `save_file.rs` to use `path_utils`**

In `crates/codemode/src/tools/save_file.rs`:

Remove the local `check_relative_path` function (lines 8–18) and its tests (`test_check_relative_path_rejects_parent_dir` and `test_check_relative_path_accepts_normal_paths`).

Add this import at the top (after the existing `use` statements):

```rust
use super::path_utils::check_relative_path;
```

- [ ] **Step 4: Run tests to verify nothing broke**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: all existing tests pass (the `check_relative_path` tests now live in `path_utils.rs` and still run).

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/tools/path_utils.rs \
        crates/codemode/src/tools/save_file.rs \
        crates/codemode/src/tools/mod.rs
git commit -m "refactor(codemode): extract check_relative_path into path_utils module"
```

---

## Chunk 2: `generateImage` tool + wiring

### Task 4: Create `generate_image.rs` — unit tests first (TDD)

**Files:**
- Create: `crates/codemode/src/tools/generate_image.rs`
- Modify: `crates/codemode/src/tools/mod.rs`

- [ ] **Step 1: Declare the module**

In `crates/codemode/src/tools/mod.rs`, add:

```rust
pub mod generate_image;
```

(`generate_image` is `pub` so `sandbox.rs` can call `tools::generate_image::register`; `path_utils` was added as `pub(crate)` in Task 3.)

- [ ] **Step 2: Create `generate_image.rs` with failing unit tests only**

Create `crates/codemode/src/tools/generate_image.rs` with just the test scaffolding (no implementation yet):

```rust
// Implementation will go here

#[cfg(test)]
mod tests {
    use boa_engine::{Context, Source};

    fn register_fn(ctx: &mut Context) {
        super::register(ctx).unwrap();
    }

    #[test]
    fn test_registers_as_function() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(b"typeof generateImage"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_no_args_returns_error_about_prompt() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(b"generateImage()"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string(ctx.root_shape(), &ctx);
        assert!(
            err.contains("prompt"),
            "error should mention 'prompt', got: {err}"
        );
    }

    #[test]
    fn test_path_traversal_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { path: "../escape.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_path_absolute_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { path: "/absolute/path.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_reference_traversal_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { reference: "../escape.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_reference_absolute_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { reference: "/absolute/ref.png" })"#,
        ));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: compilation error — `super::register` not defined.

- [ ] **Step 4: Write the full implementation**

Replace the `// Implementation will go here` comment with the full implementation. The complete file should be:

```rust
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use boa_engine::{Context, JsArgs, JsError, JsNativeError, JsResult, JsValue, NativeFunction, js_string};
use super_ragondin_rag::config::{OPENROUTER_API_URL, OPENROUTER_REFERER};

use crate::sandbox::SANDBOX_CTX;
use crate::tools::path_utils::check_relative_path;

/// Register the `generateImage(prompt, options?)` global function.
///
/// # Errors
/// Returns error if the global function cannot be registered.
#[allow(dead_code)]
pub fn register(ctx: &mut Context) -> Result<(), JsError> {
    ctx.register_global_callable(
        js_string!("generateImage"),
        1,
        NativeFunction::from_fn_ptr(generate_image_fn),
    )?;
    Ok(())
}

fn generate_image_fn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    // Parse prompt (required)
    let prompt = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    if prompt.is_empty() {
        return Err(JsNativeError::error()
            .with_message("prompt is required")
            .into());
    }

    // Parse options object
    let opts = args.get(1).and_then(|v| v.as_object().cloned());

    let path_opt = get_string_option(&opts, "path", ctx)?;
    let reference_opt = get_string_option(&opts, "reference", ctx)?;
    let aspect = get_string_option(&opts, "aspect", ctx)?.unwrap_or_else(|| "1:1".to_string());
    let size = get_string_option(&opts, "size", ctx)?.unwrap_or_else(|| "0.5K".to_string());

    // Validate paths early — before any I/O or SANDBOX_CTX access
    if let Some(p) = &path_opt {
        if Path::new(p).is_absolute() {
            return Err(JsNativeError::error()
                .with_message("path must be relative")
                .into());
        }
        check_relative_path(p)
            .map_err(|e| JsNativeError::error().with_message(e))?;
    }
    if let Some(r) = &reference_opt {
        if Path::new(r).is_absolute() {
            return Err(JsNativeError::error()
                .with_message("path must be relative")
                .into());
        }
        check_relative_path(r)
            .map_err(|e| JsNativeError::error().with_message(e))?;
    }

    // Read sandbox context
    let (api_key, image_model, handle, sync_dir) = SANDBOX_CTX.with(|cell| {
        let borrow = cell.borrow();
        let sandbox = borrow.as_ref().ok_or_else(|| {
            JsNativeError::error().with_message("sandbox context not initialized")
        })?;
        Ok::<_, JsError>((
            sandbox.config.api_key.clone(),
            sandbox.config.image_model.clone(),
            sandbox.handle.clone(),
            sandbox.sync_dir.clone(),
        ))
    })?;

    // If reference given, read file and encode as base64 data URL
    let reference_data_url: Option<String> = if let Some(r) = reference_opt {
        let ref_path = sync_dir.join(&r);
        let bytes = std::fs::read(&ref_path)
            .map_err(|e| JsNativeError::error().with_message(e.to_string()))?;
        let mime = infer::get(&bytes)
            .map(|t| t.mime_type().to_string())
            .unwrap_or_else(|| "image/png".to_string());
        Some(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
    } else {
        None
    };

    // Compute absolute save path (if requested)
    let save_path: Option<PathBuf> = path_opt.map(|p| sync_dir.join(p));

    // Run async HTTP call
    let b64 = handle.block_on(async move {
        generate_image_async(
            &api_key,
            &image_model,
            &prompt,
            &aspect,
            &size,
            reference_data_url.as_deref(),
            save_path.as_deref(),
        )
        .await
        .map_err(|e| JsNativeError::error().with_message(e.to_string()))
    })?;

    Ok(JsValue::String(boa_engine::JsString::from(b64.as_str())))
}

/// Extract an optional string field from a Boa options object.
fn get_string_option(
    opts: &Option<boa_engine::object::JsObject>,
    key: &str,
    ctx: &mut Context,
) -> JsResult<Option<String>> {
    let Some(obj) = opts else { return Ok(None) };
    let val = obj
        .get(boa_engine::JsString::from(key), ctx)
        .unwrap_or(JsValue::undefined());
    if val.is_undefined() || val.is_null() {
        return Ok(None);
    }
    Ok(Some(val.to_string(ctx)?.to_std_string_escaped()))
}

async fn generate_image_async(
    api_key: &str,
    model: &str,
    prompt: &str,
    aspect: &str,
    size: &str,
    reference_data_url: Option<&str>,
    save_path: Option<&Path>,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    // Build message content array
    let mut content: Vec<serde_json::Value> = Vec::new();
    if let Some(ref_url) = reference_data_url {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": ref_url}
        }));
    }
    content.push(serde_json::json!({"type": "text", "text": prompt}));

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "modalities": ["image", "text"],
        "image_config": {
            "aspect_ratio": aspect,
            "image_size": size
        }
    });

    let resp = client
        .post(OPENROUTER_API_URL)
        .bearer_auth(api_key)
        .header("HTTP-Referer", OPENROUTER_REFERER)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenRouter error {status}: {body_text}");
    }

    let json: serde_json::Value = resp.json().await?;

    // Extract image URL from the non-standard `images` field
    let images = json["choices"][0]["message"]["images"].as_array();
    let image_url = images
        .and_then(|arr| arr.first())
        .and_then(|img| img["image_url"]["url"].as_str());

    let url = match image_url {
        Some(u) => u.to_string(),
        None => {
            let msg_content = json["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("");
            if msg_content.is_empty() {
                anyhow::bail!("no image returned by model");
            } else {
                anyhow::bail!("no image returned by model: {msg_content}");
            }
        }
    };

    // Decode image to raw bytes
    let bytes: Vec<u8> = if url.starts_with("data:image/") {
        let comma = url
            .find(',')
            .ok_or_else(|| anyhow::anyhow!("invalid base64 in response"))?;
        let b64 = &url[comma + 1..];
        STANDARD
            .decode(b64)
            .map_err(|_| anyhow::anyhow!("invalid base64 in response"))?
    } else {
        // Plain HTTPS URL — fetch with same client (120 s timeout inherited)
        client.get(&url).send().await?.bytes().await?.to_vec()
    };

    // Write to file if a save path was given
    if let Some(path) = save_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &bytes)?;
    }

    Ok(STANDARD.encode(&bytes))
}

#[cfg(test)]
mod tests {
    use boa_engine::{Context, Source};

    fn register_fn(ctx: &mut Context) {
        super::register(ctx).unwrap();
    }

    #[test]
    fn test_registers_as_function() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(b"typeof generateImage"));
        assert_eq!(
            result.unwrap().as_string().unwrap().to_std_string_escaped(),
            "function"
        );
    }

    #[test]
    fn test_no_args_returns_error_about_prompt() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(b"generateImage()"));
        assert!(result.is_err());
        // The error message should contain "prompt"
        // (Boa propagates JsNativeError as a caught exception string)
        let err_str = format!("{:?}", result.unwrap_err());
        assert!(
            err_str.contains("prompt"),
            "error should mention 'prompt', got: {err_str}"
        );
    }

    #[test]
    fn test_path_traversal_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { path: "../escape.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_path_absolute_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { path: "/absolute/path.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_reference_traversal_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { reference: "../escape.png" })"#,
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_reference_absolute_rejected() {
        let mut ctx = Context::default();
        register_fn(&mut ctx);
        let result = ctx.eval(Source::from_bytes(
            br#"generateImage("test", { reference: "/absolute/ref.png" })"#,
        ));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 5: Run unit tests to verify they pass**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: all tests pass, including the 6 new unit tests.

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/codemode/src/tools/generate_image.rs \
        crates/codemode/src/tools/mod.rs
git commit -m "feat(codemode): add generateImage JS tool"
```

---

### Task 5: Wire `generateImage` into sandbox, engine, and prompt

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`
- Modify: `crates/codemode/src/engine.rs`
- Modify: `crates/codemode/src/prompt.rs`

- [ ] **Step 1: Register `generateImage` in `sandbox.rs`**

In `crates/codemode/src/sandbox.rs`, in the `run_boa` method, add after the `list_dirs::register` call:

```rust
tools::generate_image::register(&mut ctx)
    .map_err(|e| format!("JS error: register generateImage: {e}"))?;
```

Also in `test_sandbox_globals_registered`, add `"generateImage"` to the `fn_name` slice:

```rust
for fn_name in &[
    "search",
    "listFiles",
    "getDocument",
    "subAgent",
    "saveFile",
    "listDirs",
    "generateImage",
] {
```

- [ ] **Step 2: Update `engine.rs` tool description**

In `crates/codemode/src/engine.rs`, in `execute_js_tool_definition`, update the description string to include `generateImage()`:

```rust
"description": "Execute JavaScript code in a sandbox. Use the search(), listFiles(), getDocument(), subAgent(), saveFile(), listDirs(), and generateImage() functions to query the document database, write files, and generate images.",
```

- [ ] **Step 3: Update `prompt.rs`**

In `crates/codemode/src/prompt.rs`, add `generateImage` to the Available JavaScript functions section (after the `listDirs` entry):

```
  generateImage(prompt, options?)
    Generate an image via OpenRouter and return it as a base64 string.
    Options: { path, aspect, size, reference }
      path: relative path in sync_dir to save the image (e.g. "images/out.png")
      aspect: aspect ratio string, e.g. "1:1", "16:9", "4:3" (default: "1:1")
      size: "0.5K" | "1K" | "2K" | "4K" (default: "0.5K")
      reference: relative path to an existing image for image-to-image generation
    Returns: base64-encoded image string (without the data: prefix)
    Side effect: if path is given, the image is written to sync_dir/path
```

Add a usage example (after the existing `saveFile` example):

```
// Generate a watercolor-style mindmap and save it
const b64 = generateImage(
  "Watercolor mindmap: key topics from the meeting notes",
  { path: "images/mindmap.png", aspect: "4:3", size: "1K" }
)
```

Also update the `test_prompt_contains_key_elements` test to assert `generateImage` is mentioned:

```rust
assert!(p.contains("generateImage("));
```

- [ ] **Step 4: Run all tests**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: all tests pass, including `test_sandbox_globals_registered` (now checks `generateImage`) and `test_prompt_contains_key_elements`.

- [ ] **Step 5: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/codemode/src/sandbox.rs \
        crates/codemode/src/engine.rs \
        crates/codemode/src/prompt.rs
git commit -m "feat(codemode): wire generateImage into sandbox, engine, and prompt"
```

---

### Task 6: Add sandbox integration tests + update AGENTS.md

**Files:**
- Modify: `crates/codemode/src/sandbox.rs`
- Modify: `AGENTS.md`

- [ ] **Step 1: Add sandbox tests for `generateImage` error cases**

In `crates/codemode/src/sandbox.rs`, in the `#[cfg(test)]` block, add these tests that use the full sandbox (no API key needed — they all fail before making an HTTP call):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_generate_image_rejects_path_traversal() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox.execute(r#"generateImage("test", { path: "../escape.png" })"#);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(msg.contains("path") || msg.contains("escapes"), "got: {msg}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_generate_image_rejects_reference_traversal() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox.execute(r#"generateImage("test", { reference: "../secret.png" })"#);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(msg.contains("path") || msg.contains("escapes"), "got: {msg}");
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
```

- [ ] **Step 2: Add integration tests for `generateImage` (ignored)**

In `crates/codemode/src/sandbox.rs`, add these ignored integration tests at the end of the `#[cfg(test)]` block:

```rust
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires OPENROUTER_API_KEY"]
async fn test_generate_image_basic() {
    let (sandbox, _db_dir, _sync_dir) = make_sandbox().await;
    let result = sandbox
        .execute(r#"generateImage("a simple red circle on white background", { size: "0.5K" })"#)
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
    use base64::Engine as _;
    let (sandbox, _db_dir, sync_dir) = make_sandbox().await;

    // Write a minimal valid 1x1 PNG as the reference image
    // This is a hardcoded minimal PNG (67 bytes)
    let minimal_png: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + type
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // bit depth etc.
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT length + type
        0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, // IDAT data
        0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, // IDAT data
        0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND length + type
        0x44, 0xAE, 0x42, 0x60, 0x82,                   // IEND data
    ];
    std::fs::write(sync_dir.path().join("ref.png"), minimal_png).unwrap();

    let result = sandbox
        .execute(r#"generateImage("enhance this image with warm colors", { reference: "ref.png", size: "0.5K" })"#)
        .expect("generateImage with reference should succeed");
    let b64: String = serde_json::from_str(&result).expect("result should be a JSON string");
    assert!(!b64.is_empty(), "should return non-empty base64");
}
```

Note: the integration tests need `use base64::Engine as _;` — add this import at the top of the `tests` module if not already present.

- [ ] **Step 3: Run all non-ignored tests to make sure nothing broke**

```bash
cargo test -p super-ragondin-codemode -q
```

Expected: all non-ignored tests pass.

- [ ] **Step 4: Update `AGENTS.md`**

In `AGENTS.md`, find the RAG Environment Variables table and add a new row:

```
| `OPENROUTER_IMAGE_MODEL` | `google/gemini-3.1-flash-image-preview` | Image generation model |
```

(Add it after the `OPENROUTER_SUBAGENT_MODEL` row.)

- [ ] **Step 5: Run the full test suite**

```bash
cargo test -q
```

Expected: all non-ignored tests pass across the entire workspace.

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all && cargo clippy --all-features
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/codemode/src/sandbox.rs AGENTS.md
git commit -m "test(codemode): add generateImage sandbox tests; update AGENTS.md"
```
