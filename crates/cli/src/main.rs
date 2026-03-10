use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use super_ragondin_sync::config::Config;
use super_ragondin_sync::error::{Error, Result};
use super_ragondin_sync::local::watcher::{WatchEvent, Watcher};
use super_ragondin_sync::model::PlanResult;
use super_ragondin_sync::planner::Planner;
use super_ragondin_sync::remote::auth::OAuthClient;
use super_ragondin_sync::store::TreeStore;
use super_ragondin_sync::sync::engine::SyncEngine;

fn main() -> Result<()> {
    super_ragondin_sync::logging::init();

    let args: Vec<String> = env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("init") => cmd_init(&args[2..]),
        Some("auth") => cmd_auth(),
        Some("sync") => cmd_sync(),
        Some("watch") => cmd_watch(),
        Some("status") => cmd_status(),
        Some("ask") => cmd_ask(&args[2..]),
        _ => {
            println!("Usage: super-ragondin <command>");
            println!();
            println!("Commands:");
            println!("  init <instance-url> <sync-dir>  Initialize configuration");
            println!("  auth                             Authenticate with Cozy");
            println!("  sync                             Run one sync cycle");
            println!("  watch                            Watch and sync continuously");
            println!("  status                           Show sync status");
            println!("  ask <question>                   Ask a question about your files");
            Ok(())
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin")
        .join("config.json")
}

fn cmd_init(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        println!("Usage: super-ragondin init <instance-url> <sync-dir>");
        return Ok(());
    }

    let instance_url = &args[0];
    let sync_dir = PathBuf::from(&args[1]);

    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin");

    let config = Config {
        instance_url: instance_url.clone(),
        sync_dir: sync_dir.clone(),
        data_dir: data_dir.clone(),
        oauth_client: None,
        last_seq: None,
    };

    fs::create_dir_all(&sync_dir)?;
    fs::create_dir_all(&data_dir)?;
    fs::create_dir_all(config.staging_dir())?;

    config.save(&config_path())?;

    tracing::info!(
        instance_url,
        sync_dir = %sync_dir.display(),
        data_dir = %data_dir.display(),
        "⚙️ Initialized super-ragondin, run 'super-ragondin auth' to authenticate"
    );

    Ok(())
}

fn cmd_auth() -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found. Run 'init' first.".to_string()))?;

    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let oauth =
            OAuthClient::register(&config.instance_url, "Super Ragondin", "super-ragondin").await?;

        let state = uuid::Uuid::new_v4().to_string();
        let auth_url = oauth.authorization_url(&state);

        println!("Open this URL in your browser to authorize:");
        println!("{auth_url}");
        println!();
        println!("After authorizing, paste the authorization code here:");

        let mut code = String::new();
        std::io::stdin().read_line(&mut code)?;
        let code = code.trim();

        let mut oauth = oauth;
        oauth.exchange_code(code).await?;

        config.oauth_client = Some(oauth);
        config.save(&config_path())?;

        tracing::info!("🔑 Authentication successful");

        Ok::<_, Error>(())
    })?;

    Ok(())
}

fn cmd_sync() -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let client = open_client(&config)?;
    let mut engine = open_engine(&config)?;

    let rt = tokio::runtime::Runtime::new()?;
    run_sync_cycle(&rt, &mut engine, &client, &mut config)?;

    Ok(())
}

fn open_client(config: &Config) -> Result<super_ragondin_sync::remote::client::CozyClient> {
    let oauth = config
        .oauth_client
        .as_ref()
        .ok_or_else(|| Error::NotFound("Not authenticated".to_string()))?;

    let access_token = oauth
        .access_token()
        .ok_or_else(|| Error::NotFound("No access token".to_string()))?;

    Ok(super_ragondin_sync::remote::client::CozyClient::new(
        &config.instance_url,
        access_token,
    ))
}

