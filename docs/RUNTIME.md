# The runtime model, and the record of the Tokio retrofit

The service's control plane runs on Tokio. This document says where Tokio
is and where it must never be, what runs on which task and thread, how the
media threads and the pool exchange data and events, and what to do when
I/O is added. Its second half is the record of the retrofit that moved the
service there after phase 2: what changed, what was measured, what differs
from the handoff, and what is left for Cody.

The retrofit changed no behaviour: no control API message, no file format,
no AT command, nothing in `docs/TELEPHONY.md`. A `rustmodem-ctl` or a manager
built before it works against a service built after it.

## Where Tokio is

| Place | Use |
|---|---|
| `service/svc` (library) | Owns the runtime. The pool's task, the serial bridge, the SIP socket's task, the wake channel of the media threads, shutdown |
| `service/svc` (program) | Builds the runtime, starts the tasks, the refresh of the lines' bindings |
| `service/ctlsrv` | The control server: a reading task and a writing task for each client |
| `service/pipe`, feature `server` | The named pipe server, on `tokio::net::windows::named_pipe` |
| `tools/demosvc` | The same runner as the service |

`service/telephony` has no I/O left in it. Its socket was moved into
`service/svc` (`sipsock.rs`); the crate keeps the `SipTransport` trait the
router is given its datagrams through, and nothing else of a network.

## Where Tokio is not, and must not be

| Place | Why |
|---|---|
| `dte/at`, `modem/engine`, `service/msg`, `service/config`, `service/ctlproto`, `service/pool`, `voip/sdp`, `voip/rtp`, `voip/sip`, `service/telephony`, `ui/model`, `ui/glyph` | Pure state machines given bytes and a time. That is what makes them testable on Linux with a clock the test moves |
| `service/media` | Real-time. A dedicated thread for each call, with nothing on the tick path that can allocate, lock or yield to a scheduler |
| `dte/serial` | Overlapped I/O on a COM handle has no place in Tokio's reactor. Each port has a thread, which talks to the pool's task through channels |
| The capture writer | It has its own thread, fed by a ring |
| `ui/manager`, `ui/win32`, `tools/ctl` | A Win32 message loop and a command-line client. They use the blocking pipe client |
| `tools/sippeer` | The test peer has a clock and sockets of its own and was left as it is: converting it would have made no test simpler |

A pure crate or a media thread that gains a Tokio dependency is a defect.
`ci/pure.sh` fails the build if Tokio is anywhere in the dependency graph,
tests included, of a pure crate, of `media`, `telephony` or `sippeer`, or of
`ui/win32` and `ui/manager`; and in what `rustmodem-ctl` itself is built
from. `rustmodem-ctl`'s tests are the one allowed exception: they run its
commands against the real control server, which is on Tokio, so Tokio is a
dependency of those tests and of nothing the program links.

`service/pipe` has both ends of the pipe. The listener, with Tokio and the
control server it serves, is behind the crate's `server` feature, which the
service and the demo service turn on. The manager leaves it off and gets
the blocking client alone.

## What runs where

One multi-thread runtime, built by `svc::runtime::build`: two worker
threads, `rm-ctl-0` and `rm-ctl-1`, with timers and I/O. Console mode and
service mode both use it; under the service control manager `service_main`
builds it and waits on it. The threads the runtime makes for blocking work
take the names after the workers'.

**Tasks, on the runtime**

| Task | What it does | Why a task |
|---|---|---|
| The pool's task (`svc::pooltask`) | Owns the `Pool`, and is the only code that touches it. Sleeps until the pool's next deadline; wakes for a request, a port's news, a SIP datagram, a media event, the 10 ms look at the calls' data, or the stop signal; then gives the pool a turn | One owner, woken by everything, waiting on any number of sources |
| The control listener | Accepts clients on the named pipe | Pipe I/O |
| A reading task for each client | Reads requests, carries each out whole, queues the answer | Pipe I/O. A request for the pool is sent to the pool's task and its answer awaited |
| A writing task for each client | Writes what is queued for the client | So that a client slow to read holds up nobody but itself |
| The SIP socket's task (`svc::sipsock`) | Reads the UDP socket and passes each datagram to the pool's task | Socket I/O; TCP and TLS later are more of the same |
| The binding refresh | Every 30 seconds looks up the serial ports and tells the pool what each is bound to | A timer. The lookup itself is SetupAPI and registry work, done with `spawn_blocking` |

