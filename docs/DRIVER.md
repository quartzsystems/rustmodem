# The RustModem port driver

RustModem has its own virtual COM port. Each line that uses it has one
port, a `COM` name that Windows lists as a serial port, with a modem
attached to it that Windows lists as a modem. Routing and Remote Access,
or a terminal, opens the port as it would a hardware modem's; the service
is the modem behind it. No second port, no null-modem pair and no other
project's driver is involved.

This is the reference for the driver as built: the design, the control
interface, what a host can and cannot expect of the port, how to trace it,
how to recover a port that is stuck, and how to build it.
`docs/DRIVER_SPIKE.md` is the record of how the design was arrived at,
`docs/SIGNING.md` of how the package is signed, and `docs/INSTALL.md` of
how it gets onto a machine.

**What has been tried and what has not.** Everything here was run on
GitHub's Windows Server 2022 runners, where `ci\install-test.ps1` installs
the driver, makes ports and runs calls through them on every push that
touches it. Nothing here has been run on Windows 11, on a machine with
Secure Boot on, or under Routing and Remote Access: those are the manual
criteria of `docs/PHASE5.md`.

## The design

| Piece | Where | What it is |
|---|---|---|
| The port's rules | `driver/vcomcore` | A state machine with no Windows in it: requests, octets and the time go in, completions come out. Pure; tested on Linux |
| The control interface | `driver/vcomapi` | The I/O control codes and layouts the service speaks to the driver. Pure. The driver and the service's backend both take them from here |
| The driver | `driver/vcom`, `rmvcom.dll` | A UMDF 2 driver, version 2.33 of the framework. It makes the device, hands each request to the port and completes requests as the port says. It decides nothing |
| The modem's INF | `driver/modeminf`, `rmmodem.inf` | Generated from the AT interpreter's own tables, so the responses Windows is told to expect are the ones `dte/at` sends |
| The service's backend | `dte/serial/src/vcom.rs` | `VcomBackend`: the service's end of a port |
| Making and removing ports | `install/sys`, `install/setup` | SetupAPI, in the setup program. The service makes no devices and has no right to |

The driver is a user-mode driver. It is a DLL loaded by Windows' driver
host process, `WUDFHost.exe`; nothing of RustModem runs in the kernel, and
a fault in the driver ends a host process and not the machine.

`driver/` is a Cargo workspace of its own, outside the root workspace,
because building the driver needs a Windows Driver Kit and its dependency
graph has `bindgen` and `cc`, which the root `deny.toml` bans. An ordinary
`cargo build` of the root needs no kit. `vcomcore`, `vcomapi` and
`modeminf` are members of the root workspace, where CI tests them.

### Two devices for each port

A RustModem port is two devices, both root-enumerated, both made by the
setup program:

| Device | Class | Hardware id | Driver |
|---|---|---|---|
| The port | Ports | `RUSTMODEM\VCOM` | `rmvcom.dll` under `WUDFRd.sys`, by `rmvcom.inf` |
| The modem | Modem | `RUSTMODEM\SOFTMODEM` | None of ours: Windows' modem driver, told by `rmmodem.inf` what to send and what to expect, with `AttachedTo` naming the port |

To Windows the pair is a legacy modem on a serial port, which is exactly
what the modem stack, TAPI and Routing and Remote Access know how to use.
The other model, one device of class Modem with our driver beneath
Windows' modem driver, was tried in the spike and set aside:
`docs/DRIVER_SPIKE.md`, section 3.

The port's name is chosen before the device is installed, written where
the Ports class installer reads it, and claimed in Windows' COM port
database so that nothing else is given the number. Setup gives the next
free number from `COM10` up unless told one.

The driver adds the port to `HKLM\HARDWARE\DEVICEMAP\SERIALCOMM`, which is
where programs look for the serial ports of a machine, under the value
name `\Device\RustModem` followed by the port's name, and takes it out
when the device goes. The service knows a RustModem port from any other by
that value name.

