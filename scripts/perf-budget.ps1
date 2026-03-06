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

function Test-DockerReady {
    $dockerCommand = Get-Command "docker" -ErrorAction SilentlyContinue
    if ($null -eq $dockerCommand) {
        return $false
    }
    docker info --format "{{.ServerVersion}}" *> $null
    return $LASTEXITCODE -eq 0
}

function Invoke-PerfBudgetDocker([string]$RepoRootPath) {
    Write-Host "Retrying performance budget in Docker fallback..."
    $command = @(
        "cargo test",
        "--package gitgraph-lab-core",
        "--release",
        "--locked",
        "application::graph_queries::tests::perf_budget_direction_optimizing_bfs",
        "--",
        "--exact",
        "--ignored",
        "--nocapture"
    ) -join " "

    docker run --rm `
        -v "${RepoRootPath}:/work" `
        -v "gitgraph_cargo_registry:/usr/local/cargo/registry" `
        -v "gitgraph_cargo_git:/usr/local/cargo/git" `
        -v "gitgraph_rustup:/usr/local/rustup" `
        -v "gitgraph_target:/cargo-target" `
        -e "CARGO_TARGET_DIR=/cargo-target" `
        -e "RUSTUP_HOME=/usr/local/rustup" `
        -w /work `
        rust:1.94 `
        bash -c $command
    if ($LASTEXITCODE -ne 0) {
        throw "Performance budget check failed in Docker fallback."
    }
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

    Write-Host "Running graph traversal performance budget..."
    $hostStdoutLog = New-TemporaryFile
    $hostStderrLog = New-TemporaryFile
    try {
        $hostProcess = Start-Process `
            -FilePath "cargo" `
            -ArgumentList @(
                "test",
                "--package",
                "gitgraph-lab-core",
                "--release",
                "--locked",
                "application::graph_queries::tests::perf_budget_direction_optimizing_bfs",
                "--",
                "--exact",
                "--ignored",
                "--nocapture"
            ) `
            -NoNewWindow `
            -Wait `
            -PassThru `
            -RedirectStandardOutput $hostStdoutLog.FullName `
            -RedirectStandardError $hostStderrLog.FullName

        $hostStdout = Get-Content -Path $hostStdoutLog.FullName -Raw
        $hostStderr = Get-Content -Path $hostStderrLog.FullName -Raw
        $hostOutputText = (($hostStdout, $hostStderr) -join [Environment]::NewLine).TrimEnd()
        if (-not [string]::IsNullOrWhiteSpace($hostOutputText)) {
            Write-Host $hostOutputText
        }
    }
    finally {
        Remove-Item -Path $hostStdoutLog.FullName -ErrorAction SilentlyContinue
        Remove-Item -Path $hostStderrLog.FullName -ErrorAction SilentlyContinue
    }

    if ($hostProcess.ExitCode -ne 0) {
        $isPolicyBlocked = $hostOutputText -match "Application Control policy has blocked this file" `
            -or $hostOutputText -match "os error 4551"

        if ($isPolicyBlocked -and (Test-DockerReady)) {
            Invoke-PerfBudgetDocker -RepoRootPath $repoRootPath
            return
        }

        throw "Performance budget check failed."
    }
    Write-Host "Performance budget passed."
}
finally {
    Pop-Location
}