**Threads, each for a reason**

| Thread | What it does | Why not a task |
|---|---|---|
| `rm-port-<port>`, one for each open port (`svc::bridge`) | Owns the port's `SerialBackend`, waits on its handle, reads, writes and sets signals | A COM handle's overlapped I/O is not something the reactor can wait on |
| `media-<n>`, one for each line | Carries a call: waits for an RTP packet, runs the engine for each frame, sends | Real-time, at the "Pro Audio" class. Nothing may make it wait |
| `capture` | Writes capture files from the rings the media threads fill | File writes, kept off the media threads |
| The main thread | Waits for the stop signal, then stops the service | It is the thread the system gave the program |

The log is not a thread: a line is written by whoever logs it, under a
lock, as before.

The demo service adds two threads of its own, `hosts` and `callers`, which
play the programs on the ports and the far end of the calls. They are the
demonstration, not the runner.

## The pool's task

`Pool` is synchronous and pure, as it was. In outline the task is:

```rust
loop {
    tokio::select! {
        Some(request) = requests.recv()     => request.run(&mut pool),
        _ = wake.notified()                 => {}  // a port's thread, or the SIP socket
        _ = MediaEvents::next(&mut media)   => {}  // a media thread has an event
        _ = drain.tick(), if calls_up       => {}  // the data of the calls, every 10 ms
        _ = sleep_until(deadline)           => {}
        _ = stopping.wait()                 => break,
    }
    pool.service(now());
    deadline = pool.next_deadline();
}
```

- **Requests.** The control server reaches the pool through a
  `ctlsrv::PoolLink`: a request is a closure sent on an unbounded channel
  and run against the pool by its task, and the answer comes back on a
  oneshot channel. The requests the server makes are the pool's own
  `Command`s, unchanged.
- **The deadline.** `Pool::next_deadline` is the pool's next 20 ms tick.
  Everything the pool times is looked at in a turn of its loop and so is
  seen by then: the interpreters' S-register timers and the engines an
  inline runner carries are stepped by the tick itself; the router, which
  has the ring cadence, the ring timeout and the SIP retransmissions and
  registrations, is asked what it has at every turn; so is a port that is
  to be opened again. The pool is therefore served at least as often as
  its thread served it, and sooner when something arrives.
- **Time.** The monotonic clock is Tokio's, so a test that pauses the
  clock moves the pool's time. The wall clock is given to the task.
- **Stopping.** The stop signal is one `tokio::sync::watch` channel
  (`svc::shutdown`). Ctrl+C, Ctrl+Break, the console closing and the
  service's stop control all send on it, and every task selects on it. The
  pool's task then shuts the pool down as it always did, hanging up and
  sending BYE for every call, removing every registration and releasing
  every port, and gives the far ends up to three seconds to answer. The SIP
  socket's task does not stop at the signal: it ends when the router lets
  go of the socket, so that those answers are heard.

## The serial bridge

Each port's thread and the pool's task exchange messages:

| Direction | Channel | What |
|---|---|---|
| Port to pool | `tokio::sync::mpsc`, 16 chunks of up to 512 octets | What the host wrote. Sent only when there is room: a line that is not being read leaves the octets in the port, as when the pool read the port itself |
| Port to pool | `tokio::sync::mpsc`, 64 messages, `blocking_send` | What the host did (DTR, RTS, opened, closed), and a failure of the port. A channel of its own, so that a line that is not being read still hears its host hang up |
| Pool to port | `std::sync::mpsc`, and on Windows an event the thread waits on beside the port's handle | Octets to write and signals to set. No more than 4,096 octets are ever on their way to a host, so a host that does not read holds them up in the pool, where the line's high-water mark is |

