use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const DEBOUNCE_QUIET: Duration = Duration::from_millis(200);
const DEBOUNCE_MAX: Duration = Duration::from_secs(5);
const RECONNECT_DELAY: Duration = Duration::from_secs(10);

pub struct RealtimeListener {
    instance_url: String,
    access_token: String,
}

impl RealtimeListener {
    #[must_use]
    pub fn new(instance_url: &str, access_token: &str) -> Self {
        Self {
            instance_url: instance_url.trim_end_matches('/').to_string(),
            access_token: access_token.to_string(),
        }
    }

    /// Spawns the WebSocket listener as a tokio task.
    /// Returns a receiver that gets `()` on each remote change (debounced).
    #[must_use]
    pub fn start(self, cancel: CancellationToken) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            self.run_loop(tx, cancel).await;
        });
        rx
    }

    async fn run_loop(&self, tx: mpsc::Sender<()>, cancel: CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                return;
            }

            match self.connect_and_listen(&tx, &cancel).await {
                Ok(()) => return, // cancelled or channel closed
                Err(StopReason::AuthRejected) => {
                    tracing::error!("🔌 Realtime auth rejected (403), stopping");
                    return;
                }
                Err(StopReason::Disconnected(e)) => {
                    tracing::warn!(error = %e, "🔌 Realtime disconnected, reconnecting in 10s");
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        () = tokio::time::sleep(RECONNECT_DELAY) => {}
                    }
                }
            }
        }
    }

    async fn connect_and_listen(
        &self,
        tx: &mpsc::Sender<()>,
        cancel: &CancellationToken,
    ) -> std::result::Result<(), StopReason> {
        let ws_url = build_ws_url(&self.instance_url);
        tracing::info!(url = %ws_url, "🔌 Connecting to realtime WebSocket");

        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| StopReason::Disconnected(e.to_string()))?;

        let (mut sink, mut stream) = ws_stream.split();

        // Authenticate
        let auth_msg = auth_message(&self.access_token);
        sink.send(Message::Text(auth_msg.into()))
            .await
            .map_err(|e| StopReason::Disconnected(e.to_string()))?;

        // Wait for auth response
        check_auth_response(&mut stream).await?;

        // Subscribe
        let sub_msg = subscribe_message();
        sink.send(Message::Text(sub_msg.into()))
            .await
            .map_err(|e| StopReason::Disconnected(e.to_string()))?;

        tracing::info!("🔌 Realtime WebSocket connected and subscribed");

        // Listen for events with debouncing
        debounced_listen(&mut stream, tx, cancel).await
    }
}

enum StopReason {
    AuthRejected,
    Disconnected(String),
}

fn build_ws_url(instance_url: &str) -> String {
    let url = instance_url.trim_end_matches('/');
    let ws_scheme = if url.starts_with("https://") {
        "wss"
    } else {
        "ws"
    };
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    format!("{ws_scheme}://{host}/realtime/")
}

fn auth_message(access_token: &str) -> String {
    serde_json::json!({
        "method": "AUTH",
        "payload": access_token
    })
    .to_string()
}

fn subscribe_message() -> String {
    serde_json::json!({
        "method": "SUBSCRIBE",
        "payload": {
            "type": "io.cozy.files"
        }
    })
    .to_string()
}

fn is_event_message(text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    matches!(
        v.get("event").and_then(|e| e.as_str()),
        Some("CREATED" | "UPDATED" | "DELETED")
    )
}

