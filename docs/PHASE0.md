# Phase 0 record: foundation and shell

Phase 0 builds the whole product skeleton with no telephony, no DSP and no
GUI: a service that owns lines, speaks the AT command set on COM ports,
connects two lines to each other through a stand-in "null" modem, and is
controlled over a named pipe.

All eleven work packages are built and committed. The four automated exit
criteria pass. The three manual ones need a machine with com0com and an
elevated prompt; they are a checklist at the end.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| com0com signal mapping | `dte/serial/src/com0com.rs` (three `NEEDS-CODY` markers), `docs/COM0COM.md` | The com0com backend has never opened a real port: no pair was available. Run the checklist in `docs/COM0COM.md`. The pin parameter names in the `setupc change` commands are from memory of `setupc help`; check them there |
| V.250 clause numbers | Comments in `dte/at/src/`, the clause column in `docs/AT.md` | The clause citations were written from memory of the Recommendation's structure. The text was not to hand. Check them against V.250 before relying on them |
| Decisions made without asking | "Decisions to confirm" below | Ground rule 9 says to ask when the handoff is silent on something that changes an interface. Nobody was there to ask, so each was decided and is listed for review |
| Manual exit criteria 5, 6, 7 | "Manual checklist" below | Run them |
| Architecture guide | `docs/ARCHITECTURE.md` | Drop in the exported guide; the file holds a one-paragraph summary until then |

## What was built

### 1. Workspace

The layout, root files, CI and check scripts as the handoff lists them.
`ci/headers.sh` checks the three header lines on every Rust file, tracked or
untracked-but-not-ignored. `ci/version.sh` and `ci/version.ps1` check that
`Cargo.toml` agrees with `VERSION`, and `prepare <version>` updates both. The
toolchain is pinned at 1.100.0, the stable release when the phase was built.

### 2. `service/msg`

59 messages in `messages.toml`, compiled in with `include_str!` and parsed
once. `message(id, &[args])` returns a `Message` that displays as
`RMD0010: text`. Tests check that every id is unique, well-formed and in a
phase 0 range, and that each message uses exactly the variables it documents.

### 3. `service/config`

The schema of the handoff, validation that reports every problem at once with
key and line number, and write-back through `toml_edit` that keeps comments,
ordering and unknown sections. Two property tests: any valid configuration
round-trips, and comments survive an edit.

### 4. `dte/at`

The interpreter as a pure state machine: `AtInterpreter::step(Input, &mut
Vec<Output>)`. Every command and register of the handoff's tables is built
and documented in `docs/AT.md`.

- `tests/table.rs`: one case for each row of both tables.
- `tests/transcripts/`: 46 transcripts, including the Windows standard-modem
  init strings and a full dial, connect, escape, hang-up exchange.
- `tests/escape.rs`: property tests of the escape sequence around the guard
  boundaries, one of them against the rule stated independently over a
  timeline, and a property test that any interleaving of inputs leaves the
  interpreter in a valid state.
- `tools/fuzz/fuzz_targets/at_input.rs`: the fuzz target.

### 5. `modem/engine`

The `ModemEngine` trait, `ModemEvent`, `ConnectInfo`, and `NullEngine`. The
null engine frames data bytes into the octet stream with one escape octet,
sends an idle octet when it has nothing to say, and reports the carrier lost
after a configurable number of ticks of silence.

### 6. `dte/serial`

The `SerialBackend` trait, `MemoryBackend` with its `MemoryHost` test handle,
and `Com0comBackend` for Windows. The com0com backend uses overlapped I/O
through the `windows` crate. It drives DTR, RTS, OUT1 and OUT2 with
`IOCTL_SERIAL_SET_MODEM_CONTROL` and expects the pair wired as
`docs/COM0COM.md` describes.

### 7. `service/pool`

`Pool`, `Line`, the `CallRouter` trait and `NullSwitch`. The pool reads no
clock and opens no files: the caller gives it the time, the serial ports, an
event sink and a call log writer. 35 tests drive it on memory ports with a
mock clock: each bullet of the null switch, 1 MB each way, `+++`, `ATO`,
`ATH`, the operator's actions, configuration changes, port failures and
shutdown.

### 8. `service/ctlproto` and `service/ctlsrv`

`ctlproto` holds the serde types, the fifteen methods and the five events,
with no I/O. `docs/CONTROL_API.md` is generated from tables in
`ctlproto/src/doc.rs`, and the tests check those tables against the types and
the file against the tables.

