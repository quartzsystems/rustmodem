# Changelog

All notable changes to rustmodem are recorded here. Versions follow the
`VERSION` file.

## 0.0.2 (phase 1: the manager)

The operator's window, and what it needs from the service. The record of the
phase is [docs/PHASE1.md](docs/PHASE1.md).

- **Manager.** `rustmodem-manager.exe`, RustModem Manager: one window, built from the stock Win32
  common controls, that connects to the service over the control pipe. A
  menu bar, a toolbar with the line filter, the line grid, the inbound call
  statistics, the connect rates, the event log and a status bar. Lines are
  answered, hung up, reset, enabled, disabled, added, edited and removed
  from it. It keeps itself connected, and offers to start again as an
  administrator when the service's pipe does not admit it.
- **Dialogs.** Connect to service, New line and Edit line, Settings (a
  property sheet: Service and Control), Ports and drivers, Call history with
  export to CSV, and an AT console: a terminal on a COM port of the machine,
  with DTR and RTS to set and DCD, DSR, CTS and RI shown. Statistics and the
  event log can be exported, and the Windows service started, stopped,
  restarted, installed and removed from the File menu.
- **Control API.** Five methods more, all additions to protocol 1:
  `stats.get`, `log.tail`, `config.get`, `config.set` and `ports.list`. A
  log level or log directory set through `config.set` takes effect at once;
  only a new pipe name waits for the service to start again.
  `service.info` says how the service is run and when it loaded its
  configuration; a line says what is bound to its port, and counts its AT
  commands. `rustmodemctl` has `stats`, `ports` and `log`.
- **Stored profiles.** `AT&W` stores the active profile, and `ATZ`, a reset
  and a restart of the service bring it back. Profiles are kept in
  `profiles.toml` beside the configuration file.
- **Demo service.** `rustmodem-demosvc.exe`: a real pool on in-memory ports
  with scripted calls, on a pipe that admits its user, so that the manager
  can be developed and tested with no com0com and no elevated prompt.
- **Messages.** The range `RMD4000` to `RMD4999` for the manager. Every
  error the manager shows is an `RMD` message in a task dialog.
- **Tests.** The main window and every dialog are compared with golden
  pictures blessed on the CI runner image; a smoke test runs the manager for
  a minute against a busy demo service; a reconnect test kills the service
  under it.

The places of telephony in the window (the SIP menu, the SIP and Hunt group
columns, the RTP rows) are there and disabled, each with its reason in the
status bar. Not yet verified by a person: the look beside a stock Windows
program, scaling, the manager against the real service, elevation, and the
AT console on a real port. They are on the manual checklist in the phase
record.

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
