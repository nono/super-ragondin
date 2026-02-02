# Cozy Desktop Sync PoC Implementation Plan

**Goal:** Build a proof-of-concept file synchronization client for Cozy Cloud in Rust, using the 3-tree model (Remote/Local/Synced) with simulation-based testing.

**Architecture:** Single control thread orchestrating all sync logic (deterministic for testing), with dedicated threads for I/O (filesystem, network). The 3-tree model stores: Remote (Cozy state), Local (disk state), Synced (last known synchronized state). A planner derives operations by comparing trees. Simulation testing mocks I/O for reproducible randomized tests.

**Tech Stack:**
- Rust (2021 edition)
- tokio (async runtime for network I/O)
- fjall (embedded key-value store for the 3 trees)
- inotify-rs (Linux filesystem watching)
- reqwest (HTTP client for Cozy API)
- proptest (property-based testing)
- serde/serde_json (serialization)

---

## Phase 1: Project Setup & Core Data Model

### Task 1: Initialize Rust Project

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`

**Step 1: Create Cargo.toml with dependencies**

```toml
[package]
name = "cozy-desktop"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
fjall = "2"
inotify = "0.11"
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
md-5 = "0.10"
hex = "0.4"
url = "2"

[dev-dependencies]
proptest = "1"
tempfile = "3"
tokio-test = "0.4"
wiremock = "0.6"
```

**Step 2: Create src/lib.rs**

```rust
pub mod model;
pub mod store;
pub mod planner;
pub mod remote;
pub mod local;
pub mod sync;
pub mod error;

#[cfg(test)]
pub mod simulator;
```

**Step 3: Create src/main.rs**

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Cozy Desktop starting...");
}
```

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: Successful build

**Step 5: Commit**

```bash
git add Cargo.toml src/
git commit -m "chore: initialize Rust project with dependencies"
```

---

### Task 2: Define Core Data Model

**Files:**
- Create: `src/model.rs`
- Create: `src/error.rs`

**Step 1: Create error types**

```rust
// src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Store error: {0}")]
    Store(#[from] fjall::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Conflict: {0}")]
    Conflict(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

**Step 2: Create core model types**

```rust
// src/model.rs
use serde::{Deserialize, Serialize};

/// Unique identifier for a node (file or directory)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Type of filesystem node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    File,
    Directory,
}

/// A node in the filesystem tree (either local or remote)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Unique identifier
    pub id: NodeId,
    /// Parent directory ID (None for root)
    pub parent_id: Option<NodeId>,
    /// Name of the file or directory
    pub name: String,
    /// Type: file or directory
    pub node_type: NodeType,
    /// MD5 checksum (files only)
    pub md5sum: Option<String>,
    /// Size in bytes (files only)
    pub size: Option<u64>,
    /// Last modification timestamp (Unix epoch seconds)
    pub updated_at: i64,
    /// CouchDB revision (remote only)
    pub rev: Option<String>,
}

impl Node {
    pub fn is_file(&self) -> bool {
        self.node_type == NodeType::File
    }

    pub fn is_dir(&self) -> bool {
        self.node_type == NodeType::Directory
    }
}

/// Which tree a node belongs to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeType {
    Remote,
    Local,
    Synced,
}

/// An operation to perform to synchronize trees
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// Download file from remote to local
    Download { node_id: NodeId },
    /// Upload file from local to remote
    Upload { node_id: NodeId },
    /// Create directory locally
    CreateLocalDir { node_id: NodeId },
    /// Create directory on remote
    CreateRemoteDir { node_id: NodeId },
    /// Delete file/dir locally
    DeleteLocal { node_id: NodeId },
    /// Delete file/dir on remote (trash)
    DeleteRemote { node_id: NodeId },
    /// Move/rename locally
    MoveLocal { node_id: NodeId, new_parent_id: NodeId, new_name: String },
    /// Move/rename on remote
    MoveRemote { node_id: NodeId, new_parent_id: NodeId, new_name: String },
    /// Conflict detected, needs resolution
    Conflict { node_id: NodeId, reason: String },
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Successful build

**Step 4: Commit**

```bash
git add src/model.rs src/error.rs
git commit -m "feat: add core data model types"
```

---

### Task 3: Implement Tree Store with fjall

**Files:**
- Create: `src/store.rs`
- Create: `src/store/tree.rs`
- Create: `tests/store_tests.rs`

**Step 1: Write failing test for tree store**

```rust
// tests/store_tests.rs
use cozy_desktop::model::{Node, NodeId, NodeType};
use cozy_desktop::store::TreeStore;
use tempfile::tempdir;

#[test]
fn test_insert_and_get_node() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let node = Node {
        id: NodeId::new("test-id"),
        parent_id: None,
        name: "root".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1234567890,
        rev: None,
    };

    store.insert_local(&node).unwrap();
    let retrieved = store.get_local(&node.id).unwrap().unwrap();

    assert_eq!(retrieved.name, "root");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_insert_and_get_node`
Expected: FAIL (module not found)

**Step 3: Implement TreeStore**

```rust
// src/store.rs
pub mod tree;
pub use tree::TreeStore;
```

```rust
// src/store/tree.rs
use crate::error::Result;
use crate::model::{Node, NodeId, TreeType};
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle};
use std::path::Path;

/// Persistent storage for the 3 trees using fjall
pub struct TreeStore {
    keyspace: Keyspace,
    remote: PartitionHandle,
    local: PartitionHandle,
    synced: PartitionHandle,
}

impl TreeStore {
    pub fn open(path: &Path) -> Result<Self> {
        let keyspace = Config::new(path).open()?;

        let remote = keyspace.open_partition("remote", PartitionCreateOptions::default())?;
        let local = keyspace.open_partition("local", PartitionCreateOptions::default())?;
        let synced = keyspace.open_partition("synced", PartitionCreateOptions::default())?;

        Ok(Self {
            keyspace,
            remote,
            local,
            synced,
        })
    }

    fn partition(&self, tree: TreeType) -> &PartitionHandle {
        match tree {
            TreeType::Remote => &self.remote,
            TreeType::Local => &self.local,
            TreeType::Synced => &self.synced,
        }
    }

    pub fn insert(&self, tree: TreeType, node: &Node) -> Result<()> {
        let key = node.id.as_str().as_bytes();
        let value = serde_json::to_vec(node)?;
        self.partition(tree).insert(key, value)?;
        Ok(())
    }

    pub fn get(&self, tree: TreeType, id: &NodeId) -> Result<Option<Node>> {
        let key = id.as_str().as_bytes();
        match self.partition(tree).get(key)? {
            Some(bytes) => {
                let node: Node = serde_json::from_slice(&bytes)?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    pub fn delete(&self, tree: TreeType, id: &NodeId) -> Result<()> {
        let key = id.as_str().as_bytes();
        self.partition(tree).remove(key)?;
        Ok(())
    }

    pub fn list_children(&self, tree: TreeType, parent_id: &NodeId) -> Result<Vec<Node>> {
        let mut children = Vec::new();
        for item in self.partition(tree).iter() {
            let (_, value) = item?;
            let node: Node = serde_json::from_slice(&value)?;
            if node.parent_id.as_ref() == Some(parent_id) {
                children.push(node);
            }
        }
        Ok(children)
    }

    pub fn list_all(&self, tree: TreeType) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        for item in self.partition(tree).iter() {
            let (_, value) = item?;
            let node: Node = serde_json::from_slice(&value)?;
            nodes.push(node);
        }
        Ok(nodes)
    }

    // Convenience methods for each tree
    pub fn insert_remote(&self, node: &Node) -> Result<()> {
        self.insert(TreeType::Remote, node)
    }

    pub fn insert_local(&self, node: &Node) -> Result<()> {
        self.insert(TreeType::Local, node)
    }

    pub fn insert_synced(&self, node: &Node) -> Result<()> {
        self.insert(TreeType::Synced, node)
    }

    pub fn get_remote(&self, id: &NodeId) -> Result<Option<Node>> {
        self.get(TreeType::Remote, id)
    }

    pub fn get_local(&self, id: &NodeId) -> Result<Option<Node>> {
        self.get(TreeType::Local, id)
    }

    pub fn get_synced(&self, id: &NodeId) -> Result<Option<Node>> {
        self.get(TreeType::Synced, id)
    }

    pub fn flush(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        Ok(())
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_insert_and_get_node`
Expected: PASS

**Step 5: Add more tests**

