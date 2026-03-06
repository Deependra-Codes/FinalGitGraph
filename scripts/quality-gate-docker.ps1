param(
    [string]$Image = "rust:1.94",
    [switch]$Fast
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

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

docker run --rm `
    -v "${repoRoot}:/work" `
    -v "gitgraph_cargo_registry:/usr/local/cargo/registry" `
    -v "gitgraph_cargo_git:/usr/local/cargo/git" `
    -v "gitgraph_rustup:/usr/local/rustup" `
    -v "gitgraph_target:/cargo-target" `
    -e "CARGO_TARGET_DIR=/cargo-target" `
    -e "RUSTUP_HOME=/usr/local/rustup" `
    -w /work `
    $Image `
    bash -c $command

if ($LASTEXITCODE -ne 0) {
    throw "Docker quality gate failed."
}

Write-Host ""
Write-Host "Docker quality gate passed."
