param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [switch]$Fast,
    [switch]$SkipPreflight,
    [string]$CargoTargetRoot = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}
if ([string]::IsNullOrWhiteSpace($CargoTargetRoot)) {
    $CargoTargetRoot = Join-Path $env:USERPROFILE ".cargo-target\gitgraph-lab"
}
if (-not (Test-Path $CargoTargetRoot)) {
    New-Item -Path $CargoTargetRoot -ItemType Directory -Force | Out-Null
}
$env:CARGO_TARGET_DIR = $CargoTargetRoot

function Invoke-Step {
    param(
        [string]$Title,
        [scriptblock]$Action
    )

    Write-Host ""
    Write-Host $Title
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "Step failed: $Title"
    }
}

Push-Location $repoRoot
try {
    if (-not $SkipPreflight) {
        & "$PSScriptRoot\preflight.ps1" -Arch $Arch
    }

    Invoke-Step -Title "Running rustfmt check..." -Action { cargo fmt --all -- --check }
    Invoke-Step -Title "Running clippy..." -Action { cargo clippy --workspace --all-targets -- -D warnings }
    Invoke-Step -Title "Running cargo check..." -Action { cargo check --workspace --locked }

    if (-not $Fast) {
        Invoke-Step -Title "Running cargo test..." -Action { cargo test --workspace --locked }
    }

    Write-Host ""
    Write-Host "Quality gate passed."
}
finally {
    Pop-Location
}
