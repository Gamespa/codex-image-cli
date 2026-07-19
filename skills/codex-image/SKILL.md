---
name: codex-image
description: Generate or edit raster images through the active Codex OpenAI-compatible provider with codex-image-cli. Use when the user asks to create an image or change an existing image with an optional mask, and the result should be saved as a local file.
---

# Codex Image

Use the `codex-image` CLI to call the active Codex provider's Images Generations or Images Edits endpoint.
The CLI resolves the configured base URL and credential itself. Do not read, print, pass, or persist API keys.

## Generate

Launch one image generation in a detached worker. This avoids false failures when the Codex command runner returns before a slow image request has finished:

```powershell
$promptVariable = "CODEX_IMAGE_SKILL_PROMPT_$PID"
[Environment]::SetEnvironmentVariable($promptVariable, "<concrete visual description>", 'Process')
try {
  powershell -NoProfile -ExecutionPolicy Bypass -File "$HOME/.codex/skills/codex-image/scripts/invoke-codex-image.ps1" `
    -Start `
    -PromptEnv $promptVariable `
    -OutputDir generated_images
} finally {
  [Environment]::SetEnvironmentVariable($promptVariable, $null, 'Process')
}
```

Record the returned `statePath`, then poll it with short commands until it reports a terminal status:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "$HOME/.codex/skills/codex-image/scripts/invoke-codex-image.ps1" `
  -Status `
  -StatePath "<statePath>"
```

Poll with short commands while the status is `running`. The default request timeout is 180 seconds,
and the worker adds a 30-second shutdown grace period. Do not judge a blank foreground result or an
empty output directory as failure. On `failed`, inspect the returned stderr path. On `timed_out`, do
not retry automatically because the final state is ambiguous. On `succeeded`, inspect only the image
paths returned by the status command.

Pass `-Model` only when the user specifies a model. Pass `-Size` when the requested asset needs a
specific aspect ratio or resolution. Pass `-Count` only when multiple distinct variants are explicitly
requested. Use `-TimeoutSeconds` or `-MaxImageMiB` only when the request needs a larger bound.

After generation, inspect the saved image before using it in an artifact. Report the output path, not the prompt or credential.

## Edit

Use `-Edit` with `-Image` to change an existing PNG, JPEG, WebP, or GIF image. Pass `-Mask` only
when the user supplies a mask; transparent mask regions are eligible for editing. The selected
provider must support `POST /v1/images/edits`.

```powershell
$promptVariable = "CODEX_IMAGE_SKILL_PROMPT_$PID"
[Environment]::SetEnvironmentVariable($promptVariable, "<concrete edit instruction>", 'Process')
try {
  powershell -NoProfile -ExecutionPolicy Bypass -File "$HOME/.codex/skills/codex-image/scripts/invoke-codex-image.ps1" `
    -Start `
    -Edit `
    -Image .\source.png `
    -Mask .\mask.png `
    -PromptEnv $promptVariable `
    -OutputDir generated_images
} finally {
  [Environment]::SetEnvironmentVariable($promptVariable, $null, 'Process')
}
```

## Constraints

- The default model is `gpt-image-2`; preserve an explicit model choice.
- The image count is limited to 10. Treat every requested image as a potentially billable operation.
- Treat `timed_out` or a missing final status as ambiguous. Do not retry automatically; retain the state and log paths, then confirm with the user.
- Run state never contains the prompt. Completed state and log files older than seven days are removed when a new run starts.
- Do not use this CLI for variations or non-OpenAI-compatible providers. Explain the unsupported operation instead.
- If `codex-image` is unavailable, install the CLI from the repository with `cargo install --path .` before retrying.
