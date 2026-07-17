use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::home_dir;

#[derive(Clone)]
pub struct CodexConnection {
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CodexConfig {
    model_provider: Option<String>,
    openai_base_url: Option<String>,
    #[serde(default)]
    model_providers: std::collections::BTreeMap<String, ModelProvider>,
}

#[derive(Debug, Deserialize, Default)]
struct ModelProvider {
    base_url: Option<String>,
    env_key: Option<String>,
    requires_openai_auth: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CodexAuth {
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
}

impl CodexConnection {
    pub fn resolve(
        codex_home: Option<&Path>,
        base_url_override: Option<String>,
        api_key_env_override: Option<String>,
        no_auth: bool,
    ) -> Result<Self> {
        let needs_config =
            base_url_override.is_none() || (!no_auth && api_key_env_override.is_none());
        let codex_home = needs_config
            .then(|| resolve_codex_home(codex_home))
            .transpose()?;
        let config = codex_home.as_deref().map(load_config).transpose()?;
        let provider = config.as_ref().map(active_provider).transpose()?.flatten();
        let base_url = base_url_override
            .or_else(|| provider.and_then(|provider| provider.base_url.clone()))
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.openai_base_url.clone())
            })
            .or_else(|| config.as_ref().and_then(default_base_url))
            .ok_or_else(|| anyhow!("no base URL configured; set --base-url or configure Codex"))?;
        let base_url = validate_base_url(&base_url)?;
        let api_key = resolve_api_key(
            codex_home.as_deref(),
            config.as_ref(),
            provider,
            api_key_env_override,
            no_auth,
        )?;

        Ok(Self { base_url, api_key })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
        }
    }
}

fn resolve_codex_home(codex_home: Option<&Path>) -> Result<PathBuf> {
    if let Some(codex_home) = codex_home {
        return Ok(codex_home.to_path_buf());
    }
    if let Some(codex_home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home));
    }
    Ok(home_dir()?.join(".codex"))
}

fn load_config(codex_home: &Path) -> Result<CodexConfig> {
    let path = codex_home.join("config.toml");
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("read Codex config at {}", path.display()))?;
    toml::from_str(&contents).context("parse Codex config.toml")
}

fn active_provider(config: &CodexConfig) -> Result<Option<&ModelProvider>> {
    let Some(name) = config.model_provider.as_deref() else {
        return Ok(None);
    };
    if matches!(name, "openai" | "ollama" | "lmstudio") {
        return Ok(None);
    }
    config
        .model_providers
        .get(name)
        .map(Some)
        .ok_or_else(|| anyhow!("active Codex model provider {name:?} is not configured"))
}

fn default_base_url(config: &CodexConfig) -> Option<String> {
    match config.model_provider.as_deref().unwrap_or("openai") {
        "openai" => Some("https://api.openai.com/v1".to_owned()),
        "ollama" => Some("http://127.0.0.1:11434/v1".to_owned()),
        "lmstudio" => Some("http://127.0.0.1:1234/v1".to_owned()),
        _ => None,
    }
}

fn validate_base_url(base_url: &str) -> Result<String> {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        bail!("provider base URL must not be empty")
    }
    let parsed = reqwest::Url::parse(base_url).context("parse provider base URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("provider base URL must use http or https")
    }
    if parsed.host_str().is_none() {
        bail!("provider base URL must include a host")
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("provider base URL must not contain credentials")
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("provider base URL must not contain a query or fragment")
    }
    Ok(base_url.to_owned())
}

