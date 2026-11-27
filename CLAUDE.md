# rustmodem

rustmodem is a software modem pool for 64-bit Windows, a Quartz Systems
product. A Windows service terminates SIP/RTP, runs one in-house softmodem per
call, and presents each line to Windows as a modem on a COM port, so RRAS
answers dial-up calls as if the modems were hardware. A manager GUI and a CLI
control the service over a local API; everything is in-house Rust under MIT.

## Ground rules

1. **Commits.** Every commit is authored and committed under Cody's git
   identity only, as configured in the repository. No Claude author or
   committer, no `Co-Authored-By` trailer, no "Generated with" line, no
   session link in a message or pull request. If the configured identity is
   missing or is not Cody's, stop and ask. Messages are one-line conventional
   commits. Never force-push or rewrite pushed history.
2. **Licence header.** Every source file starts with exactly these three
   lines, before any module documentation. Files with another comment syntax
   (TOML, PowerShell, shell) carry the same text in that syntax.

   ```rust
   // Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
   // licensed under the terms of the MIT license which can be found in the
   // root directory of this project.
   ```

3. **References, not copies.** VoIPModem, libv90modem, spandsp, linmodem,
   com0com and other GPL or LGPL projects may be read as references for
   behaviour, algorithms and edge cases. Never copy from them: no pasted code,
   no line-by-line translation, no reproduction of their file layout, function
   structure, identifiers or comments. Do not vendor, link or bundle them.
   Cite the ITU-T clause in a comment where behaviour comes from a
   Recommendation. Record which references were consulted in the phase record.
4. **Dependencies.** Only licences on the allow list in `deny.toml`. No GPL,
   LGPL or AGPL anywhere in the graph. Prefer few, well-known crates. No crate
   that compiles or links C unless there is no alternative; ask first.
5. **Platform.** The binaries are `x86_64-pc-windows-msvc` only. The pure
   crates must also build and pass tests on Linux, where CI runs them.
6. **No scope creep.** Build only the current phase. Where later work will
   attach, leave the seam and nothing more.
7. **Markers.** Unfinished or deferred code is marked
   `BLOCKED(phaseN): reason` or `NEEDS-CODY: reason`. No bare `TODO`, `FIXME`
   or `unimplemented!()`.
8. **`unsafe`.** `#![forbid(unsafe_code)]` in every crate except `dte/serial`,
   `service/svc`, `service/pipe`, `service/media`, `ui/win32` and
   `tools/alloccount`, where each
   `unsafe` block carries a `// SAFETY:` comment. In `service/media` it is
   in `os.rs` only, for thread priority (MMCSS) and the socket calls `std`
   cannot make; every other module of the crate forbids it. In
   `service/svc` it is in the program only: the library, which is the
   runner, forbids it. Everything the manager needs from Win32 goes through
   a safe wrapper in `ui/win32`.
   `tools/alloccount` is the counting allocator of the tests that assert
   a path allocates nothing: an allocator cannot be written without
   `unsafe`, so it is there and in no modem crate or test of one.
9. **Ask, do not guess,** when the handoff is silent on something that changes
   an interface. Record the answer in the phase record.
