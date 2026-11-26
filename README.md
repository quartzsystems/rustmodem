# rustmodem

A software modem pool for 64-bit Windows, by Quartz Systems.

A Windows service terminates SIP/RTP, runs one in-house softmodem per call,
and presents each line to Windows as a modem on a COM port, so Routing and
Remote Access (RRAS) answers dial-up calls as if the modems were hardware. A
manager GUI and a command-line client control the service over a local API.
Everything is written in-house in Rust under the MIT licence.

## Status

Phase 1 of ten: the product skeleton and its operator's window. There is no
telephony and no DSP yet. The service owns lines, speaks the AT command set
on COM ports, connects two lines to each other through a stand-in "null"
modem, and is controlled over a named pipe. The manager, `rustmodem-manager.exe`,
shows every line live and carries the operator's actions. See
[docs/PHASE0.md](docs/PHASE0.md) and [docs/PHASE1.md](docs/PHASE1.md).

## Layout

| Folder | Contents |
|---|---|
| `modem/` | The modem engine trait and the null engine |
| `dte/` | The AT command interpreter and the serial backends |
| `service/` | Messages, configuration, control API, the pool and the service |
| `ui/` | The manager: its data model, its icons, the Win32 wrappers and the window |
| `tools/` | `rustmodemctl`, the demo service and the fuzz targets |
| `docs/` | Architecture, the AT command set, the control API, com0com set-up |
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
pass. See [docs/PHASE1.md](docs/PHASE1.md).

The fuzz target needs a nightly toolchain and cargo-fuzz, on Linux or WSL:

```
cargo +nightly fuzz run --fuzz-dir tools/fuzz at_input -- -max_total_time=600
```

## Licence

MIT. See [LICENSE](LICENSE).
