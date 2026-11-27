# Driver spike: what is true, and what was decided

Phase 5 begins by finding out whether a virtual COM port driver can be
written in Rust as a user-mode driver, how each line should appear to
Windows, how its device is made, and whether a package signed by a
certificate of our own installs. This is the report. The spike's code was
throwaway and is not in the tree; what it did and what happened is here.

**The driver is written in Rust, as a UMDF 2 driver, on Microsoft's
`windows-drivers-rs` crates. Each line is a device of class Ports with a
modem of class Modem attached to its COM port, both made by setup through
SetupAPI, with no wizard and no reboot.** That is device model B of the
handoff. Model A was tried two ways and does not pass.

Everything below that says "on the runners" was done on GitHub-hosted
`windows-2022` (Windows Server 2022 Datacenter, 10.0.20348) and
`windows-2025` (Windows Server 2025 Datacenter, 10.0.26100), on
27 November 2026, and came out the same on both unless it says otherwise.
Nothing was installed on the development machine, and nothing has been
tried on Windows 11.

## What needs Cody

| Item | What to do |
|---|---|
| Secure Boot on, test-signing off | **Not tested, and not testable on a runner:** both images have Secure Boot off and test-signing on. The check is criteria 8 and 9 of the phase's manual checklist, on a Windows 11 and a Windows Server virtual machine, with the real setup program. `NEEDS-CODY`. Asked on 27 November, Cody said that if a driver without Microsoft's signature needs Secure Boot off, that is accepted, so the phase goes on without waiting for the check. See "Signing" for why it is not expected to be needed |
| RRAS, Phone and Modem, Query Modem | Not tested: the runners have Routing and Remote Access disabled and no desktop. Criteria 8 to 12 of the manual checklist. `NEEDS-CODY` |
| `bindgen` and `cc` in the driver's workspace | Asked on 27 November; allowed. See "Dependencies" |
| Decisions made in the spike | "Decisions" below. Say if any should change |

## How the spike was run

Cody chose, when asked: driver builds on GitHub runners only, since the
development machine has no WDK and little disk; installation on runners and
on Cody's virtual machines only, never on the development machine; and the
throwaway code on a branch, `spike/driver`, deleted when this report was
written. Eleven workflow runs were made. The branch held a UMDF driver of
about 500 lines that is one serial port answering `OK` to every line and
keeping a log of every request it is given, which a host reads back through
the port; three INFs; a program of 250 lines that makes and removes devices
through SetupAPI and the software-device API; and a script that packages,
signs, installs, exercises and removes all of it and prints what happened.

## 1. Toolchain

**A UMDF 2 driver builds from the crates.io releases.** No revision of the
repository is needed, although its README still says the published crates
support only KMDF.

| Crate | Version |
|---|---|
| `wdk` | 0.5.0 |
| `wdk-sys` | 0.5.2 |
| `wdk-build` | 0.6.0 |
| `wdk-panic` | 0.5.0 |
| `wdk-macros` | 0.5.2 |
| `cargo-wdk` (a tool, not a dependency) | 0.2.0 |

- The driver model is chosen in the manifest:
  `[package.metadata.wdk.driver-model]` with `driver-type = "UMDF"`,
  `umdf-version-major = 2`, `target-umdf-version-minor = 33`. The crate is
  a `cdylib`, its build script is one call,
  `wdk_build::configure_wdk_binary_build()`, and it is built with
  `-C target-feature=+crt-static`.
- **A UMDF driver in Rust has `std`.** Threads, locks, collections and
  `String` are there. No allocator crate and no `no_std`.
- **WDK 10.0.26100** was used on both images. `wdk-build` finds a kit
  through `WDKContentRoot`, then the registry's `KitsRoot10`: an installed
  kit is found with nothing set, and no eWDK prompt is needed.
- **LLVM 20.1.8**, as the images have it, works for the bindings. The
  crates' README asks for 17.0.6.
- `cargo wdk build` also packages: `stampinf`, `inf2cat`, `infverif`,
  `signtool` with a test certificate it makes. The spike ran those tools
  itself instead, because the real build signs with a certificate that is
  a parameter and has more than one INF in a package. Plain `cargo build`
  is enough to make the DLL.