```rust
// Append to tests/store_tests.rs

#[test]
fn test_list_children() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let parent = Node {
        id: NodeId::new("parent"),
        parent_id: None,
        name: "docs".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: None,
    };

    let child1 = Node {
        id: NodeId::new("child1"),
        parent_id: Some(NodeId::new("parent")),
        name: "file1.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("abc123".to_string()),
        size: Some(100),
        updated_at: 1001,
        rev: None,
    };

    let child2 = Node {
        id: NodeId::new("child2"),
        parent_id: Some(NodeId::new("parent")),
        name: "file2.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("def456".to_string()),
        size: Some(200),
        updated_at: 1002,
        rev: None,
    };

    store.insert_local(&parent).unwrap();
    store.insert_local(&child1).unwrap();
    store.insert_local(&child2).unwrap();

    let children = store.list_children(cozy_desktop::model::TreeType::Local, &parent.id).unwrap();
    assert_eq!(children.len(), 2);
}

#[test]
fn test_three_trees_independent() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    let node = Node {
        id: NodeId::new("shared-id"),
        parent_id: None,
        name: "test".to_string(),
        node_type: NodeType::File,
        md5sum: Some("hash".to_string()),
        size: Some(50),
        updated_at: 1000,
        rev: None,
    };

    // Insert only in remote
    store.insert_remote(&node).unwrap();

    // Should exist in remote, not in local or synced
    assert!(store.get_remote(&node.id).unwrap().is_some());
    assert!(store.get_local(&node.id).unwrap().is_none());
    assert!(store.get_synced(&node.id).unwrap().is_none());
}
```

**Step 6: Run all tests**

Run: `cargo test`
Expected: All tests pass

**Step 7: Commit**

```bash
git add src/store.rs src/store/ tests/
git commit -m "feat: implement TreeStore with fjall for 3-tree model"
```

---

## Phase 2: Remote Watcher (Cozy API)

### Task 4: Implement OAuth2 Client Registration

**Files:**
- Create: `src/remote.rs`
- Create: `src/remote/auth.rs`
- Create: `src/remote/client.rs`
- Create: `tests/remote_tests.rs`

**Step 1: Write failing test for OAuth registration**

```rust
// tests/remote_tests.rs
use cozy_desktop::remote::auth::OAuthClient;
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

    let client = OAuthClient::register(
        &mock_server.uri(),
        "Cozy Desktop Test",
        "cozy-desktop",
    ).await.unwrap();

    assert_eq!(client.client_id, "test-client-id");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_register_oauth_client`
Expected: FAIL (module not found)

**Step 3: Implement OAuth client**

```rust
// src/remote.rs
pub mod auth;
pub mod client;
```

```rust
// src/remote/auth.rs
use crate::error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
struct RegisterRequest {
    redirect_uris: Vec<String>,
    client_name: String,
    software_id: String,
    client_kind: String,
    client_uri: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterResponse {
    pub client_id: String,
    pub client_secret: String,
    pub registration_access_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthClient {
    pub instance_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub registration_access_token: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

impl OAuthClient {
    pub async fn register(
        instance_url: &str,
        client_name: &str,
        software_id: &str,
    ) -> Result<Self> {
        let http = reqwest::Client::new();

        let request = RegisterRequest {
            redirect_uris: vec!["http://localhost:8080/callback".to_string()],
            client_name: client_name.to_string(),
            software_id: software_id.to_string(),
            client_kind: "desktop".to_string(),
            client_uri: "https://github.com/cozy/cozy-desktop".to_string(),
        };

        let resp: RegisterResponse = http
            .post(format!("{}/auth/register", instance_url))
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(Self {
            instance_url: instance_url.to_string(),
            client_id: resp.client_id,
            client_secret: resp.client_secret,
            registration_access_token: resp.registration_access_token,
            access_token: None,
            refresh_token: None,
        })
    }

    pub fn authorization_url(&self, state: &str) -> String {
        format!(
            "{}/auth/authorize?client_id={}&redirect_uri={}&state={}&response_type=code&scope={}",
            self.instance_url,
            self.client_id,
            urlencoding::encode("http://localhost:8080/callback"),
            state,
            urlencoding::encode("io.cozy.files")
        )
    }

    pub async fn exchange_code(&mut self, code: &str) -> Result<()> {
        let http = reqwest::Client::new();

        #[derive(Serialize)]
        struct TokenRequest<'a> {
            grant_type: &'a str,
            code: &'a str,
            client_id: &'a str,
            client_secret: &'a str,
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
        }

        let resp: TokenResponse = http
            .post(format!("{}/auth/access_token", self.instance_url))
            .form(&TokenRequest {
                grant_type: "authorization_code",
                code,
                client_id: &self.client_id,
                client_secret: &self.client_secret,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.access_token = Some(resp.access_token);
        self.refresh_token = Some(resp.refresh_token);
        Ok(())
    }

    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }
}
```

**Step 4: Add urlencoding dependency**

Add to Cargo.toml:
```toml
urlencoding = "2"
```

**Step 5: Run test to verify it passes**

Run: `cargo test test_register_oauth_client`
Expected: PASS

**Step 6: Commit**

```bash
git add Cargo.toml src/remote.rs src/remote/
git commit -m "feat: implement OAuth2 client registration"
```

---

### Task 5: Implement Cozy Files API Client

**Files:**
- Create: `src/remote/client.rs`
- Modify: `tests/remote_tests.rs`

**Step 1: Write failing test for changes feed**

```rust
// Append to tests/remote_tests.rs

use cozy_desktop::remote::client::CozyClient;
use cozy_desktop::model::NodeType;

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
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_fetch_changes`
Expected: FAIL

**Step 3: Implement CozyClient**

```rust
// src/remote/client.rs
use crate::error::Result;
use crate::model::{Node, NodeId, NodeType};
use serde::{Deserialize, Serialize};

pub struct CozyClient {
    instance_url: String,
    access_token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangesResponse {
    pub last_seq: String,
    pub results: Vec<ChangeResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangeResult {
    pub id: String,
    pub seq: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(flatten)]
    pub node: Node,
}

#[derive(Debug, Deserialize)]
struct RawChangeResult {
    id: String,
    seq: String,
    #[serde(default)]
    deleted: bool,
    doc: Option<RawDoc>,
}

#[derive(Debug, Deserialize)]
struct RawDoc {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_rev")]
    rev: String,
    #[serde(rename = "type")]
    doc_type: String,
    name: String,
    dir_id: Option<String>,
    md5sum: Option<String>,
    size: Option<u64>,
    updated_at: String,
    #[serde(default)]
    trashed: bool,
}

#[derive(Debug, Deserialize)]
struct RawChangesResponse {
    last_seq: String,
    results: Vec<RawChangeResult>,
}

impl CozyClient {
    pub fn new(instance_url: &str, access_token: &str) -> Self {
        Self {
            instance_url: instance_url.trim_end_matches('/').to_string(),
            access_token: access_token.to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn fetch_changes(&self, since: Option<&str>) -> Result<ChangesResponse> {
        let mut url = format!("{}/files/_changes?include_docs=true", self.instance_url);
        if let Some(seq) = since {
            url.push_str(&format!("&since={}", seq));
        }

        let raw: RawChangesResponse = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let results = raw
            .results
            .into_iter()
            .filter_map(|r| {
                if r.deleted {
                    Some(ChangeResult {
                        id: r.id.clone(),
                        seq: r.seq,
                        deleted: true,
                        node: Node {
                            id: NodeId::new(r.id),
                            parent_id: None,
                            name: String::new(),
                            node_type: NodeType::File,
                            md5sum: None,
                            size: None,
                            updated_at: 0,
                            rev: None,
                        },
                    })
                } else {
                    r.doc.map(|doc| {
                        let node_type = if doc.doc_type == "directory" {
                            NodeType::Directory
                        } else {
                            NodeType::File
                        };
                        ChangeResult {
                            id: r.id,
                            seq: r.seq,
                            deleted: false,
                            node: Node {
                                id: NodeId::new(&doc.id),
                                parent_id: doc.dir_id.map(NodeId::new),
                                name: doc.name,
                                node_type,
                                md5sum: doc.md5sum,
                                size: doc.size,
                                updated_at: parse_timestamp(&doc.updated_at),
                                rev: Some(doc.rev),
                            },
                        }
                    })
                }
            })
            .collect();

        Ok(ChangesResponse {
            last_seq: raw.last_seq,
            results,
        })
    }

    pub async fn download_file(&self, file_id: &NodeId) -> Result<bytes::Bytes> {
        let url = format!("{}/files/download/{}", self.instance_url, file_id.as_str());
        let bytes = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(bytes)
    }

    pub async fn upload_file(
        &self,
        parent_id: &NodeId,
        name: &str,
        content: Vec<u8>,
        md5sum: &str,
    ) -> Result<Node> {
        let url = format!(
            "{}/files/{}?Type=file&Name={}",
            self.instance_url,
            parent_id.as_str(),
            urlencoding::encode(name)
        );

        let resp: serde_json::Value = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-MD5", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, hex::decode(md5sum).unwrap_or_default()))
            .header("Content-Type", "application/octet-stream")
            .body(content)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        parse_file_response(&resp)
    }

    pub async fn create_directory(&self, parent_id: &NodeId, name: &str) -> Result<Node> {
        let url = format!(
            "{}/files/{}?Type=directory&Name={}",
            self.instance_url,
            parent_id.as_str(),
            urlencoding::encode(name)
        );

        let resp: serde_json::Value = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        parse_file_response(&resp)
    }

    pub async fn trash(&self, id: &NodeId) -> Result<()> {
        let url = format!("{}/files/{}", self.instance_url, id.as_str());

        self.http
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&serde_json::json!({
                "data": {
                    "type": "io.cozy.files",
                    "id": id.as_str(),
                    "attributes": {
                        "move_to_trash": true
                    }
                }
            }))
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    pub async fn move_node(
        &self,
        id: &NodeId,
        new_parent_id: &NodeId,
        new_name: &str,
    ) -> Result<Node> {
        let url = format!("{}/files/{}", self.instance_url, id.as_str());

        let resp: serde_json::Value = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&serde_json::json!({
                "data": {
                    "type": "io.cozy.files",
                    "id": id.as_str(),
                    "attributes": {
                        "name": new_name,
                        "dir_id": new_parent_id.as_str()
                    }
                }
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        parse_file_response(&resp)
    }
}

fn parse_timestamp(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

fn parse_file_response(json: &serde_json::Value) -> Result<Node> {
    let data = &json["data"];
    let attrs = &data["attributes"];

    let node_type = if attrs["type"].as_str() == Some("directory") {
        NodeType::Directory
    } else {
        NodeType::File
    };

    Ok(Node {
        id: NodeId::new(data["id"].as_str().unwrap_or("")),
        parent_id: attrs["dir_id"].as_str().map(NodeId::new),
        name: attrs["name"].as_str().unwrap_or("").to_string(),
        node_type,
        md5sum: attrs["md5sum"].as_str().map(String::from),
        size: attrs["size"].as_u64(),
        updated_at: parse_timestamp(attrs["updated_at"].as_str().unwrap_or("")),
        rev: data["meta"]["rev"].as_str().map(String::from),
    })
}
```

