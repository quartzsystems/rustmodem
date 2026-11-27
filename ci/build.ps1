# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# Builds RustModem whole: the programs, the driver package and its
# catalog, everything signed, and rustmodem-setup.exe with all of it
# inside. Needs a Windows Driver Kit and LLVM for the driver;
# docs\DRIVER.md says how to get them.
#
#   ci\build.ps1 -Thumbprint <sha1> [-LabCertificate <file.cer>] [-Out <dir>]
#
# -Thumbprint       the code-signing certificate to sign with, by its SHA-1
#                   thumbprint, in the current user's or the machine's My
#                   store. With none, nothing is signed, and the driver
#                   will not install.
# -LabCertificate   the certificate of -Thumbprint as a .cer file, for a lab
#                   build: the setup program then carries it, and offers to
#                   make the machine trust it. A build signed for
#                   distribution is made without this.
# -TimestampUrl     an RFC 3161 timestamp server, for a build that is to
#                   outlive its certificate.
#
# The private key is never read by this script: signtool asks the
# certificate store to sign. Nothing here writes a key or a password to
# the output, the log or the setup program.

param(
    [string]$Thumbprint = '',
    [string]$LabCertificate = '',
    [string]$TimestampUrl = '',
    [string]$Out = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (-not $Out) { $Out = Join-Path $root 'target\setup' }
$stage = Join-Path $Out 'stage'

function Invoke-Tool([string]$File, [string[]]$Arguments) {
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$(Split-Path $File -Leaf) failed with $LASTEXITCODE" }
}

function Find-SignTool {
    $roots = @($env:WDKBinRoot, $env:WindowsSdkBinPath, 'C:\Program Files (x86)\Windows Kits\10\bin') |
        Where-Object { $_ -and (Test-Path $_) }
    $found = Get-ChildItem $roots -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $found) { throw 'signtool.exe was not found in a Windows SDK or Driver Kit' }
    $found.FullName
}

function Protect-File([string[]]$Files) {
    if (-not $Thumbprint) { return }
    $arguments = @('sign', '/fd', 'SHA256', '/sha1', $Thumbprint)
    if ($TimestampUrl) { $arguments += @('/tr', $TimestampUrl, '/td', 'SHA256') }
    Invoke-Tool (Find-SignTool) ($arguments + $Files)
}

if ($LabCertificate -and -not $Thumbprint) { throw '-LabCertificate needs -Thumbprint: a lab build is signed' }

Push-Location $root
try {
    Invoke-Tool 'cargo' @('build', '--release', '--locked', '-p', 'svc', '-p', 'ctl', '-p', 'manager', '-p', 'setup')
} finally {
    Pop-Location
}

if (Test-Path $Out) { Remove-Item $Out -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null

# The programs, signed. The setup program is not staged: it is what the
# stage is packed into.
$release = Join-Path $root 'target\release'
$programs = 'rustmodem-svc.exe', 'rustmodem-manager.exe', 'rustmodem-ctl.exe'
foreach ($program in $programs) { Copy-Item (Join-Path $release $program) $stage }
Protect-File ($programs | ForEach-Object { Join-Path $stage $_ })

# The driver package, which signs its own DLL and catalog.
& (Join-Path $root 'driver\package.ps1') -Out (Join-Path $stage 'driver') -Thumbprint $Thumbprint
if ($LabCertificate) { Copy-Item $LabCertificate (Join-Path $stage 'driver\lab.cer') }

# What a person reads, and the configuration a new install starts with.
foreach ($file in 'LICENSE', 'README.md', 'CHANGELOG.md') { Copy-Item (Join-Path $root $file) $stage }
New-Item -ItemType Directory (Join-Path $stage 'docs') | Out-Null
Copy-Item (Join-Path $root 'docs\*.md') (Join-Path $stage 'docs')
Copy-Item (Join-Path $root 'docs\rustmodem.sample.toml') (Join-Path $stage 'docs')
Copy-Item (Join-Path $root 'install\setup\rustmodem.default.toml') $stage

# The setup program packs the stage into a copy of itself, and the copy
# is signed: the signature goes after what it carries.
$setup = Join-Path $Out 'rustmodem-setup.exe'
Invoke-Tool (Join-Path $release 'rustmodem-setup.exe') @('pack', $stage, $setup)
Protect-File @($setup)

'{0:N1} MB  {1}' -f ((Get-Item $setup).Length / 1MB), $setup
if (-not $Thumbprint) { 'NOT SIGNED: the driver in this setup program will not install.' }
elseif ($LabCertificate) { 'A lab build: it asks to trust the RustModem Lab certificate.' }
