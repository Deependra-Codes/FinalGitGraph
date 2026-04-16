# platform: any
param(
    [string]$Image = "rust:1.94",
    [switch]$Fast,
    [string]$WindowsDataRoot = "D:\docker-data\gitgraph-lab"
)

$ErrorActionPreference = "Stop"

function Normalize-AbsolutePath {
    param([string]$PathValue)

    return [System.IO.Path]::GetFullPath($PathValue).TrimEnd('\', '/')
}

function Assert-WindowsPathOnDDrive {
    param(
        [string]$PathValue,
        [string]$Label
    )

    $normalized = Normalize-AbsolutePath -PathValue $PathValue
    if ($normalized -notmatch '^[dD]:\\') {
        throw ("{0} must be on D: drive. Received: {1}" -f $Label, $normalized)
    }
}

function Get-DockerCacheMountArgs {
    param(
        [string]$RepoRootPath,
        [string]$DataRootPath
    )

    if ($env:OS -eq "Windows_NT") {
        if (-not (Test-Path "D:\")) {
            throw "D: drive is required on Windows for Docker quality gate."
        }
        Assert-WindowsPathOnDDrive -PathValue $RepoRootPath -Label "Repository root"
        Assert-WindowsPathOnDDrive -PathValue $DataRootPath -Label "Docker data root"

        $cacheRoot = Normalize-AbsolutePath -PathValue $DataRootPath
        $registryRoot = Join-Path $cacheRoot "cargo-registry"
        $cargoGitRoot = Join-Path $cacheRoot "cargo-git"
        $rustupRoot = Join-Path $cacheRoot "rustup"
        $targetRoot = Join-Path $cacheRoot "cargo-target"
        $tempRoot = Join-Path $cacheRoot "cargo-temp"

        foreach ($dir in @($cacheRoot, $registryRoot, $cargoGitRoot, $rustupRoot, $targetRoot, $tempRoot)) {
            if (-not (Test-Path $dir)) {
                New-Item -Path $dir -ItemType Directory -Force | Out-Null
            }
        }

        return @(
            "-v", "${RepoRootPath}:/work",
            "-v", "${registryRoot}:/usr/local/cargo/registry",
            "-v", "${cargoGitRoot}:/usr/local/cargo/git",
            "-v", "${rustupRoot}:/usr/local/rustup",
            "-v", "${targetRoot}:/cargo-target",
            "-v", "${tempRoot}:/cargo-temp"
        )
    }

    return @(
        "-v", "${RepoRootPath}:/work",
        "-v", "gitgraph_cargo_registry:/usr/local/cargo/registry",
        "-v", "gitgraph_cargo_git:/usr/local/cargo/git",
        "-v", "gitgraph_rustup:/usr/local/rustup",
        "-v", "gitgraph_target:/cargo-target"
    )
}

$repoRoot = Normalize-AbsolutePath -PathValue (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

docker info --format "{{.ServerVersion}}" *> $null
if ($LASTEXITCODE -ne 0) {
    throw "Docker daemon is not running. Start Docker Desktop first."
}

$steps = @(
    "cargo fmt --all -- --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "cargo check --workspace --locked"
)

if (-not $Fast) {
    $steps += "cargo test --workspace --locked"
}

$command = ($steps -join " && ")
$mountArgs = Get-DockerCacheMountArgs -RepoRootPath $repoRoot -DataRootPath $WindowsDataRoot

$dockerArgs = @(
    "run",
    "--rm",
    "--security-opt",
    "no-new-privileges:true"
)
$dockerArgs += $mountArgs
$dockerArgs += @(
    "-e",
    "CARGO_TARGET_DIR=/cargo-target",
    "-e",
    "RUSTUP_HOME=/usr/local/rustup",
    "-e",
    "TMPDIR=/cargo-temp",
    "-e",
    "PATH=/usr/local/cargo/bin:/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "-w",
    "/work",
    $Image,
    "bash",
    "-c",
    $command
)

& docker @dockerArgs
if ($LASTEXITCODE -ne 0) {
    throw "Docker quality gate failed."
}

Write-Host ""
Write-Host "Docker quality gate passed."
