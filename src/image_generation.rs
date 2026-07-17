use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::codex_config::CodexConnection;

const USER_AGENT: &str = concat!("codex-image-cli/", env!("CARGO_PKG_VERSION"));

pub struct GenerationRequest {
    pub prompt: String,
    pub model: String,
    pub size: String,
    pub image_count: u32,
    pub output_dir: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct GenerationSummary {
    pub model: String,
    pub images: Vec<GeneratedImage>,
}

#[derive(Debug, Serialize)]
pub struct GeneratedImage {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Serialize)]
struct ImagesGenerationsBody<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u32,
    size: &'a str,
}

#[derive(Deserialize)]
struct ImagesGenerationsResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize)]
struct ImageData {
    b64_json: Option<String>,
    url: Option<String>,
}

pub fn generate_images(
    connection: &CodexConnection,
    request: &GenerationRequest,
) -> Result<GenerationSummary> {
    if request.prompt.trim().is_empty() {
        bail!("--prompt must not be empty")
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent(USER_AGENT)
        .build()
        .context("build HTTP client")?;
    let endpoint = format!(
        "{}/images/generations",
        connection.base_url.trim_end_matches('/')
    );
    let response = client
        .post(&endpoint)
        .bearer_auth(&connection.api_key)
        .json(&ImagesGenerationsBody {
            model: &request.model,
            prompt: &request.prompt,
            n: request.image_count,
            size: &request.size,
        })
        .send()
        .context("send image generation request")?;
    let status = response.status();
    let response_body = response.text().context("read image generation response")?;
    if !status.is_success() {
        bail!(
            "image generation failed with HTTP {status}: {}",
            truncate_error(&response_body)
        );
    }
    let payload: ImagesGenerationsResponse =
        serde_json::from_str(&response_body).context("parse image generation response")?;
    if payload.data.is_empty() {
        bail!("image generation response did not contain images")
    }

    fs::create_dir_all(&request.output_dir)
        .with_context(|| format!("create output directory {}", request.output_dir.display()))?;
    let mut images = Vec::with_capacity(payload.data.len());
    for (index, image) in payload.data.into_iter().enumerate() {
        let path = image_path(&request.output_dir, index);
        if let Some(encoded) = image.b64_json.as_deref() {
            let bytes = STANDARD
                .decode(encoded)
                .context("decode b64_json image data")?;
            fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
        } else if let Some(url) = image.url.as_deref() {
            let bytes = client
                .get(url)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .context("download generated image URL")?
                .bytes()
                .context("read generated image URL")?;
            fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
        } else {
            return Err(anyhow!("image response item had neither b64_json nor url"));
        }
        images.push(GeneratedImage {
            path,
            url: image.url,
        });
    }

    Ok(GenerationSummary {
        model: request.model.clone(),
        images,
    })
}

fn image_path(output_dir: &Path, index: usize) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    output_dir.join(format!("codex-image-{timestamp}-{}.png", index + 1))
}

fn truncate_error(value: &str) -> &str {
    value.get(..1_000).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::{image_path, truncate_error};
    use std::path::Path;

    #[test]
    fn image_paths_have_png_extension() {
        let path = image_path(Path::new("images"), 1);
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("png")
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-2.png")
        );
    }

    #[test]
    fn error_messages_are_bounded() {
        let error = "x".repeat(1_200);
        assert_eq!(truncate_error(&error).len(), 1_000);
    }
}
