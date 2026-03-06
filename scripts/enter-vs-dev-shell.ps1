param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [string]$CargoTargetRoot = ""
)

$ErrorActionPreference = "Stop"

function Resolve-VsDevCmdPath {
    $vswhere = "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $installationPath = & $vswhere `
            -latest `
            -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath
        if ($LASTEXITCODE -eq 0 -and $installationPath) {
            $candidate = Join-Path $installationPath "Common7\Tools\VsDevCmd.bat"
            if (Test-Path $candidate) {
                return $candidate
            }
        }
    }

    $fallback = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"
    if (Test-Path $fallback) {
        return $fallback
    }

    throw "Could not find VsDevCmd.bat. Install Visual Studio Build Tools with C++ workload."
}

$vsDevCmd = Resolve-VsDevCmdPath
$cmd = '"' + $vsDevCmd + '" -no_logo -arch=' + $Arch + ' -host_arch=' + $Arch + ' && set'
$envLines = & cmd /c $cmd

if ($LASTEXITCODE -ne 0) {
    throw "Failed to initialize Visual Studio developer environment."
}

foreach ($line in $envLines) {
    if ($line -notmatch "=") {
        continue
    }

    $firstEq = $line.IndexOf("=")
    if ($firstEq -le 0) {
        continue
    }

    $name = $line.Substring(0, $firstEq)
    $value = $line.Substring($firstEq + 1)
    Set-Item -Path ("Env:" + $name) -Value $value
}

if ([string]::IsNullOrWhiteSpace($CargoTargetRoot)) {
    $CargoTargetRoot = Join-Path $env:USERPROFILE ".cargo-target\gitgraph-lab"
}
if (-not (Test-Path $CargoTargetRoot)) {
    New-Item -Path $CargoTargetRoot -ItemType Directory -Force | Out-Null
}
$env:CARGO_TARGET_DIR = $CargoTargetRoot

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

Write-Host "VS Dev environment loaded for $Arch. Try: where cl"
Write-Host "CARGO_TARGET_DIR: $($env:CARGO_TARGET_DIR)"