**Where the kit comes from**

| | `windows-2022` | `windows-2025`, which is `windows-latest` |
|---|---|---|
| WDK on the image | 10.0.26100, installed | None. The image's maintainers have said it will not come back |
| Getting one | Nothing to do | `nuget install Microsoft.Windows.WDK.x64` (10.0.26100.6584, which brings the SDK packages): 84 seconds, 1.4 GB, then five environment variables set by hand: `Version_Number`, `WindowsSdkBinPath`, `WDKContentRoot`, `WDKBinRoot`, `WDKToolRoot` |
| First build of the driver, nothing cached | 4 minutes | 6.5 minutes |
| The whole job: build, package, sign, install, exercise, remove | 6 minutes | 10 minutes |

On a developer's machine the same two ways work: the WDK's installer with
its matching SDK and Visual Studio 2022, or the NuGet packages in any
directory with those variables set. The eWDK is a 20 GB image and is not
needed. The development machine has Visual Studio Build Tools 2019 and the
19041 SDK, which match no kit the crates support; the NuGet packages are
the way there, and were not tried.

`inf2cat` accepts `/os:ServerFE_X64,Server2025_X64,10_CO_X64,10_GE_X64`,
which is Server 2022, Server 2025 and Windows 11 21H2 and 24H2. `stampinf`
puts in the architecture, the UMDF version and the driver's version and
date.

## 2. The framework surface

Every call goes through one macro over generated bindings,
`call_unsafe_wdf_function_binding!`, and every configuration structure is
filled in by hand, because the C headers' initialising functions are
inline and are not in the bindings. The spike's layer over the framework
compiled at the first attempt and each of these was then seen to work on
the runners:

| Needed | Result |
|---|---|
| Device creation | `WdfDeviceCreate` |
| A default queue and a manual queue | `WdfIoQueueCreate`, parallel and manual. A wait parked in the manual queue was taken out and completed by a later request |
| Device I/O control dispatch | Every serial request reached the driver with its code and lengths |
| Create and close callbacks that see the file name | Opening `\\.\COM3\control` gave the create callback the name `\control`; a plain open gave none. The caller's process id is there too |
| Cancellation | A read parked by the driver itself and marked with `WdfRequestMarkCancelableEx` was cancelled by `CancelIoEx`: the cancel routine ran and the program saw `ERROR_OPERATION_ABORTED` |
| Timers | `WdfTimerCreate` and `WdfTimerStart`; the callback ran |
| Device interfaces | `WdfDeviceCreateDeviceInterface`, with and without a reference string |
| A symbolic link | `WdfDeviceCreateSymbolicLink` to `\DosDevices\Global\COMn` |
| The device's hardware key | `WdfDeviceOpenRegistryKey` with `PLUGPLAY_REGKEY_DEVICE`, and `WdfRegistryQueryValue` for `PortName` |
| The `SERIALCOMM` entry | `WdfDeviceOpenDevicemapKey` and `WdfRegistryAssignUnicodeString`. **The key must be opened with `KEY_SET_VALUE` alone**: `KEY_WRITE` is refused with `STATUS_INVALID_PARAMETER`. With the value written, `SerialPort.GetPortNames` lists the port; without it, it does not |
| A security descriptor on the device | Not as in a kernel driver: `WdfDeviceInitAssignSDDLString` and `WdfDeviceInitSetExclusive` are for KMDF only. A descriptor for the whole device goes in the INF. One interface cannot have its own, so the control side is guarded another way, which is the next row |
| Who is opening | `WdfRequestImpersonate` in the create callback, then the thread's token: the driver read the caller's SID. It needs `UmdfImpersonationLevel = Impersonation` in the INF and is refused with `STATUS_ACCESS_DENIED` without it. A caller in the kernel, which is what Windows' modem driver is, cannot be impersonated at all, and the call is refused for it |

Nothing the driver needs is missing. What the crates do not give is
safety: all of it is `unsafe`, as the handoff expected.

**A killed driver host.** With the port installed and idle, every
`WUDFHost.exe` was killed. The next open of the port succeeded and the
port answered: the framework starts the host again, and the device stays
started. What happens to a port that is open at the time is criterion 16.

