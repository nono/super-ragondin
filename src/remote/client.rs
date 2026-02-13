use base64::Engine;

use crate::error::{Error, Result};
use crate::model::{NodeType, RemoteId, RemoteNode};
use serde::Deserialize;

pub struct CozyClient {
    instance_url: String,
    access_token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct ChangesResponse {
    pub last_seq: String,
    pub results: Vec<ChangeResult>,
}

#[derive(Debug, Clone)]
pub struct ChangeResult {
    pub id: String,
    pub seq: String,
    pub deleted: bool,
    pub node: RemoteNode,
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
}

#[derive(Debug, Deserialize)]
struct RawChangesResponse {
    last_seq: String,
    results: Vec<RawChangeResult>,
}

#[derive(Debug, Deserialize)]
struct FileResponseData {
    id: String,
    attributes: FileAttributes,
    meta: Option<FileMeta>,
}

#[derive(Debug, Deserialize)]
struct FileAttributes {
    #[serde(rename = "type")]
    node_type: Option<String>,
    name: String,
    dir_id: Option<String>,
    md5sum: Option<String>,
    size: Option<u64>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileMeta {
    rev: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileResponse {
    data: FileResponseData,
}

impl CozyClient {
    #[must_use]
    pub fn new(instance_url: &str, access_token: &str) -> Self {
        Self {
            instance_url: instance_url.trim_end_matches('/').to_string(),
            access_token: access_token.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Fetch changes from the Cozy files API.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns an error,
    /// or a change result is missing its document.
    pub async fn fetch_changes(&self, since: Option<&str>) -> Result<ChangesResponse> {
        tracing::info!(since = ?since, "🌐 Fetching remote changes");
        let mut url = reqwest::Url::parse(&format!("{}/files/_changes", self.instance_url))?;
        url.query_pairs_mut().append_pair("include_docs", "true");
        if let Some(seq) = since {
            url.query_pairs_mut().append_pair("since", seq);
        }

        let raw: RawChangesResponse = self
            .http
            .get(url)
            .bearer_auth(&self.access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut results = Vec::with_capacity(raw.results.len());
        for r in raw.results {
            let change_result = if r.deleted {
                tracing::debug!(id = &r.id, "🗑️ Remote document deleted");
                ChangeResult {
                    id: r.id.clone(),
                    seq: r.seq,
                    deleted: true,
                    node: RemoteNode {
                        id: RemoteId::new(r.id),
                        parent_id: None,
                        name: String::new(),
                        node_type: NodeType::File,
                        md5sum: None,
                        size: None,
                        updated_at: 0,
                        rev: String::new(),
                    },
                }
            } else {
                let doc = r.doc.ok_or_else(|| Error::MissingDocument(r.id.clone()))?;
                let node_type = if doc.doc_type == "directory" {
                    NodeType::Directory
                } else {
                    NodeType::File
                };
                ChangeResult {
                    id: r.id,
                    seq: r.seq,
                    deleted: false,
                    node: RemoteNode {
                        id: RemoteId::new(&doc.id),
                        parent_id: doc.dir_id.map(RemoteId::new),
                        name: doc.name,
                        node_type,
                        md5sum: doc.md5sum,
                        size: doc.size,
                        updated_at: parse_timestamp(&doc.updated_at)?,
                        rev: doc.rev,
                    },
                }
            };
            results.push(change_result);
        }

        tracing::info!(
            count = results.len(),
            last_seq = &raw.last_seq,
            "🌐 Received remote changes"
        );
        Ok(ChangesResponse {
            last_seq: raw.last_seq,
            results,
        })
    }

    /// Download a file from Cozy.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn download_file(&self, file_id: &RemoteId) -> Result<bytes::Bytes> {
        tracing::info!(file_id = file_id.as_str(), "📥 Downloading file");
        let url = format!("{}/files/download/{}", self.instance_url, file_id.as_str());
        let bytes = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        tracing::debug!(
            file_id = file_id.as_str(),
            size = bytes.len(),
            "📥 Download complete"
        );
        Ok(bytes)
    }

    /// Upload a file to Cozy.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns an error,
    /// or the MD5 hash is invalid.
    pub async fn upload_file(
        &self,
        parent_id: &RemoteId,
        name: &str,
        content: Vec<u8>,
        md5sum: &str,
    ) -> Result<RemoteNode> {
        tracing::info!(
            parent_id = parent_id.as_str(),
            name,
            size = content.len(),
            "📤 Uploading file"
        );
        let mut url = reqwest::Url::parse(&format!(
            "{}/files/{}",
            self.instance_url,
            parent_id.as_str()
        ))?;
        url.query_pairs_mut()
            .append_pair("Type", "file")
            .append_pair("Name", name);

        let md5_bytes = hex::decode(md5sum).map_err(|_| Error::InvalidMd5(md5sum.to_string()))?;
        let md5_base64 = base64::engine::general_purpose::STANDARD.encode(&md5_bytes);

        let resp: FileResponse = self
            .http
            .post(url)
            .bearer_auth(&self.access_token)
            .header("Content-MD5", md5_base64)
            .header("Content-Type", "application/octet-stream")
            .body(content)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        parse_file_response(resp)
    }

    /// Create a directory on Cozy.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn create_directory(&self, parent_id: &RemoteId, name: &str) -> Result<RemoteNode> {
        tracing::info!(
            parent_id = parent_id.as_str(),
            name,
            "📁 Creating remote directory"
        );
        let mut url = reqwest::Url::parse(&format!(
            "{}/files/{}",
            self.instance_url,
            parent_id.as_str()
        ))?;
        url.query_pairs_mut()
            .append_pair("Type", "directory")
            .append_pair("Name", name);

        let resp: FileResponse = self
            .http
            .post(url)
            .bearer_auth(&self.access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        parse_file_response(resp)
    }

    /// Trash a file or directory on Cozy.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn trash(&self, id: &RemoteId) -> Result<()> {
        tracing::info!(id = id.as_str(), "🗑️ Trashing remote document");
        let url = format!("{}/files/{}", self.instance_url, id.as_str());

        self.http
            .delete(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    /// Move or rename a file or directory on Cozy.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn move_node(
        &self,
        id: &RemoteId,
        new_parent_id: &RemoteId,
        new_name: &str,
    ) -> Result<RemoteNode> {
        tracing::info!(
            id = id.as_str(),
            new_parent_id = new_parent_id.as_str(),
            new_name,
            "🔀 Moving remote document"
        );
        let url = format!("{}/files/{}", self.instance_url, id.as_str());

        let resp: FileResponse = self
            .http
            .patch(&url)
            .bearer_auth(&self.access_token)
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

        parse_file_response(resp)
    }
}

fn parse_timestamp(s: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp())
        .map_err(|_| Error::InvalidTimestamp(s.to_string()))
}

fn parse_file_response(resp: FileResponse) -> Result<RemoteNode> {
    let data = resp.data;
    let attrs = data.attributes;

    let node_type = if attrs.node_type.as_deref() == Some("directory") {
        NodeType::Directory
    } else {
        NodeType::File
    };

    let updated_at = match &attrs.updated_at {
        Some(ts) => parse_timestamp(ts)?,
        None => 0,
    };

    Ok(RemoteNode {
        id: RemoteId::new(data.id),
        parent_id: attrs.dir_id.map(RemoteId::new),
        name: attrs.name,
        node_type,
        md5sum: attrs.md5sum,
        size: attrs.size,
        updated_at,
        rev: data.meta.and_then(|m| m.rev).unwrap_or_default(),
    })
}
