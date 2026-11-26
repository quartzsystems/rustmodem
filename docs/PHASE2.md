# Phase 2 record: telephony

Phase 2 brings telephone calls to the lines. The service speaks SIP and RTP
itself, both written in-house: it takes calls on a trunk listener and routes
them through routes and hunt groups, registers lines to a PBX as extensions,
rings the line's COM port, answers when the host answers, and carries G.711
both ways sample-exactly. Nothing demodulates the audio yet. An echo engine
sends it back, and the null engine of phase 0 is carried across real RTP to
show that the path alters nothing.

All twelve work packages are built and committed. The seven automated exit
criteria pass. The seven manual ones need a gateway, a telephone and a PBX;
they are a checklist at the end. Nothing of the manual seven has been done:
the service has never met a SIP device that is not its own test peer, and
has never sent a packet off the loopback interface.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| Manual exit criteria 8 to 14 | "Manual checklist" below | Run them. Until then every claim about a real gateway is untested |
| Gateway settings | `docs/GATEWAYS.md` (`NEEDS-CODY`) | The checklist of what to turn off is written. The tables for the SPA112, the MP-124 and the VG310 have the items and no setting names: fill them in from each device as it is configured |
| Windows Firewall | `docs/TELEPHONY.md`, "Windows Firewall" (`NEEDS-CODY`) | Two `netsh` commands are written as expected and have not been run. Criterion 14 replaces them with the ones used |
| Decisions made without asking | "Decisions to confirm" below | The handoff left these open and nobody was there to ask. Several change the control API. Each was decided and is listed for review |
| The RFCs were not to hand | "References consulted" below | Every section number cited in a comment is from memory of the documents. Check them before relying on one |
| The About picture | `ui/manager/tests/goldens/about.png` | It was taken at 0.0.2, as phase 1's was at 0.0.1. The digit is well inside what the comparison allows |
| Still open from phases 0 and 1 | `docs/PHASE0.md`, `docs/PHASE1.md` | The com0com signal mapping, the port inventory's registry keys, the V.250 clause numbers and both manual checklists are as they were |

## What was built

### 1. `voip/sdp`

