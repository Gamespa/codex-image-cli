use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::blocking::{
    Client, Response,
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use crate::codex_config::CodexConnection;

const USER_AGENT: &str = concat!("codex-image-cli/", env!("CARGO_PKG_VERSION"));
const MAX_RESPONSE_BYTES: u64 = 80 * 1024 * 1024;
const MAX_ERROR_BYTES: u64 = 64 * 1024;
const MAX_PROMPT_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_COUNT: u32 = 10;

pub struct GenerationRequest {
    pub prompt: String,
    pub model: String,
    pub size: String,
    pub image_count: u32,
    pub output_dir: PathBuf,
    pub timeout: Duration,
    pub max_image_bytes: u64,
}

pub struct EditRequest {
    pub prompt: String,
    pub image: PathBuf,
    pub mask: Option<PathBuf>,
    pub model: String,
    pub size: String,
    pub image_count: u32,
    pub output_dir: PathBuf,
    pub timeout: Duration,
    pub max_image_bytes: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
}

impl ImageFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
        }
    }
}

struct StagedImage {
    path: PathBuf,
    final_path: PathBuf,
    url: Option<String>,
}

pub fn generate_images(
    connection: &CodexConnection,
    request: &GenerationRequest,
) -> Result<GenerationSummary> {
    if request.prompt.trim().is_empty() {
        bail!("--prompt must not be empty")
    }
    if request.prompt.len() > MAX_PROMPT_BYTES {
        bail!("--prompt exceeds the {MAX_PROMPT_BYTES}-byte limit")
    }
    if request.model.trim().is_empty() {
        bail!("--model must not be empty")
    }
    if request.size.trim().is_empty() {
        bail!("--size must not be empty")
    }
    if !(1..=MAX_IMAGE_COUNT).contains(&request.image_count) {
        bail!("--n must be between 1 and {MAX_IMAGE_COUNT}")
    }
    if request.timeout.is_zero() {
        bail!("generation timeout must be greater than zero")
    }
    if request.max_image_bytes == 0 {
        bail!("maximum image size must be greater than zero")
    }

    let deadline = Instant::now() + request.timeout;
    let client = Client::builder()
        .connect_timeout(request.timeout.min(Duration::from_secs(30)))
        .user_agent(USER_AGENT)
        .build()
        .context("build HTTP client")?;
    let endpoint = format!(
        "{}/images/generations",
        connection.base_url().trim_end_matches('/')
    );
    let mut request_builder = client.post(&endpoint).json(&ImagesGenerationsBody {
        model: &request.model,
        prompt: &request.prompt,
        n: request.image_count,
        size: &request.size,
    });
    if let Some(api_key) = connection.api_key() {
        request_builder = request_builder.bearer_auth(api_key);
    }
    let response = request_builder
        .timeout(remaining(deadline)?)
        .send()
        .context("send image generation request")?;
    let images = process_image_response(
        &client,
        deadline,
        response,
        request.image_count,
        &request.output_dir,
        request.max_image_bytes,
        "generation",
    )?;
    Ok(GenerationSummary {
        model: request.model.clone(),
        images,
    })
}

pub fn edit_images(
    connection: &CodexConnection,
    request: &EditRequest,
) -> Result<GenerationSummary> {
    validate_request(
        &request.prompt,
        &request.model,
        &request.size,
        request.image_count,
        request.timeout,
        request.max_image_bytes,
    )?;

    let deadline = Instant::now() + request.timeout;
    let client = Client::builder()
        .connect_timeout(request.timeout.min(Duration::from_secs(30)))
        .user_agent(USER_AGENT)
        .build()
        .context("build HTTP client")?;
    let mut form = Form::new()
        .text("model", request.model.clone())
        .text("prompt", request.prompt.clone())
        .text("n", request.image_count.to_string())
        .text("size", request.size.clone())
        .part(
            "image",
            image_part(&request.image, request.max_image_bytes).context("prepare source image")?,
        );
    if let Some(mask) = request.mask.as_deref() {
        form = form.part(
            "mask",
            image_part(mask, request.max_image_bytes).context("prepare mask image")?,
        );
    }
    let endpoint = format!(
        "{}/images/edits",
        connection.base_url().trim_end_matches('/')
    );
    let mut request_builder = client.post(&endpoint).multipart(form);
    if let Some(api_key) = connection.api_key() {
        request_builder = request_builder.bearer_auth(api_key);
    }
    let response = request_builder
        .timeout(remaining(deadline)?)
        .send()
        .context("send image edit request")?;
    let images = process_image_response(
        &client,
        deadline,
        response,
        request.image_count,
        &request.output_dir,
        request.max_image_bytes,
        "edit",
    )?;
    Ok(GenerationSummary {
        model: request.model.clone(),
        images,
    })
}

