use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use super_ragondin_sync::config::Config;
use super_ragondin_sync::error::{Error, Result};
use super_ragondin_sync::ignore::IgnoreRules;
use super_ragondin_sync::local::watcher::WatchEventKind;
use super_ragondin_sync::model::PlanResult;
use super_ragondin_sync::planner::Planner;
use super_ragondin_sync::remote::auth::OAuthClient;
use super_ragondin_sync::store::TreeStore;
use super_ragondin_sync::sync::engine::SyncEngine;
use super_ragondin_sync::watcher_mux::{SyncTrigger, start_watchers};

fn main() -> Result<()> {
    super_ragondin_sync::logging::init("super-ragondin-cli");

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
            println!(
                "  ask [--web] <question>           Ask a question (--web enables web search)"
            );
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

    Config::validate_sync_dir(&sync_dir)?;

    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("super-ragondin");

    let api_key = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());

    let config = Config {
        instance_url: instance_url.clone(),
        sync_dir: sync_dir.clone(),
        data_dir: data_dir.clone(),
        oauth_client: None,
        last_seq: None,
        api_key,
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
    let rules = IgnoreRules::load(Some(&config.syncignore_path()));
    Ok(SyncEngine::new(
        store,
        config.sync_dir.clone(),
        config.staging_dir(),
        rules,
    ))
}

fn cmd_watch() -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let client = open_client(&config)?;
    let mut engine = open_engine(&config)?;
    let rt = tokio::runtime::Runtime::new()?;
    let (rx, cancel) = start_watchers(&config)?;

    tracing::info!(sync_dir = %config.sync_dir.display(), "👁️ Watching for changes, press Ctrl+C to stop");

    // Files with pending writes: path -> timestamp of first write event.
    // A file is "pending" after Create/Modify until we receive CloseWrite.
    let mut pending_writes: HashMap<PathBuf, Instant> = HashMap::new();
    let mut pending = true;
    let mut last_sync: Option<Instant> = None;
    let debounce = Duration::from_millis(200);
    let write_timeout = Duration::from_secs(30);

    loop {
        let debounce_ok = last_sync.is_none_or(|t| t.elapsed() > debounce);

        // Expire stale pending writes (safety net for mmap / non-close writers)
        let now = Instant::now();
        let expired: Vec<_> = pending_writes
            .iter()
            .filter(|&(_, &t)| now.duration_since(t) > write_timeout)
            .map(|(p, _)| p.clone())
            .collect();
        for path in &expired {
            tracing::debug!(path = %path.display(), "⏰ Pending write timed out, releasing");
            pending_writes.remove(path);
        }
        if !expired.is_empty() {
            pending = true;
        }

        if pending && debounce_ok && pending_writes.is_empty() {
            tracing::info!("🔄 Syncing");
            pending = false;
            if let Err(e) = run_sync_cycle(&rt, &mut engine, &client, &mut config) {
                tracing::error!(error = %e, "❌ Sync failed");
            }
            last_sync = Some(Instant::now());
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(trigger) => {
                match &trigger {
                    SyncTrigger::Local(event) => {
                        tracing::debug!(event = ?event, "👁️ Local watch event received");
                        match event.kind {
                            WatchEventKind::Create | WatchEventKind::Modify if !event.is_dir => {
                                pending_writes
                                    .entry(event.path.clone())
                                    .or_insert_with(Instant::now);
                            }
                            WatchEventKind::CloseWrite => {
                                if pending_writes.remove(&event.path).is_some() {
                                    tracing::debug!(
                                        path = %event.path.display(),
                                        "✅ Write complete (CLOSE_WRITE)"
                                    );
                                }
                            }
                            WatchEventKind::Delete => {
                                pending_writes.remove(&event.path);
                            }
                            _ => {}
                        }
                    }
                    SyncTrigger::Remote => {
                        tracing::info!("🔌 Remote change notification received");
                    }
                }
                pending = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_sync.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                    pending = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::error!("❌ All event sources disconnected");
                break;
            }
        }
    }

    cancel.cancel();
    Ok(())
}

fn run_sync_cycle(
    rt: &tokio::runtime::Runtime,
    engine: &mut SyncEngine,
    client: &super_ragondin_sync::remote::client::CozyClient,
    config: &mut Config,
) -> Result<()> {
    use super_ragondin_rag::config::RagConfig;
    use super_ragondin_rag::indexer::reconcile_if_configured;

    let last_seq =
        rt.block_on(engine.fetch_and_apply_remote_changes(client, config.last_seq.as_deref()))?;
    config.last_seq = Some(last_seq);
    config.save(&config_path())?;

    rt.block_on(engine.run_cycle_async(client))?;

    let api_key = RagConfig::resolve_api_key(config.api_key.as_deref());
    let synced = engine.store().list_all_synced()?;
    rt.block_on(reconcile_if_configured(
        api_key,
        config.rag_dir(),
        &synced,
        &config.sync_dir,
    ));

    Ok(())
}

