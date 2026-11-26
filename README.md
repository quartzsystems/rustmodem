# rustmodem

A software modem pool for 64-bit Windows, by Quartz Systems.

A Windows service terminates SIP/RTP, runs one in-house softmodem per call,
and presents each line to Windows as a modem on a COM port, so Routing and
Remote Access (RRAS) answers dial-up calls as if the modems were hardware. A
manager GUI and a command-line client control the service over a local API.
Everything is written in-house in Rust under the MIT licence.

## Status

Phase 0 of ten: the product skeleton. There is no telephony, no DSP and no
GUI yet. The service owns lines, speaks the AT command set on COM ports,
connects two lines to each other through a stand-in "null" modem, and is
controlled over a named pipe. See [docs/PHASE0.md](docs/PHASE0.md).

## Layout

| Folder | Contents |
|---|---|
| `modem/` | The modem engine trait and the null engine |
| `dte/` | The AT command interpreter and the serial backends |
| `service/` | Messages, configuration, control API, the pool and the service |
| `tools/` | `rustmodemctl` and the fuzz targets |
| `docs/` | Architecture, the AT command set, the control API, com0com set-up |
| `ci/` | The licence header and version checks |

## Building

The toolchain is pinned in `rust-toolchain.toml`. The binaries build for
`x86_64-pc-windows-msvc` only.

```
cargo build --release -p svc -p ctl
```

This produces `target/release/rustmodem-svc.exe` and
`target/release/rustmodemctl.exe`.

## Checks

```
bash ci/headers.sh
bash ci/version.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo test --workspace
```

## Licence

MIT. See [LICENSE](LICENSE).
