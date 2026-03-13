# Design: `generateImage` JS Tool for Code Mode

**Date:** 2026-03-13
**Status:** Approved

## Overview

Add a `generateImage(prompt, options?)` JavaScript global function to the codemode sandbox. It calls the OpenRouter image-generation API (`google/gemini-3.1-flash-image-preview`) and returns the generated image as a base64 string. If `options.path` is provided, the image is also saved to the sync directory.

## JS API

```js
generateImage(prompt, options?)
```

**Parameters:**
- `prompt` (string, required): Image description / generation prompt
- `options` (object, optional):
  - `path` (string): Relative path within sync_dir to save the image (e.g. `"images/mindmap.png"`). No extension enforcement — the file is written as-is under the given name.
  - `aspect` (string): Aspect ratio passed literally to the API `image_config.aspect_ratio` field (e.g. `"1:1"`, `"16:9"`, `"3:2"`). Default: `"1:1"`. Invalid values are passed through; the API error is surfaced to JS.
  - `size` (string): Image size passed literally to the API `image_config.image_size` field. One of `"0.5K"`, `"1K"`, `"2K"`, `"4K"`. Default: `"0.5K"`. Invalid values are passed through; the API error is surfaced to JS.
  - `reference` (string): Relative path within sync_dir to an existing image for image-to-image generation.

**Returns:** base64-encoded image string (the raw base64 data, without the `data:...;base64,` prefix)

**Side effect:** If `options.path` is provided, the image bytes are written to `sync_dir/path`.

**Example usage:**
```js
// Generate and save a watercolor mindmap
const b64 = generateImage(
  "A colorful watercolor-style mindmap of the key topics from these notes: ...",
  { path: "images/mindmap.png", aspect: "4:3", size: "1K" }
);
// b64 is also returned for further use (e.g. embedding in HTML)
```

## Architecture

### New file: `crates/codemode/src/tools/generate_image.rs`

Follows the same pattern as `sub_agent.rs`:

1. Parse args from Boa: `prompt` (required string), `options` object (optional). Error if `prompt` is absent or empty.
2. Read `SANDBOX_CTX` for `config` (→ `api_key`, `image_model`), `handle`, `sync_dir`.
3. If `options.reference` is given:
   a. **Validate first**: check `!path.is_absolute()` (error: `"path must be relative"`), then call `path_utils::check_relative_path` (error: `"path escapes sync directory"`), before any file I/O.
   b. Read `sync_dir/reference` as bytes.
   c. Detect MIME type: call `infer::get(&bytes).map(|t| t.mime_type().to_string())`, fall back to `"image/png"` if `None`.
   d. Build a base64 data URL: `format!("data:{mime};base64,{}", STANDARD.encode(&bytes))`.
4. Build the OpenRouter request body (top-level fields). Always include both `modalities` and `image_config` regardless of whether a reference image is provided or whether `aspect`/`size` are at their defaults:
   ```json
   {
     "model": "<config.image_model>",
     "messages": [{"role": "user", "content": [
       // optional: {"type": "image_url", "image_url": {"url": "<data URL>"}}
       {"type": "text", "text": "<prompt>"}
     ]}],
     "modalities": ["image", "text"],
     "image_config": {
       "aspect_ratio": "<aspect>",
       "image_size": "<size>"
     }
   }
   ```
   `modalities` is a top-level OpenRouter field that instructs the model to return both image and text outputs. It is required for image generation with this model. If the configured `image_model` does not support these fields, the API error will be surfaced as a `JsNativeError`.
5. Build a single `reqwest::Client` with a 120 s timeout. Execute the API call via `handle.block_on(...)`. Reuse the same client for any HTTPS image fetch in step 6.
6. Extract the image bytes from the response:
   - Expected response shape (non-standard OpenRouter extension for image-capable models):
     ```json
     {
       "choices": [{
         "message": {
           "role": "assistant",
           "content": "",
           "images": [{"image_url": {"url": "data:image/png;base64,<base64data>"}}]
         }
       }]
     }
     ```
   - Check `choices[0].message.images[0].image_url.url`.
   - If the URL starts with `data:image/`:
     - Strip the prefix up to and including the first `,` to get the raw base64 string.
     - Decode to bytes with `STANDARD.decode(...)`.
   - If the URL starts with `https://` (plain HTTPS URL):
     - Fetch it with the same `reqwest::Client` inside the same `block_on` call using `.bytes().await` (no streaming, no explicit body-size limit beyond the 120 s timeout).
   - If `images` is absent or empty: check `choices[0].message.content`; if non-empty, include it in the error: `JsNativeError::error(format!("no image returned by model: {content}"))`. Otherwise return `JsNativeError::error("no image returned by model")`.
7. If `options.path` is provided:
   - **Validate first**: check `!path.is_absolute()` (error: `"path must be relative"`), then `path_utils::check_relative_path` (error: `"path escapes sync directory"`).
   - `if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)? }` — skip if there is no parent component (e.g. `"foo.png"` at the root of sync_dir). Then `std::fs::write(sync_dir/path, &bytes)`.
   - This matches the existing behaviour in `save_file.rs`.
8. Re-encode `bytes` as base64 string (`STANDARD.encode(&bytes)`) and return as `JsValue::String`.

### Dependency: add `infer` to codemode

The `infer` crate is already used in `crates/rag` but not in `crates/codemode`. Add it:

