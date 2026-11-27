# Phase 5 record: driver and installer

Phase 5 gives RustModem a virtual COM port of its own and a setup program.
A line no longer needs com0com: it has one port, with a modem attached,
made by setup or from the manager, and Windows sees a modem on a serial
port. One program, `rustmodem-setup.exe`, installs everything, the driver
among it, and removes everything. The version at the end of the phase is
0.2.0. Nothing is tagged.

All nine work packages are built and committed, and the seven automated
exit criteria pass. The eleven manual ones are a checklist at the end, and
**none of them has been done.** What that leaves unknown is not small:

- **Everything was installed on GitHub's Windows Server 2022 runners and
  nowhere else.** Those machines have Secure Boot off and test-signing on,
  and neither can be changed there. Whether the driver, signed with a
  certificate of our own, installs with Secure Boot on and test-signing
  off is not known. It is expected to, for the reasons in
  `docs/SIGNING.md`, and that is a belief.
- **Nothing was installed on Windows 11**, or on any client edition.
- **Routing and Remote Access has not opened a RustModem port.** The modem
  is listed by Windows and answers every command Windows is known to send
  it. No call has been answered through it by anything but a test.
- **Nothing was installed on the development machine**, as agreed at the
  start. The driver has never been built there either: there is no kit on
  it. Every build of the driver was on a runner.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| Manual exit criteria 8 to 18 | "Manual checklist" below | Run them, on virtual machines with snapshots, 8 and 9 first: they answer the Secure Boot question, and everything else stands on it |
| Secure Boot | `docs/SIGNING.md`, "What is established and what is not" | Asked on 27 November, you said that if the driver needs Secure Boot off, that is accepted. If criteria 8 and 9 fail with it on, `docs/INSTALL.md` needs one sentence changed from "may" to "must", and nothing else changes |
| The signing decision | `docs/SIGNING.md`, "The path to distribution"; criterion 18 | Lab certificate only, or a certificate from a public authority, or certification. The costs there are for deciding with, not quotations |
| Secrets for the release workflow | `.github/workflows/release.yml` | With no `SIGNING_PFX` secret a tagged release is signed with a certificate made for that run and thrown away, which makes a lab build nobody can verify twice. Before tagging, either add the secrets `SIGNING_PFX` and `SIGNING_PFX_PASSWORD` and the variable `SIGNING_IS_LAB`, or decide that a per-run certificate is what 0.2.0 ships with. The workflow has never run |
| Decisions made without asking | "Decisions to confirm" below | Several are visible: the names of things in Windows, what the manager offers, what setup does to a machine that ran 0.1.0 |
| A phase 4 test that fails now and then | `link/v42/tests/lapm.rs`, `any_loss_below_the_threshold_loses_no_data` | It failed once on CI's Linux job, on a commit that touched no modem code, and twice here. Its bound on loss, 0.3, is close enough to where LAPM gives up after ten tries that an unlucky seed gets there: `seed = 184186, loss = 0.26726441511185195, len = 1282`; `seed = 538528, loss = 0.28158749521364035, len = 2528`; `seed = 130172, loss = 0.2553175845259741, len = 1665`. **It was not changed**: it is the modem's, and whether the bound or the retry count is what should move is yours to say. The CI job was run again and passed. It will fail again |
| Three older tests that each failed once under load | `service/svc/tests/peer.rs`, `a_modem_line_carries_data_from_a_peer_whose_clock_is_off`; `service/svc/tests/soak.rs`; `ui/manager/tests/telephony.rs`, `the_sip_and_media_settings_are_sent_and_a_restart_offered_when_one_is_needed` | Each once, here, in a run of the whole workspace with the machine busy, and each passed alone straight afterwards: the peer test three times of three, the soak twice of two, the manager's three of three. The peer test said "what the host wrote the peer read"; the soak, "line 3, host to peer: an octet arrived that was not the one sent", which is the message of the races phase 3 and phase 4 mended and so perhaps a third; the manager's was shown the restart offer after a refusal that should have kept the sheet open. None is of this phase's code, none was changed, and **none is understood** |
| The whole workspace's tests are not reliable on the development machine | | Three runs of `cargo test --workspace` on the finished tree: the first had the three failures above and the LAPM one; the second had none, 1,358 of 1,358; the third, straight after the full rebuild that the new version causes, had nine, all in `ui/manager/tests/` (three: a dialog that did not come up, a service not found again in time) and `service/svc/tests/peer.rs` (six: "what the host wrote the peer read"). All nine passed when their four test programs were run again one at a time on the idle machine. They are tests of real time, with media threads and windows, on a machine whose disk is 99% full and which was doing other things; that is an explanation that fits and was not proved. CI ran the same tree and failed nothing but the pictures that were waiting for their goldens, though its Windows job stops at the first failing test program and so did not reach the peer tests or the soak in those runs. **The first full run of CI on `main` after this record is pushed is the one to read** |
| Two red commits on `main` | "Where the build differs" below | `feat(vcom): UMDF virtual COM port driver`, on the Windows job, from a race in a phase 2 test that the next commit mends; and that mending commit, on the Linux job, from the LAPM test above |
| A mistake of mine on your machine | | While writing the port commands I ran `rustmodem-setup port remove COM99` here to see its message. The setup program asked Windows for an administrator, and you were shown that prompt. Nothing was changed: there was no such port, and nothing else was run. It should not have happened; setup's commands were run only on runners after that |
| The pictures of the manager | `ui/manager/tests/goldens/` | Ports and drivers, New line, Edit line and About changed, and `new-line-port.png` and `remove-line-port.png` are new. The other twenty-five still match. They were taken by CI on a branch, as before. Look at them |
| com0com is still unverified | `docs/COM0COM.md`, criterion 13 | As since phase 0. The conformance suite now gives it a test to pass |
| Still open from earlier phases | `docs/PHASE0.md` to `docs/PHASE4.md`, `docs/RUNTIME.md` | Their manual checklists and `NEEDS-CODY` items are as they were. Phase 4's say the modem has connected only to itself, and that is still so |

