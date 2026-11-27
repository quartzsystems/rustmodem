# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# Installs RustModem on this machine with the setup program, uses it, and
# removes it, failing at the first thing that is not as it should be. It
# is what CI runs on a machine made for the run and thrown away after.
#
# DO NOT RUN THIS ON A MACHINE THAT MATTERS. It installs a driver, a
# service and a lab certificate, makes devices, and removes RustModem and
# its configuration when it is done.
#
#   ci\install-test.ps1 -Setup target\setup\rustmodem-setup.exe
#
# It needs an administrator, and a lab build of the setup program.

param(
    [Parameter(Mandatory)]
    [string]$Setup
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$Setup = (Resolve-Path $Setup).Path
$installDir = Join-Path $env:ProgramFiles 'Quartz Systems\RustModem'
$dataDir = Join-Path $env:ProgramData 'Quartz Systems\RustModem'
$installed = Join-Path $installDir 'rustmodem-setup.exe'
$failures = 0

function Section([string]$Name) { ''; "==== $Name" }

function Check([string]$What, [bool]$Holds) {
    if ($Holds) { "ok    $What" }
    else { "FAIL  $What"; $script:failures++ }
}

# Runs a program with a time limit, and gives its exit code. What it
# prints is shown. A setup program that asked a question would otherwise
# wait for ever.
function Invoke-Limited([int]$Seconds, [string]$File, [string[]]$Arguments) {
    $out = [IO.Path]::GetTempFileName(); $err = [IO.Path]::GetTempFileName()
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -NoNewWindow -PassThru `
        -RedirectStandardOutput $out -RedirectStandardError $err
    if (-not $process.WaitForExit($Seconds * 1000)) {
        Stop-Process -Id $process.Id -Force
        Get-Content $out, $err | Out-Host
        "    still running after $Seconds seconds, and stopped" | Out-Host
        return -1
    }
    Get-Content $out, $err | ForEach-Object { "    $_" } | Out-Host
    $process.ExitCode
}

function Test-Rule([string]$Name) {
    netsh advfirewall firewall show rule "name=$Name" | Out-Null
    $LASTEXITCODE -eq 0
}

function Get-RustModemDevices {
    Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
        Where-Object { ($_.HardwareID -join ' ') -match 'RUSTMODEM\\' }
}

Section 'what setup would do'
$code = Invoke-Limited 60 $Setup @('plan')
Check 'the plan is printed' ($code -eq 0)

Section 'a silent install that is not told to trust the certificate is refused'
$code = Invoke-Limited 120 $Setup @('/quiet', '--log', 'C:\setup-refused.log')
Check 'exit code 4, cancelled' ($code -eq 4)
Check 'nothing was installed' (-not (Test-Path $installDir))

Section 'install'
$code = Invoke-Limited 600 $Setup @('/quiet', '--trust-lab-certificate', '--log', 'C:\setup-install.log')
Get-Content C:\setup-install.log -ErrorAction SilentlyContinue | ForEach-Object { "    $_" }
Check 'exit code 0' ($code -eq 0)
foreach ($file in 'rustmodem-svc.exe', 'rustmodem-manager.exe', 'rustmodem-ctl.exe', 'rustmodem-setup.exe',
    'driver\rmvcom.dll', 'driver\rmvcom.inf', 'driver\rmmodem.inf', 'driver\rmvcom.cat', 'LICENSE', 'docs\INSTALL.md') {
    Check "installed: $file" (Test-Path (Join-Path $installDir $file))
}
Check 'no file kept from before is left' (-not (Get-ChildItem $installDir -Recurse -Filter *.old))
Check 'a configuration was written' (Test-Path (Join-Path $dataDir 'rustmodem.toml'))
$service = Get-CimInstance Win32_Service -Filter "Name='rustmodem'"
Check 'the service is registered' ($null -ne $service)
Check "the service runs under its own account ($($service.StartName))" ($service.StartName -eq 'NT SERVICE\rustmodem')
Check "the service is running ($($service.State))" ($service.State -eq 'Running')
Check "the service is called RustModem Modem Pool ($($service.DisplayName))" ($service.DisplayName -ceq 'RustModem Modem Pool')
$acl = (Get-Acl $dataDir).Access | ForEach-Object { "$($_.IdentityReference)" }
"    the data directory admits: $($acl -join ', ')"
Check 'the data directory admits the service' ($acl -contains 'NT SERVICE\rustmodem')
Check 'the data directory does not admit ordinary users' (-not ($acl -match 'Users|Everyone'))
Check 'the firewall rule for SIP' (Test-Rule 'RustModem SIP')
Check 'the firewall rule for RTP' (Test-Rule 'RustModem RTP')
$policy = Get-NetQosPolicy -Name 'RustModem RTP' -ErrorAction SilentlyContinue
Check "the QoS policy marks DSCP 46 ($($policy.DSCPAction))" ($policy.DSCPAction -eq 46)
$entry = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\RustModem' -ErrorAction SilentlyContinue
Check "listed as installed: $($entry.DisplayName) $($entry.DisplayVersion)" ($entry.DisplayName -eq 'RustModem')
Check 'the Start menu has the manager' (Test-Path (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\RustModem\RustModem Manager.lnk'))
Check 'the lab certificate is a trusted publisher' ($null -ne (Get-ChildItem Cert:\LocalMachine\TrustedPublisher | Where-Object Subject -eq 'CN=RustModem Lab'))
Check 'the driver package is in the driver store' ((pnputil /enum-drivers | Out-String) -match 'rmvcom\.inf')
$event = Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = 'RustModem' } -MaxEvents 5 -ErrorAction SilentlyContinue |
    Where-Object Id -eq 1 | Select-Object -First 1
"    event 1: $($event.Message -replace '\s+', ' ')"
Check 'the service start is in the Event Log, with its text and no note that the description is missing' `
    ($event -and $event.Message -match 'RMD0001' -and $event.Message -notmatch 'cannot be found')
& (Join-Path $installDir 'rustmodem-ctl.exe') info
Check 'rustmodem-ctl reaches the service' ($LASTEXITCODE -eq 0)

Section 'ports'
$code = Invoke-Limited 300 $installed @('port', 'add', '--com', '40', '--count', '2')
Check 'port add: exit code 0' ($code -eq 0)
$listed = & $installed port list
$listed | ForEach-Object { "    $_" }
Check 'port list has COM40 and COM41, started, with their modems' `
    (($listed -match "^COM40`tRustModem Softmodem( #\d+)?`t.*`tstarted$") -and ($listed -match "^COM41`tRustModem Softmodem( #\d+)?`t.*`tstarted$"))
# Windows numbers the second of a kind: "RustModem Softmodem #2".
$modems = Get-CimInstance Win32_POTSModem | Where-Object Name -like 'RustModem Softmodem*'
Check "Windows lists two RustModem modems, on $($modems.AttachedTo -join ' and ')" (($modems | Measure-Object).Count -eq 2)
Check 'programs that list ports list them' (([System.IO.Ports.SerialPort]::GetPortNames() -contains 'COM40') -and ([System.IO.Ports.SerialPort]::GetPortNames() -contains 'COM41'))
$code = Invoke-Limited 120 $installed @('port', 'add', '--com', '40')
Check 'a number that is taken is refused' ($code -eq 5)

Section 'the conformance suite on a port'
$env:RM_VCOM_PORT = 'COM40'
Push-Location $root
cargo test --locked -p serial --test conformance -- --nocapture --test-threads=1
Check 'the conformance suite passes on COM40' ($LASTEXITCODE -eq 0)
Pop-Location
Remove-Item Env:RM_VCOM_PORT

Section 'a line on a port, through the service'
Add-Content (Join-Path $dataDir 'rustmodem.toml') "`n[[line]]`nid = 1`nserial = { backend = `"vcom`", port = `"COM41`" }`n"
Restart-Service rustmodem
Start-Sleep -Seconds 3
& (Join-Path $installDir 'rustmodem-ctl.exe') lines
# A second and a third client, one after another: the pipe goes on
# serving after its first.
& (Join-Path $installDir 'rustmodem-ctl.exe') info | Out-Null
Check 'the service serves one control client after another' ($LASTEXITCODE -eq 0)
& (Join-Path $installDir 'rustmodem-ctl.exe') info | Out-Null
Check 'and another' ($LASTEXITCODE -eq 0)
"    the service's log:"
Get-Content (Join-Path $dataDir 'logs\rustmodem.log') -Tail 12 -ErrorAction SilentlyContinue | ForEach-Object { "    $_" }
$port = New-Object System.IO.Ports.SerialPort 'COM41', 115200
$port.ReadTimeout = 3000
$port.Open()
$port.Write("ATE0`r"); Start-Sleep -Milliseconds 500; $port.ReadExisting() | Out-Null
$port.Write("ATI6`r"); Start-Sleep -Milliseconds 500
$answer = $port.ReadExisting()
"    ATI6 was answered: $($answer -replace "`r", '<cr>' -replace "`n", '<lf>')"
Check 'the modem on COM41 answers, through the service under its own account' ($answer -match 'RustModem Softmodem' -and $answer -match 'OK')
Check 'the service logged no error' (-not ((Get-Content (Join-Path $dataDir 'logs\rustmodem.log') -Raw) -match ' ERROR '))
Check 'the modem holds DSR and CTS toward the host' ($port.DsrHolding -and $port.CtsHolding)
$port.Close()

Section 'the driver host is killed'
Get-Process WUDFHost -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 8
$port = New-Object System.IO.Ports.SerialPort 'COM41', 115200
$port.Open()
$port.Write("ATE0`r"); Start-Sleep -Milliseconds 800; $port.ReadExisting() | Out-Null
$port.Write("AT`r"); Start-Sleep -Milliseconds 800
$answer = $port.ReadExisting()
$port.Close()
"    AT was answered: $($answer -replace "`r", '<cr>' -replace "`n", '<lf>')"
Check 'the port works again, and the service has it again, with nothing reinstalled' ($answer -match 'OK')

Section 'repair'
Remove-Item (Join-Path $installDir 'rustmodem-ctl.exe')
$code = Invoke-Limited 600 $installed @('repair', '/quiet', '--log', 'C:\setup-repair.log')
Check 'repair: exit code 0' ($code -eq 0)
Check 'the file that was removed is back' (Test-Path (Join-Path $installDir 'rustmodem-ctl.exe'))
Check 'the configuration was kept, with its line' ((Get-Content (Join-Path $dataDir 'rustmodem.toml') -Raw) -match 'COM41')
Check 'the ports were kept' ((& $installed port list | Measure-Object).Count -eq 2)
Check 'the service is running again' ((Get-Service rustmodem).Status -eq 'Running')

Section 'an install over an install is an upgrade that keeps everything'
$code = Invoke-Limited 600 $Setup @('/quiet', '--log', 'C:\setup-upgrade.log')
Check 'exit code 0, with no certificate question: it is trusted already' ($code -eq 0)
Check 'the ports were kept' ((& $installed port list | Measure-Object).Count -eq 2)
Check 'the service is running again' ((Get-Service rustmodem).Status -eq 'Running')

Section 'remove a port'
$code = Invoke-Limited 300 $installed @('port', 'remove', 'COM40')
Check 'port remove: exit code 0' ($code -eq 0)
Check 'one port is left' ((& $installed port list | Measure-Object).Count -eq 1)
Check 'COM40 is no longer listed to programs' (-not ([System.IO.Ports.SerialPort]::GetPortNames() -contains 'COM40'))

Section 'uninstall, keeping the configuration'
$code = Invoke-Limited 600 $installed @('uninstall', '/quiet', '--log', 'C:\setup-uninstall.log')
Get-Content C:\setup-uninstall.log -ErrorAction SilentlyContinue | ForEach-Object { "    $_" }
Check 'uninstall: exit code 0' ($code -eq 0)
Start-Sleep -Seconds 8
Check 'no RustModem device is left' (-not (Get-RustModemDevices))
Check 'no RustModem modem is left' (-not (Get-CimInstance Win32_POTSModem | Where-Object Name -like 'RustModem Softmodem*'))
Check 'the driver package is out of the driver store' (-not ((pnputil /enum-drivers | Out-String) -match 'rmvcom\.inf|rmmodem\.inf'))
Check 'the service is gone' (-not (Get-Service rustmodem -ErrorAction SilentlyContinue))
Check 'the firewall rules are gone' (-not (Test-Rule 'RustModem SIP') -and -not (Test-Rule 'RustModem RTP'))
Check 'the QoS policy is gone' (-not (Get-NetQosPolicy -Name 'RustModem RTP' -ErrorAction SilentlyContinue))
Check 'the certificate is out of both stores' (-not (Get-ChildItem Cert:\LocalMachine\TrustedPublisher, Cert:\LocalMachine\Root | Where-Object Subject -eq 'CN=RustModem Lab'))
Check 'it is no longer listed as installed' (-not (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\RustModem'))
Check 'the event source is gone' (-not (Test-Path 'HKLM:\SYSTEM\CurrentControlSet\Services\EventLog\Application\RustModem'))
Check 'the Start menu folder is gone' (-not (Test-Path (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\RustModem')))
if (Test-Path $installDir) { Get-ChildItem $installDir -Recurse | ForEach-Object { "    left: $($_.FullName)" } }
Check 'the install directory is gone' (-not (Test-Path $installDir))
Check 'the configuration is kept' (Test-Path (Join-Path $dataDir 'rustmodem.toml'))

Section 'install again over the kept configuration, and remove everything'
$code = Invoke-Limited 600 $Setup @('/quiet', '--trust-lab-certificate', '--log', 'C:\setup-again.log')
Check 'install: exit code 0' ($code -eq 0)
Check 'the configuration that was kept is the one in use' ((Get-Content (Join-Path $dataDir 'rustmodem.toml') -Raw) -match 'COM41')
$code = Invoke-Limited 600 $installed @('uninstall', '--purge', '/quiet')
Check 'uninstall --purge: exit code 0' ($code -eq 0)
Start-Sleep -Seconds 8
Check 'the configuration and the logs are gone' (-not (Test-Path $dataDir))
Check 'the install directory is gone' (-not (Test-Path $installDir))
$code = Invoke-Limited 120 $Setup @('uninstall', '/quiet')
Check 'an uninstall with nothing installed says so: exit code 7' ($code -eq 7)

''
if ($failures -gt 0) { "$failures checks FAILED"; exit 1 }
'every check passed'
# The last program run above ended with a code of its own, which is not
# this script's.
exit 0
