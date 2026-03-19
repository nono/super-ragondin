use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Remote Cozy document ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteId(pub String);

impl RemoteId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RemoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Type of filesystem node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeType {
    File,
    Directory,
}

/// A node in the remote Cozy tree, keyed by `RemoteId`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteNode {
    pub id: RemoteId,
    pub parent_id: Option<RemoteId>,
    pub name: String,
    pub node_type: NodeType,
    pub md5sum: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_u64")]
    pub size: Option<u64>,
    pub updated_at: i64,
    /// `CouchDB` revision
    pub rev: String,
}

/// Content type of a mail part
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MailContentType {
    #[serde(rename = "text/plain")]
    TextPlain,
    #[serde(rename = "text/html")]
    TextHtml,
}

/// A part of a mail message
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailPart {
    #[serde(rename = "type")]
    pub content_type: MailContentType,
    pub body: String,
}

impl MailPart {
    #[must_use]
    pub fn plain(body: impl Into<String>) -> Self {
        Self {
            content_type: MailContentType::TextPlain,
            body: body.into(),
        }
    }

    #[must_use]
    pub fn html(body: impl Into<String>) -> Self {
        Self {
            content_type: MailContentType::TextHtml,
            body: body.into(),
        }
    }
}

/// Deserialize a JSON value that may be a string or a number as `Option<u64>`.
///
/// The Cozy API sometimes returns `size` as a JSON string instead of a number.
/// This deserializer handles both representations.
///
/// # Errors
/// Returns an error if the value is a string that cannot be parsed as `u64`.
pub fn deserialize_string_or_u64<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrU64 {
        U64(u64),
        Str(String),
    }

    let opt: Option<StringOrU64> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(StringOrU64::U64(n)) => Ok(Some(n)),
        Some(StringOrU64::Str(s)) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_id_creation() {
        let id = RemoteId::new("abc-123-def");
        assert_eq!(id.as_str(), "abc-123-def");
    }

    #[test]
    fn remote_id_display() {
        let id = RemoteId::new("abc-123");
        assert_eq!(format!("{id}"), "abc-123");
    }

    #[test]
    fn mail_part_plain() {
        let part = MailPart::plain("hello");
        assert_eq!(part.content_type, MailContentType::TextPlain);
        assert_eq!(part.body, "hello");
    }

    #[test]
    fn mail_part_html() {
        let part = MailPart::html("<p>hello</p>");
        assert_eq!(part.content_type, MailContentType::TextHtml);
        assert_eq!(part.body, "<p>hello</p>");
    }

    #[test]
    fn remote_node_serialization() {
        let node = RemoteNode {
            id: RemoteId::new("remote-123"),
            parent_id: Some(RemoteId::new("parent-456")),
            name: "doc.pdf".to_string(),
            node_type: NodeType::File,
            md5sum: Some("def456".to_string()),
            size: Some(2048),
            updated_at: 1_706_886_400,
            rev: "2-xyz".to_string(),
        };

        let json = serde_json::to_string(&node).unwrap();
        let deserialized: RemoteNode = serde_json::from_str(&json).unwrap();
        assert_eq!(node, deserialized);
    }
}
