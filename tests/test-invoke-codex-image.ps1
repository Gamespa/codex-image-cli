[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$wrapper = Join-Path $repositoryRoot 'skills/codex-image/scripts/invoke-codex-image.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ("codex-image-wrapper-test-" + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($testRoot) | Out-Null

function Assert-Equal($Expected, $Actual, [string]$Message) {
    if ($Expected -ne $Actual) {
        throw "$Message Expected '$Expected', got '$Actual'."
    }
}

function Wait-ForResult([string]$StatePath) {
    $result = $null
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        Start-Sleep -Milliseconds 50
        $result = & $wrapper -Status -StatePath $StatePath |
            ConvertFrom-Json
        if ($result.status -ne 'running') {
            return $result
        }
    }
    throw "Wrapper did not finish for state $StatePath"
}

try {
    $fakeScript = Join-Path $testRoot 'fake-image-command.ps1'
    $fakeCommand = Join-Path $testRoot 'fake-image-command.cmd'
    @'
param([Parameter(ValueFromRemainingArguments = $true)] [string[]]$CliArguments)
$ErrorActionPreference = 'Stop'

function Get-Argument([string]$Name) {
    $index = [Array]::IndexOf($CliArguments, $Name)
    if ($index -lt 0 -or $index + 1 -ge $CliArguments.Count) {
        throw "Missing argument $Name"
    }
    $CliArguments[$index + 1]
}

$promptEnvName = Get-Argument '--prompt-env'
$prompt = [Environment]::GetEnvironmentVariable($promptEnvName, [EnvironmentVariableTarget]::Process)
$operation = $CliArguments[0]
$expectedOperation = if ($env:CODEX_IMAGE_EXPECTED_OPERATION) { $env:CODEX_IMAGE_EXPECTED_OPERATION } else { 'generate' }
if ($operation -ne $expectedOperation) {
    [Console]::Error.WriteLine("operation mismatch: $operation")
    exit 8
}
$expectedImage = $env:CODEX_IMAGE_EXPECTED_IMAGE
$expectedMask = $env:CODEX_IMAGE_EXPECTED_MASK
if ($operation -eq 'edit') {
    if ((Get-Argument '--image') -ne $expectedImage) {
        [Console]::Error.WriteLine('image mismatch')
        exit 10
    }
    if ((Get-Argument '--mask') -ne $expectedMask) {
        [Console]::Error.WriteLine('mask mismatch')
        exit 11
    }
}
$outputDir = Get-Argument '--output-dir'
$countIndex = [Array]::IndexOf($CliArguments, '--n')
$count = if ($countIndex -ge 0) { [int]$CliArguments[$countIndex + 1] } else { 1 }
[IO.Directory]::CreateDirectory($outputDir) | Out-Null

$images = @()
for ($index = 1; $index -le $count; $index++) {
    $path = Join-Path $outputDir ("codex-image-fake-$index.png")
    [IO.File]::WriteAllBytes($path, [byte[]](137, 80, 78, 71, 13, 10, 26, 10))
    $images += [ordered]@{ path = $path }
}

if ($prompt -eq 'fail after image') {
    [Console]::Error.WriteLine('intentional failure')
    exit 7
}
if ($prompt -ne $env:CODEX_IMAGE_EXPECTED_PROMPT) {
    [Console]::Error.WriteLine("prompt mismatch: $prompt")
    exit 9
}

[ordered]@{ model = 'fake-model'; images = $images } | ConvertTo-Json -Depth 5 -Compress
exit 0
'@ | Set-Content -LiteralPath $fakeScript -Encoding utf8
    "@powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$fakeScript`" %*" |
        Set-Content -LiteralPath $fakeCommand -Encoding ascii

    $failedStart = & $wrapper `
        -Start -Prompt 'fail after image' -OutputDir (Join-Path $testRoot 'failed') -Command $fakeCommand |
        ConvertFrom-Json
    $failed = Wait-ForResult $failedStart.statePath
    Assert-Equal 'failed' $failed.status 'A non-zero command must fail.'
    Assert-Equal 7 $failed.exitCode 'The child exit code must be retained.'

    $env:CODEX_IMAGE_EXPECTED_PROMPT = 'quoted "value" and trailing\path'
    $successStart = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $wrapper `
        -Start -PromptEnv CODEX_IMAGE_EXPECTED_PROMPT -OutputDir (Join-Path $testRoot 'success') `
        -Count 2 -Command $fakeCommand |
        ConvertFrom-Json
    $success = Wait-ForResult $successStart.statePath
    if ($success.status -ne 'succeeded') {
        $stderr = Get-Content -LiteralPath $success.stderrPath -Raw -ErrorAction SilentlyContinue
        throw "Unexpected success result: $($success | ConvertTo-Json -Depth 8 -Compress); stderr: $stderr"
    }
    Assert-Equal 'succeeded' $success.status 'A valid JSON summary must succeed.'
    Assert-Equal 0 $success.exitCode 'A successful command must retain exit code zero.'
    Assert-Equal 2 @($success.images).Count 'The wrapper must return the expected image count.'

    $sourceImage = Join-Path $testRoot 'source.png'
    $maskImage = Join-Path $testRoot 'mask.png'
    [IO.File]::WriteAllBytes($sourceImage, [byte[]](137, 80, 78, 71, 13, 10, 26, 10))
    [IO.File]::WriteAllBytes($maskImage, [byte[]](137, 80, 78, 71, 13, 10, 26, 10))
    $env:CODEX_IMAGE_EXPECTED_OPERATION = 'edit'
    $env:CODEX_IMAGE_EXPECTED_IMAGE = [IO.Path]::GetFullPath($sourceImage)
    $env:CODEX_IMAGE_EXPECTED_MASK = [IO.Path]::GetFullPath($maskImage)
    $editStart = & $wrapper `
        -Start -Edit -Prompt $env:CODEX_IMAGE_EXPECTED_PROMPT -Image $sourceImage -Mask $maskImage `
        -OutputDir (Join-Path $testRoot 'edit') -Command $fakeCommand |
        ConvertFrom-Json
    $edit = Wait-ForResult $editStart.statePath
    Assert-Equal 'succeeded' $edit.status 'An edit command must use the source image and mask.'
    Assert-Equal 1 @($edit.images).Count 'An edit command must return the requested image count.'
    Remove-Item Env:CODEX_IMAGE_EXPECTED_OPERATION
    Remove-Item Env:CODEX_IMAGE_EXPECTED_IMAGE
    Remove-Item Env:CODEX_IMAGE_EXPECTED_MASK

    $timeoutProcess = Start-Process -FilePath 'powershell.exe' `
        -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 10') `
        -WindowStyle Hidden -PassThru
    try {
        $timeoutStatePath = Join-Path $testRoot 'timeout.state.json'
        [ordered]@{
            outputDir = $testRoot
            expectedCount = 1
            processId = $timeoutProcess.Id
            processStartedAt = $timeoutProcess.StartTime.ToUniversalTime().ToString('o')
            deadlineAt = [DateTime]::UtcNow.AddSeconds(-1).ToString('o')
            timeoutSeconds = 1
            resultPath = (Join-Path $testRoot 'timeout.result.json')
            stdoutPath = (Join-Path $testRoot 'timeout.stdout.log')
            stderrPath = (Join-Path $testRoot 'timeout.stderr.log')
            createdAt = [DateTime]::UtcNow.AddMinutes(-1).ToString('o')
        } | ConvertTo-Json -Depth 5 -Compress |
            Set-Content -LiteralPath $timeoutStatePath -Encoding utf8
        $timeout = & $wrapper -Status -StatePath $timeoutStatePath | ConvertFrom-Json
        Assert-Equal 'timed_out' $timeout.status 'A worker beyond its grace period must time out.'
    }
    finally {
        Stop-Process -Id $timeoutProcess.Id -Force -ErrorAction SilentlyContinue
    }

    $cleanupOutput = Join-Path $testRoot 'cleanup'
    $cleanupRuns = Join-Path $cleanupOutput '.codex-image-runs'
    [IO.Directory]::CreateDirectory($cleanupRuns) | Out-Null
    $staleState = Join-Path $cleanupRuns 'stale.state.json'
    $staleLog = Join-Path $cleanupRuns 'stale.stderr.log'
    $invalidState = Join-Path $cleanupRuns 'invalid.state.json'
    $invalidLog = Join-Path $cleanupRuns 'invalid.stdout.log'
    $orphanLog = Join-Path $cleanupRuns 'orphan.stderr.log'
    [ordered]@{
        processId = 2147483647
        processStartedAt = [DateTime]::UtcNow.AddDays(-10).ToString('o')
    } | ConvertTo-Json -Compress | Set-Content -LiteralPath $staleState -Encoding utf8
    Set-Content -LiteralPath $staleLog -Value 'stale' -Encoding utf8
    Set-Content -LiteralPath $invalidState -Value '{}' -Encoding utf8
    Set-Content -LiteralPath $invalidLog -Value 'invalid' -Encoding utf8
    Set-Content -LiteralPath $orphanLog -Value 'orphan' -Encoding utf8
    (Get-Item -LiteralPath $staleState).LastWriteTimeUtc = [DateTime]::UtcNow.AddDays(-10)
    (Get-Item -LiteralPath $staleLog).LastWriteTimeUtc = [DateTime]::UtcNow.AddDays(-10)
    (Get-Item -LiteralPath $invalidState).LastWriteTimeUtc = [DateTime]::UtcNow.AddDays(-10)
    (Get-Item -LiteralPath $invalidLog).LastWriteTimeUtc = [DateTime]::UtcNow.AddDays(-10)
    (Get-Item -LiteralPath $orphanLog).LastWriteTimeUtc = [DateTime]::UtcNow.AddDays(-10)

    $cleanupStart = & $wrapper -Start -Prompt $env:CODEX_IMAGE_EXPECTED_PROMPT `
        -OutputDir $cleanupOutput -Command $fakeCommand -RetentionDays 7 | ConvertFrom-Json
    $cleanup = Wait-ForResult $cleanupStart.statePath
    Assert-Equal 'succeeded' $cleanup.status 'Cleanup must not affect a new run.'
    Assert-Equal $false (Test-Path -LiteralPath $staleState) 'Expired state must be removed.'
    Assert-Equal $false (Test-Path -LiteralPath $staleLog) 'Expired logs must be removed.'
    Assert-Equal $false (Test-Path -LiteralPath $invalidState) 'Invalid expired state must be removed.'
    Assert-Equal $false (Test-Path -LiteralPath $invalidLog) 'Logs for invalid state must be removed.'
    Assert-Equal $false (Test-Path -LiteralPath $orphanLog) 'Expired orphan logs must be removed.'

    Write-Output 'PowerShell wrapper tests passed.'
}
finally {
    Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
