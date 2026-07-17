---
name: codex-image
description: Generate raster images through the active Codex OpenAI-compatible provider with codex-image-cli. Use when the user asks to generate an image, illustration, texture, sprite, mockup, or other bitmap asset and the result should be saved as a local file.
---

# Codex Image

Use the `codex-image` CLI to call the active Codex provider's Images Generations endpoint.
The CLI resolves the configured base URL and credential itself. Do not read, print, pass, or persist API keys.

## Generate

Run one image by default and save it inside the current workspace:

```powershell
codex-image generate `
  --prompt "<concrete visual description>" `
  --output-dir generated_images
```

Use `--model` only when the user specifies a model. Use `--size` when the requested asset needs a specific aspect ratio or resolution. Use `--n` only when multiple distinct variants are explicitly requested.

After generation, inspect the saved image before using it in an artifact. Report the output path, not the prompt or credential.

## Constraints

- The default model is `gpt-image-2`; preserve an explicit model choice.
- Treat each requested image as a potentially billable operation.
- Do not retry a timed-out or ambiguous request automatically. Confirm the upstream result or ask the user before another generation.
- Do not use this CLI for image edits, masks, variations, or non-OpenAI-compatible providers. Explain the unsupported operation instead.
- If `codex-image` is unavailable, install the CLI from the repository with `cargo install --path .` before retrying.
