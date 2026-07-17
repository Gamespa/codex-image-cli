# codex-image-cli

`codex-image-cli` adds an Images Generations command for the provider currently configured in Codex.
It reads the active provider base URL from `~/.codex/config.toml` and resolves credentials from the
provider's `env_key` or, when configured, `~/.codex/auth.json`. It never writes credentials to disk,
prints them, or accepts them as command-line arguments.

## Usage

```powershell
cargo run -- generate `
  --prompt "A red circle centered on a white background" `
  --model gpt-image-2 `
  --size 1024x1024
```

Images are written to `generated_images/` by default. The command writes a JSON summary containing
the generated file paths and any upstream URLs.

Use `--output-dir` to choose another directory. Use `--base-url` and `--api-key-env` only when you
intend to override the active Codex provider for a single command:

```powershell
$env:CODEX_IMAGE_KEY = "..."
cargo run -- generate `
  --base-url https://router.example/v1 `
  --api-key-env CODEX_IMAGE_KEY `
  --prompt "A paper-cut mountain landscape"
```

## Provider Requirements

The provider must support the OpenAI-compatible `POST /v1/images/generations` endpoint and return
an image URL or `b64_json`. The CLI adds a stable `User-Agent` and defaults to a `1024x1024` request.

## Security

- Do not pass API keys with command-line flags.
- Do not commit `.codex/`, `auth.json`, generated images, or `.env` files.
- Prompts are sent to the configured provider but are not saved by this CLI.