The pool sees an ordinary `SerialBackend` over those channels, so `Pool`
and `Line` did not change. Dropping it tells the thread, which writes what
it was last given, sets the last signals, releases the port and ends; the
drop waits for that, so a port can be opened again the moment it is let go.

No thread waits on more than its own port and one event, so nothing is
left of the 64 handles one Windows wait can take.

## Media threads and the pool's task

The media threads did not change, and `service/media` has no Tokio in it.
What changed is how they wake the pool.

- **Events.** A call connecting, retraining, losing its carrier or stalling
  is put in the call's own lock-free ring, as before, where the pool reads
  it. The media thread then calls the waker it was given. The service's
  waker (`svc::mediawake`) sends a notice on a bounded
  `tokio::sync::mpsc` channel with `try_send`; the pool's task waits on
  it. The channel holds 256 notices, and one is enough to have the task
  look at every call, so it fills only if the task is not running at all.
  If it is full the waker counts it and says the notice did not go, and
  the media thread tries again at its next turn. It never waits.
- **Data.** The octets of a call cross in lock-free rings and are not
  signalled. While any call is up the pool's task looks at the rings every
  10 ms. That is the most this adds to the time a modem's data takes; the
  pool's thread looked every 20 ms.
- **Capture errors** are logged by the capture writer's own thread, as
  before. They do not go through the pool.

What `try_send` does, from the source of Tokio 1.54.1, as the handoff asked
to have confirmed:

- It takes a permit from the channel's semaphore with a compare-and-swap
  loop (`batch_semaphore::Semaphore::try_acquire`) and returns `Full` at
  once if there is none. It never waits for room, and takes no lock to
  find out.
- Storing the message can allocate: the channel is a linked list of blocks
  of 32 slots, and a send into a slot past the last block makes a new one
  (`list::Tx::find_block`, `Block::grow`). The receiver gives blocks back
  for reuse, so in practice it is at most one allocation for every 32
  messages.
- It then wakes the receiving task. From a thread that is not the
  runtime's, that puts the task on the scheduler's injection queue, which
  takes a lock for a moment, and may unpark a worker thread.

So `try_send` cannot block on the channel, and it can allocate and take a
short lock. The blocks cannot be made ahead of time, so the steady-state
path is kept clear of it the other way: a media thread calls its waker
only when its engine has reported an event, which is a few times in a call,
and never for data or on an ordinary tick. On an ordinary tick the only
addition to phase 2's loop is one test of a flag. The phase 2 waker, on
the same occasions, made a system call (`SetEvent`).

## Adding I/O

- **A socket, a pipe, a timer, anything the reactor can wait on** goes in
  `service/svc`, as a task that owns it and passes what arrives to the
  pool's task through a bounded channel, waking it. What cannot be passed
  is dropped and counted, never waited on, when the protocol recovers from
  a loss; otherwise the channel is the back-pressure. SIP over TCP or TLS
  is a transport beside `sipsock.rs`; remote management is a
  `ctlsrv::Listener` beside the pipe.
- **The logic the I/O serves stays sans-I/O**: given bytes and the time,
  giving back bytes, timers and events, in a crate with no runtime. The
  task is a shell around it.
- **Something that must block, or that Windows will only do on a handle of
  its own,** gets a thread that owns it and channels to the pool's task,
  as a serial port has. A short blocking call, such as the port lookup,
  uses `spawn_blocking`.
- **Nothing is added to the media tick path** that allocates, locks, logs
  or waits. What a media thread has to say goes in its call's ring.
- **Only the pool's task touches the pool.** Everything else asks it
  through a `PoolLink`.

---

# Record of the retrofit

Six work packages, six commits, version 0.0.4. The five automated exit
criteria pass. The five manual ones are a checklist at the end; none of
them has been done.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| Manual exit criteria 6 to 10 | "Manual checklist" below (`NEEDS-CODY`) | Run them. The com0com backend has still never opened a real port, and now does so from a thread of its own; the service has still never been started by the service control manager |
| Decisions made without asking | "Decisions to confirm" below | The handoff left these open. Each was decided and is listed |
| A change that was not mine in the first commit | "Where the build differs" below | `.gitignore` gained `/refs/itu/` in commit 12c8d9f. Say if it should not be there |
| The failure of a write to a port is reported differently | "Where the build differs" below | A log message id changes in one failure path. Say if that needs another answer |