### Two sides of one device

A host opens `\\.\COM10` and has a serial port. The service opens the same
device as `\\.\COM10\control` and is the modem's side of it. Octets
written on one side are read on the other, through two bounded queues, and
the control side stands where a modem's lines would be: it sets CTS, DSR,
RI and DCD and is told of DTR and RTS.

- **One host at a time.** A second open of the COM name fails while the
  first is open, as a serial port's does.
- **One control client at a time**, and only for SYSTEM, an administrator
  or the service's account, `NT SERVICE\rustmodem`. The driver
  impersonates the caller at the open and checks its token; anyone else is
  refused with access denied. The INF asks for
  `UmdfImpersonationLevel = Impersonation` so that it can.
- **A port with no service behind it is still a port.** The host can open
  it and sees a modem that is not there: its lines are low, and what it
  writes goes nowhere. When the service opens the control side again the
  port goes on.

## The control interface

`driver/vcomapi/src/lib.rs` is the definition; this is a summary. Every
code is buffered and every number little-endian. The device type is
`0x8A51`, in the range Windows leaves to vendors.

| Code | Function | In | Out | Does |
|---|---|---|---|---|
| `IOCTL_READ` | `0x800` | | Octets | Reads what the host wrote. Waits until there is at least one octet |
| `IOCTL_WRITE` | `0x801` | Octets | `u32` | Writes octets for the host. Answers how many were taken, which is fewer than given when the host's queue is full |
| `IOCTL_SET_LINES` | `0x802` | `u32` | | Sets the lines toward the host: CTS `0x1`, DSR `0x2`, RI `0x4`, DCD `0x8` |
| `IOCTL_WAIT_EVENT` | `0x803` | | 8 octets | Waits for the next thing the host does. One wait at a time |
| `IOCTL_GET_STATE` | `0x804` | | 20 octets | The state of the host's side |
| `IOCTL_BREAK` | `0x805` | | | Sends a break toward the host |

An event is a kind and a value, each a `u32`:

| Kind | Event | Value |
|---|---|---|
| 1 | The host opened the port | |
| 2 | The host closed the port | |
| 3 | DTR changed | 1 on, 0 off |
| 4 | RTS changed | 1 on, 0 off |
| 5 | A stored setting changed | |
| 6 | The host purged | Bit 0 the queue toward the host, bit 1 the queue from it |
| 7 | The host began or ended a break | 1 began, 0 ended |
| 8 | More happened than was kept | Read the state |

The state is five `u32`: the interface's version, which is 1; flags, bit 0
the host has the port open, bit 1 DTR, bit 2 RTS, bit 3 a break; the baud
rate the host stored; how many octets the port would take for the host
now; how many from the host wait to be read. `VcomBackend` refuses a
driver whose version it does not know, with `RMD9003`.

The device also has a device interface,
`{5F0D2A0B-91C4-4E57-A31E-524D434F4D31}`. It is there for finding the
ports. The control side is not opened through it.

## What the port does for a host

The rules are those of the Win32 documentation for `COMMTIMEOUTS`,
`SetCommMask`, `WaitCommEvent`, `PurgeComm` and `GetCommModemStatus`, and
are decided and tested in `driver/vcomcore`.

**Implemented:**

- Reads and writes, with all five timeouts of `COMMTIMEOUTS`: the interval
  timeout, the total timeouts, and the special case of an interval of
  `MAXDWORD` that returns at once with what there is.
- `SetCommMask` and `WaitCommEvent` for `EV_RXCHAR`, `EV_RXFLAG`,
  `EV_TXEMPTY`, `EV_CTS`, `EV_DSR`, `EV_RLSD`, `EV_BREAK` and `EV_RING`. A
  wait is ended by a new mask, as Windows has it. `EV_RING` is raised when
  RI goes high.