**Step 4: Add dependencies**

Add to Cargo.toml:
```toml
chrono = { version = "0.4", features = ["serde"] }
base64 = "0.22"
bytes = "1"
```

**Step 5: Run test**

Run: `cargo test test_fetch_changes`
Expected: PASS

**Step 6: Commit**

```bash
git add Cargo.toml src/remote/client.rs tests/remote_tests.rs
git commit -m "feat: implement Cozy files API client"
```

---

## Phase 3: Local Watcher (inotify)

### Task 6: Implement Local Scanner

**Files:**
- Create: `src/local.rs`
- Create: `src/local/scanner.rs`
- Create: `tests/local_tests.rs`

**Step 1: Write failing test**

```rust
// tests/local_tests.rs
use cozy_desktop::local::scanner::Scanner;
use cozy_desktop::model::NodeType;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_scan_directory() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Create test structure
    fs::create_dir(root.join("subdir")).unwrap();
    fs::write(root.join("file.txt"), b"hello").unwrap();
    fs::write(root.join("subdir/nested.txt"), b"world").unwrap();

    let scanner = Scanner::new(root);
    let nodes = scanner.scan().unwrap();

    assert_eq!(nodes.len(), 3);

    let file = nodes.iter().find(|n| n.name == "file.txt").unwrap();
    assert_eq!(file.node_type, NodeType::File);
    assert_eq!(file.size, Some(5));

    let subdir = nodes.iter().find(|n| n.name == "subdir").unwrap();
    assert_eq!(subdir.node_type, NodeType::Directory);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_scan_directory`
Expected: FAIL

**Step 3: Implement Scanner**

```rust
// src/local.rs
pub mod scanner;
pub mod watcher;
```

```rust
// src/local/scanner.rs
use crate::error::Result;
use crate::model::{Node, NodeId, NodeType};
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub struct Scanner {
    root: PathBuf,
}

impl Scanner {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn scan(&self) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        let mut inode_to_id: HashMap<u64, NodeId> = HashMap::new();
        let mut path_to_id: HashMap<PathBuf, NodeId> = HashMap::new();
        let mut id_counter = 0u64;

        self.scan_recursive(&self.root, None, &mut nodes, &mut inode_to_id, &mut path_to_id, &mut id_counter)?;
        Ok(nodes)
    }

    fn scan_recursive(
        &self,
        path: &Path,
        parent_id: Option<NodeId>,
        nodes: &mut Vec<Node>,
        inode_to_id: &mut HashMap<u64, NodeId>,
        path_to_id: &mut HashMap<PathBuf, NodeId>,
        id_counter: &mut u64,
    ) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            let metadata = entry.metadata()?;
            let inode = metadata.ino();

            // Generate or reuse ID based on inode
            let id = inode_to_id
                .entry(inode)
                .or_insert_with(|| {
                    *id_counter += 1;
                    NodeId::new(format!("local-{}", id_counter))
                })
                .clone();

            path_to_id.insert(entry_path.clone(), id.clone());

            let name = entry
                .file_name()
                .to_string_lossy()
                .to_string();

            let (node_type, md5sum, size) = if metadata.is_dir() {
                (NodeType::Directory, None, None)
            } else {
                let size = metadata.len();
                let md5sum = self.compute_md5(&entry_path)?;
                (NodeType::File, Some(md5sum), Some(size))
            };

            let node = Node {
                id: id.clone(),
                parent_id: parent_id.clone(),
                name,
                node_type,
                md5sum,
                size,
                updated_at: metadata.mtime(),
                rev: None,
            };

            nodes.push(node);

            if metadata.is_dir() {
                self.scan_recursive(&entry_path, Some(id), nodes, inode_to_id, path_to_id, id_counter)?;
            }
        }

        Ok(())
    }

    fn compute_md5(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)?;
        let mut hasher = Md5::new();
        let mut buffer = [0u8; 8192];

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        Ok(hex::encode(hasher.finalize()))
    }

    pub fn scan_file(&self, path: &Path) -> Result<Option<Node>> {
        if !path.exists() {
            return Ok(None);
        }

        let metadata = fs::metadata(path)?;
        let inode = metadata.ino();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let (node_type, md5sum, size) = if metadata.is_dir() {
            (NodeType::Directory, None, None)
        } else {
            let size = metadata.len();
            let md5sum = self.compute_md5(path)?;
            (NodeType::File, Some(md5sum), Some(size))
        };

        Ok(Some(Node {
            id: NodeId::new(format!("local-{}", inode)),
            parent_id: None, // Caller must set this
            name,
            node_type,
            md5sum,
            size,
            updated_at: metadata.mtime(),
            rev: None,
        }))
    }
}
```

**Step 4: Run test**

Run: `cargo test test_scan_directory`
Expected: PASS

**Step 5: Commit**

```bash
git add src/local.rs src/local/ tests/local_tests.rs
git commit -m "feat: implement local filesystem scanner"
```

---

### Task 7: Implement inotify Watcher

**Files:**
- Create: `src/local/watcher.rs`
- Modify: `tests/local_tests.rs`

**Step 1: Write failing test**

```rust
// Append to tests/local_tests.rs

use cozy_desktop::local::watcher::{Watcher, WatchEvent, WatchEventKind};
use std::time::Duration;
use std::thread;

#[test]
fn test_watcher_detects_file_create() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = Watcher::new(&root, tx).unwrap();

    thread::spawn(move || {
        watcher.run().unwrap();
    });

    // Give watcher time to start
    thread::sleep(Duration::from_millis(100));

    // Create a file
    fs::write(root.join("new_file.txt"), b"test").unwrap();

    // Wait for event
    let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(event.kind, WatchEventKind::Create | WatchEventKind::Modify));
}
```

**Step 2: Implement Watcher**

