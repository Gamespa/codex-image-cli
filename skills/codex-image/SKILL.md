---
name: codex-image
description: Generate raster images through the active Codex OpenAI-compatible provider with codex-image-cli. Use when the user asks to generate an image, illustration, texture, sprite, mockup, or other bitmap asset and the result should be saved as a local file.
---

# Codex Image

Use the `codex-image` CLI to call the active Codex provider's Images Generations endpoint.
The CLI resolves the configured base URL and credential itself. Do not read, print, pass, or persist API keys.

## Generate

Launch one image generation in a detached worker. This avoids false failures when the Codex command runner returns before a slow image request has finished:

```powershell
powershell -ExecutionPolicy Bypass -File "$HOME/.codex/skills/codex-image/scripts/invoke-codex-image.ps1" `
  -Start `
  -Prompt "<concrete visual description>" `
  -OutputDir generated_images
```

Record the returned `statePath`, then poll it with short commands until it reports `succeeded` or `failed`:

```powershell
powershell -ExecutionPolicy Bypass -File "$HOME/.codex/skills/codex-image/scripts/invoke-codex-image.ps1" `
  -Status `
  -StatePath "<statePath>"
```

Keep polling for up to 240 seconds. Do not judge a blank foreground command result or a temporarily empty output directory as failure. On `failed`, inspect the returned log path. On `succeeded`, inspect only the returned image paths before using them.

Pass `-Model` only when the user specifies a model. Pass `-Size` when the requested asset needs a specific aspect ratio or resolution. Pass `-Count` only when multiple distinct variants are explicitly requested.

After generation, inspect the saved image before using it in an artifact. Report the output path, not the prompt or credential.

## Constraints

- The default model is `gpt-image-2`; preserve an explicit model choice.
- Treat each requested image as a potentially billable operation.
- Treat a missing status result after 240 seconds as ambiguous. Do not retry it automatically; retain the state and log files, then confirm with the user.
- Do not use this CLI for image edits, masks, variations, or non-OpenAI-compatible providers. Explain the unsupported operation instead.
- If `codex-image` is unavailable, install the CLI from the repository with `cargo install --path .` before retrying.
