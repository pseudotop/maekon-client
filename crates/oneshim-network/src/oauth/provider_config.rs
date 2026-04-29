//! OAuth provider configurations — verified presets for each supported provider.
//!
//! Provider config includes both the auth endpoints (for token exchange) and
//! the API base URL (for making authenticated requests after login).

/// OAuth provider configuration.
#[derive(Debug, Clone)]
pub struct OAuthProviderConfig {
    pub provider_id: String,
    pub issuer: String,
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub scopes: Vec<String>,
    pub callback_port: u16,
    pub callback_path: String,
    /// API base URL for authenticated requests.
    ///
    /// OpenAI Codex uses different base URLs depending on auth mode:
    /// - ChatGPT OAuth: `https://chatgpt.com/backend-api/codex`
    /// - API key:       `https://api.openai.com/v1`
    ///
    /// Ref: `openai/codex` `codex-rs/core/src/model_provider_info.rs`
    pub api_base_url: String,
    /// Provider-specific query parameters that must be included when opening
    /// the authorization URL.
    pub authorization_extra_params: Vec<(String, String)>,
}

impl OAuthProviderConfig {
    /// OpenAI Codex preset — verified from `openai/codex` repository source.
    ///
    /// - Port: `codex-rs/login/src/server.rs:48` → `DEFAULT_PORT = 1455`
    /// - Client ID: `codex-rs/core/src/auth.rs:744`
    /// - Issuer: `https://auth.openai.com`
    /// - PKCE: S256
    /// - API base URL: `codex-rs/core/src/model_provider_info.rs` (ChatGPT auth path)
    /// - Version header: `codex-rs/core/src/model_provider_info.rs:create_openai_provider()`
    /// - Plan gating: GPT-5.4/5.3-codex restricted to paid ChatGPT subscriptions
    pub fn openai_codex() -> Self {
        Self {
            provider_id: "openai".into(),
            issuer: "https://auth.openai.com".into(),
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".into(),
            authorization_endpoint: "https://auth.openai.com/oauth/authorize".into(),
            token_endpoint: "https://auth.openai.com/oauth/token".into(),
            scopes: vec![
                "openid".into(),
                "profile".into(),
                "email".into(),
                "offline_access".into(),
            ],
            callback_port: 1455,
            callback_path: "/auth/callback".into(),
            // ChatGPT OAuth tokens use the ChatGPT backend, not the standard API.
            // Ref: model_provider_info.rs → AuthMode::Chatgpt → chatgpt.com/backend-api/codex
            api_base_url: "https://chatgpt.com/backend-api/codex".into(),
            authorization_extra_params: Vec::new(),
        }
    }

    /// Google Cloud Vision native-app OAuth preset.
    ///
    /// This requires an app-owned Desktop OAuth client ID created in Google Cloud.
    /// The Vision API itself accepts bearer tokens, but Google does not provide a
    /// shared public client ID for third-party desktop apps.
    pub fn google_cloud_vision(client_id: impl Into<String>) -> Self {
        Self {
            provider_id: "google".into(),
            issuer: "https://accounts.google.com".into(),
            client_id: client_id.into(),
            authorization_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_endpoint: "https://oauth2.googleapis.com/token".into(),
            scopes: vec!["https://www.googleapis.com/auth/cloud-platform".into()],
            callback_port: 1456,
            callback_path: "/auth/callback".into(),
            api_base_url: "https://vision.googleapis.com/v1/images:annotate".into(),
            authorization_extra_params: vec![
                ("access_type".into(), "offline".into()),
                ("include_granted_scopes".into(), "true".into()),
            ],
        }
    }

    /// Standard OpenAI API base URL (for API key authentication).
    pub const OPENAI_API_BASE_URL: &'static str = "https://api.openai.com/v1";

    /// Construct the full redirect URI.
    pub fn redirect_uri(&self) -> String {
        format!(
            "http://localhost:{}{}",
            self.callback_port, self.callback_path
        )
    }

    /// Build the full authorization URL with PKCE parameters.
    ///
    /// Returns `Err` if `authorization_endpoint` is not a valid URL.
    pub fn authorization_url(
        &self,
        state: &str,
        pkce_challenge: &str,
    ) -> Result<String, url::ParseError> {
        let scope = self.scopes.join(" ");
        let redirect = self.redirect_uri();
        let mut url = url::Url::parse(&self.authorization_endpoint)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &self.client_id);
            query.append_pair("redirect_uri", &redirect);
            query.append_pair("scope", &scope);
            query.append_pair("state", state);
            query.append_pair("code_challenge", pkce_challenge);
            query.append_pair("code_challenge_method", "S256");
            for (key, value) in &self.authorization_extra_params {
                query.append_pair(key, value);
            }
        }
        Ok(url.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_preset_has_correct_port() {
        let config = OAuthProviderConfig::openai_codex();
        assert_eq!(config.callback_port, 1455);
    }

    #[test]
    fn openai_preset_has_correct_client_id() {
        let config = OAuthProviderConfig::openai_codex();
        assert_eq!(config.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
    }

    #[test]
    fn openai_redirect_uri() {
        let config = OAuthProviderConfig::openai_codex();
        assert_eq!(config.redirect_uri(), "http://localhost:1455/auth/callback");
    }

    #[test]
    fn authorization_url_contains_required_params() {
        let config = OAuthProviderConfig::openai_codex();
        let url = config
            .authorization_url("test_state", "test_challenge")
            .expect("known-good preset URL should parse");

        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("redirect_uri="));
        assert!(url.contains("localhost%3A1455"));
        assert!(url.contains("state=test_state"));
        assert!(url.contains("code_challenge=test_challenge"));
        assert!(url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn scopes_include_offline_access() {
        let config = OAuthProviderConfig::openai_codex();
        assert!(config.scopes.contains(&"offline_access".to_string()));
    }

    #[test]
    fn openai_preset_has_chatgpt_api_base_url() {
        let config = OAuthProviderConfig::openai_codex();
        assert_eq!(config.api_base_url, "https://chatgpt.com/backend-api/codex");
    }

    #[test]
    fn google_preset_has_expected_endpoints() {
        let config = OAuthProviderConfig::google_cloud_vision("desktop-client-id");
        assert_eq!(
            config.authorization_endpoint,
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(config.token_endpoint, "https://oauth2.googleapis.com/token");
        assert_eq!(config.callback_port, 1456);
        assert_eq!(
            config.api_base_url,
            "https://vision.googleapis.com/v1/images:annotate"
        );
    }

    #[test]
    fn google_authorization_url_includes_offline_access_hint() {
        let config = OAuthProviderConfig::google_cloud_vision("desktop-client-id");
        let url = config
            .authorization_url("state", "challenge")
            .expect("known-good preset URL should parse");

        assert!(url.contains("access_type=offline"));
        assert!(url.contains("include_granted_scopes=true"));
        assert!(url.contains("client_id=desktop-client-id"));
    }

    #[test]
    fn openai_api_key_base_url_constant() {
        assert_eq!(
            OAuthProviderConfig::OPENAI_API_BASE_URL,
            "https://api.openai.com/v1"
        );
    }
}