```rust
// src/local/watcher.rs
use crate::error::Result;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum WatchEventKind {
    Create,
    Modify,
    Delete,
    MovedFrom,
    MovedTo,
}

#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub path: PathBuf,
    pub kind: WatchEventKind,
    pub is_dir: bool,
    pub cookie: Option<u32>,
}

pub struct Watcher {
    inotify: Inotify,
    root: PathBuf,
    tx: Sender<WatchEvent>,
    watches: HashMap<WatchDescriptor, PathBuf>,
}

impl Watcher {
    pub fn new(root: &Path, tx: Sender<WatchEvent>) -> Result<Self> {
        let inotify = Inotify::init()?;
        let mut watcher = Self {
            inotify,
            root: root.to_path_buf(),
            tx,
            watches: HashMap::new(),
        };

        watcher.add_watch_recursive(root)?;
        Ok(watcher)
    }

    fn add_watch_recursive(&mut self, path: &Path) -> Result<()> {
        let mask = WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::MODIFY
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO
            | WatchMask::CLOSE_WRITE;

        let wd = self.inotify.watches().add(path, mask)?;
        self.watches.insert(wd, path.to_path_buf());

        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    self.add_watch_recursive(&entry.path())?;
                }
            }
        }

        Ok(())
    }

    pub fn run(&mut self) -> Result<()> {
        let mut buffer = [0u8; 4096];

        loop {
            let events = self.inotify.read_events_blocking(&mut buffer)?;

            for event in events {
                let base_path = self
                    .watches
                    .get(&event.wd)
                    .cloned()
                    .unwrap_or_else(|| self.root.clone());

                let path = if let Some(name) = event.name {
                    base_path.join(name)
                } else {
                    base_path
                };

                let is_dir = event.mask.contains(EventMask::ISDIR);

                let kind = if event.mask.contains(EventMask::CREATE) {
                    // Add watch for new directories
                    if is_dir {
                        let _ = self.add_watch_recursive(&path);
                    }
                    WatchEventKind::Create
                } else if event.mask.contains(EventMask::MODIFY)
                    || event.mask.contains(EventMask::CLOSE_WRITE)
                {
                    WatchEventKind::Modify
                } else if event.mask.contains(EventMask::DELETE) {
                    WatchEventKind::Delete
                } else if event.mask.contains(EventMask::MOVED_FROM) {
                    WatchEventKind::MovedFrom
                } else if event.mask.contains(EventMask::MOVED_TO) {
                    if is_dir {
                        let _ = self.add_watch_recursive(&path);
                    }
                    WatchEventKind::MovedTo
                } else {
                    continue;
                };

                let watch_event = WatchEvent {
                    path,
                    kind,
                    is_dir,
                    cookie: event.cookie.into(),
                };

                if self.tx.send(watch_event).is_err() {
                    return Ok(()); // Receiver dropped, exit gracefully
                }
            }
        }
    }
}
```

**Step 3: Run test**

Run: `cargo test test_watcher_detects_file_create -- --ignored`
Note: This test may be flaky in CI; mark as `#[ignore]` for now.

**Step 4: Commit**

```bash
git add src/local/watcher.rs tests/local_tests.rs
git commit -m "feat: implement inotify-based file watcher"
```

---

## Phase 4: Sync Planner

### Task 8: Implement Sync Planner

**Files:**
- Create: `src/planner.rs`
- Create: `tests/planner_tests.rs`

**Step 1: Write failing tests for planner**

```rust
// tests/planner_tests.rs
use cozy_desktop::model::{Node, NodeId, NodeType, SyncOp};
use cozy_desktop::planner::Planner;
use cozy_desktop::store::TreeStore;
use tempfile::tempdir;

fn make_file(id: &str, name: &str, parent: Option<&str>, md5: &str) -> Node {
    Node {
        id: NodeId::new(id),
        parent_id: parent.map(NodeId::new),
        name: name.to_string(),
        node_type: NodeType::File,
        md5sum: Some(md5.to_string()),
        size: Some(100),
        updated_at: 1000,
        rev: None,
    }
}

fn make_dir(id: &str, name: &str, parent: Option<&str>) -> Node {
    Node {
        id: NodeId::new(id),
        parent_id: parent.map(NodeId::new),
        name: name.to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 1000,
        rev: None,
    }
}

#[test]
fn test_new_remote_file_generates_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // File exists in remote, not in local or synced
    let file = make_file("f1", "doc.txt", Some("root"), "abc123");
    store.insert_remote(&file).unwrap();

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], SyncOp::Download { node_id } if node_id.as_str() == "f1"));
}

#[test]
fn test_new_local_file_generates_upload() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // File exists in local, not in remote or synced
    let file = make_file("f1", "doc.txt", Some("root"), "abc123");
    store.insert_local(&file).unwrap();

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], SyncOp::Upload { node_id } if node_id.as_str() == "f1"));
}

#[test]
fn test_synced_file_no_ops() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // Same file in all three trees
    let file = make_file("f1", "doc.txt", Some("root"), "abc123");
    store.insert_remote(&file).unwrap();
    store.insert_local(&file).unwrap();
    store.insert_synced(&file).unwrap();

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert!(ops.is_empty());
}

#[test]
fn test_remote_modified_generates_download() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // File in synced and local with old hash
    let old_file = make_file("f1", "doc.txt", Some("root"), "old_hash");
    store.insert_local(&old_file).unwrap();
    store.insert_synced(&old_file).unwrap();

    // Remote has new hash
    let new_file = make_file("f1", "doc.txt", Some("root"), "new_hash");
    store.insert_remote(&new_file).unwrap();

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], SyncOp::Download { .. }));
}

#[test]
fn test_local_modified_generates_upload() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // File in synced and remote with old hash
    let old_file = make_file("f1", "doc.txt", Some("root"), "old_hash");
    store.insert_remote(&old_file).unwrap();
    store.insert_synced(&old_file).unwrap();

    // Local has new hash
    let new_file = make_file("f1", "doc.txt", Some("root"), "new_hash");
    store.insert_local(&new_file).unwrap();

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], SyncOp::Upload { .. }));
}

#[test]
fn test_both_modified_generates_conflict() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // Synced has original
    let synced_file = make_file("f1", "doc.txt", Some("root"), "original");
    store.insert_synced(&synced_file).unwrap();

    // Remote modified
    let remote_file = make_file("f1", "doc.txt", Some("root"), "remote_change");
    store.insert_remote(&remote_file).unwrap();

    // Local modified differently
    let local_file = make_file("f1", "doc.txt", Some("root"), "local_change");
    store.insert_local(&local_file).unwrap();

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], SyncOp::Conflict { .. }));
}

#[test]
fn test_remote_deleted_generates_local_delete() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();

    // File was synced
    let file = make_file("f1", "doc.txt", Some("root"), "hash");
    store.insert_synced(&file).unwrap();
    store.insert_local(&file).unwrap();
    // Not in remote (deleted)

    let planner = Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], SyncOp::DeleteLocal { .. }));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test planner`
Expected: FAIL

**Step 3: Implement Planner**

```rust
// src/planner.rs
use crate::error::Result;
use crate::model::{Node, NodeId, SyncOp, TreeType};
use crate::store::TreeStore;
use std::collections::HashSet;

pub struct Planner<'a> {
    store: &'a TreeStore,
}

impl<'a> Planner<'a> {
    pub fn new(store: &'a TreeStore) -> Self {
        Self { store }
    }

    pub fn plan(&self) -> Result<Vec<SyncOp>> {
        let mut ops = Vec::new();
        let mut processed = HashSet::new();

        // Collect all node IDs from all trees
        let remote_nodes = self.store.list_all(TreeType::Remote)?;
        let local_nodes = self.store.list_all(TreeType::Local)?;
        let synced_nodes = self.store.list_all(TreeType::Synced)?;

        let all_ids: HashSet<NodeId> = remote_nodes
            .iter()
            .chain(local_nodes.iter())
            .chain(synced_nodes.iter())
            .map(|n| n.id.clone())
            .collect();

        for id in all_ids {
            if processed.contains(&id) {
                continue;
            }
            processed.insert(id.clone());

            let remote = self.store.get_remote(&id)?;
            let local = self.store.get_local(&id)?;
            let synced = self.store.get_synced(&id)?;

            if let Some(op) = self.plan_node(&id, remote.as_ref(), local.as_ref(), synced.as_ref()) {
                ops.push(op);
            }
        }

        // Sort ops: directories before files, creates before moves, deletes last
        ops.sort_by_key(|op| match op {
            SyncOp::CreateLocalDir { .. } | SyncOp::CreateRemoteDir { .. } => 0,
            SyncOp::Download { .. } | SyncOp::Upload { .. } => 1,
            SyncOp::MoveLocal { .. } | SyncOp::MoveRemote { .. } => 2,
            SyncOp::DeleteLocal { .. } | SyncOp::DeleteRemote { .. } => 3,
            SyncOp::Conflict { .. } => 4,
        });

        Ok(ops)
    }

    fn plan_node(
        &self,
        id: &NodeId,
        remote: Option<&Node>,
        local: Option<&Node>,
        synced: Option<&Node>,
    ) -> Option<SyncOp> {
        match (remote, local, synced) {
            // All three match: fully synced
            (Some(r), Some(l), Some(s)) if self.nodes_equal(r, l) && self.nodes_equal(l, s) => None,

            // Remote and local match but synced differs or missing: update synced (no op needed)
            (Some(r), Some(l), _) if self.nodes_equal(r, l) => None,

            // Only in remote: download
            (Some(_r), None, None) => Some(SyncOp::Download { node_id: id.clone() }),

            // Only in local: upload
            (None, Some(_l), None) => Some(SyncOp::Upload { node_id: id.clone() }),

            // In remote and synced, not local: was deleted locally
            (Some(_r), None, Some(_s)) => Some(SyncOp::DeleteRemote { node_id: id.clone() }),

            // In local and synced, not remote: was deleted remotely
            (None, Some(_l), Some(_s)) => Some(SyncOp::DeleteLocal { node_id: id.clone() }),

            // Only in synced: was deleted on both sides, nothing to do
            (None, None, Some(_s)) => None,

            // In remote and local, not synced: both created (conflict)
            (Some(r), Some(l), None) if !self.nodes_equal(r, l) => Some(SyncOp::Conflict {
                node_id: id.clone(),
                reason: "Created on both sides with different content".to_string(),
            }),

            // Remote modified (local matches synced)
            (Some(r), Some(l), Some(s)) if self.nodes_equal(l, s) && !self.nodes_equal(r, s) => {
                Some(SyncOp::Download { node_id: id.clone() })
            }

            // Local modified (remote matches synced)
            (Some(r), Some(l), Some(s)) if self.nodes_equal(r, s) && !self.nodes_equal(l, s) => {
                Some(SyncOp::Upload { node_id: id.clone() })
            }

            // Both modified differently: conflict
            (Some(r), Some(l), Some(s))
                if !self.nodes_equal(r, s) && !self.nodes_equal(l, s) && !self.nodes_equal(r, l) =>
            {
                Some(SyncOp::Conflict {
                    node_id: id.clone(),
                    reason: "Modified on both sides".to_string(),
                })
            }

            // Fallback
            _ => None,
        }
    }

    fn nodes_equal(&self, a: &Node, b: &Node) -> bool {
        // For files, compare md5sum
        // For directories, compare name and parent
        if a.node_type != b.node_type {
            return false;
        }

        if a.name != b.name || a.parent_id != b.parent_id {
            return false;
        }

        if a.is_file() {
            a.md5sum == b.md5sum
        } else {
            true
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test planner`
Expected: All tests pass

