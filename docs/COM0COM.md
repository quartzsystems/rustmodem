# Setting up com0com for RustModem

**com0com is the alternative backend.** Since 0.2.0 RustModem has a port
driver of its own, which the setup program installs and which needs none
of what follows: `docs/INSTALL.md` and `docs/DRIVER.md`. A line can still
be put on a [com0com](https://com0com.sourceforge.net/) null-modem pair,
with `backend = "com0com"`, and a machine that ran 0.1.0 goes on working
as it was. This document is for those lines. The differences between the
two backends that a host can see are in `docs/DRIVER.md`.

With com0com, each RustModem line reaches Windows through a null-modem
pair. The host (RRAS, a terminal, the Windows modem driver) opens one port
of the pair; RustModem opens the other.

com0com is a separate GPL product. RustModem does not include it, link to it
or copy from it; it only opens a port the driver provides. Install it from
its own distribution.

> **NEEDS-CODY: verify on a real pair.** No com0com pair was available when
> the backend was written. The pin parameters and their values below are as
> `setupc help` lists them, written from memory; com0com's `ReadMe.txt`
> itself only says that "OUT1 can be wired to DCD and OUT2 to RING" and that
> an application sets them with `IOCTL_SERIAL_SET_MODEM_CONTROL`. Check each
> command against `setupc help` and each signal with the checklist at the end.

## What the backend expects

A null-modem pair has no modem end. Each side drives its own output lines
(DTR, RTS, OUT1, OUT2) and the driver wires them to the other side's input
lines (DSR, CTS, DCD, RI) as the pair is configured. A modem has to drive DCD,
DSR, CTS and RI toward the host, so RustModem uses all four of its outputs:

| The modem sets | RustModem drives, on its port | The host's port must be wired | The host sees |
|---|---|---|---|
| DSR | DTR | `dsr=rdtr` (the default) | DSR |
| CTS | RTS | `cts=rrts` (the default) | CTS |
| DCD | OUT1 | `dcd=rout1` | DCD |
| RI | OUT2 | `ri=rout2` | RI |

In the other direction:

| The host does | RustModem's port must be wired | RustModem sees it on | Meaning |
|---|---|---|---|
| Drives DTR | `dsr=rdtr` (the default) | DSR | DTR |
| Drives RTS | `cts=rrts` (the default) | CTS | RTS |
| Opens or closes the port | `dcd=ropen` | DCD | Host opened, host closed |

If RustModem's port is left without `dcd=ropen`, it cannot see the open
itself. With com0com's default wiring its DCD then follows the host's DTR, and
RustModem takes DTR going on as the host having opened the port. A line is
ready when the port is open and DTR is on, so both set-ups work; `dcd=ropen`
is the better one.

RustModem opens its port at 115200 bit/s, 8 data bits, no parity, 1 stop bit,
with no flow control in the driver. With baud rate emulation off (the com0com
default) the rate does not matter.

## Commands

Run these in an elevated prompt, in the com0com directory. `CNCA0` and `CNCB0`
are the names com0com gives the first pair; a second pair is `CNCA1` and
`CNCB1`, and so on.

Install one pair. The first port gets a COM name chosen by Windows; the second
keeps its `CNCB` name and is RustModem's:

```
setupc install PortName=COM# -
```

See which names the pair got:

```
setupc list
```

Wire the host's side, so that it sees DCD and RI from RustModem:

```
setupc change CNCA0 dcd=rout1,ri=rout2
```

Wire RustModem's side, so that it sees the host open the port:

```
setupc change CNCB0 dcd=ropen
```

To give the host's port a particular COM number:

```
setupc change CNCA0 RealPortName=COM8
```

Repeat for each line with the next pair (`CNCA1` and `CNCB1`, and so on).

Leave `EmuBR` and `EmuOverrun` off, which is their default.

## The RustModem configuration

Each line names RustModem's port of its pair:

```toml
[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }

[[line]]
id = 2
serial = { backend = "com0com", port = "CNCB1" }
```

The host then uses `COM8` (or whatever name `CNCA0` was given) as line 1.

## Checking a pair

With the service running in console mode (`rustmodem-svc --console`), open the
host's port in a terminal program at any speed and check each of these.

- [ ] `rustmodem-ctl lines` shows the line as `not_ready` before the terminal
      opens the port and `idle` once it has.
- [ ] The terminal shows DSR and CTS on, and DCD and RI off.
- [ ] Typing `AT` and Enter gives `OK`.
- [ ] `AT&C0` turns DCD on in the terminal; `AT&C1` turns it off again.
- [ ] Dialling the line from another line (`ATD1` on line 2) turns RI on for
      two seconds in every six, and the terminal shows `RING`.
- [ ] After `ATA`, both terminals show `CONNECT` and DCD on, and what is typed
      in one appears in the other.
- [ ] Dropping DTR in one terminal (or closing it) gives `NO CARRIER` in the
      other.
- [ ] Closing the terminal returns the line to `not_ready`.

If DCD or RI never change, the host's port is not wired with `dcd=rout1` and
`ri=rout2`. If the line never leaves `not_ready`, check that RustModem's port
name in the configuration is the `CNCB` one and that DTR is on in the
terminal.
