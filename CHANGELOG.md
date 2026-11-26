# Changelog

All notable changes to rustmodem are recorded here. Versions follow the
`VERSION` file.

## 0.0.1 (phase 0: foundation and shell)

The product skeleton, with no telephony, no DSP and no GUI. The record of the
phase is [docs/PHASE0.md](docs/PHASE0.md).

- **Workspace.** A folder per category, one crate per subfolder. CI on
  Windows and Linux, the licence header check, the version check, and a
  licence allow list in `deny.toml`.
- **Messages.** The `RMD` message catalogue: every user-visible error is a
  message with an id, a severity, a text, a cause and a recovery.
- **Configuration.** The TOML configuration file, validated all at once with
  the key and line number of each problem, and written back with comments and
  ordering kept.
- **AT commands.** A Hayes-compatible command interpreter written from ITU-T
  V.250 as a pure state machine: command lines, the registers, result codes,
  the escape sequence, ringing and auto-answer. See [docs/AT.md](docs/AT.md).
- **Modem engine.** The `ModemEngine` trait every later modem implements, and
  a null engine that carries bytes between two lines.
- **Serial.** The `SerialBackend` trait, an in-memory backend for tests, and a
  backend for com0com null-modem pairs. See
  [docs/COM0COM.md](docs/COM0COM.md).
- **Pool.** Lines, the line state machine, and a null switch that lets one
  line call another with `ATD<line id>`. Call records are kept in memory and
  appended to `calls.jsonl`.
- **Control API.** A versioned JSON API over a named pipe open to SYSTEM and
  Administrators: fifteen methods and five events. See
  [docs/CONTROL_API.md](docs/CONTROL_API.md).
- **Service.** `rustmodem-svc.exe`: a Windows service with a console mode,
  rotating log files and Event Log entries for start, stop and fatal errors.
- **Client.** `rustmodemctl.exe`: lists lines and calls, answers, hangs up,
  resets, enables and disables lines, reloads the configuration, and watches
  events.

Not yet verified on real hardware: the com0com backend, and running under the
service control manager. Both are on the manual checklist in the phase record.
