mod codex_config;
mod image_generation;

use std::{env, path::PathBuf, process::ExitCode, time::Duration};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use codex_config::CodexConnection;
use image_generation::{GenerationRequest, MAX_IMAGE_COUNT, generate_images};

const DEFAULT_MODEL: &str = "gpt-image-2";
const DEFAULT_TIMEOUT_SECONDS: u64 = 180;
const DEFAULT_MAX_IMAGE_MIB: u64 = 50;

#[derive(Debug, Parser)]
#[command(
    name = "codex-image",
    version,
    about = "Generate images through the active Codex provider"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate one or more images with the active Codex provider.
    Generate {
        /// Image prompt. This value is sent only to the configured image endpoint.
        #[arg(
            long,
            required_unless_present = "prompt_env",
            conflicts_with = "prompt_env"
        )]
        prompt: Option<String>,

        /// Read the image prompt from this environment variable.
        #[arg(long, conflicts_with = "prompt")]
        prompt_env: Option<String>,

        /// Image model exposed by the active provider.
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,

        /// Requested image size.
        #[arg(long, default_value = "1024x1024")]
        size: String,

        /// Number of images to request.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=MAX_IMAGE_COUNT as i64))]
        n: u32,

        /// Directory for downloaded or decoded images.
        #[arg(long, default_value = "generated_images")]
        output_dir: PathBuf,

        /// Override the Codex provider base URL for this command.
        #[arg(long, env = "CODEX_IMAGE_BASE_URL")]
        base_url: Option<String>,

        /// Read the provider API key from this environment variable instead of Codex auth.
        #[arg(long, env = "CODEX_IMAGE_API_KEY_ENV")]
        api_key_env: Option<String>,

        /// Send no Authorization header. Intended for trusted local providers.
        #[arg(long, conflicts_with = "api_key_env")]
        no_auth: bool,

        /// Overall generation and download deadline in seconds.
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECONDS, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout_seconds: u64,

        /// Maximum decoded or downloaded size of one image in MiB.
        #[arg(long, default_value_t = DEFAULT_MAX_IMAGE_MIB, value_parser = clap::value_parser!(u64).range(1..=1024))]
        max_image_mib: u64,

        /// Use a different Codex home directory. Defaults to CODEX_HOME or ~/.codex.
        #[arg(long, env = "CODEX_HOME")]
        codex_home: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate {
            prompt,
            prompt_env,
            model,
            size,
            n,
            output_dir,
            base_url,
            api_key_env,
            no_auth,
            timeout_seconds,
            max_image_mib,
            codex_home,
        } => {
            let prompt = match (prompt, prompt_env) {
                (Some(prompt), None) => prompt,
                (None, Some(prompt_env)) => env::var(&prompt_env).with_context(|| {
                    format!("read prompt from environment variable {prompt_env}")
                })?,
                _ => unreachable!("clap enforces exactly one prompt source"),
            };
            let connection =
                CodexConnection::resolve(codex_home.as_deref(), base_url, api_key_env, no_auth)
                    .context("resolve the active Codex provider")?;
            let max_image_bytes = max_image_mib
                .checked_mul(1024 * 1024)
                .context("--max-image-mib is too large")?;
            let summary = generate_images(
                &connection,
                &GenerationRequest {
                    prompt,
                    model,
                    size,
                    image_count: n,
                    output_dir,
                    timeout: Duration::from_secs(timeout_seconds),
                    max_image_bytes,
                },
            )?;
            println!("{}", serde_json::to_string(&summary)?);
            Ok(())
        }
    }
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .context("USERPROFILE or HOME is not set")
}