## What was built

### 1. The spike

`docs/DRIVER_SPIKE.md`. Six questions, answered by doing each on a runner
with throwaway code on a branch that was then deleted: the driver can be
written in Rust on Microsoft's `windows-drivers-rs` crates as published;
a line is a Ports device with a Modem device attached to it; both are made
through SetupAPI with no wizard and no reboot; a package signed with a
certificate the machine trusts installs silently, and one it does not
trust hangs on a question nobody sees. No stop condition was met.

### 2. `driver/vcomcore`

The port as a state machine with no Windows in it. Reads and writes with
every timeout of `COMMTIMEOUTS`, the wait mask and its events, purges,
breaks, the modem lines, the stored settings, two bounded queues, and the
two sides. It is given the time and gives back completions, so the tests
have a clock of their own: `tests/port.rs` for the rules one at a time,
`tests/integrity.rs` for property tests that no octet is lost, repeated or
reordered under any interleaving of both sides, and `tests/ioctl.rs` for
the two interfaces as Windows asks for them. A property test found a read
with a total timeout of zero left waiting for ever, before the driver
existed to find it the hard way.

### 3. `driver/vcom`

The UMDF 2 driver, `rmvcom.dll`, in a workspace of its own. It makes the
device, the COM name's link and the `SERIALCOMM` value, tells the two
sides apart by the name in the open, checks who is opening the control
side, hands every request to the port and completes requests as the port
says. `driver/vcomapi` has the control interface's codes and layouts, for
the driver and the service both. `driver/package.ps1` makes the package;
the `driver` workflow builds it on every push.

### 4. `dte/serial`

`VcomBackend`, the service's end of a port, and a conformance suite of
nine tests that runs against every backend there is a port for.
`backend = "vcom"` in the configuration.

### 5. The modem

`driver/modeminf` generates the modem's INF from the AT interpreter's own
tables, so that the responses Windows expects are the ones the modem
sends: 300 to 2400 bit/s, with and without error control and compression.
`dte/at` gained what Windows sends a modem it is getting to know: `+GMI`,
`+GMM`, `+GMR`, `+GSN`, `+GCAP`, `+FCLASS`, `#CLS`, `+GCI` and `I1` to
`I7`. `docs/AT.md` has each.

### 6. `install/setup` and `install/sys`

The setup program. An install, an upgrade, a repair and an uninstall are
each a plan: a list of steps, each with its undo, made from what is found
on the machine. A step that fails undoes those before it, last first. The
plan is made and run against a `System` and a `Devices` that are traits,
so every plan and every port command is tested on Linux against fakes.
The real ones are in `install/setup/src/windows.rs`, which runs Windows'
own tools where there is one an administrator could run by hand, and in
`install/sys`, which has the `unsafe` Win32 that has no tool: SetupAPI for
the devices, the COM port database, the registry, the Start menu.

The service now runs under `NT SERVICE\rustmodem`, a virtual account with
no rights but those setup gives it, and has an event source with a message
table, which closes the two markers phase 0 left for this phase. The QoS
policy closes the one phase 2 left.

### 7. Signing and the build