`ctlsrv` is the server behind a `Listener`/`Connection` transport trait, with
an in-memory transport for tests. Each client has a bounded queue: the oldest
`log` events are dropped first, state events are never dropped, and a client
that falls further behind is disconnected.

### 9. `service/svc`

`rustmodem-svc.exe`: `--console`, `--install`, `--uninstall`, and service
mode through the `windows-service` crate. It has the rotating log (10 MiB a
file, ten kept), the Event Log, the named pipe transport, and the pool's
thread, which sleeps on the ports' wait handles between ticks.

### 10. `tools/ctl`

`rustmodemctl.exe` with the commands of the handoff, tables by default and
`--json` for scripts.

### 11. This record

## Markers

Every `BLOCKED` and `NEEDS-CODY` in the tree:

| Marker | Location | What is deferred |
|---|---|---|
| `BLOCKED(phase1)` | `dte/at/src/command.rs`, the `&W` command (and its row in `docs/AT.md`) | `&W` returns `OK` and stores nothing. Stored profiles come with the manager |
| `BLOCKED(phase2)` | `service/pool/src/lib.rs`, `TICK_MS` | Engine ticks move from the pool's 20 ms timer to RTP packet arrival on media threads |
| `BLOCKED(phase2)` | `service/pool/src/router.rs`, module documentation | A SIP router replaces `NullSwitch` behind the `CallRouter` trait |
| `BLOCKED(phase5)` | `service/svc/src/service.rs`, `install` | The service runs as LocalSystem. A dedicated account comes with the installer |
| `BLOCKED(phase5)` | `service/svc/src/eventlog.rs`, module documentation | No message file is registered for the event source, so the Event Viewer shows each event's text after a "description cannot be found" note |
| `BLOCKED(phase9)` | `service/ctlsrv/src/lib.rs`, module documentation | TLS for remote control is one more transport behind `Listener` |
| `NEEDS-CODY` | `dte/serial/src/com0com.rs`, module documentation | Verify the signal mapping on a real pair |
| `NEEDS-CODY` | `dte/serial/src/com0com.rs`, `set_signals` | Verify that OUT1 and OUT2 reach the host's DCD and RI |
| `NEEDS-CODY` | `dte/serial/src/com0com.rs`, `poll` | Verify how the host's DTR, RTS and open state arrive |
| `NEEDS-CODY` | `docs/COM0COM.md` | Verify the set-up commands against `setupc help` |

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| The pipe server is in `service/ctlsrv` | The named pipe transport is in `service/svc/src/pipe.rs`. `ctlsrv` has the server, the transport trait and the in-memory transport | The pipe needs `unsafe` Win32 calls, and ground rule 8 forbids `unsafe` in `ctlsrv`. The test "the same over a real pipe" is `service/svc/src/pipe_tests.rs` |
| Work package 8's commit has the control API "over a named pipe" | The pipe itself arrived in work package 9's commit | Same reason: it is in `svc` |
| CI has a Windows job and a Linux job | CI has a third job that fuzzes `at_input` for 10 minutes | Exit criterion 4 is listed as automated and "must pass in CI" |
| Start, stop and fatal errors go to the Event Log | They do in service mode only | Console mode logs to the console. Writing to the Event Log from every test run and every foreground run would fill it with noise |
| With no arguments the program expects the service control manager | It does. A service installed with a configuration path is started as `rustmodem-svc --service <path>` | The service control manager passes the path as a process argument, and the program has to tell it from a mode |
| Pool events include "counters" | There is no counters event. `line.state` carries the whole line, counters included, and `lines.get` reports them at any time | The control API's event list has no counters event |
| `rustmodemctl` exits 0, 1 or 2 | It also exits 3 when the command line is not understood | A usage error is neither an API error nor a connection error |
| Work packages are committed and checked one at a time | Work packages 4, 5 and 6 were checked together and then committed as three commits | Work on 5 and 6 went on while the 10-minute fuzz run for 4 was running. The `Cargo.lock` in the commits for 4 and 5 is the one that goes with 6 |

## Decisions to confirm

Each of these is a choice the handoff left open. They are in the code and the
documentation as described; say if any should change.

**AT interpreter (`dte/at`)**

- `LineEvent::RingEnd` was added. The pool owns the ring cadence, so it tells
  the interpreter when the ring-on interval ends and RI goes off.
- `Action::Hangup` carries a cause (command, DTR, abort, timeout, reset,
  operator), so that call records can say why a call ended.
- `Output::Trace` was added, carrying each command line and result code for
  the debug log.
- The interpreter has `operator_answer`, `operator_hangup` and
  `operator_reset` for the control API's `lines.answer`, `lines.hangup` and
  `lines.reset`.