**Step 5: Commit**

```bash
git add src/planner.rs tests/planner_tests.rs
git commit -m "feat: implement sync planner with 3-tree comparison"
```

---

## Phase 5: Sync Engine

### Task 9: Implement Sync Engine

**Files:**
- Create: `src/sync.rs`
- Create: `src/sync/engine.rs`
- Create: `tests/sync_tests.rs`

**Step 1: Write failing test**

```rust
// tests/sync_tests.rs
use cozy_desktop::model::{Node, NodeId, NodeType};
use cozy_desktop::store::TreeStore;
use cozy_desktop::sync::engine::SyncEngine;
use tempfile::tempdir;
use std::fs;

// Integration test using mock I/O (will implement with simulator later)
#[test]
fn test_sync_engine_downloads_new_remote_file() {
    let store_dir = tempdir().unwrap();
    let sync_dir = tempdir().unwrap();
    let staging_dir = tempdir().unwrap();

    let store = TreeStore::open(store_dir.path()).unwrap();

    // Add a file to remote tree
    let remote_file = Node {
        id: NodeId::new("remote-file-1"),
        parent_id: Some(NodeId::new("io.cozy.files.root-dir")),
        name: "document.txt".to_string(),
        node_type: NodeType::File,
        md5sum: Some("098f6bcd4621d373cade4e832627b4f6".to_string()), // md5("test")
        size: Some(4),
        updated_at: 1000,
        rev: Some("1-abc".to_string()),
    };
    store.insert_remote(&remote_file).unwrap();

    // Add root to local and synced
    let root = Node {
        id: NodeId::new("io.cozy.files.root-dir"),
        parent_id: None,
        name: "".to_string(),
        node_type: NodeType::Directory,
        md5sum: None,
        size: None,
        updated_at: 0,
        rev: None,
    };
    store.insert_local(&root).unwrap();
    store.insert_synced(&root).unwrap();
    store.insert_remote(&root).unwrap();

    // The engine would download the file - for now just verify planning works
    let planner = cozy_desktop::planner::Planner::new(&store);
    let ops = planner.plan().unwrap();

    assert!(!ops.is_empty());
}
```

**Step 2: Implement SyncEngine structure**

```rust
// src/sync.rs
pub mod engine;
```

```rust
// src/sync/engine.rs
use crate::error::{Error, Result};
use crate::local::scanner::Scanner;
use crate::model::{Node, NodeId, SyncOp, TreeType};
use crate::planner::Planner;
use crate::remote::client::CozyClient;
use crate::store::TreeStore;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct SyncEngine {
    store: TreeStore,
    sync_dir: PathBuf,
    staging_dir: PathBuf,
    client: CozyClient,
    // Maps local IDs to remote IDs and vice versa
    local_to_remote: HashMap<NodeId, NodeId>,
    remote_to_local: HashMap<NodeId, NodeId>,
}

impl SyncEngine {
    pub fn new(
        store: TreeStore,
        sync_dir: PathBuf,
        staging_dir: PathBuf,
        client: CozyClient,
    ) -> Self {
        Self {
            store,
            sync_dir,
            staging_dir,
            client,
            local_to_remote: HashMap::new(),
            remote_to_local: HashMap::new(),
        }
    }

    pub async fn initial_scan(&mut self) -> Result<()> {
        let scanner = Scanner::new(&self.sync_dir);
        let local_nodes = scanner.scan()?;

        for node in local_nodes {
            self.store.insert_local(&node)?;
        }

        self.store.flush()?;
        Ok(())
    }

    pub async fn fetch_remote_changes(&mut self, since: Option<&str>) -> Result<String> {
        let changes = self.client.fetch_changes(since).await?;

        for change in &changes.results {
            if change.deleted {
                self.store.delete(TreeType::Remote, &change.node.id)?;
            } else {
                self.store.insert_remote(&change.node)?;
            }
        }

        self.store.flush()?;
        Ok(changes.last_seq)
    }

    pub fn plan(&self) -> Result<Vec<SyncOp>> {
        let planner = Planner::new(&self.store);
        planner.plan()
    }

    pub async fn execute(&mut self, ops: Vec<SyncOp>) -> Result<()> {
        for op in ops {
            match &op {
                SyncOp::Download { node_id } => {
                    self.execute_download(node_id).await?;
                }
                SyncOp::Upload { node_id } => {
                    self.execute_upload(node_id).await?;
                }
                SyncOp::CreateLocalDir { node_id } => {
                    self.execute_create_local_dir(node_id)?;
                }
                SyncOp::CreateRemoteDir { node_id } => {
                    self.execute_create_remote_dir(node_id).await?;
                }
                SyncOp::DeleteLocal { node_id } => {
                    self.execute_delete_local(node_id)?;
                }
                SyncOp::DeleteRemote { node_id } => {
                    self.execute_delete_remote(node_id).await?;
                }
                SyncOp::MoveLocal { node_id, new_parent_id, new_name } => {
                    self.execute_move_local(node_id, new_parent_id, new_name)?;
                }
                SyncOp::MoveRemote { node_id, new_parent_id, new_name } => {
                    self.execute_move_remote(node_id, new_parent_id, new_name).await?;
                }
                SyncOp::Conflict { node_id, reason } => {
                    tracing::warn!("Conflict for {}: {}", node_id.as_str(), reason);
                    // For PoC: skip conflicts
                }
            }
        }

        Ok(())
    }

    async fn execute_download(&mut self, node_id: &NodeId) -> Result<()> {
        let remote_node = self
            .store
            .get_remote(node_id)?
            .ok_or_else(|| Error::NotFound(node_id.as_str().to_string()))?;

        let content = self.client.download_file(node_id).await?;

        // Write to staging first
        let staging_path = self.staging_dir.join(&remote_node.name);
        fs::write(&staging_path, &content)?;

        // Move to final location
        let final_path = self.resolve_path(&remote_node)?;
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&staging_path, &final_path)?;

        // Update local and synced trees
        let mut local_node = remote_node.clone();
        local_node.rev = None; // Local nodes don't have rev
        self.store.insert_local(&local_node)?;
        self.store.insert_synced(&remote_node)?;

        self.store.flush()?;
        Ok(())
    }

    async fn execute_upload(&mut self, node_id: &NodeId) -> Result<()> {
        let local_node = self
            .store
            .get_local(node_id)?
            .ok_or_else(|| Error::NotFound(node_id.as_str().to_string()))?;

        let local_path = self.resolve_path(&local_node)?;
        let content = fs::read(&local_path)?;

        let parent_id = local_node.parent_id.as_ref()
            .ok_or_else(|| Error::NotFound("parent".to_string()))?;

        // Find the remote parent ID
        let remote_parent_id = self.local_to_remote.get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone());

        let md5sum = local_node.md5sum.as_deref().unwrap_or("");

        let remote_node = self
            .client
            .upload_file(&remote_parent_id, &local_node.name, content, md5sum)
            .await?;

        // Link IDs
        self.local_to_remote.insert(node_id.clone(), remote_node.id.clone());
        self.remote_to_local.insert(remote_node.id.clone(), node_id.clone());

        // Update remote and synced trees
        self.store.insert_remote(&remote_node)?;
        self.store.insert_synced(&local_node)?;

        self.store.flush()?;
        Ok(())
    }

    fn execute_create_local_dir(&mut self, node_id: &NodeId) -> Result<()> {
        let remote_node = self
            .store
            .get_remote(node_id)?
            .ok_or_else(|| Error::NotFound(node_id.as_str().to_string()))?;

        let path = self.resolve_path(&remote_node)?;
        fs::create_dir_all(&path)?;

        let mut local_node = remote_node.clone();
        local_node.rev = None;
        self.store.insert_local(&local_node)?;
        self.store.insert_synced(&remote_node)?;

        self.store.flush()?;
        Ok(())
    }

    async fn execute_create_remote_dir(&mut self, node_id: &NodeId) -> Result<()> {
        let local_node = self
            .store
            .get_local(node_id)?
            .ok_or_else(|| Error::NotFound(node_id.as_str().to_string()))?;

        let parent_id = local_node.parent_id.as_ref()
            .ok_or_else(|| Error::NotFound("parent".to_string()))?;

        let remote_parent_id = self.local_to_remote.get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone());

        let remote_node = self
            .client
            .create_directory(&remote_parent_id, &local_node.name)
            .await?;

        self.local_to_remote.insert(node_id.clone(), remote_node.id.clone());
        self.remote_to_local.insert(remote_node.id.clone(), node_id.clone());

        self.store.insert_remote(&remote_node)?;
        self.store.insert_synced(&local_node)?;

        self.store.flush()?;
        Ok(())
    }

    fn execute_delete_local(&mut self, node_id: &NodeId) -> Result<()> {
        let local_node = self
            .store
            .get_local(node_id)?
            .ok_or_else(|| Error::NotFound(node_id.as_str().to_string()))?;

        let path = self.resolve_path(&local_node)?;

        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }

        self.store.delete(TreeType::Local, node_id)?;
        self.store.delete(TreeType::Synced, node_id)?;

        self.store.flush()?;
        Ok(())
    }

    async fn execute_delete_remote(&mut self, node_id: &NodeId) -> Result<()> {
        self.client.trash(node_id).await?;

        self.store.delete(TreeType::Remote, node_id)?;
        self.store.delete(TreeType::Synced, node_id)?;

        self.store.flush()?;
        Ok(())
    }

    fn execute_move_local(
        &mut self,
        node_id: &NodeId,
        new_parent_id: &NodeId,
        new_name: &str,
    ) -> Result<()> {
        let mut node = self
            .store
            .get_local(node_id)?
            .ok_or_else(|| Error::NotFound(node_id.as_str().to_string()))?;

        let old_path = self.resolve_path(&node)?;

        node.parent_id = Some(new_parent_id.clone());
        node.name = new_name.to_string();

        let new_path = self.resolve_path(&node)?;

        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&old_path, &new_path)?;

        self.store.insert_local(&node)?;

        // Update synced to match remote
        if let Some(remote) = self.store.get_remote(node_id)? {
            self.store.insert_synced(&remote)?;
        }

        self.store.flush()?;
        Ok(())
    }

    async fn execute_move_remote(
        &mut self,
        node_id: &NodeId,
        new_parent_id: &NodeId,
        new_name: &str,
    ) -> Result<()> {
        let remote_node = self
            .client
            .move_node(node_id, new_parent_id, new_name)
            .await?;

        self.store.insert_remote(&remote_node)?;

        // Update synced to match local
        if let Some(local) = self.store.get_local(node_id)? {
            self.store.insert_synced(&local)?;
        }

        self.store.flush()?;
        Ok(())
    }

    fn resolve_path(&self, node: &Node) -> Result<PathBuf> {
        let mut path_components = Vec::new();
        let mut current = Some(node.clone());

        while let Some(n) = current {
            if !n.name.is_empty() {
                path_components.push(n.name.clone());
            }
            current = match &n.parent_id {
                Some(parent_id) => self.store.get_local(parent_id)?,
                None => None,
            };
        }

        path_components.reverse();
        let mut path = self.sync_dir.clone();
        for component in path_components {
            path.push(component);
        }

        Ok(path)
    }
}
```