`ci/build.ps1` builds everything, signs it with a certificate named by
thumbprint and packs `rustmodem-setup.exe`. `ci/lab-certificate.ps1` makes
a lab certificate. `ci/install-test.ps1` is run by the `driver` workflow
on every push: on a clean runner it installs silently, makes ports, runs
the conformance suite on one, puts a line on another and talks to its
modem through the service, kills the driver host and talks again,
repairs, upgrades, removes a port, uninstalls, and looks each time at what
Windows has: some seventy checks. `docs/SIGNING.md` is the account of
signing.

### 8. The manager and the command line

Ports and drivers lists RustModem's ports first, with each one's modem,
driver version and state, and has Add port and Remove port. New line
offers to make a port, which is what it starts on where ports can be made,
or to use a RustModem port there is, or a com0com pair. Remove line, for a
line on a RustModem port, asks whether the port is to go too. The manager
does not become an administrator for any of it: it runs the setup program
as one, and Windows asks. `rustmodem-ctl ports` has the driver's version,
and `rustmodem-ctl port add` and `port remove` pass their words to the
setup program.

### 9. This record

And `docs/DRIVER.md`, `docs/INSTALL.md` written again, `docs/RRAS.md` with
nothing installed by hand, `docs/COM0COM.md` as the alternative,
`docs/TELEPHONY.md` with the firewall rules and the QoS policy as setup
makes them, `CLAUDE.md`, `README.md`, `CHANGELOG.md`, and the version.

### Commits that are not a work package

| Commit | Why |
|---|---|
| `test(media): wait for the wake that follows the stall event` | A race in a phase 2 test, lost by the Windows runner on the driver's commit |
| `refactor(ctl): rustmodem-ctl.exe` | The handoff's rename, by itself: 34 files |
| `refactor: the product name is RustModem wherever a person reads it` | The handoff's rule for the name, by itself: 47 files, none of them behaviour |
| `fix(vcom): a driver host for each port` | Found while writing criterion 16 into this record: see "Where the build differs" |

## Markers

Every `BLOCKED` and `NEEDS-CODY` in the tree. Every `BLOCKED(phase5)` is
gone.

| Marker | Location | What is deferred |
|---|---|---|
| `BLOCKED(phase7)` | `dsp/chansim/src/lib.rs` | The simulator has no group delay and no nonlinear distortion: V.34 needs them |
| `BLOCKED(phase8)` | `dsp/chansim/src/lib.rs` | The simulator has no robbed-bit signalling: V.90 needs it |
| `BLOCKED(phase9)` | `ui/manager/src/dialogs/connect.rs`, `service/ctlsrv/src/lib.rs` | From phases 0 and 1: another computer's service, and TLS for it |
| `NEEDS-CODY` | This record | The manual checklist; the decisions; the two tests |
| `NEEDS-CODY` | `docs/SIGNING.md` (two) | Secure Boot; the signing decision and what each way costs |
| `NEEDS-CODY` | `docs/DRIVER_SPIKE.md` (five) | The same Secure Boot question, and what a runner could not show |
| `NEEDS-CODY` | `docs/DRIVER.md` | The conformance suite on com0com |
| `NEEDS-CODY` | `docs/INSTALL.md` | The upgrade from 0.1.0 on a machine that had it |
| `NEEDS-CODY` | `docs/TELEPHONY.md` | The DSCP mark on the wire |
| `NEEDS-CODY` | `docs/RRAS.md` (eight) | The wording of Windows' dialogs, from phase 4, and Query Modem's |
| `NEEDS-CODY` | `dte/serial/src/com0com.rs` (three), `service/svc/src/ports.rs`, `docs/COM0COM.md` | From phases 0 and 1: com0com on a real pair |
| `NEEDS-CODY` | `docs/GATEWAYS.md`, `docs/RUNTIME.md`, `docs/PHASE0.md` to `docs/PHASE4.md` | As they were |

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| The driver's CI is a job of the `ci` workflow | A workflow of its own, `driver.yml` | It needs an image with a kit, takes twenty minutes and installs a driver. The `ci` workflow's jobs stay what they were |
| A control interface beside the serial one | The same device, opened as `\\.\COMn\control` | A second device interface would be a second thing to secure. The name in the open tells the sides apart, and the open is where the caller is checked. The device interface is there for finding ports |
| Device model A or B, by the spike | B | `docs/DRIVER_SPIKE.md`, section 3 |
| `rustmodem-gui.exe` becomes `rustmodem-manager.exe` | Nothing to rename | It has been `rustmodem-manager.exe` since phase 1. `rustmodemctl.exe` became `rustmodem-ctl.exe` |
| Setup is a Windows program | A console program with the manifest of a windowed one | It has to print for `/quiet`, `plan` and `port list`, and it shows task dialogs when it asks. One program does both |
| The payload is compressed | It is not | The crates that compress are C, or would be a new dependency for a download of a few megabytes. It is checked with SHA-256 |
| The release is a zip | The release is `rustmodem-setup.exe`, signed | The zip of 0.1.0 had no driver and could have none |
| The modem is installed in work package 5 | Its INF is made there; the device is made by setup, in 6 | Making a device is setup's, and setup did not exist yet |
| The service's display name is "RustModem" | "RustModem Modem Pool" | The service control manager drops a display name that is the service's name in another case, and shows `rustmodem` |
| Automatic port numbers | From `COM10` up | Below that are the numbers hardware and other software expect to find free |
| Unknown control codes | `STATUS_INVALID_PARAMETER` | What Windows' serial driver answers |
| `EV_RING` | Raised when RI goes high | The Win32 documentation says "a ring indicator was detected", and no more |
| DTR and RTS at the open | Follow the flow control settings | As a serial port's do |
| Every port in the driver host Windows gives it | A host process for each port | Criterion 16 asks that killing the host "drops that call only". Windows puts the devices of one driver in one host unless told otherwise, and then it drops every RustModem call. Found while writing the checklist, after the driver's commit; `UmdfHostProcessSharing = ProcessSharingDisabled` in the INF, and a check in the install test that there is a host for each port. It costs a process a port |
| Removing a line offers to remove its port | It does where the port is RustModem's and the setup program is there | A com0com pair is not the manager's to remove |
| Goldens are re-blessed | The manager's commit went to a branch first; CI took the pictures; the branch was deleted | As in the earlier phases |
| Every commit on `main` is green | Two were red | `feat(vcom): UMDF virtual COM port driver` on the Windows job: `service/media/tests/threaded.rs` looked for the wake that follows a stall before the waker had counted it. Phase 2's test, mended in the commit after. That commit, `test(media)`, on the Linux job: the LAPM property test of "What needs Cody" |
| Nothing privileged in the service | So | The service makes no device, opens no firewall and trusts no certificate. Setup does, and the manager and `rustmodem-ctl` ask setup |

