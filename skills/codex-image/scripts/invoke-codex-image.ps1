[CmdletBinding(DefaultParameterSetName = 'Start')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Start')]
    [switch]$Start,

    [Parameter(Mandatory, ParameterSetName = 'Status')]
    [switch]$Status,

    [Parameter(Mandatory, ParameterSetName = 'Start')]
    [string]$Prompt,

    [Parameter(Mandatory, ParameterSetName = 'Start')]
    [string]$OutputDir,

    [Parameter(Mandatory, ParameterSetName = 'Status')]
    [string]$StatePath,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Model,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Size,

    [Parameter(ParameterSetName = 'Start')]
    [ValidateRange(1, 100)]
    [int]$Count = 1,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Command = 'codex-image'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Json {
    param([Parameter(ValueFromPipeline = $true)] [object]$Value)
    process { $Value | ConvertTo-Json -Depth 5 -Compress }
}

function Read-State([string]$Path) {
    Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
}

function ConvertTo-WindowsArgument([string]$Value) {
    if ($Value.Length -eq 0) { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }

    $quoted = [Text.StringBuilder]::new('"')
    $backslashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') {
            $backslashes++
            continue
        }
        if ($character -eq '"') {
            [void]$quoted.Append(('\' * (($backslashes * 2) + 1)))
        } elseif ($backslashes -gt 0) {
            [void]$quoted.Append(('\' * $backslashes))
        }
        [void]$quoted.Append($character)
        $backslashes = 0
    }
    if ($backslashes -gt 0) {
        [void]$quoted.Append(('\' * ($backslashes * 2)))
    }
    [void]$quoted.Append('"')
    $quoted.ToString()
}

function Get-NewImages($State) {
    $known = @($State.initialFiles)
    Get-ChildItem -LiteralPath $State.outputDir -File -Filter 'codex-image-*.png' -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notin $known } |
        Sort-Object LastWriteTime |
        ForEach-Object { $_.FullName }
}

if ($Status) {
    $state = Read-State $StatePath
    $process = Get-Process -Id $state.processId -ErrorAction SilentlyContinue
    $isExpectedProcess = $process -and $process.StartTime.ToUniversalTime().ToString('o') -eq $state.processStartedAt
    if ($isExpectedProcess) {
        Write-Json ([ordered]@{ status = 'running'; statePath = $StatePath; processId = $state.processId })
        exit
    }

    $images = @(Get-NewImages $state)
    if ($images.Count -gt 0) {
        Write-Json ([ordered]@{
            status = 'succeeded'
            statePath = $StatePath
            images = $images
            stdoutPath = $state.stdoutPath
            stderrPath = $state.stderrPath
        })
        exit
    }

    Write-Json ([ordered]@{
        status = 'failed'
        statePath = $StatePath
        reason = 'The image process exited without creating a new image.'
        stdoutPath = $state.stdoutPath
        stderrPath = $state.stderrPath
    })
    exit
}

$resolvedCommand = Get-Command $Command -CommandType Application | Select-Object -First 1
if (-not $resolvedCommand) {
    throw "Image command '$Command' was not found."
}

$resolvedOutputDir = [IO.Path]::GetFullPath($OutputDir)
[IO.Directory]::CreateDirectory($resolvedOutputDir) | Out-Null
$runDir = Join-Path $resolvedOutputDir '.codex-image-runs'
[IO.Directory]::CreateDirectory($runDir) | Out-Null
$runId = [Guid]::NewGuid().ToString('N')
$statePath = Join-Path $runDir "$runId.json"
$stdoutPath = Join-Path $runDir "$runId.stdout.log"
$stderrPath = Join-Path $runDir "$runId.stderr.log"
$initialFiles = @(Get-ChildItem -LiteralPath $resolvedOutputDir -File -Filter 'codex-image-*.png' | ForEach-Object Name)

$arguments = @('generate', '--prompt', $Prompt, '--output-dir', $resolvedOutputDir)
if ($Model) { $arguments += @('--model', $Model) }
if ($Size) { $arguments += @('--size', $Size) }
if ($Count -ne 1) { $arguments += @('--n', $Count) }
$argumentLine = ($arguments | ForEach-Object { ConvertTo-WindowsArgument $_ }) -join ' '

$imageProcess = Start-Process -FilePath $resolvedCommand.Source -ArgumentList $argumentLine -WorkingDirectory (Get-Location) -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -WindowStyle Hidden -PassThru

[ordered]@{
    outputDir = $resolvedOutputDir
    initialFiles = $initialFiles
    processId = $imageProcess.Id
    processStartedAt = $imageProcess.StartTime.ToUniversalTime().ToString('o')
    stdoutPath = $stdoutPath
    stderrPath = $stderrPath
} | Write-Json | Set-Content -LiteralPath $statePath -Encoding utf8

Write-Json ([ordered]@{ status = 'running'; statePath = $statePath; processId = $imageProcess.Id; stdoutPath = $stdoutPath; stderrPath = $stderrPath })