**Step 3: Run test**

Run: `cargo test sync`
Expected: PASS

**Step 4: Commit**

```bash
git add src/sync.rs src/sync/ tests/sync_tests.rs
git commit -m "feat: implement sync engine with operation execution"
```

---

## Phase 6: Simulation Testing

### Task 10: Implement Simulator Framework

**Files:**
- Create: `src/simulator.rs`
- Create: `src/simulator/mock_fs.rs`
- Create: `src/simulator/mock_remote.rs`
- Create: `src/simulator/runner.rs`
- Create: `tests/simulator_tests.rs`

**Step 1: Create mock filesystem**

```rust
// src/simulator.rs
#[cfg(test)]
pub mod mock_fs;
#[cfg(test)]
pub mod mock_remote;
#[cfg(test)]
pub mod runner;
```

```rust
// src/simulator/mock_fs.rs
use crate::model::{Node, NodeId, NodeType};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MockFile {
    pub content: Vec<u8>,
    pub md5sum: String,
}

#[derive(Debug, Clone, Default)]
pub struct MockFs {
    pub files: HashMap<NodeId, MockFile>,
    pub dirs: HashMap<NodeId, ()>,
    pub nodes: HashMap<NodeId, Node>,
}

impl MockFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_file(&mut self, id: NodeId, node: Node, content: Vec<u8>) {
        let md5sum = format!("{:x}", md5::compute(&content));
        self.files.insert(id.clone(), MockFile { content, md5sum });
        self.nodes.insert(id, node);
    }

    pub fn create_dir(&mut self, id: NodeId, node: Node) {
        self.dirs.insert(id.clone(), ());
        self.nodes.insert(id, node);
    }

    pub fn read_file(&self, id: &NodeId) -> Option<&Vec<u8>> {
        self.files.get(id).map(|f| &f.content)
    }

    pub fn delete(&mut self, id: &NodeId) {
        self.files.remove(id);
        self.dirs.remove(id);
        self.nodes.remove(id);
    }

    pub fn exists(&self, id: &NodeId) -> bool {
        self.files.contains_key(id) || self.dirs.contains_key(id)
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn list_all(&self) -> Vec<&Node> {
        self.nodes.values().collect()
    }
}
```

**Step 2: Create mock remote**

```rust
// src/simulator/mock_remote.rs
use crate::model::{Node, NodeId};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct MockRemote {
    pub nodes: HashMap<NodeId, Node>,
    pub file_contents: HashMap<NodeId, Vec<u8>>,
    pub seq: u64,
    pub changes: Vec<(u64, NodeId, bool)>, // (seq, id, deleted)
}

impl MockRemote {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: Node, content: Option<Vec<u8>>) {
        self.seq += 1;
        self.changes.push((self.seq, node.id.clone(), false));
        if let Some(c) = content {
            self.file_contents.insert(node.id.clone(), c);
        }
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn delete_node(&mut self, id: &NodeId) {
        self.seq += 1;
        self.changes.push((self.seq, id.clone(), true));
        self.nodes.remove(id);
        self.file_contents.remove(id);
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn get_content(&self, id: &NodeId) -> Option<&Vec<u8>> {
        self.file_contents.get(id)
    }

    pub fn get_changes_since(&self, since_seq: u64) -> Vec<(u64, &Node, bool)> {
        self.changes
            .iter()
            .filter(|(seq, _, _)| *seq > since_seq)
            .filter_map(|(seq, id, deleted)| {
                if *deleted {
                    // For deleted, return a dummy node
                    None // Simplification for PoC
                } else {
                    self.nodes.get(id).map(|n| (*seq, n, *deleted))
                }
            })
            .collect()
    }

    pub fn current_seq(&self) -> u64 {
        self.seq
    }
}
```

**Step 3: Create simulation runner**

