# platform: any
param(
    [ValidateSet("docker", "host")]
    [string]$BackendMode = "docker",
    [switch]$Fast,
    [string]$WindowsDataRoot = "D:\docker-data\gitgraph-lab"
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $repoRoot
try {
    if ($BackendMode -eq "docker") {
        if ($Fast) {
            & "$PSScriptRoot\quality-gate-docker.ps1" -Fast -WindowsDataRoot $WindowsDataRoot
        } else {
            & "$PSScriptRoot\quality-gate-docker.ps1" -WindowsDataRoot $WindowsDataRoot
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
