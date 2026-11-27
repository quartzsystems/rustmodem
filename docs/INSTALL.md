# Installing RustModem

RustModem is installed by one program, `rustmodem-setup.exe`. It puts the
programs in place, installs the port driver, registers the service under
an account of its own, opens Windows Firewall for SIP and RTP, has Windows
mark the service's RTP for expedited forwarding, and adds RustModem
Manager to the Start menu. Nothing is installed by hand, and nothing else
needs installing: com0com is not required, though a line can still use it
(`docs\COM0COM.md`).

It needs 64-bit Windows 10 version 1803 or later, or Windows Server 2019
or later, and an account that is an administrator.

**Read this first.** A build of today is a *lab build*: its driver is
signed with a certificate of our own, which the machine has to be told to
trust, and setup asks before it does so. `docs\SIGNING.md` says what that
trust means. Such a build has been installed on GitHub's Windows Server
2022 test machines and nowhere else. It has not been installed on Windows
11, and not on a machine with Secure Boot on; if it turns out that the
driver cannot be installed with Secure Boot on, Secure Boot will have to
be off on the machine RustModem runs on. Try it on a virtual machine with
a snapshot before the machine that matters.

## Install

Run `rustmodem-setup.exe`. Windows asks for an administrator. A lab build
then asks whether to trust its certificate on this computer, and setup
says what it is about to do and asks whether to do it. Answer no to
either and nothing is changed.

Unattended:

```
rustmodem-setup.exe /quiet --trust-lab-certificate
```

`/quiet` asks nothing and shows nothing; the exit code says how it went. A
lab build that is not told `--trust-lab-certificate` installs nothing and
ends with exit code 4.

What setup does, in order. If a step fails, the steps before it are undone
and the machine is left as it was.

| Step | What | Where to see it |
|---|---|---|
| The programs | `rustmodem-svc.exe`, `rustmodem-manager.exe`, `rustmodem-ctl.exe`, a copy of `rustmodem-setup.exe`, the licence and the documentation | `%ProgramFiles%\Quartz Systems\RustModem` |
| The configuration | A first `rustmodem.toml` with no lines and SIP off, if there is none. One that is there is never touched | `%ProgramData%\Quartz Systems\RustModem` |
| Its protection | The data folder admits SYSTEM, administrators and the service's account, and no one else: the configuration holds SIP passwords | `icacls "%ProgramData%\Quartz Systems\RustModem"` |
| The certificate, for a lab build | Into the machine's Trusted Root Certification Authorities and Trusted Publishers | `certlm.msc` |
| The driver | The package of `rmvcom.inf` and `rmmodem.inf` into Windows' driver store | `pnputil /enum-drivers` |
| The service | `rustmodem`, shown as "RustModem Modem Pool", automatic start, under the virtual account `NT SERVICE\rustmodem` | `services.msc` |
| The event source | "RustModem" in the Application log | Event Viewer |
| The firewall | Two inbound rules for `rustmodem-svc.exe` alone, "RustModem SIP" and "RustModem RTP", on the UDP ports of the configuration | Windows Defender Firewall with Advanced Security |
| QoS | A policy, "RustModem RTP", that marks UDP from `rustmodem-svc.exe` with DSCP 46 | `Get-NetQosPolicy` |
| The Start menu | RustModem Manager | |
| Installed apps | RustModem, with its uninstaller | Settings, Apps |

The service is started at the end. With the first configuration it has no
lines and takes no calls: it is waiting to be given some.

The service has no administrator rights. Its account can read and write
its own data folder, open the control side of RustModem's ports and listen
on its sockets, and that is all it is given.

## Give it lines

In **RustModem Manager**, from the Start menu. It needs administrator
rights to talk to the service and offers to get them.

1. **Lines, New line.** "RustModem: create a new port" is already chosen.
   Give the line a name if you like and press OK. Windows asks for an
   administrator once more, for the setup program, which makes the port
   and its modem; the line is then added on it. **Tools, Ports and
   drivers** lists the ports, and adds and removes them without a line.
2. **Tools, Settings**, the SIP page, for the network the calls come
   from, as `docs\TELEPHONY.md` describes. After a change to the SIP port
   or the RTP range, run `rustmodem-setup repair` so that the firewall
   rules follow.

Or from an elevated prompt:

```
rustmodem-setup port add --count 4
rustmodem-ctl ports
```

and a `[[line]]` in the configuration for each port it made:

```
[[line]]
id = 1
serial = { backend = "vcom", port = "COM10" }
```

Ports are numbered from `COM10` up unless `--com` says otherwise. Each has
a modem, "RustModem Softmodem", which is what Windows dials and answers
with: `docs\RRAS.md` goes on from there.