```rust
// src/simulator/runner.rs
use crate::model::{Node, NodeId, NodeType, SyncOp, TreeType};
use crate::planner::Planner;
use crate::store::TreeStore;
use super::mock_fs::MockFs;
use super::mock_remote::MockRemote;
use std::collections::HashSet;

pub struct SimulationRunner {
    pub local_fs: MockFs,
    pub remote: MockRemote,
    pub store: TreeStore,
    pub last_seq: u64,
}

#[derive(Debug, Clone)]
pub enum SimAction {
    LocalCreateFile { id: NodeId, parent_id: NodeId, name: String, content: Vec<u8> },
    LocalDeleteFile { id: NodeId },
    LocalModifyFile { id: NodeId, content: Vec<u8> },
    RemoteCreateFile { id: NodeId, parent_id: NodeId, name: String, content: Vec<u8> },
    RemoteDeleteFile { id: NodeId },
    RemoteModifyFile { id: NodeId, content: Vec<u8> },
    Sync,
}

impl SimulationRunner {
    pub fn new(store: TreeStore) -> Self {
        Self {
            local_fs: MockFs::new(),
            remote: MockRemote::new(),
            store,
            last_seq: 0,
        }
    }

    pub fn apply(&mut self, action: SimAction) -> Result<(), String> {
        match action {
            SimAction::LocalCreateFile { id, parent_id, name, content } => {
                let md5sum = format!("{:x}", md5::compute(&content));
                let node = Node {
                    id: id.clone(),
                    parent_id: Some(parent_id),
                    name,
                    node_type: NodeType::File,
                    md5sum: Some(md5sum),
                    size: Some(content.len() as u64),
                    updated_at: 1000,
                    rev: None,
                };
                self.local_fs.create_file(id.clone(), node.clone(), content);
                self.store.insert_local(&node).map_err(|e| e.to_string())?;
            }
            SimAction::LocalDeleteFile { id } => {
                self.local_fs.delete(&id);
                self.store.delete(TreeType::Local, &id).map_err(|e| e.to_string())?;
            }
            SimAction::LocalModifyFile { id, content } => {
                if let Some(mut node) = self.local_fs.get_node(&id).cloned() {
                    node.md5sum = Some(format!("{:x}", md5::compute(&content)));
                    node.size = Some(content.len() as u64);
                    node.updated_at += 1;
                    self.local_fs.create_file(id.clone(), node.clone(), content);
                    self.store.insert_local(&node).map_err(|e| e.to_string())?;
                }
            }
            SimAction::RemoteCreateFile { id, parent_id, name, content } => {
                let md5sum = format!("{:x}", md5::compute(&content));
                let node = Node {
                    id: id.clone(),
                    parent_id: Some(parent_id),
                    name,
                    node_type: NodeType::File,
                    md5sum: Some(md5sum),
                    size: Some(content.len() as u64),
                    updated_at: 1000,
                    rev: Some("1-abc".to_string()),
                };
                self.remote.add_node(node, Some(content));
            }
            SimAction::RemoteDeleteFile { id } => {
                self.remote.delete_node(&id);
            }
            SimAction::RemoteModifyFile { id, content } => {
                if let Some(mut node) = self.remote.get_node(&id).cloned() {
                    node.md5sum = Some(format!("{:x}", md5::compute(&content)));
                    node.size = Some(content.len() as u64);
                    node.updated_at += 1;
                    self.remote.add_node(node, Some(content));
                }
            }
            SimAction::Sync => {
                self.sync()?;
            }
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<(), String> {
        // Fetch remote changes
        for (_, node, deleted) in self.remote.get_changes_since(self.last_seq) {
            if deleted {
                self.store.delete(TreeType::Remote, &node.id).map_err(|e| e.to_string())?;
            } else {
                self.store.insert_remote(node).map_err(|e| e.to_string())?;
            }
        }
        self.last_seq = self.remote.current_seq();

        // Plan
        let planner = Planner::new(&self.store);
        let ops = planner.plan().map_err(|e| e.to_string())?;

        // Execute (simplified for simulation)
        for op in ops {
            self.execute_op(op)?;
        }

        Ok(())
    }

    fn execute_op(&mut self, op: SyncOp) -> Result<(), String> {
        match op {
            SyncOp::Download { node_id } => {
                if let Some(remote_node) = self.remote.get_node(&node_id).cloned() {
                    if let Some(content) = self.remote.get_content(&node_id).cloned() {
                        self.local_fs.create_file(node_id.clone(), remote_node.clone(), content);
                    }
                    let mut local_node = remote_node.clone();
                    local_node.rev = None;
                    self.store.insert_local(&local_node).map_err(|e| e.to_string())?;
                    self.store.insert_synced(&remote_node).map_err(|e| e.to_string())?;
                }
            }
            SyncOp::Upload { node_id } => {
                if let Some(local_node) = self.local_fs.get_node(&node_id).cloned() {
                    if let Some(content) = self.local_fs.read_file(&node_id).cloned() {
                        let mut remote_node = local_node.clone();
                        remote_node.rev = Some("1-new".to_string());
                        self.remote.add_node(remote_node.clone(), Some(content));
                        self.store.insert_remote(&remote_node).map_err(|e| e.to_string())?;
                        self.store.insert_synced(&local_node).map_err(|e| e.to_string())?;
                    }
                }
            }
            SyncOp::DeleteLocal { node_id } => {
                self.local_fs.delete(&node_id);
                self.store.delete(TreeType::Local, &node_id).map_err(|e| e.to_string())?;
                self.store.delete(TreeType::Synced, &node_id).map_err(|e| e.to_string())?;
            }
            SyncOp::DeleteRemote { node_id } => {
                self.remote.delete_node(&node_id);
                self.store.delete(TreeType::Remote, &node_id).map_err(|e| e.to_string())?;
                self.store.delete(TreeType::Synced, &node_id).map_err(|e| e.to_string())?;
            }
            _ => {} // Skip other ops for now
        }
        Ok(())
    }

    /// Check invariant: after sync, local and remote should have same files
    pub fn check_convergence(&self) -> Result<(), String> {
        let local_ids: HashSet<_> = self.local_fs.nodes.keys().collect();
        let remote_ids: HashSet<_> = self.remote.nodes.keys().collect();

        if local_ids != remote_ids {
            return Err(format!(
                "Convergence failed: local has {:?}, remote has {:?}",
                local_ids.difference(&remote_ids).collect::<Vec<_>>(),
                remote_ids.difference(&local_ids).collect::<Vec<_>>()
            ));
        }

        // Check content matches
        for id in &local_ids {
            let local_node = self.local_fs.get_node(id).unwrap();
            let remote_node = self.remote.get_node(id).unwrap();

            if local_node.md5sum != remote_node.md5sum {
                return Err(format!(
                    "Content mismatch for {}: local={:?}, remote={:?}",
                    id.as_str(),
                    local_node.md5sum,
                    remote_node.md5sum
                ));
            }
        }

        Ok(())
    }
}
```

**Step 4: Create property-based tests**

```rust
// tests/simulator_tests.rs
use cozy_desktop::model::NodeId;
use cozy_desktop::simulator::runner::{SimAction, SimulationRunner};
use cozy_desktop::store::TreeStore;
use proptest::prelude::*;
use tempfile::tempdir;

fn arbitrary_node_id() -> impl Strategy<Value = NodeId> {
    "[a-z]{4}".prop_map(|s| NodeId::new(s))
}

fn arbitrary_content() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..100)
}

fn arbitrary_name() -> impl Strategy<Value = String> {
    "[a-z]{1,8}\\.txt".prop_map(|s| s)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn test_remote_create_then_sync_converges(
        id in arbitrary_node_id(),
        name in arbitrary_name(),
        content in arbitrary_content()
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store);

        // Create root dir
        let root_id = NodeId::new("root");
        runner.apply(SimAction::RemoteCreateFile {
            id: root_id.clone(),
            parent_id: NodeId::new(""),
            name: "".to_string(),
            content: vec![],
        }).unwrap();

        // Create file on remote
        runner.apply(SimAction::RemoteCreateFile {
            id: id.clone(),
            parent_id: root_id,
            name,
            content,
        }).unwrap();

        // Sync
        runner.apply(SimAction::Sync).unwrap();

        // Check convergence
        runner.check_convergence().unwrap();
    }

    #[test]
    fn test_local_create_then_sync_converges(
        id in arbitrary_node_id(),
        name in arbitrary_name(),
        content in arbitrary_content()
    ) {
        let dir = tempdir().unwrap();
        let store = TreeStore::open(dir.path()).unwrap();
        let mut runner = SimulationRunner::new(store);

        // Create root dir on both sides
        let root_id = NodeId::new("root");

        // Create file locally
        runner.apply(SimAction::LocalCreateFile {
            id: id.clone(),
            parent_id: root_id,
            name,
            content,
        }).unwrap();

        // Sync
        runner.apply(SimAction::Sync).unwrap();

        // Check convergence
        runner.check_convergence().unwrap();
    }
}

#[test]
fn test_bidirectional_creates_converge() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store);

    let root_id = NodeId::new("root");

    // Create different files on each side
    runner.apply(SimAction::LocalCreateFile {
        id: NodeId::new("local-file"),
        parent_id: root_id.clone(),
        name: "local.txt".to_string(),
        content: b"local content".to_vec(),
    }).unwrap();

    runner.apply(SimAction::RemoteCreateFile {
        id: NodeId::new("remote-file"),
        parent_id: root_id,
        name: "remote.txt".to_string(),
        content: b"remote content".to_vec(),
    }).unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // Both files should exist on both sides
    assert!(runner.local_fs.exists(&NodeId::new("local-file")));
    assert!(runner.local_fs.exists(&NodeId::new("remote-file")));
    assert!(runner.remote.get_node(&NodeId::new("local-file")).is_some());
    assert!(runner.remote.get_node(&NodeId::new("remote-file")).is_some());
}

#[test]
fn test_delete_propagates() {
    let dir = tempdir().unwrap();
    let store = TreeStore::open(dir.path()).unwrap();
    let mut runner = SimulationRunner::new(store);

    let root_id = NodeId::new("root");
    let file_id = NodeId::new("file");

    // Create file on both sides (simulating synced state)
    runner.apply(SimAction::LocalCreateFile {
        id: file_id.clone(),
        parent_id: root_id.clone(),
        name: "test.txt".to_string(),
        content: b"content".to_vec(),
    }).unwrap();

    runner.apply(SimAction::Sync).unwrap();

    // Now delete locally
    runner.apply(SimAction::LocalDeleteFile { id: file_id.clone() }).unwrap();

    // Sync
    runner.apply(SimAction::Sync).unwrap();

    // Should be deleted on remote too
    assert!(!runner.local_fs.exists(&file_id));
    assert!(runner.remote.get_node(&file_id).is_none());
}
```