10. **Tokio lives in the I/O shell of the service.** A pure crate or a media
    thread that gains a Tokio dependency is a defect; `ci/pure.sh` fails the
    build on one. `docs/RUNTIME.md` has the model, and the rule for adding
    I/O.

    Where Tokio is used:

    | Place | Use |
    |---|---|
    | `service/svc` | Owns the runtime. The pool's task, the serial bridge, the control listener, the SIP socket's task, the binding refresh, shutdown |
    | `service/ctlsrv` | The control server: a reading task and a writing task for each client |
    | `service/pipe`, feature `server` | The named pipe server on `tokio::net::windows::named_pipe` |
    | `tools/demosvc` | The same runner as the service |

    Where Tokio is not used, and must not appear as a dependency:

    | Place | Why |
    |---|---|
    | `dsp/*`, `modem/fsk`, `modem/v22`, `modem/v8`, `link/*`, `dte/at`, `modem/engine`, `service/msg`, `service/config`, `service/ctlproto`, `service/pool`, `voip/sdp`, `voip/rtp`, `voip/sip`, `service/telephony`, `ui/model`, `ui/glyph` | Pure state machines given bytes and a time. They stay that way: it is what makes them testable on Linux with a mock clock |
    | Media threads in `service/media` | Real-time. A dedicated OS thread for each call, with nothing on the tick path that can allocate, lock or yield to a scheduler |
    | Serial port I/O in `dte/serial` | Overlapped I/O on COM handles has no Tokio reactor. Each port keeps a thread and talks to the pool's task through channels |
    | The capture writer | It has its own thread fed by a ring |
    | `ui/manager`, `ui/win32`, `tools/ctl` | A Win32 message loop and a command-line client. They keep the blocking pipe client |
    | `tools/sippeer`, `tools/replay` | The test peer keeps its own sockets and clock; the replay tool reads a file |

Every user-visible error is an `RMD` message from `service/msg`. No free-text
error strings cross a crate boundary to the user. The manager shows a message
in a task dialog: the id in the title, the text as the main instruction, the
cause and the recovery in the part that expands.

The manager is built from the stock Win32 common controls and nothing else:
no toolkit that draws its own widgets, no theme, palette or font of its own.
If a stock control exists for a job, use it.

## Layout

No `crates/` directory: a folder per category at the root, one crate per
subfolder, package names equal to folder names.

```
dsp/g711            the G.711 u-law and A-law codecs (pure)
dsp/dspkit          oscillators, filters, detectors, level control,
                    resampling; the deterministic maths under them (pure)
dsp/tones           answer tone, ANSam and calling tone, and their
                    detectors (pure)
dsp/chansim         a telephone channel with its faults, from a seed, for
                    tests (pure)
modem/fsk           the V.21 and Bell 103 modulator and demodulator, and
                    start-stop framing (pure)
modem/v22           V.22, Bell 212A and V.22bis: the modulator and
                    demodulator at 1200 and 2400 bit/s (pure)
modem/v8            V.8: the signals and both ends of the procedure (pure,
                    sans-I/O)
modem/engine        ModemEngine trait, events, the Modem (V.8, V.21 and
                    Bell 103 from answer to carrier lost), NullEngine,
                    EchoEngine
link/v14            V.14: start-stop characters over a synchronous line, and
                    the character formats (pure)
link/v42            V.42: the detection phase, HDLC framing and LAPM error
                    control (pure, sans-I/O)
link/v42bis         V.42bis: data compression (pure)
dte/at              Hayes interpreter (pure)
dte/serial          SerialBackend trait, memory backend, com0com backend
voip/sdp            session descriptions, offer/answer for G.711 (pure)
voip/rtp            RTP packets, the tick-on-receive receiver, captures (pure)
voip/sip            SIP messages, transactions, dialogs, digest, registration
                    (pure, sans-I/O)
service/msg         RMD message catalogue
service/config      TOML schema, validation, write-back
service/ctlproto    control API message types
service/ctlsrv      control API server, on Tokio
service/pipe        named pipe transport: the listener, on Tokio and behind
                    the `server` feature, and the blocking client
service/pool        lines, line state machine, the media seam, null switch,
                    statistics
service/media       media threads: RTP sockets, the tick loop, call capture
service/telephony   the SIP call router: trunk, registrations, hunt groups
                    (pure, sans-I/O)
service/svc         rustmodem-svc.exe, and as a library the runner: the
                    runtime, the pool's task, the serial bridge, the SIP
                    socket
ui/model            the manager's data model (pure)
ui/glyph            icons drawn in code; .ico and .res writers (pure)
ui/win32            safe wrappers over the Win32 controls the manager uses
ui/manager          rustmodem-manager.exe, "RustModem Manager"
tools/ctl           rustmodemctl.exe
tools/demosvc       rustmodem-demosvc.exe: a pool on memory ports, scripted
tools/sippeer       rustmodem-sippeer.exe: a scriptable SIP and RTP peer for
                    tests (not shipped)
tools/replay        rustmodem-replay.exe: replays a call capture through the
                    modem engine (not shipped)
tools/alloccount    the counting allocator of the tests that assert a path
                    allocates nothing (not shipped)
tools/fuzz          cargo-fuzz targets (own lockfile, outside the workspace)
```

