# rustmodem

A software modem pool for 64-bit Windows, by Quartz Systems.

A Windows service terminates SIP/RTP, runs one in-house softmodem per call,
and presents each line to Windows as a modem on a COM port, so Routing and
Remote Access (RRAS) answers dial-up calls as if the modems were hardware. A
manager GUI and a command-line client control the service over a local API.
Everything is written in-house in Rust under the MIT licence.

## Status

Phase 4 of ten, and the first release, 0.1.0: a modem that dial-up
networking can use. A line answers a call as a modem does and connects at
2400 bit/s by V.22bis, at 1200 by V.22 or Bell 212A, or at 300 by V.21 or
Bell 103, agreeing which by V.8 where the caller has it and by what the
caller answers where it has not. At 1200 and 2400 bit/s the call has the
error control of V.42 (LAPM) and the compression of V.42bis where the far
end has them, and start-stop characters by V.14 where it has not. The modem
is written in-house from the ITU-T Recommendations, with the G.711 codecs
and the signal processing under it, and runs on the media thread of each
call. It has connected to itself, over loopback and through a simulated
telephone channel with its faults; it has not yet met a modem that is not
its own. See [docs/PHASE4.md](docs/PHASE4.md) and
[docs/MODEM.md](docs/MODEM.md); [docs/PHASE3.md](docs/PHASE3.md) is the
record of the 300 bit/s modem under it.

To install a release, [docs/INSTALL.md](docs/INSTALL.md); to answer
dial-up calls with Routing and Remote Access,
[docs/RRAS.md](docs/RRAS.md).

Under it is what the earlier phases built. The service speaks SIP and RTP
itself: it takes calls from a trunk and routes them through hunt groups,
registers lines to a PBX as extensions, rings the line's COM port, and
carries G.711 both ways sample-exactly. The manager,
`rustmodem-manager.exe`, shows every line live and carries the operator's
actions: the modem's settings, the diagnostics of a line's call and a
loopback self-test among them. See [docs/PHASE0.md](docs/PHASE0.md),
[docs/PHASE1.md](docs/PHASE1.md) and [docs/PHASE2.md](docs/PHASE2.md).

The service's control plane runs on Tokio: the pool on one task, the
control server, the named pipe and the SIP socket beside it. Each serial
port and each call has a thread of its own, and the audio path has no
runtime in it at all. [docs/RUNTIME.md](docs/RUNTIME.md) says what runs
where.

## Layout

| Folder | Contents |
|---|---|
| `dsp/` | G.711, the signal processing blocks, the tones of a call's start, and a simulated telephone channel for tests |
| `modem/` | The V.21 and Bell 103 modem, the V.22, Bell 212A and V.22bis modem, V.8, and the engine that runs a call with them; the null and echo engines |
| `link/` | What is between the modem and the host at 1200 and 2400 bit/s: V.14, V.42 with LAPM, and V.42bis |
| `dte/` | The AT command interpreter and the serial backends |
| `voip/` | SDP, RTP and SIP, written in-house and free of I/O |
| `service/` | Messages, configuration, control API, the pool, the media threads, the SIP router, and the service with its runner |
| `ui/` | The manager: its data model, its icons, the Win32 wrappers and the window |
| `tools/` | `rustmodem-ctl`, the demo service, the test SIP peer, the capture replay tool and the fuzz targets |
| `docs/` | Architecture, the runtime model, the modem, the AT command set, the control API, telephony, call capture, gateways, com0com set-up, the install from a release, and dial-up networking with RRAS |
| `ci/` | The licence header and version checks, and the check that Tokio stays out of the pure crates |

## Building

The toolchain is pinned in `rust-toolchain.toml`. The binaries build for
`x86_64-pc-windows-msvc` only.

```
cargo build --release -p svc -p ctl -p manager
```

This produces `target/release/rustmodem-svc.exe`,
`target/release/rustmodem-ctl.exe` and `target/release/rustmodem-manager.exe`.
The manager looks for `rustmodem-svc.exe` in its own folder when it is asked
to install or remove the service.

## Running

Set up com0com as [docs/COM0COM.md](docs/COM0COM.md) describes, write a
configuration file, and run the service in the foreground from an elevated
prompt:

```
rustmodem-svc --console C:\rustmodem\rustmodem.toml
rustmodem-ctl lines
rustmodem-ctl watch
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
rustmodem-ctl sip
rustmodem-ctl groups
rustmodem-ctl routes
rustmodem-ctl capture on 1
rustmodem-ctl capture check <file>
```

A line's engine is the modem unless its configuration says otherwise.
`[modem]` has the modulations it may use, whether it uses V.8, the level
it sends at, its error control and whether it compresses, and a line can
have its own; [docs/MODEM.md](docs/MODEM.md) is the reference, and
[docs/rustmodem.sample.toml](docs/rustmodem.sample.toml) a configuration
to start from. To see what the modem of a call is doing, and to try the
modems of two idle lines against each other with no network between them:

```
rustmodem-ctl diag 1
rustmodem-ctl selftest 1 2
```

The AT command set is in [docs/AT.md](docs/AT.md) and the control API in
[docs/CONTROL_API.md](docs/CONTROL_API.md).

## Checks

```
bash ci/headers.sh
bash ci/version.sh
bash ci/pure.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo test --workspace
```

The tests of the manager open its window on the desktop, against the demo
service. Its screenshot tests compare each view with a golden picture
blessed on the CI runner image; anywhere else they report what differs and
pass. See [docs/PHASE1.md](docs/PHASE1.md). The soak test holds 24 calls
over SIP and RTP on loopback for a minute, a third of them between modems,
and fails on one packet lost; see [docs/PHASE2.md](docs/PHASE2.md),
[docs/PHASE3.md](docs/PHASE3.md) and [docs/PHASE4.md](docs/PHASE4.md).

The fuzz targets (`at_input`, `sdp_parse`, `rtp_packet`, `sip_message`,
`fsk_demod`, `v8_decode`, `v22_demod`, `hdlc_deframe`, `lapm_frames`,
`v42bis_decode`, `v42bis_roundtrip`) need a nightly toolchain and
cargo-fuzz, on Linux or WSL:

```
cargo +nightly fuzz run --fuzz-dir tools/fuzz sip_message -- -max_total_time=600
```

## Licence

MIT. See [LICENSE](LICENSE).
