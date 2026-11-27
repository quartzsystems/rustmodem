# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# With no arguments: checks that the workspace version in Cargo.toml, and
# in the driver's workspace, driver/Cargo.toml, equals the VERSION file.
# With `prepare <version>`: writes that version to all three.

param(
    [string]$Command = "check",
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$versionFile = Join-Path $root "VERSION"
# The manifests that carry the version.
$manifests = @("Cargo.toml", "driver/Cargo.toml")

# Returns the index of the version line inside [workspace.package], or -1.
function Find-VersionLine([string[]]$lines) {
    $inside = $false
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $line = $lines[$i]
        if ($line -match '^\[workspace\.package\]') { $inside = $true; continue }
        if ($line -match '^\[') { $inside = $false }
        if ($inside -and $line -match '^version\s*=') { return $i }
    }
    return -1
}

if ($Command -ne "check" -and $Command -ne "prepare") {
    Write-Output "usage: ci/version.ps1 [check | prepare <version>]"
    exit 2
}
if ($Command -eq "prepare") {
    if ($Version -notmatch '^\d+\.\d+\.\d+$') {
        Write-Output "usage: ci/version.ps1 prepare <major.minor.patch>"
        exit 2
    }
    [System.IO.File]::WriteAllText($versionFile, "$Version`n")
}
$want = ([System.IO.File]::ReadAllText($versionFile)).Trim()

foreach ($manifest in $manifests) {
    $cargoFile = Join-Path $root $manifest
    $lines = [System.IO.File]::ReadAllLines($cargoFile)
    $index = Find-VersionLine $lines
    if ($index -lt 0) {
        Write-Output "$manifest has no [workspace.package] version"
        exit 1
    }
    if ($Command -eq "check") {
        $got = ""
        if ($lines[$index] -match '"([^"]*)"') { $got = $Matches[1] }
        if ($want -ne $got) {
            Write-Output "version mismatch: VERSION has $want, $manifest has $got"
            exit 1
        }
    } else {
        $lines[$index] = "version = `"$Version`""
        [System.IO.File]::WriteAllText($cargoFile, (($lines -join "`n") + "`n"))
    }
}

if ($Command -eq "check") {
    Write-Output "version $want"
} else {
    Write-Output "version set to $Version; run cargo check here and in driver/ to refresh the lockfiles"
}