## Check

- [ ] `rustmodem-ctl info` prints the version that was installed.
- [ ] `rustmodem-ctl ports` lists each RustModem port with its modem and
      the driver's version, and `rustmodem-ctl lines` each line as idle.
- [ ] Device Manager shows each port under "Ports (COM & LPT)" and each
      modem under "Modems", none with a warning.
- [ ] A terminal program opens a line's port, at any speed, and `AT` is
      answered `OK`.
- [ ] `rustmodem-ctl selftest 1 2` passes: the modems of lines 1 and 2
      call each other inside the service and each sends the other 64 KiB.

## Where things are

| What | Where |
|---|---|
| The programs and the documentation | `%ProgramFiles%\Quartz Systems\RustModem` |
| The configuration | `%ProgramData%\Quartz Systems\RustModem\rustmodem.toml` |
| Stored AT profiles (`AT&W`) | `profiles.toml` beside the configuration |
| The service's log | `%ProgramData%\Quartz Systems\RustModem\logs`, unless `[service] log_dir` names another; `rustmodem-ctl log` prints its end |
| Setup's log | `setup.log` in the same folder, or the file `--log` names |
| Call captures | `captures` under the log folder, unless `[media] capture_dir` names another, when a line's capture is on |

## Upgrade

Run the new `rustmodem-setup.exe`. It stops the service, replaces the
programs and the driver package, and starts the service again. The
configuration, the stored profiles, the ports and the lines are kept.
`CHANGELOG.md` says what a version changes in the configuration, if
anything. A setup program older than what is installed refuses to
install over it.

**From 0.1.0**, which was unpacked from a zip and had its service
registered by hand: run setup as for a new install. It brings the
service's registration to the new programs and the new account, and keeps
the configuration, which is where it was. The folder the zip was unpacked
into is not setup's and is left: delete it. The lines of 0.1.0 are on
com0com pairs and go on working; to move a line to a RustModem port, edit
the line and choose a RustModem port for it. `NEEDS-CODY`: this path is
in the plan's tests and has not been run on a machine that had 0.1.0;
criterion 15 of `docs\PHASE5.md`.

## Repair

```
rustmodem-setup repair
```

puts back any file that is missing or altered, the driver package, the
service's registration, the firewall rules and the QoS policy. It changes
no configuration and removes no port.

## Remove

Settings, Apps, RustModem, Uninstall; or

```
rustmodem-setup uninstall
rustmodem-setup uninstall --purge
```

Uninstalling removes the ports and their modems, the driver package, the
service, the event source, the firewall rules, the QoS policy, the
certificate of a lab build, the Start menu entry and the programs. The
configuration and the logs stay, for an install that may follow, unless
`--purge` is given, which removes them too.

## The setup program's commands

```
rustmodem-setup [install] [options]
rustmodem-setup repair [options]
rustmodem-setup uninstall [--purge] [options]
rustmodem-setup port add [--com N] [--count K]
rustmodem-setup port remove COMn
rustmodem-setup port list
rustmodem-setup plan [install|uninstall] [--purge]
rustmodem-setup --version
```

`plan` prints what an install or an uninstall would do on this machine, in
order, and does nothing. `rustmodem-ctl port add` and
`rustmodem-ctl port remove` are the same port commands, passed on.

| Exit code | Meaning |
|---|---|
| 0 | Done |
| 1 | The arguments were not understood |
| 2 | The setup program is damaged: what it carries does not match its checksum |
| 3 | It needs an administrator and was not given one |
| 4 | Cancelled; nothing was changed |
| 5 | A step failed, and what had been done was undone |
| 6 | This Windows is too old |
| 7 | RustModem is not installed |

Every failure is an `RMD` message, 9000 to 9499 for the driver and the
ports and 9500 to 9999 for setup, with its cause and what to do, on the
console and in setup's log.

## When it does not work

- **Setup seems to hang while installing the driver.** Windows is asking,
  on a desktop nobody is looking at, whether to trust the publisher. It
  should not happen: setup trusts the certificate before it touches the
  driver store, and gives every tool it runs a time limit. If it does,
  it is a finding; send the log.
- **The driver will not install, and the machine has Secure Boot on.**
  This is the untested case above. `docs\SIGNING.md`, "What is established
  and what is not".
- **A port has a warning in Device Manager, or a line cannot open its
  port.** `docs\DRIVER.md`, "Recovering a stuck port".
- **Calls connect and there is no audio.** The firewall rules are for the
  ports the configuration had when setup last ran: `rustmodem-setup
  repair` after changing them.
