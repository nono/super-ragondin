use std::net::SocketAddr;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super_ragondin_sync::remote::realtime::RealtimeListener;

/// Spin up a local WebSocket server that runs a handler function.
/// Returns the address for connecting.
async fn start_mock_ws_server<F, Fut>(handler: F) -> SocketAddr
where
    F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
            handler(ws_stream).await;
        }
    });

    addr
}

fn event_json(event_type: &str) -> String {
    serde_json::json!({
        "event": event_type,
        "payload": {
            "type": "io.cozy.files",
            "id": "file-123",
            "doc": {"_id": "file-123", "name": "test.txt"}
        }
    })
    .to_string()
}

#[tokio::test]
async fn handshake_and_event_triggers_nudge() {
    let addr = start_mock_ws_server(|ws_stream| async move {
        let (mut sink, mut stream) = ws_stream.split();

        // Expect AUTH message
        let msg = stream.next().await.unwrap().unwrap();
        let text = msg.into_text().unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["method"], "AUTH");
        assert_eq!(v["payload"], "test-token");

        // Send auth OK response
        sink.send(Message::Text(r#"{"event":"response","payload":{}}"#.into()))
            .await
            .unwrap();

        // Expect SUBSCRIBE message
        let msg = stream.next().await.unwrap().unwrap();
        let text = msg.into_text().unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["method"], "SUBSCRIBE");
        assert_eq!(v["payload"]["type"], "io.cozy.files");

        // Send a CREATED event
        sink.send(Message::Text(event_json("CREATED").into()))
            .await
            .unwrap();

        // Keep connection open for a bit
        tokio::time::sleep(Duration::from_secs(2)).await;
    })
    .await;

    let instance_url = format!("http://127.0.0.1:{addr}", addr = addr.port());
    let listener = RealtimeListener::new(&instance_url, "test-token");
    let cancel = CancellationToken::new();
    let mut rx = listener.start(cancel.clone());

    // Should receive a nudge within 1 second (200ms debounce + margin)
    let result = timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(result.is_ok(), "Should receive nudge");
    assert_eq!(result.unwrap(), Some(()));

    cancel.cancel();
}

#[tokio::test]
async fn server_close_triggers_reconnect_attempt() {
    // Track how many connections we get
    let (conn_tx, mut conn_rx) = mpsc::channel::<u32>(10);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let mut conn_count = 0u32;
        while let Ok((stream, _)) = listener.accept().await {
            conn_count += 1;
            let tx = conn_tx.clone();
            let count = conn_count;
            tokio::spawn(async move {
                let ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();
                let (mut sink, mut stream) = ws_stream.split();

                // Read AUTH
                let _ = stream.next().await;

                // Send auth OK
                sink.send(Message::Text(r#"{"event":"response","payload":{}}"#.into()))
                    .await
                    .unwrap();

                // Read SUBSCRIBE
                let _ = stream.next().await;

                let _ = tx.send(count).await;

                // Close immediately on first connection
                if count == 1 {
                    sink.close().await.unwrap();
                } else {
                    // Keep second connection alive
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            });
        }
    });

    let instance_url = format!("http://127.0.0.1:{port}", port = addr.port());
    let rt_listener = RealtimeListener::new(&instance_url, "test-token");
    let cancel = CancellationToken::new();
    let _rx = rt_listener.start(cancel.clone());

    // First connection
    let first = timeout(Duration::from_secs(5), conn_rx.recv()).await;
    assert_eq!(first.unwrap(), Some(1));

    // Second connection (after reconnect delay — 10s, but we wait up to 15s)
    let second = timeout(Duration::from_secs(15), conn_rx.recv()).await;
    assert_eq!(second.unwrap(), Some(2));

    cancel.cancel();
}

#[tokio::test]
async fn auth_rejection_stops_retrying() {
    let (conn_tx, mut conn_rx) = mpsc::channel::<u32>(10);

    let addr = start_mock_ws_server(move |ws_stream| async move {
        let (mut sink, mut stream) = ws_stream.split();

        // Read AUTH
        let _ = stream.next().await;

        // Send 403 error
        sink.send(Message::Text(
            r#"{"event":"error","payload":{"status":"403","message":"Forbidden"}}"#.into(),
        ))
        .await
        .unwrap();

        let _ = conn_tx.send(1).await;

        // Keep alive briefly
        tokio::time::sleep(Duration::from_secs(2)).await;
    })
    .await;

    let instance_url = format!("http://127.0.0.1:{port}", port = addr.port());
    let rt_listener = RealtimeListener::new(&instance_url, "bad-token");
    let cancel = CancellationToken::new();
    let mut rx = rt_listener.start(cancel.clone());

    // Should get the connection
    let first = timeout(Duration::from_secs(5), conn_rx.recv()).await;
    assert_eq!(first.unwrap(), Some(1));

    // The receiver should close (task exits on auth rejection, no retry)
    let result = timeout(Duration::from_secs(3), rx.recv()).await;
    assert!(result.is_ok(), "Channel should close after auth rejection");
    assert_eq!(
        result.unwrap(),
        None,
        "Should receive None (channel closed)"
    );

    cancel.cancel();
}

#[tokio::test]
async fn multiple_rapid_events_debounced_to_one_nudge() {
    let addr = start_mock_ws_server(|ws_stream| async move {
        let (mut sink, mut stream) = ws_stream.split();

        // AUTH
        let _ = stream.next().await;
        sink.send(Message::Text(r#"{"event":"response","payload":{}}"#.into()))
            .await
            .unwrap();

        // SUBSCRIBE
        let _ = stream.next().await;

        // Send 5 events in rapid succession (< 200ms apart)
        for i in 0..5 {
            let event_type = match i % 3 {
                0 => "CREATED",
                1 => "UPDATED",
                _ => "DELETED",
            };
            sink.send(Message::Text(event_json(event_type).into()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Keep connection alive
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;

    let instance_url = format!("http://127.0.0.1:{port}", port = addr.port());
    let rt_listener = RealtimeListener::new(&instance_url, "test-token");
    let cancel = CancellationToken::new();
    let mut rx = rt_listener.start(cancel.clone());

    // Should get exactly one nudge (debounced)
    let first = timeout(Duration::from_secs(2), rx.recv()).await;
    assert_eq!(first.unwrap(), Some(()));

    // Should NOT get a second nudge quickly
    let second = timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(
        second.is_err(),
        "Should not get a second nudge from the debounced batch"
    );

    cancel.cancel();
}

#[tokio::test]
async fn cancel_token_stops_listener() {
    let addr = start_mock_ws_server(|ws_stream| async move {
        let (mut sink, mut stream) = ws_stream.split();

        // AUTH
        let _ = stream.next().await;
        sink.send(Message::Text(r#"{"event":"response","payload":{}}"#.into()))
            .await
            .unwrap();

        // SUBSCRIBE
        let _ = stream.next().await;

        // Keep alive
        tokio::time::sleep(Duration::from_secs(30)).await;
    })
    .await;

    let instance_url = format!("http://127.0.0.1:{port}", port = addr.port());
    let rt_listener = RealtimeListener::new(&instance_url, "test-token");
    let cancel = CancellationToken::new();
    let mut rx = rt_listener.start(cancel.clone());

    // Wait a bit for connection to establish
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Cancel
    cancel.cancel();

    // Channel should close
    let result = timeout(Duration::from_secs(2), rx.recv()).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None);
}