Pure crates (no OS calls, must pass on Linux): `g711`, `dspkit`, `tones`,
`chansim`, `fsk`, `v22`, `v8`, `engine`, `v14`, `v42`, `v42bis`, `at`,
`sdp`, `rtp`, `sip`, `msg`, `config`, `ctlproto`, `pool`, `glyph`, `model`,
and the trait and memory backend of `serial`. `media`, `telephony`, `sippeer` and `replay` use
sockets and threads and nothing of Windows that has no counterpart: they
build and pass on Linux too, and CI runs them there. `ctlsrv` and the
library of `svc` are not pure, being on Tokio, but need nothing of Windows:
CI lints and tests them on Linux as well. The program of `svc` is for
Windows only.

The `voip` crates and the router are sans-I/O: they are given bytes and the
time and give back bytes, timers and events. The one async runtime is
Tokio, in the service's I/O shell and nowhere else (rule 10): the pool is
owned by one task, every serial port and every call has a thread, and
`docs/RUNTIME.md` says what runs where. No SIP, SDP or RTP crate: these
are in-house.

The media path is sample-exact, and that is the property to keep: for every
10 ms frame a call delivers to its modem, exactly one frame is sent back,
with no jitter buffer and no clock but the far end's. `docs/TELEPHONY.md`
says how the service behaves on a SIP network, `docs/CAPTURE.md` how a call
is captured and checked, and `docs/GATEWAYS.md` what to set on a gateway.

The modem is written from the ITU-T Recommendations, which are in the
git-ignored `refs/itu/`: the clause is cited beside every constant that
comes from one, and where a handoff and a Recommendation differ the
Recommendation wins and the phase record says so. All signal processing is
`f32` at 8000 Hz, with 1.0 the overload point of G.711; levels in dBm0 go
through `dspkit`. Nothing under `dsp/` or `modem/` reads a clock or a
random number it was not given, so a call given the same samples does the
same thing on every machine. `ModemEngine::process` is the tick path: it
allocates nothing, takes no lock, logs nothing, makes no system call and
cannot panic, and `modem/engine/tests/alloc.rs` counts to keep it so.
`docs/MODEM.md` is the modem's reference.

## Phases

| Phase | Name |
|---|---|
| 0 | Foundation and shell |
| 1 | Manager GUI (native Win32 controls) |
| 2 | Telephony (SIP, RTP, hunt groups) |
| 3 | 300 bit/s (G.711, V.21, V.8) |
| 4 | V.22bis and V.42 (first release) |
| 5 | Virtual COM driver and installer |
| 6 to 8 | V.32bis, V.34, V.90 |
| 9 | Extensions |

The record of each phase is in `docs/`: `docs/PHASE0.md`, `docs/PHASE1.md`,
`docs/PHASE2.md`, `docs/PHASE3.md`. Phases 0 to 3 are built. After phase 2 the service's
control plane was moved onto Tokio (0.0.4); `docs/RUNTIME.md` is the
runtime model and the record of that.

## Running the manager

The manager needs no com0com and no elevated prompt against the demo
service, which runs a real pool on in-memory ports with scripted calls and
admits the user who started it:

```
cargo run -p demosvc -- --script busy --lines 12
cargo run -p manager -- --demo
```

