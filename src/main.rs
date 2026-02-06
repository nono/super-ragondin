use cozy_desktop::config::Config;
use cozy_desktop::error::{Error, Result};
use cozy_desktop::local::watcher::{WatchEvent, Watcher};
use cozy_desktop::model::PlanResult;
use cozy_desktop::planner::Planner;
use cozy_desktop::remote::auth::OAuthClient;
use cozy_desktop::store::TreeStore;
use cozy_desktop::sync::engine::SyncEngine;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cozy_desktop=info".into()),
        )
        .init();

    let args: Vec<String> = env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("init") => cmd_init(&args[2..]),
        Some("auth") => cmd_auth(),
        Some("sync") => cmd_sync(),
        Some("watch") => cmd_watch(),
        Some("status") => cmd_status(),
        _ => {
            println!("Usage: cozy-desktop <command>");
            println!();
            println!("Commands:");
            println!("  init <instance-url> <sync-dir>  Initialize configuration");
            println!("  auth                             Authenticate with Cozy");
            println!("  sync                             Run one sync cycle");
            println!("  watch                            Watch and sync continuously");
            println!("  status                           Show sync status");
            Ok(())
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cozy-desktop")
        .join("config.json")
}

fn cmd_init(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        println!("Usage: cozy-desktop init <instance-url> <sync-dir>");
        return Ok(());
    }

    let instance_url = &args[0];
    let sync_dir = PathBuf::from(&args[1]);

    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cozy-desktop");

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

    tracing::info!("Initialized cozy-desktop");
    tracing::info!("  Instance: {}", instance_url);
    tracing::info!("  Sync dir: {}", sync_dir.display());
    tracing::info!("  Data dir: {}", data_dir.display());
    tracing::info!("Run 'cozy-desktop auth' to authenticate");

    Ok(())
}

fn cmd_auth() -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found. Run 'init' first.".to_string()))?;

    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let oauth =
            OAuthClient::register(&config.instance_url, "Cozy Desktop PoC", "cozy-desktop-poc")
                .await?;

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

        tracing::info!("Authentication successful!");

        Ok::<_, Error>(())
    })?;

    Ok(())
}

fn cmd_sync() -> Result<()> {
    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let oauth = config
        .oauth_client
        .as_ref()
        .ok_or_else(|| Error::NotFound("Not authenticated".to_string()))?;

    let access_token = oauth
        .access_token()
        .ok_or_else(|| Error::NotFound("No access token".to_string()))?;

    let store = TreeStore::open(&config.store_dir())?;
    let _client = cozy_desktop::remote::client::CozyClient::new(&config.instance_url, access_token);

    let mut engine = SyncEngine::new(store, config.sync_dir.clone(), config.staging_dir());

    tracing::info!("Scanning local filesystem...");
    engine.initial_scan()?;

    let ops = engine.plan()?;
    tracing::info!("Planned {} operations", ops.len());

    for op in &ops {
        match op {
            PlanResult::Op(sync_op) => tracing::info!("  {:?}", sync_op),
            PlanResult::Conflict(conflict) => tracing::warn!("  Conflict: {:?}", conflict),
            PlanResult::NoOp => {}
        }
    }

    for op in &ops {
        if let PlanResult::Op(sync_op) = op {
            engine.execute_op(sync_op)?;
        }
    }

    config.save(&config_path())?;

    tracing::info!("Sync complete!");

    Ok(())
}

fn cmd_watch() -> Result<()> {
    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    let (tx, rx) = mpsc::channel::<WatchEvent>();

    let sync_dir = config.sync_dir.clone();
    thread::spawn(move || {
        let mut watcher = Watcher::new(&sync_dir, tx).expect("Failed to create watcher");
        watcher.run().expect("Watcher failed");
    });

    tracing::info!("Watching for changes in {}", config.sync_dir.display());
    tracing::info!("Press Ctrl+C to stop");

    let mut last_sync = Instant::now();
    let debounce = Duration::from_secs(2);

    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                tracing::debug!("Event: {:?}", event);
                if last_sync.elapsed() > debounce {
                    tracing::info!("Changes detected, syncing...");
                    if let Err(e) = cmd_sync() {
                        tracing::error!("Sync failed: {}", e);
                    }
                    last_sync = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_sync.elapsed() > Duration::from_secs(30) {
                    tracing::info!("Periodic sync...");
                    if let Err(e) = cmd_sync() {
                        tracing::error!("Sync failed: {}", e);
                    }
                    last_sync = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::error!("Watcher disconnected");
                break;
            }
        }
    }

    Ok(())
}

fn cmd_status() -> Result<()> {
    let config = Config::load(&config_path())?
        .ok_or_else(|| Error::NotFound("Config not found".to_string()))?;

    println!("Cozy Desktop Status");
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
