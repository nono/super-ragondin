use base64::Engine;

use crate::error::{Error, Result};
use crate::types::{
    MailContentType, MailPart, NodeType, RemoteId, RemoteNode, deserialize_string_or_u64,
};
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
    #[serde(default)]
    name: String,
    dir_id: Option<String>,
    md5sum: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_u64")]
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
    #[serde(default, deserialize_with = "deserialize_string_or_u64")]
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
                        md5sum: normalize_md5(doc.md5sum),
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

    /// Overwrite an existing file on Cozy.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails, the server returns an error,
    /// or the MD5 hash is invalid.
    pub async fn update_file(
        &self,
        file_id: &RemoteId,
        content: Vec<u8>,
        md5sum: &str,
        expected_rev: &str,
    ) -> Result<RemoteNode> {
        tracing::info!(
            file_id = file_id.as_str(),
            size = content.len(),
            "📤 Updating file"
        );
        let url = format!("{}/files/{}", self.instance_url, file_id.as_str());

        let md5_bytes = hex::decode(md5sum).map_err(|_| Error::InvalidMd5(md5sum.to_string()))?;
        let md5_base64 = base64::engine::general_purpose::STANDARD.encode(&md5_bytes);

        let resp: FileResponse = self
            .http
            .put(&url)
            .bearer_auth(&self.access_token)
            .header("Content-MD5", md5_base64)
            .header("Content-Type", "application/octet-stream")
            .header("If-Match", expected_rev)
            .body(content)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        parse_file_response(resp)
    }

    /// Fetch the MD5 checksums of old versions of a remote file.
    ///
    /// Uses `GET /files/:file-id` which includes old versions in the
    /// `included` array. Returns a list of hex-encoded MD5 checksums.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response is invalid.
    pub async fn fetch_old_version_md5sums(&self, file_id: &RemoteId) -> Result<Vec<String>> {
        tracing::info!(file_id = file_id.as_str(), "📜 Fetching old versions");
        let url = format!("{}/files/{}", self.instance_url, file_id.as_str());
        let json: serde_json::Value = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(parse_old_version_md5sums(&json))
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

    /// Send an email to the Cozy instance owner via the sendmail worker.
    ///
    /// Uses mode `noreply`: the stack sets `from` (noreply address) and `to`
    /// (owner email) automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn send_mail(&self, subject: &str, parts: &[MailPart]) -> Result<()> {
        tracing::info!(subject, "📧 Sending mail");
        let url = format!("{}/jobs/queue/sendmail", self.instance_url);

        let json_parts: Vec<serde_json::Value> = parts
            .iter()
            .map(|p| {
                let content_type = match p.content_type {
                    MailContentType::TextPlain => "text/plain",
                    MailContentType::TextHtml => "text/html",
                };
                serde_json::json!({
                    "type": content_type,
                    "body": p.body,
                })
            })
            .collect();

        self.http
            .post(&url)
            .bearer_auth(&self.access_token)
            .header("Content-Type", "application/vnd.api+json")
            .json(&serde_json::json!({
                "data": {
                    "attributes": {
                        "arguments": {
                            "mode": "noreply",
                            "subject": subject,
                            "parts": json_parts,
                        }
                    }
                }
            }))
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
        md5sum: normalize_md5(attrs.md5sum),
        size: attrs.size,
        updated_at,
        rev: data.meta.and_then(|m| m.rev).unwrap_or_default(),
    })
}

/// Extract MD5 checksums from old versions in a file metadata response.
///
/// The `GET /files/:file-id` response includes old versions in the `included`
/// array as `io.cozy.files.versions` objects, each with an `md5sum` attribute.
/// Returns a list of hex-encoded MD5 checksums (normalized from base64).
fn parse_old_version_md5sums(json: &serde_json::Value) -> Vec<String> {
    let Some(included) = json.get("included").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    included
        .iter()
        .filter(|entry| {
            entry
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "io.cozy.files.versions")
        })
        .filter_map(|entry| {
            let md5_raw = entry
                .get("attributes")
                .and_then(|a| a.get("md5sum"))
                .and_then(|v| v.as_str())?;
            normalize_md5(Some(md5_raw.to_string()))
        })
        .collect()
}

