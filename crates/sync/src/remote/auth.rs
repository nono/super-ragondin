use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fmt;

const REDIRECT_URI: &str = "http://localhost:8080/callback";

#[derive(Clone, Serialize)]
struct RegisterRequest {
    redirect_uris: Vec<String>,
    client_name: String,
    software_id: String,
    client_kind: String,
    client_uri: String,
}

#[derive(Clone, Deserialize)]
pub struct RegisterResponse {
    pub client_id: String,
    pub client_secret: String,
    pub registration_access_token: String,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    grant_type: &'a str,
    code: &'a str,
    client_id: &'a str,
    client_secret: &'a str,
    redirect_uri: &'a str,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    grant_type: &'a str,
    refresh_token: &'a str,
    client_id: &'a str,
    client_secret: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthClient {
    pub instance_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub registration_access_token: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

impl fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthClient")
            .field("instance_url", &self.instance_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("registration_access_token", &"[REDACTED]")
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl OAuthClient {
    /// Register a new OAuth client with the Cozy instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn register(
        instance_url: &str,
        client_name: &str,
        software_id: &str,
    ) -> Result<Self> {
        let http = reqwest::Client::new();
        let normalized_url = instance_url.trim_end_matches('/');
        tracing::info!(
            instance_url = normalized_url,
            client_name,
            "🔑 Registering OAuth client"
        );

        let request = RegisterRequest {
            redirect_uris: vec![REDIRECT_URI.to_string()],
            client_name: client_name.to_string(),
            software_id: software_id.to_string(),
            client_kind: "desktop".to_string(),
            client_uri: "https://github.com/nono/super-ragondin".to_string(),
        };

        let resp: RegisterResponse = http
            .post(format!("{normalized_url}/auth/register"))
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::info!(client_id = &resp.client_id, "🔑 OAuth client registered");
        Ok(Self {
            instance_url: normalized_url.to_string(),
            client_id: resp.client_id,
            client_secret: resp.client_secret,
            registration_access_token: resp.registration_access_token,
            access_token: None,
            refresh_token: None,
        })
    }

    #[must_use]
    pub fn authorization_url(&self, state: &str) -> String {
        format!(
            "{}/auth/authorize?client_id={}&redirect_uri={}&state={}&response_type=code&scope={}",
            self.instance_url,
            urlencoding::encode(&self.client_id),
            urlencoding::encode(REDIRECT_URI),
            urlencoding::encode(state),
            urlencoding::encode("io.cozy.files")
        )
    }

    /// Exchange an authorization code for access and refresh tokens.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the server returns an error.
    pub async fn exchange_code(&mut self, code: &str) -> Result<()> {
        tracing::info!("🔑 Exchanging authorization code for tokens");
        let http = reqwest::Client::new();

        let resp: TokenResponse = http
            .post(format!("{}/auth/access_token", self.instance_url))
            .form(&TokenRequest {
                grant_type: "authorization_code",
                code,
                client_id: &self.client_id,
                client_secret: &self.client_secret,
                redirect_uri: REDIRECT_URI,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.access_token = Some(resp.access_token);
        self.refresh_token = Some(resp.refresh_token);
        tracing::info!("🔑 Token exchange successful");
        Ok(())
    }

    /// Refresh the access token using the stored refresh token.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no refresh token, or if the HTTP request fails.
    pub async fn refresh(&mut self) -> Result<()> {
        let refresh_token = self
            .refresh_token
            .as_deref()
            .ok_or_else(|| crate::error::Error::NotFound("No refresh token".to_string()))?;

        tracing::info!("🔑 Refreshing access token");
        let http = reqwest::Client::new();

        let resp: TokenResponse = http
            .post(format!("{}/auth/access_token", self.instance_url))
            .form(&RefreshRequest {
                grant_type: "refresh_token",
                refresh_token,
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
        tracing::info!("🔑 Token refresh successful");
        Ok(())
    }

    #[must_use]
    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_client_roundtrip_preserves_secrets() {
        let client = OAuthClient {
            instance_url: "https://test.mycozy.cloud".to_string(),
            client_id: "client-123".to_string(),
            client_secret: "secret-456".to_string(),
            registration_access_token: "reg-token-789".to_string(),
            access_token: Some("access-abc".to_string()),
            refresh_token: Some("refresh-def".to_string()),
        };

        let json = serde_json::to_string(&client).unwrap();
        let restored: OAuthClient = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.client_secret, "secret-456");
        assert_eq!(restored.registration_access_token, "reg-token-789");
        assert_eq!(restored.access_token, Some("access-abc".to_string()));
        assert_eq!(restored.refresh_token, Some("refresh-def".to_string()));
    }
}