fn open_engine(config: &Config) -> Result<SyncEngine> {
    let store = TreeStore::open(&config.store_dir())?;
    Ok(SyncEngine::new(
        store,
        config.sync_dir.clone(),
        config.staging_dir(),
    ))
}

fn cmd_watch() -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let client = open_client(&config)?;
    let mut engine = open_engine(&config)?;
    let rt = tokio::runtime::Runtime::new()?;

    let (tx, rx) = mpsc::channel::<WatchEvent>();

    let sync_dir = config.sync_dir.clone();
    thread::spawn(move || {
        let mut watcher = Watcher::new(&sync_dir, tx).expect("Failed to create watcher");
        watcher.run().expect("Watcher failed");
    });

    tracing::info!(sync_dir = %config.sync_dir.display(), "👁️ Watching for changes, press Ctrl+C to stop");

    let mut last_sync = Instant::now();
    let debounce = Duration::from_secs(2);

    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                tracing::debug!(event = ?event, "👁️ Watch event received");
                if last_sync.elapsed() > debounce {
                    tracing::info!("🔄 Changes detected, syncing");
                    if let Err(e) = run_sync_cycle(&rt, &mut engine, &client, &mut config) {
                        tracing::error!(error = %e, "❌ Sync failed");
                    }
                    last_sync = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_sync.elapsed() > Duration::from_secs(30) {
                    tracing::info!("🔄 Periodic sync");
                    if let Err(e) = run_sync_cycle(&rt, &mut engine, &client, &mut config) {
                        tracing::error!(error = %e, "❌ Sync failed");
                    }
                    last_sync = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::error!("❌ Watcher disconnected");
                break;
            }
        }
    }

    Ok(())
}

fn run_sync_cycle(
    rt: &tokio::runtime::Runtime,
    engine: &mut SyncEngine,
    client: &super_ragondin_sync::remote::client::CozyClient,
    config: &mut Config,
) -> Result<()> {
    use super_ragondin_rag::config::RagConfig;
    use super_ragondin_rag::embedder::OpenRouterEmbedder;
    use super_ragondin_rag::indexer::reconcile;
    use super_ragondin_rag::store::RagStore;

    let last_seq =
        rt.block_on(engine.fetch_and_apply_remote_changes(client, config.last_seq.as_deref()))?;
    config.last_seq = Some(last_seq);
    config.save(&config_path())?;

    rt.block_on(engine.run_cycle_async(client))?;

    // RAG reconciliation — only if API key is set
    let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    if !api_key.is_empty() {
        let db_path = config.sync_dir.join(".rag");
        let rag_config = RagConfig::from_env_with_db_path(db_path);
        let embedder = OpenRouterEmbedder::new(rag_config.clone());
        let store = super_ragondin_sync::store::TreeStore::open(&config.store_dir())?;
        let synced = store.list_all_synced()?;

        if let Err(e) = rt.block_on(async {
            let rag_store = RagStore::open(&rag_config.db_path).await?;
            reconcile(&synced, &config.sync_dir, &rag_store, &embedder).await
        }) {
            tracing::warn!(error = %e, "RAG reconciliation failed (non-fatal)");
        }
    }

    Ok(())
}