/// Convert an MD5 value to hex if it's base64-encoded.
///
/// The Cozy API returns md5sum as base64. We normalize to hex for
/// consistent comparison with locally computed checksums.
fn normalize_md5(md5: Option<String>) -> Option<String> {
    let s = md5?;
    if s.is_empty() {
        return None;
    }
    if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(s);
    }
    base64::engine::general_purpose::STANDARD
        .decode(&s)
        .ok()
        .and_then(|decoded| {
            if decoded.is_empty() {
                None
            } else {
                Some(hex::encode(decoded))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MailPart;

    #[tokio::test]
    async fn send_mail_posts_to_sendmail_endpoint() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/jobs/queue/sendmail"))
            .and(wiremock::matchers::header(
                "Content-Type",
                "application/vnd.api+json",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": {
                        "type": "io.cozy.jobs",
                        "id": "job-123",
                        "attributes": {
                            "domain": "test.mycozy.cloud",
                            "worker": "sendmail",
                            "state": "queued"
                        }
                    }
                })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = CozyClient::new(&server.uri(), "test-token");
        let parts = [MailPart::plain("Hello, world!")];
        client.send_mail("Test Subject", &parts).await.unwrap();
    }

    #[tokio::test]
    async fn send_mail_sends_correct_body() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/jobs/queue/sendmail"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "data": {
                    "attributes": {
                        "arguments": {
                            "mode": "noreply",
                            "subject": "Hello",
                            "parts": [
                                { "type": "text/plain", "body": "Some text" }
                            ]
                        }
                    }
                }
            })))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": {
                        "type": "io.cozy.jobs",
                        "id": "job-456",
                        "attributes": { "state": "queued" }
                    }
                })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = CozyClient::new(&server.uri(), "test-token");
        let parts = [MailPart::plain("Some text")];
        client.send_mail("Hello", &parts).await.unwrap();
    }

    #[tokio::test]
    async fn send_mail_propagates_server_error() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/jobs/queue/sendmail"))
            .respond_with(wiremock::ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let client = CozyClient::new(&server.uri(), "bad-token");
        let parts = [MailPart::plain("text")];
        let result = client.send_mail("Subject", &parts).await;
        assert!(result.is_err());
    }

    #[test]
    fn normalize_md5_none() {
        assert_eq!(normalize_md5(None), None);
    }

    #[test]
    fn normalize_md5_empty_string() {
        assert_eq!(normalize_md5(Some(String::new())), None);
    }

    #[test]
    fn normalize_md5_hex_string() {
        let hex = "d41d8cd98f00b204e9800998ecf8427e".to_string();
        assert_eq!(normalize_md5(Some(hex.clone())), Some(hex));
    }

    #[test]
    fn normalize_md5_base64_string() {
        let b64 = "1B2M2Y8AsgTpgAmY7PhCfg==".to_string();
        assert_eq!(
            normalize_md5(Some(b64)),
            Some("d41d8cd98f00b204e9800998ecf8427e".to_string())
        );
    }

    #[test]
    fn normalize_md5_invalid_base64() {
        assert_eq!(normalize_md5(Some("not-valid!!!".to_string())), None);
    }

    #[test]
    fn normalize_md5_base64_decodes_to_empty() {
        assert_eq!(normalize_md5(Some(String::new())), None);
    }

    #[test]
    fn parse_old_version_md5sums_extracts_hashes() {
        let json = serde_json::json!({
            "data": {
                "type": "io.cozy.files",
                "id": "file-123",
                "meta": { "rev": "3-abc" },
                "attributes": {
                    "type": "file",
                    "name": "hello.txt",
                    "md5sum": "ODZmYjI2OWQxOTBkMmM4NQo=",
                    "size": 12,
                    "updated_at": "2016-09-19T12:38:04Z"
                },
                "relationships": {
                    "old_versions": {
                        "data": [
                            { "type": "io.cozy.files.versions", "id": "file-123/2-bbb" },
                            { "type": "io.cozy.files.versions", "id": "file-123/1-aaa" }
                        ]
                    }
                }
            },
            "included": [
                {
                    "type": "io.cozy.files.versions",
                    "id": "file-123/2-bbb",
                    "attributes": {
                        "file_id": "file-123",
                        "md5sum": "a2lth5syMW+4r7jwNhdk3A==",
                        "size": 100,
                        "updated_at": "2016-09-20T10:00:00Z"
                    }
                },
                {
                    "type": "io.cozy.files.versions",
                    "id": "file-123/1-aaa",
                    "attributes": {
                        "file_id": "file-123",
                        "md5sum": "FBA89XXOZKFhdv37iILb2Q==",
                        "size": 200,
                        "updated_at": "2016-09-18T20:38:04Z"
                    }
                }
            ]
        });

        let result = parse_old_version_md5sums(&json);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|h| h.len() == 32));
        assert!(
            result
                .iter()
                .all(|h| h.chars().all(|c| c.is_ascii_hexdigit()))
        );
    }

    #[test]
    fn parse_old_version_md5sums_empty_when_no_versions() {
        let json = serde_json::json!({
            "data": {
                "type": "io.cozy.files",
                "id": "file-456",
                "meta": { "rev": "1-abc" },
                "attributes": {
                    "type": "file",
                    "name": "new.txt",
                    "md5sum": "ODZmYjI2OWQxOTBkMmM4NQo=",
                    "size": 5,
                    "updated_at": "2016-09-19T12:38:04Z"
                }
            }
        });

        let result = parse_old_version_md5sums(&json);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_old_version_md5sums_skips_entries_without_md5() {
        let json = serde_json::json!({
            "data": {
                "type": "io.cozy.files",
                "id": "file-789",
                "meta": { "rev": "2-abc" },
                "attributes": {
                    "type": "file",
                    "name": "partial.txt",
                    "md5sum": "ODZmYjI2OWQxOTBkMmM4NQo=",
                    "size": 5,
                    "updated_at": "2016-09-19T12:38:04Z"
                }
            },
            "included": [
                {
                    "type": "io.cozy.files.versions",
                    "id": "file-789/1-aaa",
                    "attributes": {
                        "file_id": "file-789",
                        "size": 100,
                        "updated_at": "2016-09-18T20:38:04Z"
                    }
                }
            ]
        });

        let result = parse_old_version_md5sums(&json);
        assert!(result.is_empty());
    }
}