## 3. Device model

### B: a device of class Ports, and a modem attached to its port

**The port.** A device of class Ports whose function driver is the
user-mode reflector with our driver in it. Made by the spike's program in
0.3 to 0.4 seconds with no reboot. The Ports class installer gave it the
next free COM number and put that in its name, "RustModem Spike Port
(model B) (COM3)". It started. A program opened `\\.\COM3`, read the modem
status, wrote `AT` and read `OK`, had a read cancelled, and had a wait
ended by a new mask. `System.IO.Ports.SerialPort` opened and closed it.

**The modem.** A second device, of class Modem, made the same way with an
INF that has no driver in it at all, only what Windows' modem stack reads,
and with the port's name written as `AttachedTo` in the modem's driver key
before the INF is installed. It installed and started in under half a
second with no wizard and no reboot. Windows lists it
(`Win32_POTSModem`) as "RustModem Spike Modem (on a port)", attached to
COM3, at 115200 bit/s. Its stack is Windows' modem driver over Windows'
own driver for a modem on a port, `ROOTMODEM`. During the install a system
process opened our port and sent it a modem request and serial requests:
the modem stack reaches the driver.

With the modem installed and idle, a program still opened `\\.\COM3`
directly and got `OK`.

This is what is done by hand today with com0com and "Standard Modem",
with both steps now done by a program.

### A: one device of class Modem with our driver under Windows' modem driver

Tried two ways.

- **Root-enumerated.** The Modem class installer leaves no device behind
  when asked to register one, so the device was registered past it. The
  INF then installed and the device started, but Windows put `ROOTMODEM`
  under our driver and treated the device as a modem attached to a port
  named by `AttachedTo`, which it did not have: the value read back as
  garbage, and no COM name was assigned. **A root-enumerated modem is, to
  Windows, a modem on a port.** It is model B with nothing to be attached
  to.
- **Enumerated by software** (`SwDeviceCreate`), which is the only other
  way to make a device without a bus driver. Here Windows did what model A
  wants: it chose our INF by itself, the class installer assigned COM4 and
  wrote it as both `PortName` and `AttachedTo`, and our driver loaded and
  made its link and its `SERIALCOMM` entry. But the device did not start:
  problem code 10, status `0xC000009A`, on both servers. And a device made
  this way does not outlive a restart of Windows unless a program with
  administrator rights makes it again each time, which the service must
  not be.

### Against the criteria

| Criterion | A | B |
|---|---|---|
| RRAS lists it and answers on it | Not reached: the device does not start | `NEEDS-CODY`. It is the arrangement RRAS is used with today |
| A terminal opens the COM port directly | No | Yes, on the runners, with the modem installed |
| Windows assigns and shows a COM number | Assigned as a software device; not as a root device | Yes, and one of our choosing: see section 4 |
| Made and removed with no wizard and no reboot | Made, as a software device, but not across a restart | Yes, both devices |
| How much of the INF is ordinary | One INF, with the reflector as a lower filter of a class that does not expect one | Two, both ordinary: a port and a modem |

**B is chosen.** A is preferred by the handoff only if it passes every
criterion, and it passes one. The failure to start might be found and
mended, but the device's lifetime could not be without a kernel bus
driver, which is out of scope.

What B costs is a second device for each line, which setup makes and
removes with the first, and two names in Device Manager.

## 4. Creating devices

**Root-enumerated devices through SetupAPI.** The sequence is
`SetupDiCreateDeviceInfo` with a generated id, the hardware id set as a
property, registration, and `UpdateDriverForPlugAndPlayDevices` with the
INF. Removal is `SetupDiCallClassInstaller` with `DIF_REMOVE`.

| | Root-enumerated, SetupAPI | Enumerated by software |
|---|---|---|
| Survives a restart | Yes, as Microsoft documents it. Not tested: a runner cannot restart. `NEEDS-CODY`, criterion 15 | Not unless made again by an administrator's program |
| Removed cleanly | Yes: the device and its link were gone, and the driver package then left the store with `pnputil /delete-driver /uninstall`. **The `SERIALCOMM` value stayed**, since the spike's driver never took it out: the driver must remove its value when its device goes | Yes, with the same value left |
| Choosing the COM number | Yes | The class installer chooses |
| Needs a tool of the WDK | No | No |