Session descriptions for one stream of G.711: a parser with limits, a
builder, and offer/answer (RFC 3264) as three functions, `offer`, `answer`
and `accept_answer`, that give a `Negotiated` (codec, packet time, the far
end's address, whether it holds the call) or a `Refusal` the router answers
488 for. PCMU and PCMA, 10, 20 or 30 ms. Every description says
`a=silenceSupp:off - - - -`. T.38 and a stream with port zero are refused.
A table of descriptions, property tests and a fuzz target.

### 2. `voip/rtp`

Packets, a sender, and the receiver the phase turns on. `RtpReceiver` takes
the header of each arriving packet and gives a verdict: deliver so many fill
frames and then so many frames of payload, or drop it as a duplicate, as
late, or as not this call's. It holds no audio and no clock. The verdict is
what makes the path sample-exact: the frames it says to deliver, fill
included, are the frames the far end's timestamps account for, whatever was
lost, doubled or reordered on the way. A property test holds it to that
under arbitrary loss, duplication and reordering.

### 3. and 4. `voip/sip`

Sans-I/O SIP: it is given datagrams and the time and gives back datagrams,
timers and events.

- Messages and URIs: a parser with limits (65,535 octets a datagram, 128
  headers, 8,192 octets a line), typed header access, a builder. A torture
  table in the manner of RFC 4475, property tests and a fuzz target.
- Transactions (RFC 3261 section 17, with the Accepted state of RFC 6026
  and `rport` of RFC 3581), dialogs, the retransmission of a 2xx until its
  ACK, digest authentication (MD5 and SHA-256, `qop=auth`), and a
  registration client that refreshes, takes a 423, and backs off 5, 10, 20,
  40 and 60 seconds.

### 5. Configuration

`[sip]`, `[media]`, `[[hunt_group]]`, `[[route]]`, and `engine` and
`register` on a line, validated with the rest: a group that names a line
there is not, a name or a pilot used twice, a route to nothing. A route
that an earlier route keeps from ever getting a call is a warning. A
password is never shown in a `Debug` rendering. Every table can be changed
through the editor that keeps comments: lines gain and lose registrations,
groups are added, changed and removed, and routes are added, changed,
removed and moved, in the file as in the list.

### 6. The media seam and `service/media`

The pool no longer ticks engines on its 20 ms timer. Between the pool and a
call's audio there is a seam, in `service/pool/src/media.rs`:

- `MediaCall` is the pool's end of a connected call: a lock-free ring of
  bytes each way for data (`rtrb`), a ring of events coming back (a modem
  event, or the call's media stalled), and `MediaShared`, a block of
  atomics the two ends share: the capture switch going in, the statistics
  coming out.
- `MediaPort` is the other end, held by whatever runs the call.
- `MediaRunner` is three calls: `start` takes a line's engine and the far
  end of its call and gives a `MediaCall`; `stop` gives the engine back;
  `tick` is the pool's clock, for a runner that goes by it.
- The far end is a `MediaEndpoint`, opaque to the pool: the router makes
  one and the runner knows what it is.

There are two runners. `InlineRunner` is phase 0's behaviour behind the
seam: the pool's clock ticks the engine and the two engines of a call are
joined octet for octet. Every phase 0 pool test runs on it, unchanged. The
service uses it when SIP is off. `ThreadedRunner`, in `service/media`, gives
each call a thread at the "Pro Audio" MMCSS priority with a socket of its
own. The thread waits for a packet, asks the receiver what it means, runs
the engine once for each frame it says to deliver, and sends a packet when
enough frames are made: tick-on-receive. Before the far end's first packet,
and while a call is held, a local timer stands in. The pool's thread is
woken only when a call has an event for it; data crosses in the rings.

The handoff describes the pool's end as holding "commands going in" and "a
wait handle". What was built has no command queue: hanging up is `stop`,
and the capture switch is an atomic. The wait handle is one for the whole
runner, a callback (`Waker`) the service points at an event its pool thread
waits on, not one a call.

`unsafe` is in `service/media/src/os.rs` and nowhere else in the crate:
the MMCSS calls, the sizes of the socket buffers, and `WSAPoll`.

### 7. `service/telephony`

The SIP call router, a `CallRouter` like phase 0's `NullSwitch`: the trunk
listener with its trusted peers, a registration for each line that has
one, routes and hunt groups (`hunt.rs`: first-idle, round-robin and
least-recent), incoming and outgoing calls, re-INVITE for hold and for a new
address, CANCEL, BYE, OPTIONS, and the timeouts. `docs/TELEPHONY.md` has the
behaviour as tables, row by row, and the tests go by its rows. The SIP half
of the test peer was built here, because these tests need a far end.

### 8. Engines

`EchoEngine` sends back what it receives and connects at once, at no rate,
on the carrier `ECHO`. A line's `engine` chooses between it and the null
engine; `ATI4` names the engine.

### 9. Call capture and `tools/sippeer`

- The media thread offers every packet it receives and sends to a ring; one
  writer thread for the service empties the rings into `.rmcap` files. A
  full ring drops records, counts them and marks the hole in the file; a
  tick never waits. `docs/CAPTURE.md` has the format.
- `rustmodemctl capture check` replays the received packets through the
  same receiver and counts what happened, with the slips: points where the
  frames sent stopped matching the frames taken. `capture wav` writes a
  two-channel G.711 WAV file.
- `tools/sippeer`: `Peer`, a user agent a test drives by hand; `MediaStream`,
  the RTP of a call with a clock of its own and loss, duplicates,
  reordering, jitter and a clock offset in parts per million, all from a
  seed; `Agent`, the two on sockets; a script format of one step a line;
  and the program `rustmodem-sippeer`, which runs a script.

### 10. Control API, CLI and service wiring

Twelve methods and two events, all additions to protocol 1; the list is in
`CHANGELOG.md` and the reference is `docs/CONTROL_API.md`, generated and
checked as before. `rustmodemctl` has `sip`, `reregister`, `groups`,
`routes`, `capture on|off` and the two capture file commands. The service
builds the SIP router and the threaded runner when `sip.enabled` is true,
and on stopping hangs up every call, gives up every registration, and
gives the far ends three seconds to answer. The demo service has a script
`sip`: the real router and media threads on loopback, with test peers in
the same process calling a hunt group of every line.

### 11. Manager

Everything phase 1 left marked `BLOCKED(phase2)`: the SIP and Hunt group
columns, with the registration as the row's tip and a red image on a line
whose registration has failed; Err, for the call's RTP; a hunt group to
narrow the grid to in the Show list; Re-register, Trunk peers, Hunt groups,
Routes and the SIP trace under the SIP menu; Call capture under Tools; an
engine and a registration in New line and Edit line; SIP and Media pages in
Settings; codec, jitter, loss, RTP and the processor in the panels and the
status bar; codec, lost and slips in the call history. `ui/win32` gained
what these needed and nothing else: a list box, a list view with check
boxes, a password field, the tip of a list row, and opening a folder.

### 12. This record

The soak test, `docs/GATEWAYS.md`, this file, `CLAUDE.md`, `README.md`,
`CHANGELOG.md`, and version 0.0.3.

## Tick times

A tick is one 10 ms frame through the engine, with the packet sent when it
completes one. The media thread measures each, and the statistics of a call
give the median, the 99th percentile and the longest.

| Run | Machine | Median | 99th percentile | Longest |
|---|---|---|---|---|
| Soak, 24 calls for 60 s | The development machine | 4 us | 26 us | 347 us |
| Soak, 24 calls for 10 minutes | The development machine | 3 us | 25 us | 404 us |

Each figure is the greatest of the 24 lines'. The engines of this phase do
almost nothing in a tick, so these are the cost of the path itself: the
figures to watch are the ones a real modem's tick is added to. No tick took
as long as a packet. The soak test prints its figures, so the CI log of any
run has the runner's. The numbers of the real machine are criterion 13.

## Markers

Every `BLOCKED` and `NEEDS-CODY` in the tree. Every `BLOCKED(phase2)` of
phases 0 and 1 is gone.

| Marker | Location | What is deferred |
|---|---|---|
| `BLOCKED(phase3)` | `ui/model/src/commands.rs`; `ui/manager/src/window.rs` | From phase 1: Rate cap and modulations, Line diagnostics, Loopback self-test |
| `BLOCKED(phase3)` | `ui/model/src/lines.rs` | From phase 1: the Retr. column |
| `BLOCKED(phase4)` | `ui/model/src/lines.rs` | The Err column counts the RTP's errors now; the error-control protocol's are added to them |
| `BLOCKED(phase5)` | `service/media/src/os.rs` | RTP packets are not marked with a DSCP: on Windows that is a QoS policy, which the installer makes |
| `BLOCKED(phase5)` | `service/svc/src/service.rs`, `service/svc/src/eventlog.rs` | From phase 0: the service account and the event message file |
| `BLOCKED(phase9)` | `ui/manager/src/dialogs/connect.rs`, `service/ctlsrv/src/lib.rs` | From phases 0 and 1: another computer's service, and TLS for it |
| `NEEDS-CODY` | `docs/GATEWAYS.md` | The setting names of each gateway |
| `NEEDS-CODY` | `docs/TELEPHONY.md` | The firewall rules as used |
| `NEEDS-CODY` | `service/svc/src/ports.rs`; `dte/serial/src/com0com.rs` (three), `docs/COM0COM.md` | From phases 0 and 1 |

Phase 1's `BLOCKED(phase9)` on the status bar's `CPU` part is gone too: the
handoff asked for the service's processor time in the statistics, and the
service measures it.

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| The SIP and RTP peer is work package 9 | Its SIP half was built in work package 7, its RTP half, script and program in 9 | The router's tests need a far end that is not the router |
| `Stats` and `LineInfo.engine` are in work packages 6 and 8 as they come up | Both were reported from work package 10 | They are control API additions, and the API was changed once |
| `MediaCall` has commands going in and a wait handle | Atomics and a runner-wide waker | See "The media seam" above |
| `unsafe` is allowed in `service/media` | It is in one file of it, `os.rs`; every other module forbids it | It could be kept that small. `CLAUDE.md` rule 8 says so |
| Each work package is one commit | Three more: `fix(media): keep the socket buffer size out of the Linux build` and `fix(media): wait for datagrams with a poll, not the socket read timeout` | The first repaired a commit whose Linux clippy job had failed on CI: work package 6 was pushed with only the tests run under WSL, and a constant that is dead on Linux failed clippy there. `main` had one red commit. The whole Linux job is run before every push since, and `CLAUDE.md` says to. The second is the finding below. The third is `test(media): do not time the wait that runs out`: the test of that finding also asserted that a 2 ms wait takes at least 1 ms, which the CI runner's timer did not honour once, on the manager's commit. That made a second red commit on `main`; the assertion was the test's own and not the property, and is gone |
| The media thread reads its socket with a timeout | It waits with `WSAPoll` and reads without blocking | On Windows a datagram that arrives as a receive timeout (`SO_RCVTIMEO`) expires can be lost: 2 of 720 in a probe written to find out why the megabyte test failed one run in five. With `WSAPoll`, 0 of 720, and the test 40 times of 40. `a_wait_that_runs_out_loses_no_datagram` keeps it so. The SIP reader waits the same way |
| "Goldens are blessed on the CI runner image" | The manager's commit went to a branch, `phase2-goldens`, first; CI took the pictures; the branch was deleted | As in phase 1: the pictures can only be had from a run. `main` had no failing commit for it |
| `sip.trace` is "logged at debug" in an early draft of `docs/TELEPHONY.md` | It is not logged | The trace is for the window that asks for it. `docs/TELEPHONY.md` says so now |
| The soak brings up "the service" | It brings up the pool with the SIP router, the threaded runner and memory ports in the test's process | The service's own program adds the pipe, the log and the Windows service shell, none of which a call passes through |
| Criterion 12 is manual | It was also run against the test peer: see "Exit criteria" | It could be. The manual one, through a real gateway, is still to do |

## Decisions to confirm

Each of these is a choice the handoff left open. They are in the code and
the documentation as described; say if any should change.

**Control API**

- A route is named by its place in the list, from 0: `routes.edit`,
  `routes.remove` and `routes.move { "index", "to" }` take an `index`, and
  every change to the routes answers with the whole list, since the places
  shift. A hunt group is named by its name.
- `huntgroups.edit` takes `new_name` to rename a group, and the routes to
  the group follow the new name.
- A line's registration is a nested object, `register`. In `lines.edit` an
  empty `server` removes it, and a password that is left out stays as it
  is. A line that had none needs `server`, `user` and `password` together
  (`RMD0035`). The password is never reported; `has_password` says there
  is one.
- `LineInfo.sip` also reports the registrar, the authentication user and
  the expiry, which the Edit line dialog shows. `LineInfo.capture` says
  whether the line is captured.
- The RTP statistics of a call have `slips` as well as what the handoff
  lists, and give the jitter in microseconds, as integers. "Average"
  jitter is the smoothed estimate of RFC 3550, not a mean over the call.
- In `Stats` the jitter is over the calls in progress, the packet loss
  over every call since the service started, and `rtp_kbps` over the last
  second, headers included. `cpu_percent` is of one processor.
- `restart_required` is true when `sip.enabled`, `sip.listen`,
  `media.rtp_ports` or `media.capture_dir` differ from what the service
  started with. Everything else in `[sip]` and `[media]` takes effect for
  the calls that follow.
- `service.info` has `capture_dir`, the directory captures go to, for the
  manager's Open folder. It is absent when SIP is off.
- `sip.trace` events may be dropped for a slow client, as `log` events
  are, and are not made at all while no client has asked for them.
- Capture is not a setting of the configuration: it is off for every line
  when the service starts.

**Configuration and the service**

- `sip.enabled` is false unless the file says true, even when there is a
  `[sip]` table.
- With SIP on and its listener unable to bind, the service starts without
  SIP, logs `RMD5002`, and reports the listener as `failed`.
- A name in `outbound_peer` or a registrar is resolved on the pool's
  thread, which waits for the resolver.
- Stopping gives the far ends three seconds to answer the BYEs and the
  removal of the registrations.
- A capture file is `line<NN>-<start in Unix milliseconds>.rmcap`, and the
  ring between a media thread and the writer holds 2,048 records.
- `rustmodemctl capture check` also exits with 1 when the capture itself
  has holes: what is counted around one cannot be trusted.

**Telephony**

- RFC 6026 is followed for the INVITE transaction, which the handoff does
  not name.
- Two rustmodem services calling each other have no clock between them:
  each sends when the other's packet arrives. It works, and is how the
  megabyte test runs, but nothing paces it. A real far end has a clock.

**Manager**

- The hunt groups are entries of the Show list in the toolbar, after its
  four filters. The View menu's Show submenu has the four filters only.
- The tip of a row of the grid is its SIP state. A list view has one tip a
  row, not one a cell.
- A line's row has the red image while it is idle and its registration has
  failed. Its State column still says what the line is doing.
- Err is the lost, late and resynchronised packets of the call in
  progress.
- New line and Edit line have two SIP choices, not three: "Register to a
  PBX", and the other, which reads "Trunk" when the service speaks SIP and
  "None" when it does not. Whether a line that does not register is
  reached at all is the service's setting, not the line's.
- Trunk peers changes `sip.trusted_peers` and `sip.outbound_peer` with
  `config.set`; the SIP page of Settings has the rest of `[sip]`.
- A box in Call capture takes effect when it is checked. There is no OK.
- The SIP trace keeps the last 1,000 messages.

**Tools**

- `rustmodem-sippeer` reports what goes wrong in plain text, with the line
  of the script, not as `RMD` messages: it is a tool for whoever develops
  rustmodem and is not shipped.
- The demo service's `sip` script gives the lines the engines their
  configuration names, not its demo engine, and makes every line answer by
  itself. Its frozen script gained two hunt groups and two routes, so that
  the pictures have some.

## References consulted

| Reference | For | How |
|---|---|---|
| RFC 3261, RFC 6026, RFC 3581 | `voip/sip`, `service/telephony` | Cited by section in comments. From memory: the documents were not fetched |
| RFC 3264, RFC 8866 | `voip/sdp` | As above |
| RFC 3550, RFC 3551 | `voip/rtp`, `service/media` | As above |
| RFC 7616, RFC 2617 | `voip/sip` (digest) | As above |
| RFC 4475 | The shape of the torture table in `voip/sip/tests/torture.rs` | From memory. The messages in the table were written for it |
| Microsoft's documentation of MMCSS, `WSAPoll` and `SO_RCVBUF` | `service/media/src/os.rs` | From memory of the function signatures, which the `windows` crate checks |
| The WAVE format for non-PCM data | `voip/rtp/src/capture.rs` | From memory: the `fmt` chunk of 18 octets and the `fact` chunk |

PJSIP, VoIPModem, libv90modem, spandsp, linmodem and every other GPL or
LGPL project were not opened. The session that built the phase had no
access to the network beyond `git` and the GitHub CLI.

## Dependencies

Phase 2 adds four crates, all of them the handoff's list, and nothing else.

| Crate | Used by | Licence |
|---|---|---|
| `rtrb` | `pool`, `media` | MIT OR Apache-2.0 |
| `getrandom` | `telephony` | MIT OR Apache-2.0 |
| `md-5`, `sha2` | `sip` | MIT OR Apache-2.0 |

`cargo deny check` passes. None of the four compiles C.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check`, `ci/headers.sh` and all tests pass on Windows; the pure crates, now including `voip/*`, pass on Linux | Run locally on Windows before every commit: 836 tests at the end, and the 25 pictures of the manager. The Linux job, fmt and clippy with it, was run under WSL before every push from work package 7 on. The GitHub Actions run of every commit on `main` succeeded but two, of work packages 6 and 11: see "Where the build differs". The pictures of work package 11 matched their goldens in that run |
| 2 | The phase 0 pool tests pass on `InlineRunner` with their assertions unchanged, and with `[sip]` absent the service behaves as before | `service/pool/tests/` runs on `InlineRunner`; the assertions were not touched. `a_file_with_none_of_the_new_sections_is_as_it_was` in `service/config/tests/telephony.rs`, and the control API's tests, which run a service with no SIP |
| 3 | The `RtpReceiver` sample-count property holds under arbitrary loss, duplication and reordering, both in the crate and through the real tick loop | `voip/rtp/tests/properties.rs`, and the same property through `Ticker` in `service/media/tests/ticker.rs` |
| 4 | 1 MiB each way through two `NullEngine` lines over SIP and RTP on loopback arrives intact | `a_megabyte_each_way_between_two_null_engine_lines_over_sip_and_rtp` in `service/telephony/tests/transparency.rs`: two whole services, each a pool with the router and the threaded runner |
| 5 | Every row of the incoming, outgoing and in-call behaviour in work package 7 has a test, and every addition in work package 10 has a round-trip test over the in-memory transport and a real pipe | `service/telephony/tests/incoming.rs`, `outgoing.rs`, `in_call.rs`, `registration.rs`, `hunt.rs` and `udp.rs`. `every_method_round_trips` in `service/ctlsrv/tests/api.rs` and `every_method_round_trips_over_a_real_pipe` in `service/pipe/tests/api.rs` both go through the method list the document is generated from and fail on a method with no case |
| 6 | The 60-second soak passes | `service/telephony/tests/soak.rs`, run locally: 24 calls for 60 seconds, nothing lost, resynchronised or slipped on any, 12 patterns echoed whole and 12 calls with 2,000 octets a second each way intact. It is part of `cargo test --workspace`, so CI runs it from this commit on; this commit's run is the runner's first |
| 7 | The fuzz targets `sip_message`, `sdp_parse` and `rtp_packet` each run for 10 minutes with no findings | Run under WSL: `sdp_parse` 3,776,961 runs, `rtp_packet` 97,750,844 and `sip_message` 7,278,932, each in 601 seconds, no crash, no artifact. CI runs each for 10 minutes on every push |

Criterion 12 was also run against the test peer, which is not what it asks
for but is what could be done without a gateway: a ten-minute call to an
echo line with capture on, checked as `rustmodemctl capture check` checks
it (`RM_CAPTURE_MINUTES=10 cargo test -p telephony --test peer a_clean_call`).

| Packet time | Received | Sent | Lost | Resync | Slips | Largest gap |
|---|---|---|---|---|---|---|
| 20 ms | 30,001 | 30,001 | 0 | 0 | 0 | 48.6 ms |
| 10 ms | 60,001 | 60,001 | 0 | 0 | 0 | 38.8 ms |

The largest gaps are the test peer's doing as much as the line's: the two
ran in one process on one desktop. A call with loss, duplicates,
reordering, jitter and a fast clock (`a_call_with_faults_never_slips`) lost
what was lost and never slipped.

### Manual checklist

These need the machine the service will run on, an elevated prompt, a
gateway set up as `docs/GATEWAYS.md` says, a telephone, and for criterion 9
a PBX. `10.10.20.11` stands for the gateway's address and `10.10.20.5` for
the PBX's throughout; lines 1 and 2 are on com0com pairs as in phase 0's
checklist.

A configuration to start from:

```toml
[sip]
enabled = true
listen = "0.0.0.0:5060"
trusted_peers = ["10.10.20.11"]
outbound_peer = "10.10.20.11"

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }
engine = "echo"

[[line]]
id = 2
serial = { backend = "com0com", port = "CNCB1" }
engine = "echo"

[[hunt_group]]
name = "dialup"
pilot = "5551000"
lines = [1, 2]

[[route]]
called = "5551000"
target = "group:dialup"
```

**8. Trunk.**

- [ ] Set the SPA112 to send calls to the service's address on port 5060,
      per `docs/GATEWAYS.md`, and plug a telephone into its FXS port.
- [ ] `rustmodem-svc --console C:\rustmodem\rustmodem.toml`. The log says
      `RMD5001: SIP is listening on 0.0.0.0:5060.`
- [ ] `rustmodemctl sip` shows the listener, `10.10.20.11` as a trusted
      peer, and no registrations.
- [ ] Open a terminal on the host side of line 1 (COM11 in phase 0's
      checklist). `AT` gets `OK`.
- [ ] Dial 5551000 on the telephone. The terminal shows `RING`.
- [ ] Type `ATA`. The terminal shows `CONNECT`, and the caller hears
      themselves, a moment late.
- [ ] Hang the telephone up. The terminal shows `NO CARRIER`.
- [ ] Call again, answer, and type `+++` and then `ATH`. The telephone's
      call ends.
- [ ] `rustmodemctl calls --last 2` shows both calls.

**9. Per-line.**

- [ ] Give line 2 an extension of the PBX:

  ```toml
  register = { server = "10.10.20.5", user = "202", password = "..." }
  ```

  and `rustmodemctl reload`.
- [ ] `rustmodemctl sip` shows line 2 as `registered for ... s more`.
- [ ] The manager shows `202` in line 2's SIP column, and resting the
      pointer on the row says it is registered.
- [ ] Call extension 202 from a telephone of the PBX. Line 2's terminal
      shows `RING`; line 1's shows nothing.
- [ ] Stop the PBX, or give the line a wrong password, and
      `rustmodemctl reregister 2`. `rustmodemctl sip` says why it failed,
      and the manager's row has the red image.

**10. Hunt group.**

- [ ] With both lines idle, call 5551000 and answer on line 1.
- [ ] Call 5551000 from a second telephone. Line 2 rings.
- [ ] Answer it, and call 5551000 from a third. The caller hears busy.
- [ ] `rustmodemctl stats` counts the third call as rejected.

**11. Outbound.**

- [ ] On line 1's terminal, `ATD<number of a telephone behind the gateway>`.
      The telephone rings.
- [ ] Answer it. The terminal shows `CONNECT`.
- [ ] Hang the telephone up. The terminal shows `NO CARRIER`.
- [ ] `ATD` a number that is busy and one that does not exist: `BUSY`, and
      `NO CARRIER` or `NO ANSWER`.

**12. Sample-exact for 10 minutes.**

- [ ] `rustmodemctl capture on 1`. The log says where the capture goes
      when the call begins (`RMD6006`).
- [ ] Call 5551000, answer on line 1, and leave the call up for ten
      minutes. Hang up.
- [ ] `rustmodemctl capture check <file>`: Lost 0, Resync 0, Slips 0, and
      the exit code 0 (`echo %ERRORLEVEL%`).
- [ ] Set `ptime_ms = 10` under `[media]`, and the same packet time on the
      gateway; `rustmodemctl reload`; repeat. Record both reports here.
- [ ] `rustmodemctl capture wav <file> call.wav` and listen to it: the
      caller on the left, the echo on the right.

**13. Load.**

- [ ] On the machine the service will run on:

  ```
  set RM_SOAK_MINUTES=10
  cargo test --release -p telephony --test soak -- --nocapture
  ```

- [ ] It passes, and the line it prints has the tick times: record the
      median, the 99th percentile and the longest in "Tick times" above.
- [ ] No overruns: while a real call is up, `rustmodemctl line 1 --json`
      has `"overruns":0` in the call's `media`.

**14. Windows Firewall.**

- [ ] With the firewall on, find the rules the service needs for the SIP
      port and the RTP range. The two commands in `docs/TELEPHONY.md` are
      the expected ones.
- [ ] Replace them there with the exact commands used, and remove the
      `NEEDS-CODY`.

## Known limits of phase 2

- No modulation: a call to a line reaches an engine that echoes it or
  treats the samples as the null engine's framing. A real modem that calls
  hears nothing it knows.
- SIP is over UDP and IPv4 only, with no NAT traversal: the service and
  its gateway are on one network.
- A call rings one line. It is not hunted on if that line does not answer.
- No jitter buffer, by design: a lost packet is silence to the modem, and
  a late one is dropped.
- DTMF is not sent, detected or relayed: a number is dialled in the INVITE.
- No RTCP: the far end is not told what was lost.
- A registrar or peer given by name is resolved while the pool's thread
  waits.
- RTP packets are not marked for quality of service.
- The manager opens the capture folder on the computer it runs on, which
  is the service's only while the two are the same.
