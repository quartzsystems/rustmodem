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
8. **`unsafe`.** `#![forbid(unsafe_code)]` in every crate except `dte/serial`
   and `service/svc`, where each `unsafe` block carries a `// SAFETY:` comment.
9. **Ask, do not guess,** when the handoff is silent on something that changes
   an interface. Record the answer in the phase record.

Every user-visible error is an `RMD` message from `service/msg`. No free-text
error strings cross a crate boundary to the user.

## Layout

No `crates/` directory: a folder per category at the root, one crate per
subfolder, package names equal to folder names.

```
modem/engine        ModemEngine trait, events, NullEngine
dte/at              Hayes interpreter (pure)
dte/serial          SerialBackend trait, memory backend, com0com backend
service/msg         RMD message catalogue
service/config      TOML schema, validation, write-back
service/ctlproto    control API message types
service/ctlsrv      control API server
service/pool        lines, line state machine, null switch
service/svc         rustmodem-svc.exe
tools/ctl           rustmodemctl.exe
tools/fuzz          cargo-fuzz targets (own lockfile, outside the workspace)
```

Pure crates (no OS calls, must pass on Linux): `engine`, `at`, `msg`,
`config`, `ctlproto`, `pool`, and the trait and memory backend of `serial`.

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

The record of each phase is in `docs/` (`docs/PHASE0.md` for phase 0).

## Checks

Run all of these before every commit:

```
bash ci/headers.sh
bash ci/version.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo test --workspace
cargo build --release -p svc -p ctl
```

The pure crates on Linux (CI does this; WSL works locally):

```
cargo test -p engine -p at -p serial -p msg -p config -p ctlproto -p pool
```

The fuzz target (nightly toolchain and cargo-fuzz, on Linux or WSL):

```
cargo +nightly fuzz run --fuzz-dir tools/fuzz at_input -- -max_total_time=600
```

`docs/CONTROL_API.md` is generated from `service/ctlproto`; regenerate it with
`UPDATE_DOCS=1 cargo test -p ctlproto` after changing the API.
