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