- **A port is registered through its class installer**
  (`DIF_REGISTERDEVICE`). **A modem is not:** the Modem class installer
  leaves no device, and `SetupDiRegisterDeviceInfo` is called instead.
- **The COM number can be chosen.** `PortName` written in the device's
  hardware key after registration and before the INF is installed was
  kept: the port came up as COM77 and was named so. Left alone, the class
  installer assigns the next free number. Setup must also claim a number
  it chooses in the COM port database, which the spike did not do.
- `pnputil` has no command that makes a device, and `devgen` is part of
  the WDK and may not be shipped. Setup makes its devices itself.

## 5. Signing

The package's catalog and DLL were signed with a self-signed code-signing
certificate made on the runner (`New-SelfSignedCertificate -Type
CodeSigningCert`, `signtool sign /fd SHA256`).

| The certificate is | `pnputil /add-driver` |
|---|---|
| Not trusted | **Does not fail: it waits.** A "Windows Security" window asking whether to trust the publisher is put on a desktop nobody sees, and `pnputil` waits on it for ever. A run of the spike hung for 27 minutes on it |
| In the machine's Trusted Root and Trusted Publishers stores | Adds the package silently. The catalog verifies as valid, and the store shows the signer's name |

Devices then installed and the driver loaded, on Server 2022 and on
Server 2025.

**What this does not show.** Both runners have **Secure Boot off and
test-signing on**, and a runner cannot be restarted to change either. So
the question the handoff asks, Secure Boot on and test-signing off, on
Windows 11 and on Server, is open, and is `NEEDS-CODY`.

What is known about it:

- Microsoft's documentation says a user-mode driver does not need
  Microsoft's signature, and that Plug and Play installs a package whose
  catalog is signed by a publisher in the machine's Trusted Publishers
  store without asking. It says the rule that came with Windows 10 1607,
  which Secure Boot enforces, is for kernel-mode drivers.
- This driver is not unsigned. It is signed by a certificate the machine
  has been told to trust, and it is not loaded by the kernel.
- No page of Microsoft's and no first-hand report was found that says
  what happens to such a package with Secure Boot on, either way.
  Projects that ship user-mode display drivers with a certificate of
  their own install it into the same two stores.
- Smart App Control, and an App Control policy that covers user-mode
  code, could refuse a DLL signed this way. Neither is on by default on a
  server.

So it is expected to work with Secure Boot on, and it is a belief. If it
does not, Cody has said Secure Boot off is accepted. Test-signing mode is
another switch, and nothing in this phase asks for it.

**Setup must trust the certificate before it touches the driver store,**
and must never run `pnputil` or its equivalent where a question could be
asked: a silent install that met an untrusted package would hang.

For distribution: attestation signing is now described by Microsoft as
for testing only and is not accepted by Windows Server 2016 and later;
the other ways are certification through the hardware lab kit, or a
package signed by a certificate from a public authority, which the same
documentation allows for a user-mode driver and which asks the user once
unless the publisher is already trusted. `docs/SIGNING.md`, in work
package 7, has this with its sources.

## 6. CI

**A GitHub-hosted runner can build, install and exercise the driver.** It
runs as an administrator; devices are made, opened and removed; and the
whole job is six to ten minutes. So the conformance suite of work package
4 can run in CI on the vcom backend, on a job that trusts a certificate
made for that run. It runs with test-signing on, as said.

Two things a job must do: put a time limit on every step that installs,
because a question on a hidden desktop is a job that never ends; and use
`windows-2022` while that image has its kit, or the NuGet packages on
`windows-2025`.

## What the spike found that the driver must do

- **What a host sends when it opens a port.** `SerialPort.Open` sent, in
  order: properties, modem status, then baud rate, line control, special
  characters and flow control read and written back twice, RTS and DTR
  cleared, timeouts, a wait mask, the queue sizes and a wait; on close, a
  mask of nothing, DTR cleared, and two purges. Windows' modem stack,
  installing a modem on the port, sent a modem request (device type
  `0x2b`), then set DTR, the timeouts and a purge. A request of a type
  that is not serial must be refused as a serial port refuses it.