fn cmd_ask(args: &[String]) -> Result<()> {
    use super_ragondin_rag::config::RagConfig;
    use super_ragondin_rag::embedder::OpenRouterEmbedder;
    use super_ragondin_rag::store::RagStore;
    use super_ragondin_rag::searcher::search;
    use std::io::Write;

    if args.is_empty() {
        println!("Usage: super-ragondin ask <question>");
        return Ok(());
    }
    let question = args.join(" ");

    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let db_path = config.sync_dir.join(".rag");
    let rag_config = RagConfig::from_env_with_db_path(db_path);

    if rag_config.api_key.is_empty() {
        eprintln!("Error: OPENROUTER_API_KEY environment variable not set");
        std::process::exit(1);
    }

    let embedder = OpenRouterEmbedder::new(rag_config.clone());
    let rt = tokio::runtime::Runtime::new()?;

    let chunks = rt.block_on(async {
        let rag_store = RagStore::open(&rag_config.db_path).await?;
        search(&question, &rag_store, &embedder, 5).await
    }).map_err(|e| Error::Permanent(format!("{e:#}")))?;

    if chunks.is_empty() {
        println!("No relevant documents found.");
        return Ok(());
    }

    let context: String = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("[{}] {}\n{}", i + 1, c.doc_id, c.chunk_text))
        .collect::<Vec<_>>()
        .join("\n\n");

    let system_prompt = "You are a helpful assistant. Answer the user's question using only the provided document excerpts. Be concise and accurate. Respond in the same language as the question.";
    let user_prompt = format!("Documents:\n{context}\n\nQuestion: {question}");

    let chat_model = rag_config.chat_model.clone();
    let api_key = rag_config.api_key.clone();
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": chat_model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ],
        "stream": true
    });

    rt.block_on(async {
        use futures::StreamExt;

        let resp = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(&api_key)
            .header("HTTP-Referer", "https://github.com/super-ragondin")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Permanent(e.to_string()))?;

        let mut stream = resp.bytes_stream();
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let mut done = false;

        while !done {
            match stream.next().await {
                None => break,
                Some(Err(e)) => return Err(Error::Permanent(e.to_string())),
                Some(Ok(bytes)) => {
                    let text = std::str::from_utf8(&bytes).unwrap_or("");
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                done = true;
                                break;
                            }
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(content) = v["choices"][0]["delta"]["content"].as_str() {
                                    write!(out, "{content}").ok();
                                    out.flush().ok();
                                }
                            }
                        }
                    }
                }
            }
        }
        writeln!(out).ok();
        Ok::<_, Error>(())
    })?;

    println!("\nReferences:");
    for (i, chunk) in chunks.iter().enumerate() {
        use std::time::{Duration, UNIX_EPOCH};
        let date = UNIX_EPOCH + Duration::from_secs(chunk.mtime as u64);
        let dt: chrono::DateTime<chrono::Utc> = date.into();
        println!(
            "[{}] {}  ({}, {})",
            i + 1,
            chunk.doc_id,
            chunk.mime_type,
            dt.format("%Y-%m-%d")
        );
        let preview: String = chunk.chunk_text.chars().take(80).collect();
        println!("    \"{preview}...\"");
    }

    Ok(())
}

fn cmd_status() -> Result<()> {
    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    println!("Super Ragondin Status");
    println!("-------------------");
    println!("Instance:   {}", config.instance_url);
    println!("Sync dir:   {}", config.sync_dir.display());
    println!(
        "Last seq:   {}",
        config.last_seq.as_deref().unwrap_or("none")
    );
    println!("Authenticated: {}", config.oauth_client.is_some());

    if config.store_dir().exists() {
        let store = TreeStore::open(&config.store_dir())?;
        let remote = store.list_all_remote()?;
        let local = store.list_all_local()?;
        let synced = store.list_all_synced()?;

        println!();
        println!("Trees:");
        println!("  Remote: {} nodes", remote.len());
        println!("  Local:  {} nodes", local.len());
        println!("  Synced: {} nodes", synced.len());

        let planner = Planner::new(&store, config.sync_dir);
        let ops = planner.plan()?;
        let pending_ops: Vec<_> = ops
            .iter()
            .filter(|o| !matches!(o, PlanResult::NoOp))
            .collect();
        println!();
        println!("Pending operations: {}", pending_ops.len());
        for op in pending_ops {
            match op {
                PlanResult::Op(sync_op) => println!("  {sync_op:?}"),
                PlanResult::Conflict(conflict) => println!("  Conflict: {conflict:?}"),
                PlanResult::NoOp => {}
            }
        }
    }

    Ok(())
}