fn resolve_api_key(
    codex_home: Option<&Path>,
    config: Option<&CodexConfig>,
    provider: Option<&ModelProvider>,
    api_key_env_override: Option<String>,
    no_auth: bool,
) -> Result<Option<String>> {
    if no_auth {
        return Ok(None);
    }
    let env_key =
        api_key_env_override.or_else(|| provider.and_then(|provider| provider.env_key.clone()));
    if let Some(env_key) = env_key {
        return env::var(&env_key)
            .map(Some)
            .with_context(|| format!("read API key from environment variable {env_key}"));
    }

    if !requires_openai_auth(config, provider) {
        return Ok(None);
    }

    if let Some(codex_home) = codex_home {
        let path = codex_home.join("auth.json");
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("read Codex auth at {}", path.display()))?;
        let auth: CodexAuth = serde_json::from_str(&contents).context("parse Codex auth.json")?;
        let api_key = auth
            .openai_api_key
            .filter(|api_key| !api_key.trim().is_empty());
        if api_key.is_none() {
            bail!("Codex auth.json does not contain an API key")
        }
        return Ok(api_key);
    }

    bail!("no API key source configured; pass --api-key-env or --no-auth")
}

fn requires_openai_auth(config: Option<&CodexConfig>, provider: Option<&ModelProvider>) -> bool {
    if let Some(provider) = provider {
        return provider.requires_openai_auth.unwrap_or(true);
    }
    !matches!(
        config.and_then(|config| config.model_provider.as_deref()),
        Some("ollama" | "lmstudio")
    )
}

#[cfg(test)]
mod tests {
    use super::{CodexConfig, CodexConnection, active_provider};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn selects_the_active_custom_provider() {
        let config: CodexConfig = toml::from_str(
            r#"
            model_provider = "proxy"
            [model_providers.proxy]
            base_url = "https://proxy.example/v1"
            requires_openai_auth = true
            "#,
        )
        .unwrap();

        let provider = active_provider(&config).unwrap().unwrap();
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://proxy.example/v1")
        );
        assert_eq!(provider.requires_openai_auth, Some(true));
    }

    #[test]
    fn complete_overrides_do_not_require_a_codex_config() {
        let codex_home = tempdir().unwrap();
        let connection = CodexConnection::resolve(
            Some(codex_home.path()),
            Some("https://images.example/v1".to_owned()),
            Some("PATH".to_owned()),
            false,
        )
        .unwrap();

        assert_eq!(connection.base_url(), "https://images.example/v1");
        assert!(connection.api_key().is_some());
    }

    #[test]
    fn supports_unauthenticated_custom_providers() {
        let codex_home = tempdir().unwrap();
        fs::write(
            codex_home.path().join("config.toml"),
            r#"
            model_provider = "local"
            [model_providers.local]
            base_url = "http://127.0.0.1:8080/v1"
            requires_openai_auth = false
            "#,
        )
        .unwrap();

        let connection =
            CodexConnection::resolve(Some(codex_home.path()), None, None, false).unwrap();

        assert_eq!(connection.base_url(), "http://127.0.0.1:8080/v1");
        assert_eq!(connection.api_key(), None);
    }

    #[test]
    fn supports_explicit_no_auth_without_a_codex_config() {
        let codex_home = tempdir().unwrap();
        let connection = CodexConnection::resolve(
            Some(codex_home.path()),
            Some("http://127.0.0.1:8080/v1".to_owned()),
            None,
            true,
        )
        .unwrap();

        assert_eq!(connection.api_key(), None);
    }

    #[test]
    fn uses_the_default_openai_base_url() {
        let codex_home = tempdir().unwrap();
        fs::write(
            codex_home.path().join("config.toml"),
            "model_provider = \"openai\"\n",
        )
        .unwrap();

        let connection = CodexConnection::resolve(
            Some(codex_home.path()),
            None,
            Some("PATH".to_owned()),
            false,
        )
        .unwrap();

        assert_eq!(connection.base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn rejects_unsafe_base_url_components() {
        let codex_home = tempdir().unwrap();
        for base_url in [
            "file:///tmp/images",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?route=images",
            "https://example.com/v1#images",
        ] {
            let error = CodexConnection::resolve(
                Some(codex_home.path()),
                Some(base_url.to_owned()),
                None,
                true,
            )
            .err()
            .unwrap();
            assert!(error.to_string().contains("provider base URL"));
        }
    }
}