## The backends compared

What the conformance suite found different, and what it could not look at.

| | Memory | RustModem port | com0com |
|---|---|---|---|
| The nine tests | Pass, with timeouts, wait masks and purges skipped: a memory port has no driver to have them | Pass, all nine, on a runner | Not run. `NEEDS-CODY`, criterion 13 |
| How the modem learns the host opened the port | It is told | An event from the driver | DTR going high, if the pair is wired so |
| The host's baud rate | None | Stored, reported to the service, and unused | Stored by com0com; the pair can be told to emulate it, and must not be |
| Ten mebibytes each way | Intact | Intact | |
| A port whose other side has gone | | The host sees its lines low; the port goes on when the service returns | |
| One name or two | | One: host and service open the same `COMn` | Two: one end each |
| A modem in Windows | | Made with the port | Added by hand |

## Tests whose expectations changed

- `dte/at/tests/table.rs` and transcripts 34 and 38: `I1` to `I7` have
  answers of their own, and the tests of unknown commands used `+FCLASS`
  and `+GMI` as their unknown commands, which are known now. Transcript 61
  is new.
- `ATI3` answers `RustModem` and the version where it answered
  `rustmodem`. `ATI0`, the product code, is unchanged.
- `service/config/tests/`: `vcom` is a backend. `service/msg`: the ranges
  9000 to 9499 and 9500 to 9999.
- `service/ctlproto/tests/api.rs`, `service/ctlsrv/tests/`,
  `service/pipe/tests/api.rs`, `tools/ctl/src/tests.rs`: a port has a kind
  `vcom` and may have a `driver_version`; `rustmodem-ctl ports` has a
  DRIVER column.
- `service/pipe/src/listener.rs`: the control pipe admits its owner as
  well as SYSTEM and administrators. The owner is the service's account,
  which makes each further instance of the pipe.
- `tools/demosvc`: the demo machine has two RustModem ports, `COM40` and
  `COM41`, beside its pairs.
- `ui/manager/tests/dialogs.rs`: New line starts on a RustModem port, so
  the tests of lines on pairs choose the pairs first. Four tests are new.
  `smoke.rs` presses Add port in every round. `screenshots.rs` has
  something standing in for the setup program, so that the pictures are
  of RustModem as installed, and takes two more.
- `service/media/tests/threaded.rs`: the race above.
- Every test that named `rustmodemctl`, or the data directory, or the
  product in a message.

## Decisions to confirm

Each is a choice the handoff left open or a place where the build went
another way. Say if any should change.

**What things are called**

- The port is "RustModem Port (COM10)" in Device Manager and the modem
  "RustModem Softmodem" everywhere. The modem answers `+GMI` with "Quartz
  Systems" and `+GMM` with "RustModem Softmodem".