- **A reflector section that passes every mode of `InfVerif`.** The spike
  installed the reflector's service by naming its binary, which works on
  both servers and passes the universal check with a warning, and fails
  the stricter modes. Including Windows' own `wudfrd.inf` passes them,
  but that file came with build 22000 and Server 2022 is 20348. Work
  package 3 settles it.
- **The control side is the same device as the host side.** It is told
  apart by the name in the create, and guarded by impersonating the
  caller and checking who it is. A caller that cannot be impersonated is
  refused on the control side.
- **What Query Modem sends.** The commands are in `modemui.dll` of
  Windows 11 build 26100 as text, and are the list Microsoft documented
  for Windows Server 2003: `ATQ0V1E0`, `AT+GMM`, `AT+FCLASS=?`,
  `AT#CLS=?`, `ATI1` to `ATI7`, `AT+GCI?` and `AT+GCI=?`. `+GMI`, `+GMR`,
  `+GSN` and `+GCAP` are not among them. Work package 5 answers all of
  both lists; a real run is criterion 10.
- Setup log, Server 2022: "Using WDF schema version 2.23 when section
  requires version 2.33". The device works; the directives used are all
  older than 2.23.

## Dependencies

The driver's dependency graph has `bindgen` and `cc`, which the
workspace's `deny.toml` bans: `wdk-sys` generates its bindings from the
kit's headers when it is built, and compiles one C file, which is those
same headers and nothing else, to bring out symbols the headers declare
in a way the linker would otherwise drop. Every UMDF driver also links
Microsoft's stub library, in whatever language it is written. Ground rule
4 says to ask: asked on 27 November 2026, Cody allowed both **in the
driver's workspace**. That workspace has a `deny.toml` of its own; the
workspace's is unchanged, and nothing outside `driver/` gains either.

## Decisions

| Decision | Why |
|---|---|
| Rust, UMDF 2.33, the crates.io releases above | It builds and runs. No stop condition was met |
| Device model B: a Ports device with our driver, and a Modem device attached to it | Section 3 |
| Root-enumerated devices made through SetupAPI, by setup | Section 4. They persist, are removed cleanly, need nothing of the WDK, and let the COM number be chosen |
| The package is built with the kit's tools run by our own script, not by `cargo wdk build` | Two INFs in one package, and a certificate that is a parameter |
| The driver's own state is in `std` types | It is user mode. The framework is asked only for what only it can do |
| The CI driver job is on `windows-2022` | Its kit is on the image. The NuGet steps are written down for the day it goes |
| The modem's INF has no driver in it | Windows' own drivers are the modem's stack; ours is under the port |

## References consulted

| Reference | For | How |
|---|---|---|
| `windows-drivers-rs` 0.5 and 0.6: `wdk-sys`'s build script, `wdk-build`, `wdk`'s timer, `cargo-wdk`'s README and templates, the repository's CI workflows | The toolchain | Read. MIT or Apache-2.0. The driver depends on the crates; nothing is copied from them |
| Microsoft's documentation of each framework function named above, of `SwDeviceCreate`, `pnputil`, `devgen`, `InfVerif`, `Inf2Cat`, the INF directives of the framework, driver signing, and Trusted Publishers | Sections 2 to 5 | Read on learn.microsoft.com, November 2026 |
| Microsoft's `Windows-driver-samples`: `serial/VirtualSerial2` (its README, its two INFs, `device.c`, `queue.c`) and `network/modem/fakemodem` (its README, its INF, `driver.c`) | How a user-mode serial driver and a modem INF fit together | Read for what they do: which class each INF is, that the driver reads `PortName` and makes the link and the `SERIALCOMM` value itself, which directives the reflector is given. Microsoft Public License. Nothing was copied: no code, no INF section, no layout, no names |
| The archived modem INF reference, "Responses" and "Response State" | The response entries of the spike's modem INF | Read, under `previous-versions` on learn.microsoft.com |
| Windows' own files on a machine: the Modem class's registry key, the strings of `mdminst.dll`, `modemui.dll` and `modem.sys` | That Windows' modem driver is the function driver of the class; what Query Modem sends | Looked at, not disassembled |

com0com was not opened.
