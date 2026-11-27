# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# Builds the driver and makes its package: the DLL, the port's INF and the
# modem's, each stamped with the version, and the catalog of all three. Signs the DLL and the catalog if a
# certificate is named. Needs a Windows Driver Kit and LLVM; docs/DRIVER.md
# says how to get them.
#
#   driver\package.ps1 [-Out <dir>] [-Thumbprint <sha1>] [-Profile release]

param(
    # Where the package is made. It is emptied first.
    [string]$Out = (Join-Path $PSScriptRoot 'target\package'),
    # The SHA-1 thumbprint of a code-signing certificate in the current
    # user's or the machine's My store. With none, nothing is signed.
    [string]$Thumbprint = '',
    [ValidateSet('release', 'dev')]
    [string]$Profile = 'release'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

# A tool of the kit, from an installed kit or from the NuGet packages'
# directories, 64-bit where there is one.
function Find-KitTool([string]$Name) {
    $roots = @(
        $env:WDKBinRoot, $env:WDKToolRoot, $env:WindowsSdkBinPath,
        'C:\Program Files (x86)\Windows Kits\10\bin',
        'C:\Program Files (x86)\Windows Kits\10\Tools'
    ) | Where-Object { $_ -and (Test-Path $_) }
    $found = Get-ChildItem $roots -Recurse -Filter $Name -ErrorAction SilentlyContinue |
        Sort-Object { $_.FullName -match '\\x64\\' }, FullName -Descending |
        Select-Object -First 1
    if (-not $found) { throw "$Name was not found in a Windows Driver Kit" }
    $found.FullName
}

function Invoke-Tool([string]$File, [string[]]$Arguments) {
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$(Split-Path $File -Leaf) failed with $LASTEXITCODE" }
}

$version = (Get-Content (Join-Path $root 'VERSION') -Raw).Trim()
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "VERSION is not a version: $version" }

Push-Location $PSScriptRoot
try {
    $cargo = @('build', '--locked', '-p', 'vcom')
    if ($Profile -eq 'release') { $cargo += '--release' }
    Invoke-Tool 'cargo' $cargo
} finally {
    Pop-Location
}
$built = Join-Path $PSScriptRoot ("target\{0}" -f $(if ($Profile -eq 'release') { 'release' } else { 'debug' }))

if (Test-Path $Out) { Remove-Item $Out -Recurse -Force }
New-Item -ItemType Directory $Out | Out-Null
Copy-Item (Join-Path $built 'rmvcom.dll') $Out
Copy-Item (Join-Path $PSScriptRoot 'vcom\rmvcom.inx') (Join-Path $Out 'rmvcom.inf')
# The modem's INF is generated from what the modem answers.
Push-Location $root
try {
    Invoke-Tool 'cargo' @('run', '--locked', '--quiet', '-p', 'modeminf', '--', (Join-Path $Out 'rmmodem.inf'))
} finally {
    Pop-Location
}

# The architecture, the framework's version, today's date and the
# driver's version, which is the product's with a fourth part.
foreach ($inf in 'rmvcom.inf', 'rmmodem.inf') {
    Invoke-Tool (Find-KitTool 'stampinf.exe') @(
        '-f', (Join-Path $Out $inf), '-a', 'amd64', '-u', '2.33.0', '-d', '*', '-v', "$version.0")
}

# Windows Server 2022 and 2025, Windows 11 21H2 and 24H2.
Invoke-Tool (Find-KitTool 'inf2cat.exe') @(
    "/driver:$Out", '/os:ServerFE_X64,Server2025_X64,10_CO_X64,10_GE_X64', '/uselocaltime')

foreach ($inf in 'rmvcom.inf', 'rmmodem.inf') {
    Invoke-Tool (Find-KitTool 'infverif.exe') @('/v', '/u', (Join-Path $Out $inf))
}

if ($Thumbprint) {
    Invoke-Tool (Find-KitTool 'signtool.exe') @(
        'sign', '/fd', 'SHA256', '/sha1', $Thumbprint,
        (Join-Path $Out 'rmvcom.dll'), (Join-Path $Out 'rmvcom.cat'))
}

Get-ChildItem $Out | ForEach-Object { '{0,9} {1}' -f $_.Length, $_.Name }
"package: $Out"
