# codex-image-cli

`codex-image-cli` calls an OpenAI-compatible Images Generations endpoint through the provider
configured in Codex. It resolves the active provider URL and credential without accepting API keys
as command-line values.

## Install

```powershell
cargo install --path . --locked
```

The installed binary is named `codex-image`.

## Usage

```powershell
codex-image generate `
  --prompt "A red circle centered on a white background" `
  --model gpt-image-2 `
  --size 1024x1024
```

Images are written to `generated_images/` by default. A successful command prints one compact JSON
summary containing the model, generated paths, and any upstream URLs. PNG, JPEG, WebP, and GIF are
recognized from their file signatures instead of being assigned a fixed extension.

## Edit an Image

Use `edit` to send a source image to the provider's OpenAI-compatible `POST /v1/images/edits`
endpoint. `--mask` is optional; transparent areas identify the regions available for editing.

```powershell
codex-image edit `
  --image .\source.png `
  --mask .\mask.png `
  --prompt "Replace the background with a cloudy sky" `
  --model gpt-image-2
```

Source images and masks must be PNG, JPEG, WebP, or GIF files and are each subject to
`--max-image-mib` (50 MiB by default). The selected provider must support `/v1/images/edits` for
this command.

Generation is bounded to 10 images, 50 MiB per image, and 180 seconds overall by default. Override
the operational limits when needed. Base64 API responses also have an 80 MiB total JSON limit:

```powershell
codex-image generate `
  --prompt "A high-resolution product texture" `
  --n 2 `
  --timeout-seconds 300 `
  --max-image-mib 100 `
  --output-dir generated_images
```

## Provider Resolution

The CLI reads `CODEX_HOME/config.toml` or `~/.codex/config.toml`, selects `model_provider`, and uses
the provider's `base_url` and `env_key`. Providers with `requires_openai_auth = true` fall back to
`auth.json`. OpenAI defaults to `https://api.openai.com/v1`; Ollama and LM Studio default to their
standard local URLs and do not require authentication.

The selected provider must implement `POST /v1/images/generations` and return exactly the requested
number of items using `b64_json` or an HTTP(S) image URL.

Complete command overrides do not require a Codex config file:

```powershell
$env:CODEX_IMAGE_KEY = "..."
codex-image generate `
  --base-url https://router.example/v1 `
  --api-key-env CODEX_IMAGE_KEY `
  --prompt "A paper-cut mountain landscape"
```

For a trusted local endpoint that needs no Authorization header:

```powershell
codex-image generate `
  --base-url http://127.0.0.1:8080/v1 `
  --no-auth `
  --prompt "A monochrome icon sheet"
```

Automation can use `--prompt-env NAME` instead of `--prompt`; the two options are mutually exclusive.

## Reliability

Responses and downloads have explicit size limits. URL images are streamed to a unique staging
directory, validated, and committed only after the whole batch succeeds. A failed multi-image request
therefore does not leave a partial batch that can be mistaken for success.

## Security

- API keys are read from environment variables or Codex auth and are never accepted as CLI values.
- `--no-auth` should only be used with a trusted local or otherwise protected endpoint.
- Prompts are sent to the configured provider. Direct `--prompt` values may be visible in process
  listings; automation should prefer `--prompt-env`.
- Provider error text is bounded before being printed. A provider may still echo prompt content into
  stderr, so treat failure logs as potentially sensitive.
- Do not commit `.codex/`, `auth.json`, generated images, `.env` files, or `.codex-image-runs/`.

## Codex Skill

The repository includes `skills/codex-image`. Link or copy it to
`~/.codex/skills/codex-image` after installing the CLI. The skill starts a detached generation or
edit worker, records its exit status, validates the CLI JSON summary, and cleans completed run logs
older than seven days when a new run starts.

## Development

```powershell
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tests/test-invoke-codex-image.ps1
```