## What was built

| # | Commit | What |
|---|---|---|
| 1 | `refactor(svc): Tokio runtime and shutdown channel` | `service/svc` became a library and a program. `svc::runtime` builds the runtime; `svc::shutdown` is the watch channel that the console handler and the service's stop control send on |
| 2 | `refactor(control): control server and named pipe on Tokio` | `ctlsrv`'s transport traits are async, its in-memory transport is `tokio::io::duplex`, each client has a reading and a writing task, and the pool is reached through `PoolLink`. `service/pipe`'s listener is Tokio's named pipe server; its blocking client is as it was, and the blocking `Halves` and `Closer` it gives moved into it from `ctlsrv`. The binding refresh became a task here |
| 3 | `refactor(svc): pool task and serial bridge, removing the wait-handle limit` | `svc::pooltask`, `svc::bridge`, `Pool::next_deadline`, and a test of a hundred lines. The pool's thread, its Win32 stop event and its wait on up to 64 handles are gone |
| 4 | `refactor(telephony): SIP socket on Tokio` | `svc::sipsock`. `telephony`'s `UdpTransport`, its thread and its queue are gone, and the tests that need a socket moved to `service/svc/tests/` |
| 5 | `refactor(media): wake the pool task through a channel and a drain interval` | `svc::mediawake`, the 10 ms look at the calls' data, `Pool::calls_up`, and a waker that can say its wake did not go. The soak runs on the real runner |
| 6 | `docs: runtime model and Tokio retrofit record` | The demo service on the same runner; the tests of a slow client and of shutdown; `ci/pure.sh`; this document; version 0.0.4 |

Small additions to pure crates, none of them a dependency: in
`service/pool`, `Pool::next_deadline` and `Pool::calls_up`; in
`service/media`, the `Waker` type now returns whether the wake was
delivered, and the media thread tries again if it was not.

## Tick times, before and after

The 60-second soak: 24 calls over SIP and RTP on loopback, each figure the
greatest of the 24 lines'. A tick is one 10 ms frame through the engine.
All runs are debug builds, as `cargo test` makes, on the development
machine.

| Run | Median | 99th percentile | Longest | Overruns |
|---|---|---|---|---|
| Phase 2, as its record has it | 4 us | 26 us | 347 us | not recorded |
| Before package 5, run 1 | 5 us | 31 us | 217 us | 0 |
| Before package 5, run 2 | 4 us | 37 us | 276 us | 0 |
| After, on the real runner, run 1 | 4 us | 26 us | 277 us | 0 |
| After, on the real runner, run 2 | 5 us | 26 us | 283 us | 0 |

The "before" runs were made on the day of the change with the media path
exactly as phase 2 built it; the soak did not print overruns then, and
they are zero because an overrun is a tick longer than a packet, which is
20,000 us. In the "after" runs the soak also printed that no wake of the
pool was refused.

The median and the 99th percentile are no worse. The longest tick is one
tick in the 144,000 of a run, and moves by as much as this between two
runs of the same code; it is inside the range phase 2 recorded.

The two sets were not measured the same way, and that is deliberate.
Before, the test drove the pool itself from its own thread, every 300
microseconds. After, the pool is on its task, the ports are on their
threads and the test only plays the hosts and the far end: the path the
service runs.

## Tests

No assertion was weakened or removed. Every test whose mechanics changed
is listed here; for each file the assertions were compared, by script, with
those of the commit before, and are the same but for `.await`.

