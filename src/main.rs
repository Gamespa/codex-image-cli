mod codex_config;
mod image_generation;

use std::{env, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use codex_config::CodexConnection;
use image_generation::{GenerationRequest, generate_images};

const DEFAULT_MODEL: &str = "gpt-image-2";

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
        #[arg(long)]
        prompt: String,

        /// Image model exposed by the active provider.
        #[arg(long, default_value = DEFAULT_MODEL)]
        model: String,

        /// Requested image size.
        #[arg(long, default_value = "1024x1024")]
        size: String,

        /// Number of images to request.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
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
            model,
            size,
            n,
            output_dir,
            base_url,
            api_key_env,
            codex_home,
        } => {
            let connection = CodexConnection::resolve(codex_home.as_deref(), base_url, api_key_env)
                .context("resolve the active Codex provider")?;
            let summary = generate_images(
                &connection,
                &GenerationRequest {
                    prompt,
                    model,
                    size,
                    image_count: n,
                    output_dir,
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
