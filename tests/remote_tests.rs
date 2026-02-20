use cozy_desktop::model::NodeType;
use cozy_desktop::remote::auth::OAuthClient;
use cozy_desktop::remote::client::CozyClient;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_register_oauth_client() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/auth/register"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "client_id": "test-client-id",
            "client_secret": "test-client-secret",
            "registration_access_token": "test-reg-token"
        })))
        .mount(&mock_server)
        .await;

    let client = OAuthClient::register(&mock_server.uri(), "Cozy Desktop Test", "cozy-desktop")
        .await
        .unwrap();

    assert_eq!(client.client_id, "test-client-id");
}

#[tokio::test]
async fn test_fetch_changes() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/files/_changes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "last_seq": "5-abc",
            "results": [
                {
                    "id": "file-123",
                    "seq": "5-abc",
                    "doc": {
                        "_id": "file-123",
                        "_rev": "1-def",
                        "type": "file",
                        "name": "test.txt",
                        "dir_id": "root-id",
                        "md5sum": "d41d8cd98f00b204e9800998ecf8427e",
                        "size": 0,
                        "updated_at": "2026-01-01T00:00:00Z"
                    }
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let client = CozyClient::new(&mock_server.uri(), "fake-token");
    let changes = client.fetch_changes(None).await.unwrap();

    assert_eq!(changes.last_seq, "5-abc");
    assert_eq!(changes.results.len(), 1);
    assert_eq!(changes.results[0].node.name, "test.txt");
    assert_eq!(changes.results[0].node.node_type, NodeType::File);
}

#[tokio::test]
async fn test_refresh_token() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/auth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access-token",
            "refresh_token": "new-refresh-token"
        })))
        .mount(&mock_server)
        .await;

    let mut client = OAuthClient {
        instance_url: mock_server.uri(),
        client_id: "test-client-id".to_string(),
        client_secret: "test-client-secret".to_string(),
        registration_access_token: "test-reg-token".to_string(),
        access_token: Some("old-access-token".to_string()),
        refresh_token: Some("old-refresh-token".to_string()),
    };

    client.refresh().await.unwrap();

    assert_eq!(client.access_token(), Some("new-access-token"));
    assert_eq!(client.refresh_token, Some("new-refresh-token".to_string()));
}

#[tokio::test]
async fn test_refresh_token_without_refresh_token() {
    let mut client = OAuthClient {
        instance_url: "https://test.mycozy.cloud".to_string(),
        client_id: "test-client-id".to_string(),
        client_secret: "test-client-secret".to_string(),
        registration_access_token: "test-reg-token".to_string(),
        access_token: Some("old-access-token".to_string()),
        refresh_token: None,
    };

    let err = client.refresh().await.unwrap_err();
    assert!(err.to_string().contains("No refresh token"));
}

#[tokio::test]
async fn test_update_file() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/files/file-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "id": "file-123",
                "attributes": {
                    "type": "file",
                    "name": "hello.txt",
                    "dir_id": "parent-1",
                    "md5sum": "hvsmnRkNLIX24EaM7KQqIA==",
                    "size": 12,
                    "updated_at": "2026-01-01T00:00:00Z"
                },
                "meta": {
                    "rev": "2-newrev"
                }
            }
        })))
        .mount(&mock_server)
        .await;

    let client = CozyClient::new(&mock_server.uri(), "fake-token");
    let content = b"HELLO WORLD!".to_vec();
    let md5sum = "86fb269d190d2c85f6e0468ceca42a20";
    let remote_id = cozy_desktop::model::RemoteId::new("file-123");

    let node = client
        .update_file(&remote_id, content, md5sum, "1-oldrev")
        .await
        .unwrap();

    assert_eq!(node.id.as_str(), "file-123");
    assert_eq!(node.name, "hello.txt");
    assert_eq!(node.rev, "2-newrev");
}
