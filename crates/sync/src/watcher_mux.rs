use crate::config::Config;
use crate::error::Result;
use crate::ignore::IgnoreRules;
use crate::local::watcher::{WatchEvent, Watcher};
use crate::remote::realtime::RealtimeListener;
use std::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A trigger that causes the sync loop to run a cycle.
pub enum SyncTrigger {
    Local(WatchEvent),
    Remote,
}

/// Start the local filesystem watcher and remote realtime listener.
///
/// Returns a channel receiver that yields `SyncTrigger` values whenever a
/// local or remote change is detected, plus a `CancellationToken` to stop the
/// realtime listener.
///
/// # Errors
///
/// Returns an error if the filesystem watcher cannot be initialized (e.g. the
/// sync directory does not exist or inotify is unavailable).
///
/// # Panics
///
/// The background watcher thread panics if the watcher fails after
/// initialization, or if the per-thread tokio runtime cannot be created.
pub fn start_watchers(config: &Config) -> Result<(mpsc::Receiver<SyncTrigger>, CancellationToken)> {
    let (tx, rx) = mpsc::channel::<SyncTrigger>();

    // Local filesystem watcher — create before spawning so init errors propagate to caller.
    let local_tx = tx.clone();
    let (local_watch_tx, local_watch_rx) = mpsc::channel::<WatchEvent>();
    let watcher_rules = IgnoreRules::load(Some(&config.syncignore_path()));
    let mut watcher = Watcher::new(&config.sync_dir, local_watch_tx, watcher_rules)?;
    std::thread::spawn(move || {
        watcher.run().expect("Watcher failed");
    });
    std::thread::spawn(move || {
        for event in local_watch_rx {
            if local_tx.send(SyncTrigger::Local(event)).is_err() {
                break;
            }
        }
    });

    // Realtime WebSocket listener
    let cancel = CancellationToken::new();
    let oauth = config.oauth_client.as_ref();
    if let Some(access_token) = oauth.and_then(|o| o.access_token().map(String::from)) {
        let instance_url = config.instance_url.clone();
        let remote_tx = tx;
        let cancel2 = cancel.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                let listener = RealtimeListener::new(&instance_url, &access_token);
                let mut rx = listener.start(cancel2);
                while rx.recv().await.is_some() {
                    if remote_tx.send(SyncTrigger::Remote).is_err() {
                        break;
                    }
                }
            });
        });
    } else {
        tracing::warn!("🔌 No access token, realtime notifications disabled");
    }

    Ok((rx, cancel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;

    fn config_with_sync_dir(sync_dir: PathBuf) -> Config {
        Config {
            instance_url: "https://test.mycozy.cloud".to_string(),
            sync_dir,
            data_dir: PathBuf::from("/tmp/data"),
            oauth_client: None,
            last_seq: None,
            api_key: None,
        }
    }

    #[test]
    fn start_watchers_fails_for_nonexistent_sync_dir() {
        let config = config_with_sync_dir(PathBuf::from("/nonexistent/path/that/does/not/exist"));
        let result = start_watchers(&config);
        assert!(result.is_err());
    }
}
