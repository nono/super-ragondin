use crate::config::Config;
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
/// # Panics
///
/// Panics if the filesystem watcher or the per-thread tokio runtime cannot be
/// created.
pub fn start_watchers(config: &Config) -> (mpsc::Receiver<SyncTrigger>, CancellationToken) {
    let (tx, rx) = mpsc::channel::<SyncTrigger>();

    // Local filesystem watcher
    let local_tx = tx.clone();
    let (local_watch_tx, local_watch_rx) = mpsc::channel::<WatchEvent>();
    let sync_dir = config.sync_dir.clone();
    let watcher_rules = IgnoreRules::load(Some(&config.syncignore_path()));
    std::thread::spawn(move || {
        let mut watcher = Watcher::new(&sync_dir, local_watch_tx, watcher_rules)
            .expect("Failed to create watcher");
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

    (rx, cancel)
}