- The service is `rustmodem`, shown as "RustModem Modem Pool". Its account
  is `NT SERVICE\rustmodem`.
- The lab certificate is "RustModem Lab". The firewall rules are
  "RustModem SIP" and "RustModem RTP", and the QoS policy "RustModem RTP".
- Left in lower case, as identifiers: the service's name, the pipe, the
  programs' file names, the SDP session name `s=rustmodem`, and the
  product code of `ATI0`.

**The driver**

- The control side admits SYSTEM, administrators and the service's
  account. An administrator can therefore be a port's modem, which is what
  lets the conformance suite and `rustmodem-svc --console` run.
- A caller that cannot be impersonated, which is a kernel caller, is
  refused the control side and allowed the host side: Windows' modem
  driver is such a caller.
- One driver host process for each port.
- The queues are 4,096 octets each way unless the host asks otherwise,
  from 64 to 65,536.

**Setup**

- A new install's configuration has no lines and SIP off. Setup makes no
  ports: the first is made with the first line.
- Uninstall removes the ports and keeps the configuration and the logs;
  `--purge` removes those.
- The firewall rules admit any address, for the service's program alone.
  `trusted_peers` is the service's own check.
- The QoS policy marks all UDP the service sends, SIP too.
- Windows 10 version 1803, build 17134, is the oldest Windows setup will
  install on: it is the oldest that takes the INF as written.
- A file that cannot be deleted because Windows holds it, as the event log
  holds the service's program for its messages, is moved to the temporary
  directory and left for Windows to delete.
- Over 0.1.0, setup takes the service's registration over and renames the
  data directory to the product's capitalisation, and leaves the folder
  the zip was unpacked into alone.

**The manager**

- New line starts on "create a new port" where ports can be made, on an
  existing RustModem port where there is one and none can be made, and on
  com0com otherwise.
- A port is made when OK is pressed, after everything else in the dialog
  has been checked, and before the service is asked for the line. If the
  service then refuses the line, the port stays, chosen in the dialog as
  an existing one.
- Remove port is for a port no line uses. A line's port goes with the
  line, under Remove line, which asks; its answer by default is Cancel.
- Add port and Remove port are disabled against the demo service and
  where `rustmodem-setup.exe` is not beside the manager.
- RustModem's ports are listed first in Ports and drivers.

## References consulted

| Reference | For | How |
|---|---|---|
| Everything in "References consulted" of `docs/DRIVER_SPIKE.md` | The spike, and the driver after it | As said there |
| Microsoft's documentation of the serial interface: `COMMTIMEOUTS`, `SetCommMask`, `WaitCommEvent`, `PurgeComm`, `GetCommModemStatus`, `EscapeCommFunction`, `DCB`, and the `IOCTL_SERIAL_` pages with their structures | `driver/vcomcore` | Read on learn.microsoft.com. The codes and bit values are those of the Windows headers, taken from the documentation and from the bindings `wdk-sys` generates |
| Microsoft's documentation of SetupAPI (`SetupDiCreateDeviceInfo`, `SetupDiRegisterDeviceInfo`, `SetupDiCallClassInstaller`, `DiInstallDevice`), of the COM port database (`ComDBClaimPort`), of `New-NetQosPolicy`, `netsh advfirewall`, `certutil`, `pnputil`, `icacls`, virtual service accounts, event message files and `ShellExecuteEx` | `install/sys`, `install/setup`, `ui/win32` | Read on learn.microsoft.com |
| Microsoft's documentation of driver signing, cited page by page in `docs/SIGNING.md` | `docs/SIGNING.md` | Read, November 2026; each page's date is there |
| Microsoft's `Windows-driver-samples`, `serial/VirtualSerial2` and `network/modem/fakemodem` | The driver's shape | During the spike only, as its report says: their READMEs, their INFs, `device.c`, `queue.c`, `driver.c`, for what they do. Microsoft Public License. Nothing was copied: no code, no INF section, no layout, no names. They were not opened again while the driver was written |
| The archived modem INF reference | `driver/modeminf` | Read: the layout of a response entry, and the keys of a modem's INF |
| ITU-T V.250 (07/2003) | `dte/at`: `+GMI`, `+GMM`, `+GMR`, `+GSN`, `+GCAP`, `+GCI` | From `refs/itu/`, clause 6.1 |
| What Windows sends a modem on Query Modem | `dte/at`, transcript 61 | The strings of `modemui.dll` on a runner, looked at and not disassembled |
| `qsa-install`, the installer of another Quartz Systems product | `install/setup` | Read for how its plan and payload are arranged. It is ours. Its toolkit and its compression are C and were not taken |
| **com0com** | Nothing | Not opened, at any point in the phase: not its source, not its INFs. `docs/COM0COM.md` is as phase 0 wrote it, from its `ReadMe.txt` and from memory |