| Tests | What changed in how they run |
|---|---|
| `service/ctlsrv/tests/api.rs`, 16 tests, 304 assertions | They are `#[tokio::test(start_paused = true)]`. The client is async, and the fixture's pool is a task on Tokio's clock, so what waited on the pool's timers no longer waits. The assertions are identical |
| `service/pipe/tests/api.rs`, 11 tests, 53 assertions | The listener and the server run on a runtime the tests share; the clients still block, as real ones do. Three tests ended a listener's wait for a client with a Win32 event; the wait is now given up as the server gives it up, on its stop channel. The assertions are identical |
| `tools/ctl/src/tests.rs`, 15 tests, 125 assertions | The control server runs on a runtime the tests share, and `rustmodem-ctl`'s blocking client reaches the in-memory transport through `ctlsrv::memory::blocking`. The assertions are identical |
| `soak.rs`, `transparency.rs`, `udp.rs`, `peer.rs` | Moved from `service/telephony/tests/` to `service/svc/tests/`, where the socket now is. `transparency.rs` (18 assertions), `udp.rs` (22) and `peer.rs` (34) take their SIP socket from `svc::sipsock` and are otherwise as they were: they still drive the pool themselves. Their assertions are identical |
| `soak.rs` | Also moved onto the real runner in package 5. Its 18 assertions keep their conditions; three had `states(&pool)` in the message they print and have `states()`. Two were added: that the ports were opened, and that the service stops in ten seconds |
| `service/media/tests/threaded.rs`, `capture.rs` | The wakers they give return `true`. The assertions are identical |

Unchanged in every way: the pool's tests, which run on `InlineRunner` with
a clock the test moves; all of `voip/*`; the router's tests in
`service/telephony/tests/`; the programs' tests of `rustmodem-svc` and the
demo service; every test of the manager, the screenshots among them; the
fuzz targets.

New tests, 21 of them:

| Test | Where |
|---|---|
| A hundred lines on memory ports, fifty calls at once, on the runner | `service/svc/tests/lines.rs` |
| A slow control client cannot delay the pool's task | `service/svc/tests/runner.rs` |
| The stop signal ends every task and thread, with a call up and a line registered | `service/svc/tests/runner.rs` |
| The serial bridge: both directions, a host that does not read, a pool that does not read, a failing port, release on drop, a host that closes | `service/svc/src/bridge.rs`, seven |
| The wake channel: a notice wakes the task, a full channel refuses at once and is counted | `service/svc/src/mediawake.rs`, four |
| A SIP socket whose router is behind drops datagrams and counts them | `service/svc/src/sipsock.rs` |
| The runtime's threads are named; the stop signal reaches every receiver | `service/svc/src/runtime.rs`, `shutdown.rs`, three |
| The pool's deadline, and a pool served only at its deadlines keeps the ring cadence | `service/pool/tests/deadline.rs`, two |
| A wake that is refused is tried again until it is taken | `service/media/tests/threaded.rs` |

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| Telephony's I/O is split from its logic by a feature or by moving it into `service/svc` | Moved into `service/svc`, and the four test files that need a socket with it | With a feature, the tests of the socket would not have run in the Linux job, where they run now |
| The runner is in `service/svc` | `service/svc` is a library and a program; the library has the runner and builds on Linux | The demo service has to use the same runner, and the tests of the runner have to run somewhere other than inside a program |
| The Linux job is the pure crates | It also lints and tests `ctlsrv` and the library of `svc` | Neither is pure, but neither needs Windows, and the soak and the megabyte test would otherwise have left Linux. The service's program is still not built there |
| `Some(ev) = serial_events.recv() => bridge.apply(ev)` | Each port has channels of its own, which the pool's end of the port reads; the port's thread notifies the task | A channel a port means a line that is not read holds back only itself |
| `Some(dg) = sip_datagrams.recv() => router_input(dg)` | The router takes datagrams from the channel itself, through `SipTransport`, as it took them from the old queue; the socket's task notifies the pool's task | The router pulls its input. Giving it a second way in would have changed `service/telephony` |
| Rare events go through a bounded channel | A notice goes through the channel; the event is in the call's ring | The ring is part of the seam in `service/pool`, which is pure. Putting the events on a Tokio channel would have put Tokio in the pool |
| The raw security-attributes call is the only `unsafe` in the pipe server | It is, with the two calls that make and free the descriptor it is given, and one more: `GetNamedPipeClientProcessId`, for the "process N" in the log line of a client connecting | Tokio does not give the client's process id, and leaving it out would have changed a log line |
| The port thread uses `blocking_send` | For what the host did and for a failure. What the host wrote is sent with `try_reserve` | A thread blocked on octets the pool has no room for could not set the signals the pool asks for meanwhile |
| A write or a change of signals fails where the pool asks for it | It fails a moment later, in the port's thread, and the pool is told when it next reads the port's signals: the port fails as it would have, with `RMD3005` (signals could not be read) where it would have said `RMD3003` or `RMD3004` | The pool learns which operation failed from which call returned the error, and it has not called the one that failed. A port that has gone fails its reads first in any case, which is what the pool reported before |
| `Pool::next_deadline` covers every timer | It is the next 20 ms tick, by which every timer is looked at | See "The pool's task". The pool cannot sleep past a tick while the interpreters are stepped by it |
| The demo service uses the same runner | It does, with the bridge. The frozen script's ports are read by the pool directly | That script steps the pool and the hosts by hand, one after the other, so that every run and every screenshot comes out the same |
| `tools/sippeer`: convert it if that makes the tests simpler | Not converted | It would not have |
| Each commit has only its package | Commit 12c8d9f also has one line in `.gitignore`, `/refs/itu/` | It appeared in the working tree while the package was being built, and `git add -A` took it. It is not mine, it is harmless, and pushed history is not rewritten. Files have been staged by name since |
| The binding refresh becomes a task in package 3 | In package 2 | The server's `refresh_bindings` became async there, and a thread could no longer call it |

