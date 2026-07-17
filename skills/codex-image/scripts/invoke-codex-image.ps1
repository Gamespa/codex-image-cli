[CmdletBinding(DefaultParameterSetName = 'Start')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Start')]
    [switch]$Start,

    [Parameter(Mandatory, ParameterSetName = 'Status')]
    [switch]$Status,

    [Parameter(Mandatory, ParameterSetName = 'Worker')]
    [switch]$Worker,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Prompt,

    [Parameter(ParameterSetName = 'Start')]
    [string]$PromptEnv,

    [Parameter(Mandatory, ParameterSetName = 'Start')]
    [string]$OutputDir,

    [Parameter(Mandatory, ParameterSetName = 'Status')]
    [string]$StatePath,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Model,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Size,

    [Parameter(ParameterSetName = 'Start')]
    [ValidateRange(1, 10)]
    [int]$Count = 1,

    [Parameter(ParameterSetName = 'Start')]
    [ValidateRange(1, 3600)]
    [int]$TimeoutSeconds = 180,

    [Parameter(ParameterSetName = 'Start')]
    [ValidateRange(1, 1024)]
    [int]$MaxImageMiB = 50,

    [Parameter(ParameterSetName = 'Start')]
    [ValidateRange(0, 3650)]
    [int]$RetentionDays = 7,

    [Parameter(ParameterSetName = 'Start')]
    [string]$Command = 'codex-image',

    [Parameter(Mandatory, ParameterSetName = 'Worker')]
    [string]$WorkerPayload
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Json {
    param([Parameter(ValueFromPipeline = $true)] [object]$Value)
    process { $Value | ConvertTo-Json -Depth 8 -Compress }
}

function Read-JsonFile([string]$Path) {
    Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
}

function Write-JsonFile([object]$Value, [string]$Path) {
    $temporaryPath = "$Path.tmp"
    $Value | Write-Json | Set-Content -LiteralPath $temporaryPath -Encoding utf8
    Move-Item -LiteralPath $temporaryPath -Destination $Path -Force
}

function ConvertFrom-Base64Json([string]$Value) {
    $json = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Value))
    $json | ConvertFrom-Json
}

function Test-ExpectedProcess($State) {
    $process = Get-Process -Id $State.processId -ErrorAction SilentlyContinue
    if (-not $process) {
        return $false
    }
    try {
        $startedAt = $process.StartTime
        if ($null -eq $startedAt) {
            return $false
        }
        $startedAt.ToUniversalTime().ToString('o') -eq $State.processStartedAt
    }
    catch {
        $false
    }
}

function Test-PathWithinRoot([string]$Path, [string]$Root) {
    $fullPath = [IO.Path]::GetFullPath($Path)
    $fullRoot = [IO.Path]::GetFullPath($Root).TrimEnd([IO.Path]::DirectorySeparatorChar) +
        [IO.Path]::DirectorySeparatorChar
    $fullPath.StartsWith($fullRoot, [StringComparison]::OrdinalIgnoreCase)
}

function Remove-ExpiredRuns([string]$RunDir, [int]$Days) {
    $cutoff = [DateTime]::UtcNow.AddDays(-$Days)
    Get-ChildItem -LiteralPath $RunDir -File -Filter '*.state.json' -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTimeUtc -lt $cutoff } |
        ForEach-Object {
            $state = $null
            try { $state = Read-JsonFile $_.FullName } catch { }
            $isRunning = $false
            if ($state) {
                try { $isRunning = [bool](Test-ExpectedProcess $state) } catch { }
            }
            if (-not $isRunning) {
                $runId = $_.Name.Substring(0, $_.Name.Length - '.state.json'.Length)
                Get-ChildItem -LiteralPath $RunDir -File -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name.StartsWith("$runId.", [StringComparison]::Ordinal) } |
                    ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
            }
        }

    Get-ChildItem -LiteralPath $RunDir -File -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTimeUtc -lt $cutoff -and $_.Name -notlike '*.state.json' } |
        ForEach-Object {
            $runId = $_.Name.Split('.')[0]
            $relatedState = Join-Path $RunDir "$runId.state.json"
            if (-not (Test-Path -LiteralPath $relatedState -PathType Leaf)) {
                Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
            }
        }
}