No GPL or LGPL project was opened for any crate.

## Dependencies

The root workspace gains no crate from outside it. `sha2`, for the
payload's checksum, was there already.

The driver's workspace is new, and has what the root bans: `wdk`,
`wdk-sys`, `wdk-build` and `wdk-panic` from Microsoft, MIT or Apache-2.0,
and beneath them `bindgen` and `cc`. Asked on 27 November, you allowed
both in the driver's workspace and nowhere else. `driver/deny.toml` is its
own allow list; it ignores one advisory, RUSTSEC-2024-0436, which says
that `paste`, a macro crate under `wdk-sys`, is no longer maintained.
`cargo deny check` passes in both workspaces.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check` (the driver workspace too), `ci/headers.sh`, `ci/pure.sh` and all tests pass; `driver/vcomcore` passes on Linux | Run here on Windows, and the whole Linux job under WSL, before every push; the driver's workspace by the `driver` workflow. 1,358 tests on Windows and 754 in the Linux job, and the 31 pictures of the manager. One run of the three on Windows passed whole; the other two had failures that passed alone: "What needs Cody". Two commits on `main` were red on CI: "Where the build differs" |
| 2 | An ordinary `cargo build` of the root workspace needs no WDK | The development machine has none, and every check ran on it |
| 3 | The driver package builds in CI and `InfVerif` reports no errors for the driver INF and the modem INF | The `driver` workflow, on every push. `rmmodem.inf` is valid; `rmvcom.inf` is valid with warning 2084, which `docs/DRIVER.md` explains |
| 4 | `vcomcore`'s timeout, wait-mask and data-integrity tests pass | `driver/vcomcore/tests/port.rs`, `integrity.rs` and `ioctl.rs`, on Windows and Linux |
| 5 | Where CI can install the driver: the conformance suite passes on the vcom backend | It can. `ci/install-test.ps1` runs the suite on a port it has just made: nine of nine |
| 6 | Setup's install, upgrade, repair and uninstall steps each have a test of their plan, and the port commands are tested against a fake device layer | `install/setup/tests/plan.rs`, eleven tests; `install/setup/tests/ports.rs`, twelve |
| 7 | No `BLOCKED(phase5)` marker is left in the tree | `grep -r "BLOCKED(phase5)"` finds it in the records of earlier phases and in this one |

### Manual checklist

`NEEDS-CODY`. Use virtual machines with snapshots, and take a snapshot
before each install. Nothing here should be tried first on the machine
that matters.

**The build to test.** Either the `rustmodem-setup` artifact of a run of
the `driver` workflow on `main`, which is a lab build signed with a
certificate made for that run; or one of your own, on a machine with a kit
(`docs/DRIVER.md`, "Building"):

```
ci\lab-certificate.ps1 -Out C:\keys\rustmodem-lab.cer
ci\build.ps1 -Thumbprint <the thumbprint it printed> -LabCertificate C:\keys\rustmodem-lab.cer
```

Every command below is from an elevated PowerShell prompt unless it says
otherwise. `COM10` and `COM11` are the first two ports setup makes.

**8. Clean install, client.** A fresh Windows 11, no com0com.

```
Confirm-SecureBootUEFI
bcdedit /enum "{current}" | Select-String testsigning
.\rustmodem-setup.exe
```

- [ ] `Confirm-SecureBootUEFI` says `True`, and `bcdedit` shows no
      `testsigning` line, or `No`.
- [ ] Setup asks three things and no more: Windows' prompt for an
      administrator, whether to trust the lab certificate, and whether to
      install. No other window appears, and nothing waits.
- [ ] It ends saying RustModem 0.2.0 is installed.
- [ ] `sc.exe qc rustmodem` shows `SERVICE_START_NAME : NT
      SERVICE\rustmodem`, and `sc.exe query rustmodem` shows `RUNNING`.
- [ ] In RustModem Manager, Lines, New line, OK, with "RustModem: create a
      new port" chosen: Windows asks for an administrator, and line 1
      appears on `COM10`.
- [ ] Device Manager shows "RustModem Port (COM10)" under Ports (COM &
      LPT) and "RustModem Softmodem" under Modems, neither with a
      warning.
- [ ] Phone and Modem, Modems, lists RustModem Softmodem on `COM10`.

If the driver does not install, that is the answer to the Secure Boot
question, and these say why:

```
pnputil /enum-devices /problem
Get-Content "$env:ProgramData\Quartz Systems\RustModem\logs\setup.log" -Tail 60
Select-String -Path C:\Windows\INF\setupapi.dev.log -Pattern rmvcom -Context 2,12 | Select-Object -Last 4
Get-WinEvent -LogName Microsoft-Windows-CodeIntegrity/Operational -MaxEvents 20
```

Then turn Secure Boot off in the virtual machine's firmware, go back to
the snapshot, and install again, to know that it is Secure Boot and
nothing else.

**9. Clean install, server.** The same, on the Windows Server that
Routing and Remote Access will run on.

- [ ] Every box of criterion 8.

**10. Query Modem.**

Set the log level to debug in the manager's Settings, which takes effect
at once, or put `log_level = "debug"` under `[service]` in the
configuration and `Restart-Service rustmodem`.

- [ ] Phone and Modem, Modems, RustModem Softmodem, Properties,
      Diagnostics, Query Modem: a list of commands and responses appears,
      none of them `ERROR`, with the manufacturer Quartz Systems and the
      model RustModem Softmodem among them.
- [ ] `rustmodem-ctl log --last 200` shows the commands. Paste the lines
      of that line, from the first `> AT` to the last answer, into a new
      file `dte/at/tests/hostlogs/query-modem.txt`, as the `README.md`
      there says.
- [ ] `cargo test -p at --test hostlogs` passes with the new file.
- [ ] Compare with transcript 61 in `dte/at/tests/transcripts/`, which is
      what Query Modem was believed to send. Anything Windows sent that is
      not there is a finding.

**11. Routing and Remote Access.** `docs/RRAS.md`, from "Before you
start" to the end of step 6, with a line for each modem.

- [ ] Routing and Remote Access, Ports, lists each RustModem Softmodem.
- [ ] A call from a modem to the pilot number is answered, and PPP comes
      up.
- [ ] Ports, the active port, Status: the line speed shown is the rate
      the modems agreed on, 2400 or 1200 or 300, and not 115200.
- [ ] `rustmodem-ctl line 1` shows the same rate for the call.

**12. A terminal program.** With Routing and Remote Access stopped, or on
a port it does not have.

- [ ] A terminal program, PuTTY's serial connection for one, opens
      `COM10` at any speed. `AT` and Enter gives `OK`.
- [ ] `ATI3` gives `RustModem 0.2.0`, and `ATI6` names the modem.
- [ ] Start the terminal without administrator rights and write down
      whether it opens the port. The INF gives the port no security of
      its own, so it has what Windows gives the Ports class, and what
      that admits was not looked at: either answer is a finding.

**13. The conformance suite.** On a machine with the repository and Rust,
a port that no line uses, and for the comparison a com0com pair set up as
`docs/COM0COM.md` says.

```
rustmodem-setup port add --com 40
$env:RM_VCOM_PORT = 'COM40'
cargo test -p serial --test conformance -- --nocapture --test-threads=1
$env:RM_COM0COM_PAIR = 'CNCB0,COM8'
cargo test -p serial --test conformance -- --nocapture --test-threads=1
```

- [ ] Nine tests pass on vcom.
- [ ] They pass on com0com, or each that does not is written down with
      what it printed: this is the first time anything of RustModem's has
      met a real pair.

**14. The phase 0 call, on RustModem ports.**

```
rustmodem-setup port add --count 2
```

and in `%ProgramData%\Quartz Systems\RustModem\rustmodem.toml`, with the
names `port add` printed:

```toml
[service]
log_level = "debug"