```
cargo add infer --package super-ragondin-codemode
```

### Shared path utility: `crates/codemode/src/tools/path_utils.rs`

Extract `check_relative_path` from `save_file.rs` into this new module to avoid duplication. Both `save_file` and `generate_image` import it. Update `save_file.rs` to call `super::path_utils::check_relative_path` instead of its local copy.

Note: `check_relative_path` only checks for `ParentDir` and `RootDir` path components. Absolute path rejection (`is_absolute()` check with message `"path must be relative"`) remains in each caller, matching the existing behaviour in `save_file.rs`.

### Changes to existing files

| File | Change |
|---|---|
| `crates/codemode/src/tools/mod.rs` | Add `pub mod generate_image; pub mod path_utils;` |
| `crates/codemode/src/tools/save_file.rs` | Remove local `check_relative_path`; use `super::path_utils::check_relative_path` |
| `crates/codemode/src/sandbox.rs` | Register `generateImage` in `run_boa()`; add `"generateImage"` to the `fn_name` slice in `test_sandbox_globals_registered` |
| `crates/codemode/src/engine.rs` | Add `generateImage` to `execute_js` tool description string |
| `crates/codemode/src/prompt.rs` | Document `generateImage` in the system prompt and add a usage example |
| `crates/rag/src/config.rs` | Add `image_model` field to `RagConfig` struct, `Debug` impl, and `from_env_with_db_path` constructor (see below) |
| `AGENTS.md` | Add `OPENROUTER_IMAGE_MODEL` row to the RAG Environment Variables table |

### Config change: `RagConfig`

Only `from_env_with_db_path` exists — there is no `from_env`. Add `image_model` to the struct, the manual `Debug` impl, and the constructor:

```rust
pub const OPENROUTER_IMAGE_MODEL_DEFAULT: &str = "google/gemini-3.1-flash-image-preview";

// In RagConfig struct:
pub image_model: String,

// In Debug impl, after existing fields:
.field("image_model", &self.image_model)

// In from_env_with_db_path:
image_model: std::env::var("OPENROUTER_IMAGE_MODEL")
    .unwrap_or_else(|_| OPENROUTER_IMAGE_MODEL_DEFAULT.to_string()),
```

### AGENTS.md table row to add

```
| `OPENROUTER_IMAGE_MODEL` | `google/gemini-3.1-flash-image-preview` | Image generation model |
```

## Error Handling

| Situation | Behaviour |
|---|---|
| Missing or empty `prompt` arg | `JsNativeError::error("prompt is required")` |
| `options.path` is absolute | `JsNativeError::error("path must be relative")` |
| `options.path` contains `..` | `JsNativeError::error("path escapes sync directory")` |
| `options.reference` is absolute | `JsNativeError::error("path must be relative")` (validated before file read) |
| `options.reference` contains `..` | `JsNativeError::error("path escapes sync directory")` (validated before file read) |
| `options.reference` file not found | `JsNativeError::error` with the IO error message |
| HTTP error from OpenRouter | `JsNativeError::error` with status code and response body |
| Response `images` absent/empty, content present | `JsNativeError::error(format!("no image returned by model: {content}"))` |
| Response `images` absent/empty, no content | `JsNativeError::error("no image returned by model")` |
| Invalid base64 in data URL response | `JsNativeError::error("invalid base64 in response")` |
| HTTPS image URL fetch fails | `JsNativeError::error` with the reqwest error message |
| `std::fs::write` failure when saving | `JsNativeError::error` with the IO error message |
| Invalid `aspect` or `size` value | Passed through to API; API error is surfaced as `JsNativeError::error` |

## Testing

### Unit tests in `generate_image.rs` (no API key required)

- `register()` makes `generateImage` available as a JS `"function"`
- Calling with no args returns a JS error containing `"prompt"`
- `options.path: "../escape"` is rejected (path traversal)
- `options.path: "/absolute/path"` is rejected (absolute path)
- `options.reference: "../escape"` is rejected (path traversal)
- `options.reference: "/absolute/path"` is rejected (absolute path)
- `options.reference` pointing to a valid (non-traversal) but nonexistent path returns a JS error with the IO error message

### Unit tests in `path_utils.rs`

Move the existing `check_relative_path` tests from `save_file.rs` to `path_utils.rs` (they test the extracted function directly).

### Integration tests in `generate_image.rs` (ignored, require `OPENROUTER_API_KEY`)

- Basic generation: the returned base64 string is non-empty and, when decoded, starts with a recognised image magic bytes prefix — either `\x89PNG` (PNG) or `\xFF\xD8` (JPEG). This is a partial sanity check; the model may return either format.
- `options.path` causes a file to be written at `sync_dir/path`; verify with `std::fs::read`.
- `options.reference`: write a small synthetic PNG into `sync_dir`, call `generateImage` with `reference` pointing to it; verify the call succeeds and returns a non-empty base64 string.

### Config tests in `crates/rag/src/config.rs`

Use the `temp-env` crate (required by AGENTS.md):

- Update `test_config_defaults`: add `"OPENROUTER_IMAGE_MODEL"` to the `with_vars_unset` list, and assert `config.image_model == "google/gemini-3.1-flash-image-preview"`.
- Add `test_image_model_from_env`: use `with_vars([("OPENROUTER_IMAGE_MODEL", Some("custom/img-model"))])` and assert the field is picked up.
