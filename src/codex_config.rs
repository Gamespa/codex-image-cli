use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

use crate::home_dir;

#[derive(Debug, Clone)]
pub struct CodexConnection {
    pub base_url: String,
    pub api_key: String,
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
    openai_api_key: String,
}

impl CodexConnection {
    pub fn resolve(
        codex_home: Option<&Path>,
        base_url_override: Option<String>,
        api_key_env_override: Option<String>,
    ) -> Result<Self> {
        let codex_home = codex_home
            .map(Path::to_path_buf)
            .or_else(|| env::var_os("CODEX_HOME").map(PathBuf::from))
            .unwrap_or(home_dir()?.join(".codex"));
        let config = load_config(&codex_home)?;
        let provider = active_provider(&config)?;
        let base_url = base_url_override
            .or_else(|| provider.and_then(|provider| provider.base_url.clone()))
            .or_else(|| config.openai_base_url.clone())
            .ok_or_else(|| anyhow!("no base URL configured; set --base-url or configure Codex"))?;
        let api_key = resolve_api_key(&codex_home, provider, api_key_env_override)?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
        })
    }
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

fn resolve_api_key(
    codex_home: &Path,
    provider: Option<&ModelProvider>,
    api_key_env_override: Option<String>,
) -> Result<String> {
    let env_key =
        api_key_env_override.or_else(|| provider.and_then(|provider| provider.env_key.clone()));
    if let Some(env_key) = env_key {
        return env::var(&env_key)
            .with_context(|| format!("read API key from environment variable {env_key}"));
    }

    if provider.is_none_or(|provider| provider.requires_openai_auth.unwrap_or(true)) {
        let path = codex_home.join("auth.json");
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("read Codex auth at {}", path.display()))?;
        let auth: CodexAuth = serde_json::from_str(&contents).context("parse Codex auth.json")?;
        if auth.openai_api_key.trim().is_empty() {
            bail!("Codex auth.json does not contain an API key")
        }
        return Ok(auth.openai_api_key);
    }

    bail!("the active provider has no env_key or Codex auth configuration; pass --api-key-env")
}

#[cfg(test)]
mod tests {
    use super::{CodexConfig, active_provider};

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
}