- A DTR drop with `&D2` or `&D3` reports `OK`, not `NO CARRIER`.
- The modem does not auto-answer while DTR is off, unless `&D0` is set.
- `NO ANSWER` is used at `X1` and above. `X2` adds `NO DIALTONE`, `X3` adds
  `BUSY` instead, `X4` has both.
- A dial string ending in `;` returns `OK` and stays in command mode. If the
  far end then connects, the modem is in online command mode.
- With `unknown_command = "ok"`, how much of an unknown command is skipped is
  a heuristic, described in `docs/AT.md`. Commands that start with `#` are
  taken as a whole word, so that `#CID=1` is skipped as one command.
- An S-register outside the table is treated as an unknown command.
- The defaults and test-form ranges of `+MS`, `+ES` and `+DS` are
  placeholders.
- `I0` prints `rustmodem`; `I4` prints `NULL MODEM,V.250,+MS,+ES,+DS`.
- Bytes that arrive in the same write as a dial or answer command line do not
  abort the call. Without this, the line feed after `ATD2<CR>` would.
- S7 at 0 times out at once.

**Pool (`service/pool`)**

- A line is ready when its port is open and the host has DTR on. The host
  closing the port is treated as DTR going off.
- A port that will not open is retried every 5 seconds, and the reason is on
  the line as `error`.
- A serial I/O failure during a call ends the call with reason `failed`,
  whatever `&D` says.
- `lines.hangup` on a ringing line turns the call away.
- Answering when nothing is calling gets `NO CARRIER` at once.

**Control API (`service/ctlproto`)**

- Hello is `{"hello":{"protocol":1,"version":"..."}}`.
- An error that does not answer a readable request has `"id": null`.
- `lines.answer`, `lines.hangup`, `lines.reset`, `lines.enable`,
  `lines.disable`, `lines.add` and `lines.edit` return the line afterwards.
- `lines.enable`, `lines.disable`, `lines.add`, `lines.edit` and
  `lines.remove` write the configuration file.
- `config.reload` applies the lines only. `[service]` and `[control]` take
  effect at the next start.
- Removing a line sends no event.
- A client queue holds 4096 messages and 512 `log` events.
- A control message may be at most 1 MiB.

**Service (`service/svc`)**

- Log timestamps are UTC.
- The log file rotates at 10 MiB.
- In console mode `rustmodemctl` still needs an elevated prompt, because the
  pipe's security is the same in both modes.

## References consulted

| Reference | For | How |
|---|---|---|
| ITU-T V.250 | `dte/at` | Cited by clause in comments. The citations are from memory and need checking (see "What needs Cody") |
| com0com `ReadMe.txt` | `dte/serial` | Read for how OUT1 and OUT2 can be wired to DCD and RING and how an application sets them. No com0com code was read |
| Windows driver kit `ntddser.h` | `dte/serial` | The value of `IOCTL_SERIAL_SET_MODEM_CONTROL` and the modem control register bits, from memory |

VoIPModem, libv90modem, spandsp and linmodem were not opened. Phase 0 has no
modulation to take reference from.

The workspace layout and `deny.toml` follow `quartz-systems-virtual-server`,
which is Quartz's own.

## Dependencies

| Crate | Used by | Licence |
|---|---|---|
| `serde`, `serde_json` | `ctlproto`, `ctlsrv`, `pool`, `ctl` | MIT OR Apache-2.0 |
| `toml_edit` | `msg`, `config` | MIT OR Apache-2.0 |
| `windows` | `serial`, `svc` | MIT OR Apache-2.0 |
| `windows-service` | `svc` | MIT OR Apache-2.0 |
| `proptest` (tests only) | `config`, `at`, `engine` | MIT OR Apache-2.0 |
| `libfuzzer-sys` (fuzz targets only) | `tools/fuzz` | (MIT OR Apache-2.0) AND NCSA |