`--script frozen` gives the same lines, calls and statistics on every run,
which is what the screenshot tests use; `--script idle` gives lines and no
calls; `--script sip` runs the real SIP router and media threads on the
loopback interface, with test peers calling a hunt group of every line and
the last line registered to one of them. Against the real service the manager is `rustmodem-manager.exe` with no
arguments, and needs administrator rights, which it offers to get.

## Checks

Run all of these before every commit:

```
bash ci/headers.sh
bash ci/version.sh
bash ci/pure.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo test --workspace
cargo build --release -p svc -p ctl -p manager
```

The pure crates on Linux (CI does this; WSL works locally):

```
cargo fmt --check
cargo clippy -p g711 -p dspkit -p tones -p chansim -p fsk -p v22 -p v8 -p engine -p v14 -p v42 -p v42bis -p at -p serial -p sdp -p rtp -p sip -p msg -p config -p ctlproto -p pool -p media -p telephony -p sippeer -p alloccount -p replay -p glyph -p model --all-targets -- -D warnings
cargo test -p g711 -p dspkit -p tones -p chansim -p fsk -p v22 -p v8 -p engine -p v14 -p v42 -p v42bis -p at -p serial -p sdp -p rtp -p sip -p msg -p config -p ctlproto -p pool -p media -p telephony -p sippeer -p alloccount -p replay -p glyph -p model
cargo clippy -p ctlsrv --all-targets -- -D warnings
cargo clippy -p svc --lib --test '*' -- -D warnings
cargo test -p ctlsrv
cargo test -p svc --lib --test '*'
```

Run that job whole before a push, not the tests alone: code that is dead on
Linux only fails its clippy and nothing else. The list is `PURE` in
`.github/workflows/ci.yml`, and the last four lines are the steps after it
there: the control server and the runner.

`cargo test --workspace` opens the manager's window many times over: its
tests run it against the demo service on the desktop of whoever runs them.
Two tests take a minute each: the manager's smoke test, and the soak test
(`service/svc/tests/soak.rs`), which holds 24 calls over SIP and RTP
on loopback, a third of them between modems, on the service's own runner,
and fails on one packet lost. `RM_SOAK_MINUTES=10` makes the
soak ten minutes, and `RM_CAPTURE_MINUTES=10` does the same for the
captured calls of `service/svc/tests/peer.rs`. `RM_MODEM_KIB=1` makes the
modem calls of `service/svc/tests/transparency.rs` carry 1 KiB each way
where they carry 64, and `RM_CORPUS=<dir>` keeps the captures the peer tests make for the corpus
in `modem/engine/tests/corpus/`.

The screenshot tests (`ui/manager/tests/screenshots.rs`) compare the main
window and every dialog with the goldens in `ui/manager/tests/goldens/`. The
goldens are blessed on the CI runner image, so the comparison fails the run
on CI only; elsewhere it reports what differs and passes. The pictures, and
for a view that fails its golden and a picture of the difference, go to
`target/screenshots/`, which CI uploads. After a change that is meant to
change how a view looks, take the new goldens from that artifact of a CI
run, look at each, and commit them. `RM_BLESS=1` writes the pictures of the
machine at hand as goldens, `RM_SCREENSHOTS=strict` fails on a difference
anywhere, and `RM_SCREENSHOTS=skip` takes no pictures.

The fuzz targets, `at_input`, `sdp_parse`, `rtp_packet`, `sip_message`,
`fsk_demod`, `v8_decode`, `v22_demod`, `hdlc_deframe`, `lapm_frames`,
`v42bis_decode` and `v42bis_roundtrip`
(nightly toolchain and cargo-fuzz, on Linux or WSL):

```
cargo +nightly fuzz run --fuzz-dir tools/fuzz at_input -- -max_total_time=600
```

`docs/CONTROL_API.md` is generated from `service/ctlproto`; regenerate it with
`UPDATE_DOCS=1 cargo test -p ctlproto` after changing the API.
