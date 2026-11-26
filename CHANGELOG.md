# Changelog

All notable changes to rustmodem are recorded here. Versions follow the
`VERSION` file.

## 0.0.4 (runtime retrofit)

The service's control plane runs on Tokio. Nothing the service does has
changed: no control API message, no file format, no AT command. A
`rustmodemctl` or a manager of 0.0.3 works against a service of 0.0.4. The
runtime model and the record of the change are in
[docs/RUNTIME.md](docs/RUNTIME.md).

- **One runtime.** Two worker threads, `rm-ctl-0` and `rm-ctl-1`. The pool
  is owned by one task, which sleeps until the pool's next deadline and
  wakes for whatever arrives. Stopping is one channel that Ctrl+C,
  Ctrl+Break and the service's stop control all send on.
- **No limit on lines from wait handles.** Each serial port has a thread of
  its own, which talks to the pool's task through channels. The pool no
  longer waits on its ports' handles, of which one wait took 64: with more
  than 63 lines, the ports past the 63rd were served only at the 20 ms
  tick.
- **Control server.** A reading task and a writing task for each client,
  in place of two threads. The named pipe is Tokio's, with the same
  security, and still refuses remote clients. The queue of a client that
  does not keep up is as it was.
- **SIP socket.** Read by a task, in `service/svc`. `service/telephony` has
  no socket in it and no runtime.
- **Media threads.** Unchanged, and without Tokio. They tell the pool of an
  event through a channel they never wait on, and the pool looks for the
  data of the calls every 10 ms while a call is up, where it looked every
  20 ms.
- **Clients.** `rustmodemctl` and the manager are as they were, on the
  blocking pipe client, and do not depend on Tokio.
- **Checks.** `ci/pure.sh` fails the build if Tokio is in the dependency
  graph of a pure crate, of the media threads or of the manager. The Linux
  job also lints and tests the control server and the service's runner.
- **Tests.** A hundred lines at once; a control client that does not read
  cannot delay the pool; the stop signal ends every task and thread with a
  call up and a line registered. The soak runs on the service's own runner.

A port's write that fails is now reported as `RMD3005` where it was
`RMD3003` or `RMD3004`: see "Where the build differs" in
[docs/RUNTIME.md](docs/RUNTIME.md).

## 0.0.3 (phase 2: telephony)

Real telephone calls reach the lines, over SIP and RTP written in-house.
The record of the phase is [docs/PHASE2.md](docs/PHASE2.md).

- **SIP.** The service is a SIP user agent over UDP on IPv4. It takes calls
  on a trunk listener from the addresses in `sip.trusted_peers`, and can
  register any line to a PBX as an extension, with digest authentication
  (MD5 and SHA-256). Calls a line dials go to the outbound peer, or to the
  PBX the line is registered to. What it does method by method is in
  [docs/TELEPHONY.md](docs/TELEPHONY.md).
- **Hunt groups and routes.** A called number is matched against the
  routes, in order, exact or by prefix, and rings a line or a hunt group; a
  group chooses among its idle lines first-idle, round-robin or
  least-recent.
- **Media.** One stream of G.711 (PCMU or PCMA) a call, at 10, 20 or 30 ms,
  each call on a thread of its own at the "Pro Audio" priority. The path is
  sample-exact: a frame is sent for every frame taken, with no jitter
  buffer and no clock but the far end's. Loss, late and doubled packets and
  restarted streams are counted, never smoothed over.
- **Engines.** A line runs the null engine or the new echo engine, by
  `engine` in its table. `ATI4` names it.
- **Call capture.** Every RTP packet of a line's calls written to a
  `.rmcap` file, turned on a line at a time. `rustmodemctl capture check`
  says what was lost, late, doubled and slipped and exits with 0 only for a
  clean call; `capture wav` writes the call as a two-channel G.711 WAV
  file. See [docs/CAPTURE.md](docs/CAPTURE.md).
- **Control API.** Twelve methods more, all additions to protocol 1:
  `lines.capture`, `sip.status`, `sip.reregister`, and `list`, `add`,
  `edit` and `remove` for `huntgroups` and `routes`, with `routes.move`.
  `config.get` and `config.set` carry `[sip]` and `[media]`. A line says
  how it is reached over SIP, how its registration stands, which hunt
  groups it is in and which engine it runs; a call and its record carry
  their RTP statistics; the statistics carry RTP, jitter, loss, the
  processor time of the service and SIP in a line. Two events more,
  `sip.registration` and `sip.trace`, the second only for a client that
  asks for it by name. `rustmodemctl` has `sip`, `reregister`, `groups`,
  `routes` and `capture`.
- **Manager.** The SIP menu: Re-register, Trunk peers, Hunt groups, Routes
  and a SIP trace window. Tools, Call capture. The grid shows each line's
  extension or `trunk` and its hunt groups, counts the errors of the call's
  RTP, and can be narrowed to a hunt group. New line and Edit line take an
  engine and a registration. Settings has SIP and Media pages. The panels
  and the status bar show the codec, jitter, loss, RTP and the processor;
  the call history shows the codec, lost packets and slips.
- **Configuration.** `[sip]`, `[media]`, `[[hunt_group]]`, `[[route]]`, and
  `engine` and `register` on a line. A file with none of them behaves as
  it did.
- **Demo service.** `--script sip`: the real SIP router and media threads
  on the loopback interface, with test peers placing calls.
- **Test peer.** `tools/sippeer`, not shipped: a scriptable SIP and RTP
  far end that registers, calls, answers, challenges, and sends RTP with
  loss, duplicates, reordering, jitter and a clock that runs fast or slow.
- **Messages.** The ranges `RMD5000` to `RMD5999` for telephony and
  `RMD6000` to `RMD6999` for media.
- **Tests.** Two services call each other over SIP and RTP and carry a
  megabyte each way through their null engines; a soak test holds 24 calls
  at once and fails on one packet lost, resynchronised or slipped; fuzz
  targets for SDP, RTP and SIP input.

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