- `GetCommModemStatus`, `EscapeCommFunction` for DTR, RTS and break,
  `PurgeComm`, `ClearCommError`, `SetupComm` with queues of 64 to 65,536
  octets (4,096 unless set), `TransmitCommChar`, `GetCommProperties`.
- Cancellation: a request that waits is cancelable, and is completed
  exactly once whether it ends, times out or is cancelled.

**Stored and read back, and nothing more:**

- The baud rate, the line control (data bits, parity, stop bits), the
  special characters and the flow control settings. A host sets them and
  reads back what it set. **There is no baud rate**: octets cross as fast
  as the two sides take them, at any setting. The service reads the stored
  baud rate from the state and does nothing with it.
- Flow control is reported, not emulated. DTR and RTS follow the flow
  control settings at the open, as a serial port's do, and a host that
  gives one of them to flow control is refused when it tries to set that
  line itself. The port never holds octets back because of a line or an
  XOFF: the queue's being full is the only thing that stops a write.

**Not there:**

- Parity, framing and overrun errors. Nothing here can produce one.
- A control code the port does not have is refused with
  `STATUS_INVALID_PARAMETER`, which is what Windows' own serial driver
  answers to a code it does not know.
- Port pairs, and any way for two hosts to share a port.

### Differences between the backends

The conformance suite, `dte/serial/tests/conformance.rs`, runs the same
nine tests against the memory backend, a RustModem port and a com0com
pair. Where a backend's host side cannot do something, the test says it is
skipped for that backend.

| | Memory | RustModem port | com0com |
|---|---|---|---|
| Runs | Always | With `RM_VCOM_PORT=COM40` naming a port nothing else has open | With `RM_COM0COM_PAIR=CNCB0,COM8` naming a pair set up as `docs/COM0COM.md` says |
| Timeouts, wait mask, purge | None: they belong to a driver | Tested | Tested |
| The host opening and closing | Told | Told by an event | Seen as DTR |
| Where it has run | Everywhere | On a CI runner: all nine pass | Nowhere yet. `NEEDS-CODY`: criterion 13 of `docs/PHASE5.md` |

## Tracing

The driver writes to Event Tracing for Windows under the provider
`{B7C1D5E2-3F4A-4C6B-9D8E-524D54524143}`. It writes nothing unless a port
is told to: the level is the `TraceLevel` value, a DWORD, in the port
device's hardware key, read when the device starts.

| `TraceLevel` | What is written |
|---|---|
| 0, or absent | Nothing |
| 2 | What went wrong: a device that could not start, and why |
| 4 | And the device starting and being removed, and each side's opens and closes |
| 5 | And every request. A great deal |

To trace a port, from an elevated prompt, with `COM10` as the port:

```
$port = Get-PnpDevice -Class Ports | Where-Object FriendlyName -like '*(COM10)'
$key  = "HKLM:\SYSTEM\CurrentControlSet\Enum\$($port.InstanceId)\Device Parameters"
Set-ItemProperty $key TraceLevel 4 -Type DWord
pnputil /restart-device "$($port.InstanceId)"

logman start rmvcom -p "{B7C1D5E2-3F4A-4C6B-9D8E-524D54524143}" 0 5 -o C:\rmvcom.etl -ets
# ... do what is to be traced ...
logman stop rmvcom -ets
tracerpt C:\rmvcom.etl -o C:\rmvcom.xml -of XML
```

Restarting the device hangs up its line; the service takes the port up
again by itself. Set `TraceLevel` back to 0 and restart the device again
when done.

The framework keeps its own log of a user-mode driver, and Windows records
a driver host that ended badly in the System event log under the source
`DriverFrameworks-UserMode`. Look there first when a port has a problem
code in Device Manager.

## Recovering a stuck port

In the order to try them. Each is from an elevated prompt.