## Decisions to confirm

- **`service/svc` as a library.** `svc::{runtime, shutdown, pooltask,
  bridge, sipsock, mediawake}`. The program beside it keeps what is of
  Windows only.
- **`service/pipe`'s `server` feature**, so that the manager depends on
  neither Tokio nor `ctlsrv`. The blocking `Halves` and `Closer` are
  `pipe`'s now.
- **`ctlsrv::PoolLink`** takes the place of `pool::PoolHandle` in the
  server. `PoolHandle` is still in `service/pool`, for a pool driven
  without a runtime.
- **A client's tasks end at the stop signal.** Before, client threads
  lived until the process ended. What is queued for a client is given one
  second to be written.
- **A request that has been read is carried out whole**, whatever happens
  to the client or the service meanwhile. Only the wait for the next
  request is given up. A configuration change is never left half made.
- **Memory ports are looked at every 5 ms** by their threads, which have
  no handle to wait on. A com0com port's thread waits on the port's handle
  and looks at it every 20 ms regardless, as the pool's thread did.
- **The full-channel counters are counted and not reported.** The SIP
  socket counts the datagrams it drops, and the wake channel the notices
  it refuses. Nothing shows them yet: the control API was not to change.
  Phase 2's SIP queue dropped without counting.
- **Blocking work on the pool's task is as it was on the pool's thread:**
  opening a port, resolving a registrar's name, waiting up to two seconds
  for a media thread to give an engine back. A worker thread waits
  meanwhile; the other serves the control clients.

## References consulted

| Reference | For | How |
|---|---|---|
| The source of Tokio 1.54.1 | `try_send` on a bounded channel; `named_pipe::ServerOptions`; `Notify`; `watch` | Read in the Cargo registry: `sync/mpsc/bounded.rs`, `sync/mpsc/chan.rs`, `sync/mpsc/list.rs`, `sync/mpsc/block.rs`, `sync/batch_semaphore.rs` |

No GPL or LGPL project was opened.

## Dependencies

| Crate | Used by | Licence |
|---|---|---|
| `tokio` 1.54, features `rt-multi-thread`, `net`, `time`, `sync`, `macros`, `io-util`; `test-util` for `ctlsrv`'s tests | `svc`, `ctlsrv`, `pipe` (feature `server`), `demosvc`; the tests of `ctl` | MIT |
| `tokio-macros`, `mio`, `socket2`, `bytes`, `pin-project-lite` | Tokio | MIT, or MIT OR Apache-2.0 |
| `wasi` | In the lockfile only: `mio` names it for a target rustmodem is not built for | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |

No `tokio-util`, `futures` or `async-trait`: the transport traits return
`impl Future`. `cargo deny check` passes. None of these compiles C.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check`, `ci/headers.sh` and all tests pass on Windows; the pure crates pass on Linux, and none of them depends on Tokio | Run locally on Windows before every commit, and the Linux job under WSL before every push. `ci/pure.sh` is in the Windows job |
| 2 | `docs/CONTROL_API.md`, the behaviour tables of `docs/TELEPHONY.md` and the `.rmcap` format are unchanged, and old clients work | None of the three files, nor `service/ctlproto`, `voip/*` or the router, was touched. The manager's tests and `rustmodem-ctl`'s run their clients against the new servers |
| 3 | No test assertion was weakened or removed | "Tests" above |
| 4 | 100 lines on memory ports are served correctly | `service/svc/tests/lines.rs` |
| 5 | The 60-second soak passes, with tick times no worse than phase 2's | "Tick times" above |

### Manual checklist

NEEDS-CODY: these need the machine the service will run on, an elevated
prompt, com0com, and for criteria 7 and 8 a gateway and a telephone. The
set-up is the one in the checklists of `docs/PHASE0.md` and
`docs/PHASE2.md`; what is new is what to look for.

**6. The phase 0 call by hand on two com0com pairs.**

- [ ] Go through criterion 5 of `docs/PHASE0.md`: `AT` gets `OK` on both
      ports, `ATD2`, `RING`, `ATA`, `CONNECT`, typing passes both ways,
      `+++`, `ATH`, `NO CARRIER`.
- [ ] The manager, open on the service, follows the call as it happens.
- [ ] Each port now has a thread. In the Threads tab of Process Explorer
      the service has two `rm-ctl` threads and an `rm-port` thread for each
      line; its processor time is near nothing while the lines are idle.
- [ ] Close a terminal: its line is not ready within a moment. Open it
      again: the line is idle. This is the first time the port's handle has
      been waited on from its own thread.
- [ ] Go through the signal checklist at the end of `docs/COM0COM.md`.

**7. A gateway call, and a registered line.**

- [ ] Criterion 8 of `docs/PHASE2.md`: a call from the gateway rings,
      answers with `ATA` and clears from either end.
- [ ] Criterion 9 of `docs/PHASE2.md`: a line registers; leave it past its
      expiry and `rustmodem-ctl sip` still shows it registered, with the
      time left renewed.

**8. A 10-minute captured call.**

- [ ] Criterion 12 of `docs/PHASE2.md`: `rustmodem-ctl capture check`
      reports Lost 0, Resync 0, Slips 0 and exits with 0.

**9. The 10-minute soak on the real machine.**

- [ ] `set RM_SOAK_MINUTES=10` and
      `cargo test --release -p svc --test soak -- --nocapture`.
- [ ] It passes. Record the line it prints here, beside phase 2's: the
      median, the 99th percentile, the longest tick, the overruns, which
      are to be zero, and the wakes refused, which are to be zero.

**10. The service installs, starts, stops and uninstalls.**

- [ ] Criterion 7 of `docs/PHASE0.md`: `--install`, `net start`,
      `rustmodem-ctl info`, `net stop`, `--uninstall`, and the two events
      in the Event Log.
- [ ] With a call up and a line registered, `net stop rustmodem` returns
      well inside thirty seconds; the caller's telephone is hung up; the
      PBX shows the extension unregistered; the log ends with `RMD0002`.
- [ ] In console mode, Ctrl+C with a call up does the same, and the
      prompt comes back.

## Known limits

- The 63-handle limit of phase 0 is gone.
- A registrar or peer given by name is still resolved while the pool's
  task waits, as in phase 2.
- What the SIP socket drops and what the wake channel refuses are counted
  and shown nowhere.
- The runtime has two worker threads, a number chosen and not measured.