fn process_image_response(
    client: &Client,
    deadline: Instant,
    mut response: Response,
    image_count: u32,
    output_dir: &Path,
    max_image_bytes: u64,
    operation: &str,
) -> Result<Vec<GeneratedImage>> {
    let status = response.status();
    if !status.is_success() {
        let response_body = read_prefix(&mut response, MAX_ERROR_BYTES)
            .with_context(|| format!("read image {operation} error response"))?;
        let response_body = String::from_utf8_lossy(&response_body);
        bail!(
            "image {operation} failed with HTTP {status}: {}",
            truncate_error(&response_body)
        );
    }
    let response_body = read_bounded(
        &mut response,
        MAX_RESPONSE_BYTES,
        &format!("image {operation} response"),
    )?;
    let payload: ImagesGenerationsResponse =
        serde_json::from_slice(&response_body).context("parse image generation response")?;
    if payload.data.len() != image_count as usize {
        bail!(
            "image {operation} returned {} images; expected {}",
            payload.data.len(),
            image_count
        )
    }

    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;
    let staging = Builder::new()
        .prefix(".codex-image-run-")
        .tempdir_in(output_dir)
        .with_context(|| format!("create staging directory in {}", output_dir.display()))?;
    let run_id = staging
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("codex-image-run")
        .trim_start_matches(".codex-image-run-")
        .to_owned();
    let mut staged_images = Vec::with_capacity(payload.data.len());
    for (index, image) in payload.data.into_iter().enumerate() {
        let staged_path = staging.path().join(format!("image-{}.tmp", index + 1));
        if let Some(encoded) = image.b64_json.as_deref() {
            ensure_encoded_size(encoded.len(), max_image_bytes)?;
            let bytes = STANDARD
                .decode(encoded)
                .context("decode b64_json image data")?;
            if bytes.len() as u64 > max_image_bytes {
                bail!(
                    "decoded image {} exceeds the configured size limit",
                    index + 1
                )
            }
            fs::write(&staged_path, &bytes)
                .with_context(|| format!("write {}", staged_path.display()))?;
        } else if let Some(url) = image.url.as_deref() {
            download_image(client, url, &staged_path, max_image_bytes, deadline)?;
        } else {
            return Err(anyhow!("image response item had neither b64_json nor url"));
        }

        let format = detect_image_format_file(&staged_path)?;
        let final_path = output_dir.join(format!(
            "codex-image-{run_id}-{}.{}",
            index + 1,
            format.extension()
        ));
        staged_images.push(StagedImage {
            path: staged_path,
            final_path,
            url: image.url,
        });
    }

    commit_images(staged_images)
}

fn validate_request(
    prompt: &str,
    model: &str,
    size: &str,
    image_count: u32,
    timeout: Duration,
    max_image_bytes: u64,
) -> Result<()> {
    if prompt.trim().is_empty() {
        bail!("--prompt must not be empty")
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        bail!("--prompt exceeds the {MAX_PROMPT_BYTES}-byte limit")
    }
    if model.trim().is_empty() {
        bail!("--model must not be empty")
    }
    if size.trim().is_empty() {
        bail!("--size must not be empty")
    }
    if !(1..=MAX_IMAGE_COUNT).contains(&image_count) {
        bail!("--n must be between 1 and {MAX_IMAGE_COUNT}")
    }
    if timeout.is_zero() {
        bail!("generation timeout must be greater than zero")
    }
    if max_image_bytes == 0 {
        bail!("maximum image size must be greater than zero")
    }
    Ok(())
}

fn image_part(path: &Path, max_image_bytes: u64) -> Result<Part> {
    let metadata = fs::metadata(path).with_context(|| format!("read {}", path.display()))?;
    if metadata.len() > max_image_bytes {
        bail!("{} exceeds the configured size limit", path.display())
    }
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() as u64 > max_image_bytes {
        bail!("{} exceeds the configured size limit", path.display())
    }
    let format = detect_image_format(&bytes)
        .with_context(|| format!("validate image {}", path.display()))?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("image file name is not valid Unicode")?;
    let mime = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Webp => "image/webp",
        ImageFormat::Gif => "image/gif",
    };
    Part::bytes(bytes)
        .file_name(filename.to_owned())
        .mime_str(mime)
        .context("set image MIME type")
}