if ($Worker) {
    $payload = ConvertFrom-Base64Json $WorkerPayload
    $arguments = @(
        'generate',
        '--prompt-env', [string]$payload.promptEnvName,
        '--output-dir', [string]$payload.outputDir,
        '--timeout-seconds', [string]$payload.timeoutSeconds,
        '--max-image-mib', [string]$payload.maxImageMiB
    )
    if ($payload.model) { $arguments += @('--model', [string]$payload.model) }
    if ($payload.size) { $arguments += @('--size', [string]$payload.size) }
    if ([int]$payload.count -ne 1) { $arguments += @('--n', [string]$payload.count) }

    $exitCode = 1
    try {
        [Environment]::SetEnvironmentVariable(
            [string]$payload.promptEnvName,
            [string]$payload.prompt,
            [EnvironmentVariableTarget]::Process
        )
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        & ([string]$payload.command) @arguments 1> ([string]$payload.stdoutPath) 2> ([string]$payload.stderrPath)
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorActionPreference
        if ($null -eq $exitCode) { $exitCode = 1 }
    }
    catch {
        $_ | Out-String | Set-Content -LiteralPath ([string]$payload.stderrPath) -Encoding utf8
        $exitCode = 1
    }
    finally {
        $ErrorActionPreference = 'Stop'
        [Environment]::SetEnvironmentVariable(
            [string]$payload.promptEnvName,
            $null,
            [EnvironmentVariableTarget]::Process
        )
    }

    Write-JsonFile ([ordered]@{
        exitCode = [int]$exitCode
        completedAt = [DateTime]::UtcNow.ToString('o')
    }) ([string]$payload.resultPath)
    exit ([int]$exitCode)
}

