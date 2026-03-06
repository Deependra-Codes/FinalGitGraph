param(
    [ValidateSet("docker", "host")]
    [string]$BackendMode = "docker",
    [switch]$Fast
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $repoRoot
try {
    if ($BackendMode -eq "docker") {
        if ($Fast) {
            & "$PSScriptRoot\quality-gate-docker.ps1" -Fast
        } else {
            & "$PSScriptRoot\quality-gate-docker.ps1"
        }
        exit $LASTEXITCODE
    }

    if ($Fast) {
        & "$PSScriptRoot\quality-gate.ps1" -Fast
    } else {
        & "$PSScriptRoot\quality-gate.ps1"
    }
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