1. **Look.** `rustmodem-setup port list` shows each port with its modem,
   the driver's version and whether its device is started.
   `rustmodem-ctl ports` shows what the service makes of them, and
   `rustmodem-ctl lines` shows the line's state and the message it has,
   if it could not open its port.
2. **Is something else holding it?** A port has one host at a time. A
   terminal left open, or Routing and Remote Access, is a host. Close it,
   or stop `RemoteAccess`.
3. **Restart the device.**

   ```
   $port = Get-PnpDevice -Class Ports | Where-Object FriendlyName -like '*(COM10)'
   pnputil /restart-device "$($port.InstanceId)"
   ```

   The line drops its call and the service opens the port again by
   itself.
4. **End the driver host.** Windows starts another at the next open:

   ```
   Get-Process WUDFHost | Stop-Process -Force
   ```

   This ends every user-mode driver's host on the machine, RustModem's
   and any other's, so it is for a machine where that is acceptable. The
   install test does it during a call and checks that the port works
   again without anything being reinstalled.
5. **Remove the port and make it again**, under the same name, so the
   line needs no change:

   ```
   rustmodem-setup port remove COM10
   rustmodem-setup port add --com 10
   ```
6. **Repair.** `rustmodem-setup repair` puts the driver package back in
   the driver store and installs it on every port again.

A port whose modem is missing takes a line, and a terminal can open it,
but Windows has no modem to dial or answer with: Ports and drivers in the
manager shows such a port as "No modem". Remove it and add it again.

## Building

The driver needs, beyond Rust:

| | What | Where from |
|---|---|---|
| A Windows Driver Kit | 10.0.26100 is what CI uses | It is on GitHub's `windows-2022` runner image. Elsewhere: the installer from Microsoft, or the NuGet package `Microsoft.Windows.WDK.x64` with `Microsoft.Windows.SDK.CPP`, which is 1.4 GB unpacked and needs no installer. With the NuGet packages, set `WDKContentRoot`, `WDKBinRoot`, `WDKToolRoot`, `WindowsSdkBinPath` and `Version_Number` to where they were unpacked |
| LLVM | 20.1.8 is what CI uses | `bindgen` reads the kit's headers with `libclang`. It is on the runner image; elsewhere `winget install LLVM.LLVM`, and `LIBCLANG_PATH` set to its `bin` directory if it is not found |

Then:

```
cd driver
cargo build --release             # the DLL alone
cargo clippy --all-targets -- -D warnings
cargo deny check                  # driver\deny.toml, which allows bindgen and cc

driver\package.ps1                # the package, unsigned
ci\build.ps1 -Thumbprint <sha1> -LabCertificate <file.cer>
                                  # everything, signed, as rustmodem-setup.exe
```

`driver\package.ps1` builds the DLL, generates `rmmodem.inf`, stamps both
INFs with the version in `VERSION`, makes the catalog, and runs `InfVerif`
on both INFs. `rmvcom.inf` passes with one warning, 2084, for naming
`WUDFRd.sys` as its service without copying it. The way round the warning
is to include Windows' own `wudfrd.inf`, which came with build 22000, and
Windows Server 2022 is build 20348: the warning is left so that the
package installs there.

**Without a kit** nothing of the root workspace is lost: `cargo build`,
`cargo test --workspace` and every check in `CLAUDE.md` run as before,
`driver/vcomcore`, `driver/vcomapi` and `driver/modeminf` among them. Only
`driver/vcom` itself, the package and the setup program that carries them
need the kit, and the `driver` workflow on GitHub builds those, installs
them on its runner and runs the install test, on every push. This is how the
driver was developed: no kit was installed on the development machine.

The driver's version is the product's. `ci/version.sh` checks
`driver/Cargo.toml` with the rest.

## What the driver is written from

Microsoft's documentation of the framework and of the serial interface,
and the Win32 documentation of the functions a host calls. Microsoft's
driver samples and com0com were read for behaviour and nothing was copied
from either; `docs/PHASE5.md` says exactly what was read.
