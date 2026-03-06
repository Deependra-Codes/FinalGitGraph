param(
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

Push-Location $repoRoot
try {
    Write-Host "Running graph traversal performance budget..."
    cargo test `
        --package gitgraph-lab-core `
        --release `
        --locked `
        application::graph_queries::tests::perf_budget_direction_optimizing_bfs `
        -- `
        --exact `
        --ignored `
        --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "Performance budget check failed."
    }
    Write-Host "Performance budget passed."
}
finally {
    Pop-Location
}
