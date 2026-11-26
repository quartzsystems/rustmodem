# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# With no arguments: checks that the workspace version in Cargo.toml equals
# the VERSION file. With `prepare <version>`: writes that version to both.

param(
    [string]$Command = "check",
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$versionFile = Join-Path $root "VERSION"
$cargoFile = Join-Path $root "Cargo.toml"

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

$lines = [System.IO.File]::ReadAllLines($cargoFile)
$index = Find-VersionLine $lines
if ($index -lt 0) {
    Write-Output "Cargo.toml has no [workspace.package] version"
    exit 1
}

switch ($Command) {
    "check" {
        $want = ([System.IO.File]::ReadAllText($versionFile)).Trim()
        $got = ""
        if ($lines[$index] -match '"([^"]*)"') { $got = $Matches[1] }
        if ($want -ne $got) {
            Write-Output "version mismatch: VERSION has $want, Cargo.toml has $got"
            exit 1
        }
        Write-Output "version $want"
    }
    "prepare" {
        if ($Version -notmatch '^\d+\.\d+\.\d+$') {
            Write-Output "usage: ci/version.ps1 prepare <major.minor.patch>"
            exit 2
        }
        [System.IO.File]::WriteAllText($versionFile, "$Version`n")
        $lines[$index] = "version = `"$Version`""
        [System.IO.File]::WriteAllText($cargoFile, (($lines -join "`n") + "`n"))
        Write-Output "version set to $Version; run cargo check to refresh Cargo.lock"
    }
    default {
        Write-Output "usage: ci/version.ps1 [check | prepare <version>]"
        exit 2
    }
}