[[line]]
id = 1
serial = { backend = "vcom", port = "COM10" }

[[line]]
id = 2
serial = { backend = "vcom", port = "COM11" }
```

```
Restart-Service rustmodem
rustmodem-ctl watch
```

- [ ] Open a terminal on `COM10` and one on `COM11`. In each, `AT` gives
      `OK`.
- [ ] In terminal 1, `ATD2`. Terminal 2 shows `RING`.
- [ ] In terminal 2, `ATA`. Both show `CONNECT`.
- [ ] What is typed in one terminal appears in the other.
- [ ] In terminal 1, wait a second, type `+++`, wait a second: `OK`.
- [ ] In terminal 1, `ATH`: `OK`. Terminal 2 shows `NO CARRIER`.
- [ ] `rustmodem-ctl watch` showed each change as it happened, and closing
      terminal 2 makes line 2 `not_ready`.

**15. Upgrade, repair, uninstall.**

Upgrade, from 0.1.0: on a snapshot with 0.1.0 installed from its zip as
its `docs\INSTALL.md` said, with its service registered and a
configuration with lines.

```
.\rustmodem-setup.exe
rustmodem-ctl info
rustmodem-ctl lines
```

- [ ] Setup asks whether to install, and installs.
- [ ] `info` prints 0.2.0, and `lines` lists the lines 0.1.0 had, on their
      pairs.
- [ ] `sc.exe qc rustmodem` shows the new program's path and the new
      account.

Upgrade, over itself: with 0.2.0 installed and two lines on RustModem
ports, run the same setup program again.

- [ ] It asks whether to upgrade, or to repair, and afterwards
      `rustmodem-setup port list` and `rustmodem-ctl lines` show the same
      ports and lines as before.

Repair:

```
Remove-Item "$env:ProgramFiles\Quartz Systems\RustModem\rustmodem-ctl.exe"
rustmodem-setup repair
Test-Path "$env:ProgramFiles\Quartz Systems\RustModem\rustmodem-ctl.exe"
```

- [ ] `True`.

Uninstall:

```
rustmodem-setup uninstall
Get-PnpDevice | Where-Object { $_.HardwareID -match 'RUSTMODEM' }
pnputil /enum-drivers | Select-String 'rmvcom|rmmodem'
sc.exe query rustmodem
netsh advfirewall firewall show rule name="RustModem SIP"
netsh advfirewall firewall show rule name="RustModem RTP"
Get-NetQosPolicy -Name "RustModem RTP"
certutil -store Root "RustModem Lab"
certutil -store TrustedPublisher "RustModem Lab"
Test-Path "$env:ProgramFiles\Quartz Systems\RustModem"
Test-Path "$env:ProgramData\Quartz Systems\RustModem\rustmodem.toml"
```

- [ ] In order: no devices; no driver packages; error 1060, the service
      does not exist; "No rules match", twice; no policy; no certificate,
      twice; `False`; `True`.
- [ ] Device Manager, with View, Show hidden devices: no RustModem port
      or modem, greyed or not.
- [ ] Install again, then `rustmodem-setup uninstall --purge`: the last
      `Test-Path` is `False`, and
      `Test-Path "$env:ProgramData\Quartz Systems\RustModem"` is `False`.

**16. Robustness.** Two lines on RustModem ports, each in a call: the
call of criterion 14 will do.

```
Get-Process WUDFHost | Select-Object Id, StartTime
Get-CimInstance Win32_Process -Filter "Name='WUDFHost.exe'" | Select-Object ProcessId, CommandLine
```

- [ ] There is a `WUDFHost` for each RustModem port, besides any that
      other drivers have.
- [ ] End one of them, `Stop-Process -Id <id> -Force`. One terminal shows
      `NO CARRIER`, or its port closes; **the other line's side of the
      call ends as a call does when its far end goes, and that line is
      idle and usable.** `rustmodem-ctl lines` shows one line recovering
      and the other idle.
- [ ] Within some seconds, with nothing reinstalled, a terminal opens the
      port whose host was ended, and `AT` gives `OK`.
- [ ] With both terminals open on their ports: `Stop-Service rustmodem`
      returns within its usual time. `Start-Service rustmodem`, and `AT`
      gives `OK` in both terminals without reopening them.
- [ ] After `Stop-Service rustmodem` with nothing else open:
      `rustmodem-setup port remove COM11` succeeds, which it could not if
      a handle were stuck.

**17. DSCP.** On a domain-joined machine and on a standalone one, with a
call up and Wireshark on the interface the RTP leaves by, or:

```
pktmon filter add -p 20000-20999 -t UDP
pktmon start --capture --pkt-size 64
# ... a call, some seconds ...
pktmon stop
pktmon etl2pcap PktMon.etl -o rtp.pcapng
```

- [ ] RTP from the service has the DS field `0xB8`, which is DSCP 46.
- [ ] On both machines. If it is so only on the domain-joined one, that
      is the Network Location Awareness question of `docs/TELEPHONY.md`,
      "Media": write down the edition, and whether
      `Get-NetConnectionProfile` says the network is Domain, Private or
      Public.

**18. The signing decision.**

- [ ] Read `docs/SIGNING.md`.
- [ ] Decide: lab certificate only; or a certificate from a public
      authority, tried as that document says; or certification.
- [ ] Write the decision into `docs/SIGNING.md` in place of its
      `NEEDS-CODY`.

## Known limits of phase 5

- Ports are RustModem's or com0com's. There are no port pairs of
  RustModem's own, and no way for two programs to share a port.
- There is no baud rate. A program that times its output by the rate it
  set will find the port faster than it asked for.
- Flow control is reported and not enforced; the queue is what holds a
  writer back.
- Setup is for 64-bit Windows on x64. It is not an MSI, it does not update
  itself, and it does not remove com0com.
- The manager and `rustmodem-ctl` make ports on the machine they run on,
  which is the machine the service runs on: there is no other yet.
- A lab build is all there is. Until criterion 18 is decided, RustModem
  can be installed only where the owner of the machine chooses to trust
  our key.