struct CliInteraction;

impl super_ragondin_codemode::interaction::UserInteraction for CliInteraction {
    fn ask(&self, question: &str, choices: &[&str]) -> String {
        use std::io::Write as _;
        println!("\n{question}");
        for (i, c) in choices.iter().enumerate() {
            println!("  {}. {}", i + 1, c);
        }
        print!("\nYour answer (number or free text): ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        super_ragondin_codemode::tools::ask_user::resolve_answer(&line, choices)
    }
}

fn cmd_ask(args: &[String]) -> Result<()> {
    use super_ragondin_codemode::engine::CodeModeEngine;
    use super_ragondin_rag::config::RagConfig;

    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;
    let db_path = config.rag_dir();
    let mut rag_config = RagConfig::from_env_with_db_path(db_path);
    rag_config.api_key =
        RagConfig::resolve_api_key(config.api_key.as_deref()).ok_or_else(|| {
            Error::Permanent(
                "No API key configured (set OPENROUTER_API_KEY or add api_key to config)"
                    .to_string(),
            )
        })?;
    let rt = tokio::runtime::Runtime::new()?;

    let web_search = args.iter().any(|a| a == "--web");
    let question_args: Vec<&str> = args
        .iter()
        .filter(|a| a.as_str() != "--web")
        .map(String::as_str)
        .collect();

    if question_args.is_empty() {
        rt.block_on(async {
            let engine =
                super_ragondin_codemode::suggestions::SuggestionEngine::new(rag_config, config.sync_dir)
                    .await
                    .map_err(|e| Error::Permanent(format!("{e:#}")))?;
            let cwd = std::env::current_dir().ok();
            match engine.generate(cwd).await {
                Ok(suggestions) => {
                    println!("Not sure what to ask? Here are some ideas:\n");
                    for (i, s) in suggestions.iter().enumerate() {
                        println!("{}. {}", i + 1, s);
                    }
                }
                Err(e) => {
                    if e.downcast_ref::<super_ragondin_codemode::suggestions::NoFilesIndexed>()
                        .is_some()
                    {
                        println!("No files indexed yet. Run super-ragondin sync first.");
                    } else {
                        println!(
                            "Could not generate suggestions. Try: super-ragondin ask <your question>"
                        );
                    }
                }
            }
            Ok::<_, Error>(())
        })?;
        return Ok(());
    }
    let question = question_args.join(" ");

    let cozy_client = open_client(&config).ok().map(std::sync::Arc::new);

    rt.block_on(async {
        let interaction: Option<
            std::sync::Arc<dyn super_ragondin_codemode::interaction::UserInteraction>,
        > = Some(std::sync::Arc::new(CliInteraction));
        let engine = CodeModeEngine::new(rag_config, config.sync_dir, cozy_client, interaction)
            .await
            .map_err(|e| Error::Permanent(format!("{e:#}")))?;
        let cwd = std::env::current_dir().ok();
        let answer = engine
            .ask(&question, cwd, web_search)
            .await
            .map_err(|e| Error::Permanent(format!("{e:#}")))?;
        println!("{answer}");
        Ok::<_, Error>(())
    })?;

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

        let rules = IgnoreRules::load(Some(&config.syncignore_path()));
        let planner = Planner::new(&store, config.sync_dir, &rules);
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

#[cfg(test)]
mod tests {
    use super_ragondin_codemode::tools::ask_user::resolve_answer;

    #[test]
    fn test_cli_resolve_number_picks_choice() {
        assert_eq!(resolve_answer("2", &["alpha", "beta", "gamma"]), "beta");
    }

    #[test]
    fn test_cli_resolve_first() {
        assert_eq!(resolve_answer("1", &["yes", "no"]), "yes");
    }

    #[test]
    fn test_cli_resolve_out_of_range_verbatim() {
        assert_eq!(resolve_answer("0", &["a", "b"]), "0");
        assert_eq!(resolve_answer("4", &["a", "b", "c"]), "4");
    }

    #[test]
    fn test_cli_resolve_freeform() {
        assert_eq!(
            resolve_answer("my custom answer", &["a", "b"]),
            "my custom answer"
        );
    }
}