fn ensure_encoded_size(encoded_len: usize, max_image_bytes: u64) -> Result<()> {
    let max_encoded_len = max_image_bytes
        .saturating_add(2)
        .saturating_div(3)
        .saturating_mul(4)
        .saturating_add(4);
    if encoded_len as u64 > max_encoded_len {
        bail!("base64 image exceeds the configured size limit")
    }
    Ok(())
}

fn download_image(
    client: &Client,
    url: &str,
    path: &Path,
    max_image_bytes: u64,
    deadline: Instant,
) -> Result<()> {
    let parsed_url = reqwest::Url::parse(url).context("parse generated image URL")?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        bail!("generated image URL must use http or https")
    }
    let mut response = client
        .get(parsed_url)
        .timeout(remaining(deadline)?)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .context("download generated image URL")?;
    if let Some(length) = response.content_length() {
        if length > max_image_bytes {
            bail!("generated image exceeds the configured size limit")
        }
    }
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let content_type = content_type.to_str().context("read image Content-Type")?;
        if !content_type.starts_with("image/") && content_type != "application/octet-stream" {
            bail!("generated image URL returned unsupported Content-Type {content_type:?}")
        }
    }

    let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = response
            .read(&mut buffer)
            .context("read generated image URL")?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > max_image_bytes {
            bail!("generated image exceeds the configured size limit")
        }
        file.write_all(&buffer[..read])
            .with_context(|| format!("write {}", path.display()))?;
        remaining(deadline)?;
    }
    file.flush()
        .with_context(|| format!("flush {}", path.display()))?;
    Ok(())
}

fn detect_image_format_file(path: &Path) -> Result<ImageFormat> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut header = [0_u8; 16];
    let read = file
        .read(&mut header)
        .with_context(|| format!("read {}", path.display()))?;
    detect_image_format(&header[..read])
}

fn detect_image_format(bytes: &[u8]) -> Result<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(ImageFormat::Png);
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Ok(ImageFormat::Jpeg);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Ok(ImageFormat::Webp);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(ImageFormat::Gif);
    }
    bail!("data is not a supported PNG, JPEG, WebP, or GIF image")
}

fn commit_images(staged_images: Vec<StagedImage>) -> Result<Vec<GeneratedImage>> {
    let mut committed_paths = Vec::with_capacity(staged_images.len());
    let mut images = Vec::with_capacity(staged_images.len());
    for image in staged_images {
        if let Err(error) = fs::rename(&image.path, &image.final_path) {
            for path in committed_paths {
                let _ = fs::remove_file(path);
            }
            return Err(error).with_context(|| format!("commit {}", image.final_path.display()));
        }
        committed_paths.push(image.final_path.clone());
        images.push(GeneratedImage {
            path: image.final_path,
            url: image.url,
        });
    }
    Ok(images)
}

fn read_bounded(reader: &mut impl Read, limit: u64, description: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {description}"))?;
    if bytes.len() as u64 > limit {
        bail!("{description} exceeds the {limit}-byte limit")
    }
    Ok(bytes)
}

fn read_prefix(reader: &mut impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit)
        .read_to_end(&mut bytes)
        .context("read response prefix")?;
    Ok(bytes)
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| anyhow!("image generation exceeded its overall timeout"))
}

