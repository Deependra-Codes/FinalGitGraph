# platform: any
param(
    [string]$CargoTargetRoot = "",
    [switch]$ProbeOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$repoRootPath = $repoRoot.Path
$userHome = if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
    $env:USERPROFILE
} elseif (-not [string]::IsNullOrWhiteSpace($env:HOME)) {
    $env:HOME
} else {
    (Resolve-Path "~").Path
}
$pathSeparator = if ($env:OS -eq "Windows_NT") { ";" } else { ":" }

$cargoBin = Join-Path $userHome ".cargo/bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin$pathSeparator$env:Path"
}
if ([string]::IsNullOrWhiteSpace($CargoTargetRoot)) {
    $CargoTargetRoot = Join-Path $userHome ".cargo-target/gitgraph-lab"
}
if (-not (Test-Path $CargoTargetRoot)) {
    New-Item -Path $CargoTargetRoot -ItemType Directory -Force | Out-Null
}
$env:CARGO_TARGET_DIR = $CargoTargetRoot

function Format-ElapsedSeconds {
    param([TimeSpan]$Elapsed)
    return [Math]::Round($Elapsed.TotalSeconds, 3)
}

function Test-DockerReady {
    $dockerCommand = Get-Command "docker" -ErrorAction SilentlyContinue
    if ($null -eq $dockerCommand) {
        return $false
    }
    docker info --format "{{.ServerVersion}}" *> $null
    return $LASTEXITCODE -eq 0
}

function Test-IsPolicyBlocked {
    param([string]$OutputText)
    return $OutputText -match "Application Control policy has blocked this file" `
        -or $OutputText -match "os error 4551"
}

function Invoke-ExternalPhase {
    param(
        [string]$Executable,
        [string[]]$Arguments,
        [string]$PhaseLabel
    )

    Write-Host $PhaseLabel
    $stdoutLog = New-TemporaryFile
    $stderrLog = New-TemporaryFile
    try {
        $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        $exitCode = 1
        try {
            $process = Start-Process `
                -FilePath $Executable `
                -ArgumentList $Arguments `
                -NoNewWindow `
                -Wait `
                -PassThru `
                -RedirectStandardOutput $stdoutLog.FullName `
                -RedirectStandardError $stderrLog.FullName
            $exitCode = $process.ExitCode
        }
        catch {
            $exitCode = 1
        }
        $stopwatch.Stop()

        $stdoutText = ""
        if (Test-Path $stdoutLog.FullName) {
            $stdoutText = Get-Content -Path $stdoutLog.FullName -Raw
        }

        $stderrText = ""
        if (Test-Path $stderrLog.FullName) {
            $stderrText = Get-Content -Path $stderrLog.FullName -Raw
        }

        if (-not [string]::IsNullOrWhiteSpace($stdoutText)) {
            Write-Host $stdoutText.TrimEnd()
        }
        if (-not [string]::IsNullOrWhiteSpace($stderrText)) {
            Write-Host $stderrText.TrimEnd()
        }

        $outputText = ($stdoutText + [Environment]::NewLine + $stderrText).Trim()

        return @{
            ExitCode   = $exitCode
            Duration   = $stopwatch.Elapsed
            OutputText = $outputText
        }
    }
    finally {
        Remove-Item -Path $stdoutLog.FullName -ErrorAction SilentlyContinue
        Remove-Item -Path $stderrLog.FullName -ErrorAction SilentlyContinue
    }
}

function Get-DockerRunArgs {
    param(
        [string]$RepoRootPath,
        [string]$CommandText
    )

    return @(
        "run",
        "--rm",
        "-v",
        "${RepoRootPath}:/work",
        "-v",
        "gitgraph_cargo_registry:/usr/local/cargo/registry",
        "-v",
        "gitgraph_cargo_git:/usr/local/cargo/git",
        "-v",
        "gitgraph_rustup:/usr/local/rustup",
        "-v",
        "gitgraph_target:/cargo-target",
        "-e",
        "CARGO_TARGET_DIR=/cargo-target",
        "-e",
        "RUSTUP_HOME=/usr/local/rustup",
        "-e",
        "PATH=/usr/local/cargo/bin:/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "-w",
        "/work",
        "rust:1.94",
        "bash",
        "-c",
        $CommandText
    )
}

function Invoke-DockerPhase {
    param(
        [string]$RepoRootPath,
        [string]$CommandText,
        [string]$PhaseLabel
    )

    $dockerArgs = Get-DockerRunArgs -RepoRootPath $RepoRootPath -CommandText $CommandText
    return Invoke-ExternalPhase -Executable "docker" -Arguments $dockerArgs -PhaseLabel $PhaseLabel
}