async fn check_auth_response(
    stream: &mut SplitStream<WsStream>,
) -> std::result::Result<(), StopReason> {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
                    && v.get("event").and_then(|e| e.as_str()) == Some("error")
                {
                    let status = v
                        .get("payload")
                        .and_then(|p| p.get("status"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    if status == "403" {
                        return Err(StopReason::AuthRejected);
                    }
                }
                return Ok(());
            }
            Ok(Message::Close(frame)) => {
                let code = frame.as_ref().map(|f| f.code);
                if code
                    == Some(
                        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
                    )
                {
                    return Err(StopReason::AuthRejected);
                }
                return Err(StopReason::Disconnected(
                    "Connection closed during auth".to_string(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(StopReason::Disconnected(e.to_string())),
        }
    }
    Err(StopReason::Disconnected(
        "Stream ended during auth".to_string(),
    ))
}

async fn debounced_listen(
    stream: &mut SplitStream<WsStream>,
    tx: &mpsc::Sender<()>,
    cancel: &CancellationToken,
) -> std::result::Result<(), StopReason> {
    let mut first_event: Option<Instant> = None;
    let mut last_event: Option<Instant> = None;

    loop {
        let sleep_duration = match (first_event, last_event) {
            (Some(first), Some(last)) => {
                let quiet_deadline = last + DEBOUNCE_QUIET;
                let max_deadline = first + DEBOUNCE_MAX;
                let deadline = quiet_deadline.min(max_deadline);
                Some(deadline.saturating_duration_since(Instant::now()))
            }
            _ => None,
        };

        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if is_event_message(&text) {
                            let now = Instant::now();
                            if first_event.is_none() {
                                first_event = Some(now);
                            }
                            last_event = Some(now);
                        } else {
                            tracing::debug!(text = %text, "🔌 Non-event message");
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        if first_event.is_some() {
                            let _ = tx.send(()).await;
                        }
                        return Err(StopReason::Disconnected("WebSocket closed".to_string()));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        if first_event.is_some() {
                            let _ = tx.send(()).await;
                        }
                        return Err(StopReason::Disconnected(e.to_string()));
                    }
                    None => {
                        if first_event.is_some() {
                            let _ = tx.send(()).await;
                        }
                        return Err(StopReason::Disconnected("Stream ended".to_string()));
                    }
                }
            }
            () = async {
                if let Some(d) = sleep_duration {
                    tokio::time::sleep(d).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                // Debounce timer fired — send nudge
                first_event = None;
                last_event = None;
                if tx.send(()).await.is_err() {
                    return Ok(()); // receiver dropped
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_message_format() {
        let msg = auth_message("my-token-123");
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["method"], "AUTH");
        assert_eq!(v["payload"], "my-token-123");
    }

    #[test]
    fn subscribe_message_format() {
        let msg = subscribe_message();
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["method"], "SUBSCRIBE");
        assert_eq!(v["payload"]["type"], "io.cozy.files");
    }

    #[test]
    fn build_ws_url_https() {
        assert_eq!(
            build_ws_url("https://alice.mycozy.cloud"),
            "wss://alice.mycozy.cloud/realtime/"
        );
    }

    #[test]
    fn build_ws_url_http_localhost() {
        assert_eq!(
            build_ws_url("http://alice.localhost:8080"),
            "ws://alice.localhost:8080/realtime/"
        );
    }

    #[test]
    fn build_ws_url_strips_trailing_slash() {
        assert_eq!(
            build_ws_url("https://alice.mycozy.cloud/"),
            "wss://alice.mycozy.cloud/realtime/"
        );
    }

    #[test]
    fn is_event_message_created() {
        let msg = r#"{"event":"CREATED","payload":{"type":"io.cozy.files","id":"abc","doc":{}}}"#;
        assert!(is_event_message(msg));
    }

    #[test]
    fn is_event_message_updated() {
        let msg = r#"{"event":"UPDATED","payload":{"type":"io.cozy.files","id":"abc","doc":{}}}"#;
        assert!(is_event_message(msg));
    }

    #[test]
    fn is_event_message_deleted() {
        let msg = r#"{"event":"DELETED","payload":{"type":"io.cozy.files","id":"abc","doc":{}}}"#;
        assert!(is_event_message(msg));
    }

    #[test]
    fn is_event_message_other() {
        let msg = r#"{"event":"error","payload":{"status":"403"}}"#;
        assert!(!is_event_message(msg));
    }

    #[test]
    fn is_event_message_invalid_json() {
        assert!(!is_event_message("not json"));
    }

    #[test]
    fn realtime_listener_new() {
        let listener = RealtimeListener::new("https://alice.mycozy.cloud", "token-123");
        assert_eq!(listener.instance_url, "https://alice.mycozy.cloud");
        assert_eq!(listener.access_token, "token-123");
    }

    #[test]
    fn realtime_listener_strips_trailing_slash() {
        let listener = RealtimeListener::new("https://alice.mycozy.cloud/", "token-123");
        assert_eq!(listener.instance_url, "https://alice.mycozy.cloud");
    }
}