fn truncate_error(value: &str) -> &str {
    let mut end = value.len().min(1_000);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::{
        GenerationRequest, ImageFormat, detect_image_format, generate_images, truncate_error,
    };
    use crate::codex_config::CodexConnection;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::json;
    use std::{
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };
    use tempfile::tempdir;

    struct MockResponse {
        status: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    }

    fn spawn_server(build_responses: impl FnOnce(&str) -> Vec<MockResponse>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let responses = build_responses(&base_url);
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                let headers = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status,
                    response.content_type,
                    response.body.len()
                );
                stream.write_all(headers.as_bytes()).unwrap();
                stream.write_all(&response.body).unwrap();
            }
        });
        base_url
    }

    fn read_request(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .or_else(|| line.strip_prefix("Content-Length: "))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if request.len() >= header_end + content_length {
                break;
            }
        }
    }

    fn generation_request(output_dir: &std::path::Path, image_count: u32) -> GenerationRequest {
        GenerationRequest {
            prompt: "test image".to_owned(),
            model: "test-model".to_owned(),
            size: "1024x1024".to_owned(),
            image_count,
            output_dir: output_dir.to_path_buf(),
            timeout: Duration::from_secs(5),
            max_image_bytes: 1024,
        }
    }

    #[test]
    fn detects_supported_image_formats() {
        assert_eq!(
            detect_image_format(b"\x89PNG\r\n\x1a\nrest").unwrap(),
            ImageFormat::Png
        );
        assert_eq!(
            detect_image_format(b"\xff\xd8\xffrest").unwrap(),
            ImageFormat::Jpeg
        );
        assert_eq!(
            detect_image_format(b"RIFF\x04\x00\x00\x00WEBPrest").unwrap(),
            ImageFormat::Webp
        );
        assert_eq!(
            detect_image_format(b"GIF89arest").unwrap(),
            ImageFormat::Gif
        );
    }

    #[test]
    fn error_messages_are_bounded() {
        let error = "x".repeat(1_200);
        assert_eq!(truncate_error(&error).len(), 1_000);
    }

    #[test]
    fn unicode_error_messages_are_bounded() {
        let error = format!("{}tail", "界".repeat(334));
        let truncated = truncate_error(&error);

        assert!(truncated.len() <= 1_000);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn commits_a_valid_base64_batch_with_the_detected_extension() {
        let jpeg = b"\xff\xd8\xffmock-jpeg";
        let base_url = spawn_server(|_| {
            vec![MockResponse {
                status: "200 OK",
                content_type: "application/json",
                body: serde_json::to_vec(&json!({
                    "data": [{ "b64_json": STANDARD.encode(jpeg) }]
                }))
                .unwrap(),
            }]
        });
        let output = tempdir().unwrap();
        let connection = CodexConnection::new(format!("{base_url}/v1"), None);

        let summary = generate_images(&connection, &generation_request(output.path(), 1)).unwrap();

        assert_eq!(summary.images.len(), 1);
        assert_eq!(
            summary.images[0]
                .path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("jpg")
        );
        assert_eq!(fs::read(&summary.images[0].path).unwrap(), jpeg);
    }

    #[test]
    fn does_not_commit_a_partial_batch() {
        let base_url = spawn_server(|_| {
            vec![MockResponse {
                status: "200 OK",
                content_type: "application/json",
                body: serde_json::to_vec(&json!({
                    "data": [
                        { "b64_json": STANDARD.encode(b"\x89PNG\r\n\x1a\nmock") },
                        {}
                    ]
                }))
                .unwrap(),
            }]
        });
        let output = tempdir().unwrap();
        let connection = CodexConnection::new(format!("{base_url}/v1"), None);

        let error =
            generate_images(&connection, &generation_request(output.path(), 2)).unwrap_err();

        assert!(error.to_string().contains("neither b64_json nor url"));
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    }

    #[test]
    fn rejects_an_incomplete_response_before_creating_output() {
        let base_url = spawn_server(|_| {
            vec![MockResponse {
                status: "200 OK",
                content_type: "application/json",
                body: serde_json::to_vec(&json!({
                    "data": [{ "b64_json": STANDARD.encode(b"\x89PNG\r\n\x1a\nmock") }]
                }))
                .unwrap(),
            }]
        });
        let parent = tempdir().unwrap();
        let output = parent.path().join("images");
        let connection = CodexConnection::new(format!("{base_url}/v1"), None);

        let error = generate_images(&connection, &generation_request(&output, 2)).unwrap_err();

        assert!(error.to_string().contains("returned 1 images; expected 2"));
        assert!(!output.exists());
    }

    #[test]
    fn rejects_oversized_url_images_without_committing_files() {
        let base_url = spawn_server(|base_url| {
            vec![
                MockResponse {
                    status: "200 OK",
                    content_type: "application/json",
                    body: serde_json::to_vec(&json!({
                        "data": [{ "url": format!("{base_url}/image") }]
                    }))
                    .unwrap(),
                },
                MockResponse {
                    status: "200 OK",
                    content_type: "image/png",
                    body: b"\x89PNG\r\n\x1a\nmock".to_vec(),
                },
            ]
        });
        let output = tempdir().unwrap();
        let connection = CodexConnection::new(format!("{base_url}/v1"), None);
        let mut request = generation_request(output.path(), 1);
        request.max_image_bytes = 8;

        let error = generate_images(&connection, &request).unwrap_err();

        assert!(error.to_string().contains("size limit"));
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 0);
    }
}