function Invoke-PerfBudgetDocker {
    param([string]$RepoRootPath)

    Write-Host "Retrying performance budget in Docker fallback..."
    $buildCommand = "cargo test --package gitgraph-lab-core --release --locked --test indexer_smoke --no-run"
    $runCommand = @(
        "cargo test",
        "--package gitgraph-lab-core",
        "--release",
        "--locked",
        "--test indexer_smoke",
        "lmdb_index_and_graph_queries_work_end_to_end",
        "--",
        "--exact",
        "--nocapture"
    ) -join " "

    $buildResult = Invoke-DockerPhase `
        -RepoRootPath $RepoRootPath `
        -CommandText $buildCommand `
        -PhaseLabel "Docker build phase (perf budget test binary)..."
    if ($buildResult.ExitCode -ne 0) {
        throw "Performance budget build failed in Docker fallback."
    }

    $executeResult = Invoke-DockerPhase `
        -RepoRootPath $RepoRootPath `
        -CommandText $runCommand `
        -PhaseLabel "Docker execute phase (perf budget test)..."
    if ($executeResult.ExitCode -ne 0) {
        throw "Performance budget execution failed in Docker fallback."
    }
    if ($executeResult.OutputText -match "running 0 tests") {
        throw "Performance budget execution in Docker fallback matched 0 tests. Update the perf budget test selector."
    }

    $totalSeconds = Format-ElapsedSeconds($buildResult.Duration + $executeResult.Duration)
    Write-Host ("Docker build phase duration (s): " + (Format-ElapsedSeconds $buildResult.Duration))
    Write-Host ("Docker execute phase duration (s): " + (Format-ElapsedSeconds $executeResult.Duration))
    Write-Host ("Docker total duration (s): " + $totalSeconds)
    Write-Host "Performance budget passed (Docker fallback)."
}

Push-Location $repoRoot
try {
    if ($ProbeOnly) {
        $cargoCommand = Get-Command "cargo" -ErrorAction SilentlyContinue
        if ($null -eq $cargoCommand) {
            throw "cargo was not found in PATH while probing performance script."
        }
        Write-Host "Performance budget probe passed."
        Write-Host "Home: $userHome"
        Write-Host "Cargo target: $CargoTargetRoot"
        return
    }

    $buildArgs = @(
        "test",
        "--package",
        "gitgraph-lab-core",
        "--release",
        "--locked",
        "--test",
        "indexer_smoke",
        "--no-run"
    )
    $runArgs = @(
        "test",
        "--package",
        "gitgraph-lab-core",
        "--release",
        "--locked",
        "--test",
        "indexer_smoke",
        "lmdb_index_and_graph_queries_work_end_to_end",
        "--",
        "--exact",
        "--nocapture"
    )

    $buildResult = Invoke-ExternalPhase `
        -Executable "cargo" `
        -Arguments $buildArgs `
        -PhaseLabel "Host build phase (perf budget test binary)..."
    $combinedOutputText = $buildResult.OutputText
    if ($buildResult.ExitCode -ne 0) {
        if ((Test-IsPolicyBlocked $combinedOutputText) -and (Test-DockerReady)) {
            Invoke-PerfBudgetDocker -RepoRootPath $repoRootPath
            return
        }
        throw "Performance budget build failed."
    }

    $executeResult = Invoke-ExternalPhase `
        -Executable "cargo" `
        -Arguments $runArgs `
        -PhaseLabel "Host execute phase (perf budget test)..."
    $combinedOutputText = ($combinedOutputText + [Environment]::NewLine + $executeResult.OutputText).Trim()
    if ($executeResult.ExitCode -ne 0) {
        if ((Test-IsPolicyBlocked $combinedOutputText) -and (Test-DockerReady)) {
            Invoke-PerfBudgetDocker -RepoRootPath $repoRootPath
            return
        }
        throw "Performance budget execution failed."
    }
    if ($executeResult.OutputText -match "running 0 tests") {
        throw "Performance budget execution matched 0 tests. Update the perf budget test selector."
    }

    $totalSeconds = Format-ElapsedSeconds($buildResult.Duration + $executeResult.Duration)
    Write-Host ("Host build phase duration (s): " + (Format-ElapsedSeconds $buildResult.Duration))
    Write-Host ("Host execute phase duration (s): " + (Format-ElapsedSeconds $executeResult.Duration))
    Write-Host ("Host total duration (s): " + $totalSeconds)
    Write-Host "Performance budget passed."
}
finally {
    Pop-Location
}