`cargo deny check` passes. `libfuzzer-sys` compiles libFuzzer, which is C++.
It is the fuzzing engine `cargo-fuzz` needs, and the handoff asks for
cargo-fuzz targets. It is in `tools/fuzz` only, which is outside the workspace
and outside `deny.toml`'s graph, and nothing of it is in either binary.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check`, `ci/headers.sh` and all tests pass on Windows; the pure crates pass on Linux | Run locally on Windows before every commit: 169 tests. The pure crates were run under WSL (Ubuntu) with the pinned toolchain. The GitHub Actions run for every pushed commit succeeded |
| 2 | A complete call between two lines on memory backends | `a_complete_call_with_a_megabyte_each_way` in `service/pool/tests/calls.rs`: `ATD2`, `RING` on line 2, `ATA`, `CONNECT` on both, 1 MiB each way compared byte for byte, `+++`, `OK`, `ATH`, `NO CARRIER` on the far side, and the call records |
| 3 | Every method in `docs/CONTROL_API.md` has a round-trip test | `every_method_round_trips` in `service/ctlsrv/tests/api.rs` goes through the method list the document is generated from and fails on a method with no case. `every_method_round_trips_over_a_real_pipe` in `service/svc/src/pipe_tests.rs` does the same over a named pipe |
| 4 | The `at_input` fuzz target runs for 10 minutes with no findings | Run under WSL with nightly 1.101.0 and cargo-fuzz 0.13.2: 1,533,516 runs in 601 seconds, no crash, no artifact. `dte/at` has not changed since. CI now runs the same for 10 minutes on every push |

### Manual checklist

These need an elevated prompt. Criterion 5 also needs com0com.

**5. Two com0com pairs: `AT` gets `OK`, and a call works by hand.**

- [ ] Set up two pairs as `docs/COM0COM.md` describes:

  ```
  setupc install PortName=COM# -
  setupc install PortName=COM# -
  setupc list
  setupc change CNCA0 dcd=rout1,ri=rout2
  setupc change CNCB0 dcd=ropen
  setupc change CNCA1 dcd=rout1,ri=rout2
  setupc change CNCB1 dcd=ropen
  ```

- [ ] Write `C:\ProgramData\Quartz Systems\rustmodem\rustmodem.toml`:

  ```toml
  [service]
  log_level = "debug"

  [[line]]
  id = 1
  serial = { backend = "com0com", port = "CNCB0" }

  [[line]]
  id = 2
  serial = { backend = "com0com", port = "CNCB1" }
  ```

- [ ] Start the service in the foreground:

  ```
  rustmodem-svc --console
  ```

- [ ] Open a terminal on each host-side port (the COM names `setupc list`
      shows for `CNCA0` and `CNCA1`).
- [ ] In each terminal, `AT` and Enter gives `OK`.
- [ ] In terminal 1, `ATD2`. Terminal 2 shows `RING`.
- [ ] In terminal 2, `ATA`. Both show `CONNECT`.
- [ ] What is typed in one terminal appears in the other.
- [ ] In terminal 1, wait a second, type `+++`, wait a second: `OK`.
- [ ] In terminal 1, `ATH`: `OK`. Terminal 2 shows `NO CARRIER`.
- [ ] Go through the signal checklist at the end of `docs/COM0COM.md`.

**6. `rustmodemctl watch` shows each state change as it happens.**

- [ ] With the service running as above, in an elevated prompt:

  ```
  rustmodemctl watch
  ```

- [ ] Repeat the call. Each of these appears as it happens: line 1
      handshaking, line 2 ringing, both online, both calls ended, both idle.
- [ ] Close terminal 2: line 2 becomes `not_ready`.
- [ ] `rustmodemctl lines` and `rustmodemctl calls` show the lines and the
      calls just made.

**7. The service installs, starts, stops and uninstalls.**

- [ ] Stop the foreground service with Ctrl+C. It logs `RMD0002` and exits.
- [ ] In an elevated prompt:

  ```
  rustmodem-svc --install
  net start rustmodem
  rustmodemctl info
  net stop rustmodem
  rustmodem-svc --uninstall
  ```

- [ ] Each command succeeds.
- [ ] The Event Log has the start and the stop:

  ```
  powershell -Command "Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='rustmodem'} -MaxEvents 5 | Format-List TimeCreated, Id, Message"
  ```

  Event 1 is `RMD0001`, the start; event 2 is `RMD0002`, the stop. The text
  follows a note that the description cannot be found, which is the
  `BLOCKED(phase5)` above.
- [ ] `C:\ProgramData\Quartz Systems\rustmodem\logs\rustmodem.log` has the
      same two messages.

## Known limits of phase 0

- The com0com backend is untested on a real port.
- The service code paths under the service control manager (`--install`,
  `--uninstall`, start and stop as a service, the Event Log) are built and
  compile but were not run: they need an elevated prompt. Console mode was run
  end to end by the tests, including a clean stop on Ctrl+Break.
- ~~The pool waits on at most 63 port handles between ticks. With more
  lines than that, the remaining ports are served at the next 20 ms tick.~~
  Gone since 0.0.4: each port has a thread of its own. See
  [RUNTIME.md](RUNTIME.md).
- The null engine carries about 8000 bytes a second. It is a stand-in, not a
  modem.