**Step 5: Run simulation tests**

Run: `cargo test simulator`
Expected: All tests pass

**Step 6: Commit**

```bash
git add src/simulator.rs src/simulator/ tests/simulator_tests.rs
git commit -m "feat: implement simulation testing framework with proptest"
```

---

## Phase 7: CLI & Integration

### Task 11: Create CLI Application

**Files:**
- Modify: `src/main.rs`
- Create: `src/config.rs`

**Step 1: Implement configuration**

```rust
// src/config.rs
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::fs;
use crate::error::Result;
use crate::remote::auth::OAuthClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub instance_url: String,
    pub sync_dir: PathBuf,
    pub data_dir: PathBuf,
    pub oauth_client: Option<OAuthClient>,
    pub last_seq: Option<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(Some(config))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.data_dir.join("staging")
    }

    pub fn store_dir(&self) -> PathBuf {
        self.data_dir.join("store")
    }
}
```

**Step 2: Implement main CLI**

```rust
// src/main.rs
use cozy_desktop::config::Config;
use cozy_desktop::error::Result;
use cozy_desktop::local::scanner::Scanner;
use cozy_desktop::local::watcher::{Watcher, WatchEvent};
use cozy_desktop::model::TreeType;
use cozy_desktop::planner::Planner;
use cozy_desktop::remote::auth::OAuthClient;
use cozy_desktop::remote::client::CozyClient;
use cozy_desktop::store::TreeStore;
use cozy_desktop::sync::engine::SyncEngine;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
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

    match args.get(1).map(|s| s.as_str()) {
        Some("init") => cmd_init(&args[2..]),
        Some("auth") => cmd_auth(&args[2..]),
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

    // Create directories
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

fn cmd_auth(args: &[String]) -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| cozy_desktop::error::Error::NotFound("Config not found. Run 'init' first.".to_string()))?;

    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let oauth = OAuthClient::register(
            &config.instance_url,
            "Cozy Desktop PoC",
            "cozy-desktop-poc",
        )
        .await?;

        let state = uuid::Uuid::new_v4().to_string();
        let auth_url = oauth.authorization_url(&state);

        println!("Open this URL in your browser to authorize:");
        println!("{}", auth_url);
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

        Ok::<_, cozy_desktop::error::Error>(())
    })?;

    Ok(())
}

fn cmd_sync() -> Result<()> {
    let mut config = Config::load(&config_path())?
        .ok_or_else(|| cozy_desktop::error::Error::NotFound("Config not found".to_string()))?;

    let oauth = config.oauth_client.as_ref()
        .ok_or_else(|| cozy_desktop::error::Error::NotFound("Not authenticated".to_string()))?;

    let access_token = oauth.access_token()
        .ok_or_else(|| cozy_desktop::error::Error::NotFound("No access token".to_string()))?;

    let store = TreeStore::open(&config.store_dir())?;
    let client = CozyClient::new(&config.instance_url, access_token);

    let mut engine = SyncEngine::new(
        store,
        config.sync_dir.clone(),
        config.staging_dir(),
        client,
    );

    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        // Initial scan
        tracing::info!("Scanning local filesystem...");
        engine.initial_scan().await?;

        // Fetch remote changes
        tracing::info!("Fetching remote changes...");
        let last_seq = engine.fetch_remote_changes(config.last_seq.as_deref()).await?;

        // Plan and execute
        let ops = engine.plan()?;
        tracing::info!("Planning {} operations", ops.len());

        for op in &ops {
            tracing::info!("  {:?}", op);
        }

        engine.execute(ops).await?;

        // Save last_seq
        config.last_seq = Some(last_seq);
        config.save(&config_path())?;

        tracing::info!("Sync complete!");

        Ok::<_, cozy_desktop::error::Error>(())
    })?;

    Ok(())
}

fn cmd_watch() -> Result<()> {
    let config = Config::load(&config_path())?
        .ok_or_else(|| cozy_desktop::error::Error::NotFound("Config not found".to_string()))?;

    let (tx, rx) = mpsc::channel::<WatchEvent>();

    let sync_dir = config.sync_dir.clone();
    thread::spawn(move || {
        let mut watcher = Watcher::new(&sync_dir, tx).unwrap();
        watcher.run().unwrap();
    });

    tracing::info!("Watching for changes in {}", config.sync_dir.display());
    tracing::info!("Press Ctrl+C to stop");

    // Debounce events and trigger sync
    let mut last_sync = std::time::Instant::now();
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
                    last_sync = std::time::Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Periodic sync every 30 seconds
                if last_sync.elapsed() > Duration::from_secs(30) {
                    tracing::info!("Periodic sync...");
                    if let Err(e) = cmd_sync() {
                        tracing::error!("Sync failed: {}", e);
                    }
                    last_sync = std::time::Instant::now();
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
        .ok_or_else(|| cozy_desktop::error::Error::NotFound("Config not found".to_string()))?;

    println!("Cozy Desktop Status");
    println!("-------------------");
    println!("Instance:   {}", config.instance_url);
    println!("Sync dir:   {}", config.sync_dir.display());
    println!("Last seq:   {}", config.last_seq.as_deref().unwrap_or("none"));
    println!("Authenticated: {}", config.oauth_client.is_some());

    if config.store_dir().exists() {
        let store = TreeStore::open(&config.store_dir())?;
        let remote = store.list_all(TreeType::Remote)?;
        let local = store.list_all(TreeType::Local)?;
        let synced = store.list_all(TreeType::Synced)?;

        println!();
        println!("Trees:");
        println!("  Remote: {} nodes", remote.len());
        println!("  Local:  {} nodes", local.len());
        println!("  Synced: {} nodes", synced.len());

        let planner = Planner::new(&store);
        let ops = planner.plan()?;
        println!();
        println!("Pending operations: {}", ops.len());
        for op in &ops {
            println!("  {:?}", op);
        }
    }

    Ok(())
}
```

**Step 3: Add dependencies**

Add to Cargo.toml:
```toml
dirs = "5"
uuid = { version = "1", features = ["v4"] }
```

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: Successful build

**Step 5: Commit**

```bash
git add Cargo.toml src/main.rs src/config.rs
git commit -m "feat: implement CLI application"
```

---

## Summary

This plan creates a working proof-of-concept with:

1. **3-tree data model** (Remote/Local/Synced) stored in fjall
2. **Remote API client** for Cozy files with OAuth2
3. **Local filesystem watcher** using inotify
4. **Sync planner** that derives operations from tree comparisons
5. **Sync engine** that executes operations
6. **Simulation testing** with property-based tests
7. **CLI application** for init/auth/sync/watch

### Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLI (main.rs)                           │
├─────────────────────────────────────────────────────────────────┤
│                      SyncEngine (sync/)                         │
│  ┌─────────┐    ┌─────────┐    ┌─────────────────────────────┐  │
│  │ Scanner │    │ Planner │    │        Executor             │  │
│  └────┬────┘    └────┬────┘    └──────────────┬──────────────┘  │
│       │              │                        │                 │
├───────┴──────────────┴────────────────────────┴─────────────────┤
│                      TreeStore (fjall)                          │
│   ┌──────────┐    ┌──────────┐    ┌──────────┐                  │
│   │  Remote  │    │  Local   │    │  Synced  │                  │
│   └──────────┘    └──────────┘    └──────────┘                  │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────────┐    ┌─────────────────────┐             │
│  │  CozyClient (HTTP)  │    │  Watcher (inotify)  │             │
│  └─────────────────────┘    └─────────────────────┘             │
└─────────────────────────────────────────────────────────────────┘
```

### Next Steps (after PoC)

- Handle conflicts with user prompts
- Implement move detection
- Add xattr support for offline moves
- WebSocket realtime for instant remote sync
- Selective sync (ignore patterns)
- Retry with exponential backoff
