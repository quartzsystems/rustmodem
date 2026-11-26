# rustmodem

A software modem pool for 64-bit Windows, by Quartz Systems.

A Windows service terminates SIP/RTP, runs one in-house softmodem per call,
and presents each line to Windows as a modem on a COM port, so Routing and
Remote Access (RRAS) answers dial-up calls as if the modems were hardware. A
manager GUI and a command-line client control the service over a local API.
Everything is written in-house in Rust under the MIT licence.

## Status

Phase 2 of ten: telephony. Real telephone calls reach the lines. The service
speaks SIP and RTP itself: it takes calls from a trunk and routes them
through hunt groups, registers lines to a PBX as extensions, rings the
line's COM port, and carries G.711 both ways sample-exactly. There is no
DSP yet: nothing demodulates the audio. A stand-in engine echoes it, and
the "null" modem of phase 0 is carried across real RTP to prove the path
alters nothing. The manager, `rustmodem-manager.exe`, shows every line live
and carries the operator's actions, the SIP ones among them. See
[docs/PHASE0.md](docs/PHASE0.md), [docs/PHASE1.md](docs/PHASE1.md) and
[docs/PHASE2.md](docs/PHASE2.md).

## Layout

| Folder | Contents |
|---|---|
| `modem/` | The modem engine trait, the null engine and the echo engine |
| `dte/` | The AT command interpreter and the serial backends |
| `voip/` | SDP, RTP and SIP, written in-house and free of I/O |
| `service/` | Messages, configuration, control API, the pool, the media threads, the SIP router and the service |
| `ui/` | The manager: its data model, its icons, the Win32 wrappers and the window |
| `tools/` | `rustmodemctl`, the demo service, the test SIP peer and the fuzz targets |
| `docs/` | Architecture, the AT command set, the control API, telephony, call capture, gateways, com0com set-up |
| `ci/` | The licence header and version checks |

## Building

The toolchain is pinned in `rust-toolchain.toml`. The binaries build for
`x86_64-pc-windows-msvc` only.

```
cargo build --release -p svc -p ctl -p manager
```

This produces `target/release/rustmodem-svc.exe`,
`target/release/rustmodemctl.exe` and `target/release/rustmodem-manager.exe`.
The manager looks for `rustmodem-svc.exe` in its own folder when it is asked
to install or remove the service.

## Running

Set up com0com as [docs/COM0COM.md](docs/COM0COM.md) describes, write a
configuration file, and run the service in the foreground from an elevated
prompt:

```
rustmodem-svc --console C:\rustmodem\rustmodem.toml
rustmodemctl lines
rustmodemctl watch
```

Or install it as the Windows service `rustmodem`:

```
rustmodem-svc --install
net start rustmodem
```

Then start the manager, `rustmodem-manager.exe`. The service's pipe admits
SYSTEM and Administrators only; started without administrator rights the
manager says so and offers to start again with them.

To try the manager with no com0com and no elevated prompt, run it against
the demo service, a real pool on in-memory ports with scripted calls:

```
cargo run -p demosvc -- --script busy --lines 12
cargo run -p manager -- --demo
```

To see calls arrive over SIP with no gateway, the demo service has a script
that runs the real SIP router and media threads on the loopback interface,
with test peers placing calls:

```
cargo run -p demosvc -- --script sip --lines 8
cargo run -p manager -- --demo
```

For telephone calls, add `[sip]` to the configuration: the addresses that
may send calls, hunt groups and routes for the numbers they call, and a
`register` table on a line that is an extension of a PBX.
[docs/TELEPHONY.md](docs/TELEPHONY.md) is the reference, and
[docs/GATEWAYS.md](docs/GATEWAYS.md) says what to turn off on the gateway
in front. A call can be captured and checked for lost and slipped samples:
[docs/CAPTURE.md](docs/CAPTURE.md).

```
rustmodemctl sip
rustmodemctl groups
rustmodemctl routes
rustmodemctl capture on 1
rustmodemctl capture check <file>
```

The AT command set is in [docs/AT.md](docs/AT.md) and the control API in
[docs/CONTROL_API.md](docs/CONTROL_API.md).

## Checks

```
bash ci/headers.sh
bash ci/version.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo test --workspace
```

The tests of the manager open its window on the desktop, against the demo
service. Its screenshot tests compare each view with a golden picture
blessed on the CI runner image; anywhere else they report what differs and
pass. See [docs/PHASE1.md](docs/PHASE1.md). The soak test holds 24 calls
over SIP and RTP on loopback for a minute and fails on one packet lost; see
[docs/PHASE2.md](docs/PHASE2.md).

The fuzz targets (`at_input`, `sdp_parse`, `rtp_packet`, `sip_message`) need
a nightly toolchain and cargo-fuzz, on Linux or WSL:

```
cargo +nightly fuzz run --fuzz-dir tools/fuzz sip_message -- -max_total_time=600
```

## Licence

MIT. See [LICENSE](LICENSE).
