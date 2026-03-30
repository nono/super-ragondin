# Web Fetch & Web Search for Codemode JS Sandbox

## Summary

Add two new JS sandbox functions: `webFetch(url)` (always available) for HTTP GET requests, and `webSearch(query, options?)` (opt-in per question) for web search via Exa on OpenRouter.

## Motivation

The codemode LLM agent currently operates only on the local document database. Web access enables it to fetch reference material, check URLs, and search the web when the user explicitly allows it.

## API Design

### `webFetch(url)`

```js
webFetch(url)
// Returns: { status: number, contentType: string, body: string }
```

- Always registered (no gate)
- Anonymous HTTP GET via reqwest
- 30-second timeout
- 1 MB max body size (truncated beyond that)
- Only text content types populate `body`; binary content returns empty string
- Follows redirects (reqwest default)
- User-Agent: `"SuperRagondin/0.1"`

### `webSearch(query, options?)`

```js
webSearch(query, options?)
// options: { limit } (default: 5, max: 10)
// Returns: [{ title, url, snippet }, ...]
```

- Conditionally registered only when the `--web` flag is passed
- Calls OpenRouter with the Exa model (`exa/exa` by default)
- New config field: `search_model` / env var `OPENROUTER_SEARCH_MODEL`
- Parses Exa's response into structured array
- Throws JS error on API failure (LLM can recover)

## Opt-in Mechanism

Web search is gated per question via a flag. `webFetch` is always available.

- **CLI**: `ask --web "my question"`
- **GUI**: `ask_question` Tauri command gains a `web_search: bool` parameter

The flag flows: CLI/GUI → `CodeModeEngine::ask(web_search: bool)` → `Sandbox` → `SandboxContext.web_search_enabled` → conditional registration (same pattern as `askUser` / `interaction`).

## System Prompt

- `system_prompt(interactive: bool)` becomes `system_prompt(interactive: bool, web_search: bool)`
- `webFetch()` docs always included
- `webSearch()` docs included only when `web_search` is true
- Example combining both: `webSearch("topic")` → `webFetch(results[0].url)` → `subAgent()` pipeline
- `execute_js_tool_definition()` description updated to mention new functions

## Files

| File | Change |
|---|---|
| `crates/codemode/src/tools/web_fetch.rs` | New — `webFetch` implementation |
| `crates/codemode/src/tools/web_search.rs` | New — `webSearch` implementation |
| `crates/codemode/src/tools/mod.rs` | Add `pub mod web_fetch; pub mod web_search;` |
| `crates/codemode/src/sandbox.rs` | Add `web_search_enabled: bool` to `SandboxContext`, plumb to `Sandbox::new()`, conditional registration |
| `crates/codemode/src/engine.rs` | Add `web_search: bool` param to `ask()`, pass through to `Sandbox` |
| `crates/codemode/src/prompt.rs` | Update signature, add `webFetch`/`webSearch` docs |
| `crates/rag/src/config.rs` | Add `search_model` field + `OPENROUTER_SEARCH_MODEL` env var |
| `crates/cli/src/main.rs` | Add `--web` flag to `ask` subcommand |
| `crates/gui/src/commands.rs` | Add `web_search` parameter |

## Testing

- **Unit**: `register()` works, `typeof` checks, conditional registration of `webSearch`
- **Wiremock**: mock HTTP server for `webFetch` (status, content type, body, timeout, truncation)
- **Wiremock**: mock OpenRouter for `webSearch` (parse Exa response)
- **Integration** (`#[ignore]`): real Exa calls requiring `OPENROUTER_API_KEY`