if ($Status) {
    $state = Read-JsonFile $StatePath
    if (Test-ExpectedProcess $state) {
        $deadlineAt = [DateTime]::Parse(
            [string]$state.deadlineAt,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        if ([DateTime]::UtcNow -gt $deadlineAt.ToUniversalTime()) {
            Write-Json ([ordered]@{
                status = 'timed_out'
                statePath = $StatePath
                processId = $state.processId
                reason = 'The worker exceeded the configured timeout and grace period; its final state is ambiguous.'
                stdoutPath = $state.stdoutPath
                stderrPath = $state.stderrPath
            })
            exit
        }
        Write-Json ([ordered]@{
            status = 'running'
            statePath = $StatePath
            processId = $state.processId
            timeoutSeconds = $state.timeoutSeconds
        })
        exit
    }

    if (-not (Test-Path -LiteralPath $state.resultPath -PathType Leaf)) {
        Write-Json ([ordered]@{
            status = 'failed'
            statePath = $StatePath
            reason = 'The worker exited without recording its exit status.'
            stdoutPath = $state.stdoutPath
            stderrPath = $state.stderrPath
        })
        exit
    }

    $result = Read-JsonFile $state.resultPath
    if ([int]$result.exitCode -ne 0) {
        Write-Json ([ordered]@{
            status = 'failed'
            statePath = $StatePath
            exitCode = [int]$result.exitCode
            reason = 'The image command returned a non-zero exit code.'
            stdoutPath = $state.stdoutPath
            stderrPath = $state.stderrPath
        })
        exit
    }

    try {
        $summary = Read-JsonFile $state.stdoutPath
        $images = @($summary.images)
        if ($images.Count -ne [int]$state.expectedCount) {
            throw "Expected $($state.expectedCount) images, found $($images.Count)."
        }
        $imagePaths = @()
        foreach ($image in $images) {
            $imagePath = [string]$image.path
            if (-not (Test-PathWithinRoot $imagePath $state.outputDir)) {
                throw "Image path is outside the requested output directory: $imagePath"
            }
            if (-not (Test-Path -LiteralPath $imagePath -PathType Leaf)) {
                throw "Image path does not exist: $imagePath"
            }
            $imagePaths += [IO.Path]::GetFullPath($imagePath)
        }
    }
    catch {
        Write-Json ([ordered]@{
            status = 'failed'
            statePath = $StatePath
            exitCode = 0
            reason = "The image command returned an invalid success summary: $($_.Exception.Message)"
            stdoutPath = $state.stdoutPath
            stderrPath = $state.stderrPath
        })
        exit
    }

    Write-Json ([ordered]@{
        status = 'succeeded'
        statePath = $StatePath
        exitCode = 0
        images = $imagePaths
        stdoutPath = $state.stdoutPath
        stderrPath = $state.stderrPath
    })
    exit
}

$hasPrompt = $PSBoundParameters.ContainsKey('Prompt')
$hasPromptEnv = $PSBoundParameters.ContainsKey('PromptEnv')
if ($hasPrompt -eq $hasPromptEnv) {
    throw 'Pass exactly one of -Prompt or -PromptEnv.'
}
if ($hasPromptEnv) {
    $Prompt = [Environment]::GetEnvironmentVariable(
        $PromptEnv,
        [EnvironmentVariableTarget]::Process
    )
    if ($null -eq $Prompt) {
        throw "Prompt environment variable '$PromptEnv' is not set."
    }
}

$resolvedCommand = Get-Command $Command -CommandType Application, ExternalScript |
    Select-Object -First 1
if (-not $resolvedCommand) {
    throw "Image command '$Command' was not found."
}

$resolvedOutputDir = [IO.Path]::GetFullPath($OutputDir)
[IO.Directory]::CreateDirectory($resolvedOutputDir) | Out-Null
$runDir = Join-Path $resolvedOutputDir '.codex-image-runs'
[IO.Directory]::CreateDirectory($runDir) | Out-Null
Remove-ExpiredRuns $runDir $RetentionDays

$runId = [Guid]::NewGuid().ToString('N')
$statePath = Join-Path $runDir "$runId.state.json"
$resultPath = Join-Path $runDir "$runId.result.json"
$stdoutPath = Join-Path $runDir "$runId.stdout.log"
$stderrPath = Join-Path $runDir "$runId.stderr.log"
$payload = [ordered]@{
    command = $resolvedCommand.Source
    prompt = $Prompt
    promptEnvName = "CODEX_IMAGE_PROMPT_$runId"
    outputDir = $resolvedOutputDir
    model = $Model
    size = $Size
    count = $Count
    timeoutSeconds = $TimeoutSeconds
    maxImageMiB = $MaxImageMiB
    stdoutPath = $stdoutPath
    stderrPath = $stderrPath
    resultPath = $resultPath
}
$payloadJson = $payload | ConvertTo-Json -Depth 5 -Compress
$payloadBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payloadJson))
$scriptPathBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($PSCommandPath))
$bootstrap = @"
`$scriptPath = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$scriptPathBase64'))
& `$scriptPath -Worker -WorkerPayload '$payloadBase64'
exit `$LASTEXITCODE
"@
$encodedBootstrap = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bootstrap))
$workerArguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', $encodedBootstrap)
$imageProcess = Start-Process -FilePath 'powershell.exe' -ArgumentList $workerArguments `
    -WorkingDirectory (Get-Location) -WindowStyle Hidden -PassThru
$processStartedAt = $imageProcess.StartTime.ToUniversalTime().ToString('o')
$deadlineAt = $imageProcess.StartTime.ToUniversalTime().AddSeconds($TimeoutSeconds + 30).ToString('o')

Write-JsonFile ([ordered]@{
    outputDir = $resolvedOutputDir
    expectedCount = $Count
    processId = $imageProcess.Id
    processStartedAt = $processStartedAt
    deadlineAt = $deadlineAt
    timeoutSeconds = $TimeoutSeconds
    resultPath = $resultPath
    stdoutPath = $stdoutPath
    stderrPath = $stderrPath
    createdAt = [DateTime]::UtcNow.ToString('o')
}) $statePath

Write-Json ([ordered]@{
    status = 'running'
    statePath = $statePath
    processId = $imageProcess.Id
    timeoutSeconds = $TimeoutSeconds
    stdoutPath = $stdoutPath
    stderrPath = $stderrPath
})
