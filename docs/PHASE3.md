# Phase 3 record: 300 bit/s

Phase 3 brings the first modem. A line whose engine is `modem`, which is now
the default, answers a call as a modem does and connects at 300 bit/s, by
V.21 or Bell 103, agreeing which by V.8 where the caller has it and falling
back as V.25 has it where the caller has not. Everything under it is new
and in-house: the G.711 codecs, the signal processing blocks, the tones of
a call's start, the frequency-shift modem, V.8, a simulated telephone
channel to test them through, and the engine that runs a call on the media
thread.

All ten work packages are built and committed. The eight automated exit
criteria pass. The seven manual ones need the Courier, the SPA112 and the
phase 2 checklist run first; they are a checklist at the end. Nothing of
the manual seven has been done: **the modem has connected only to itself.**
It has never heard a modem that is not its own, and every claim here about
a real one is untested.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| Manual exit criteria 9 to 15 | "Manual checklist" below | Run them, the phase 2 checklist first (criterion 9). Until then the modem is proven against itself and a simulator |
| Captures of real calls | `modem/engine/tests/corpus/README.md` (`NEEDS-CODY`) | Criterion 13: each call of the checklist, captured, goes in the corpus with what a replay of it does |
| Decisions made without asking | "Decisions to confirm" below | The handoff left these open and nobody was there to ask. Several are in the configuration, the AT command set and the control API |
| `unsafe` in a new crate | `tools/alloccount`, `CLAUDE.md` rule 8 | The handoff forbids `unsafe` in every new crate and asks for a test that counts allocations, which needs an allocator, which cannot be written without it. It is in a crate of its own that only tests use, and rule 8 names it. Say if it should be done another way |
| Bell 103's frequencies | `modem/fsk/src/lib.rs` | They are the handoff's. No Bell publication is in `refs/`, so they were checked against nothing |
| Two red commits on `main` | "Where the build differs" below | Both on the Linux job, and both races in tests that the slower runner lost, not faults in what was committed. Both tests are repaired |
| The pictures of the manager | `ui/manager/tests/goldens/` | Every picture of the main window and of the line's dialog changed, and four are new. They were taken by CI on a branch, as in phases 1 and 2. Look at them |
| Still open from earlier phases | `docs/PHASE0.md` to `docs/PHASE2.md`, `docs/RUNTIME.md` | Their manual checklists and `NEEDS-CODY` items are as they were |

## What was built

### 1. `dsp/g711`

The u-law and A-law codecs of G.711 as tables: 256 values to decode, and an
encoder that follows the Recommendation's decision values. `f32` both ways,
with 1.0 the overload point, and the 14-bit and 13-bit integers of the
tables for the tests. The conversions between the laws (Tables 3 and 4).
The tests hold every code against the Recommendation's tables.

### 2. `dsp/dspkit`

The blocks everything else is made of: a numerically controlled oscillator
with a 32-bit phase, windowed-sinc FIR filters, biquads, a DC blocker,
Goertzel detectors, one that slides, a level meter in dBm0, a carrier
detector with two thresholds and two times, automatic gain control, a
moving average, a fractional delay, a resampler good for 5% either way, and
a Hilbert transformer with a frequency shifter. Under them is a module of
`sin`, `cos`, `exp`, `ln` and the rest made of the four operations alone,
so that a filter's taps are the same bits on every machine.

### 3. `dsp/tones`

Answer tone in its four kinds, plain, with phase reversals, modulated at
15 Hz (ANSam) and both; calling tone; and their detectors. The answer tone
detector says which kind it hears, within 115 ms at a signal 10 dB over
noise, and did not once call ten minutes of noise or eight of speech-like
sound a tone.

### 4. `modem/fsk`

V.21 and Bell 103. The modulator keeps its phase continuous and has a
transmit filter that puts what it sends more than 50 dB down in the other
band. The demodulator is a band filter and two correlators one bit long;
its soft output is which tone is the stronger. A line signal detector at
the levels of V.21 clause 8.3. Start-stop framing both ways: 8N1, the
receiver timed from the edge of each start bit, with framing errors and
breaks reported. "Bit error rate" below has what it does against theory.

A fuzz target found a defect in this package after it was pushed: on noise,
a false start bit restarted the bit clock, and bits came out at up to twice
the line rate. The receiver was hardened and a test holds it; that is the
commit `fix(fsk): keep the start-stop receiver's bit rate bounded on
noise`.

### 5. `modem/v8`

The signals of V.8, CI, CM, JM and CJ, with their coding and decoding, and
the two ends of the procedure as state machines that are given bits and
the time and give back bits, timers and an outcome. They are driven by the
start-stop receiver's bits. A second fuzz target.

### 6. `dsp/chansim`

A telephone channel between two modems, from a seed: for each direction a
pad, gain, a sloped loop, a second coding, a frequency shift, a clock that
is off, noise, delay, slips, echo and lost packets. `Duplex` runs two
engines through it and collects what they do, and is what the modem's own
tests are written on.

### 7. `modem/engine` and the service

`Modem` is the call: silence, answer tone, V.8 or the fallback, training,
data, and the carrier lost. It is one structure of a fixed size, everything
a call needs is made when the call is answered or begun, and its tick
allocates nothing. `docs/MODEM.md` has what it does step by step.

In the service: a `[modem]` table and a `modem` table on a line; `engine =
"modem"` as the default; `+MS`, `B0`, `B1` and `S9` in the AT interpreter,
with `S9`, `S10` and `+MS` given to the engine when a call begins; the
engine told the law of the call's G.711; and the engine's snapshot of
itself carried from the media thread to the pool ten times a second,
through atomics, without the tick waiting on anything.

### 8. Replay and the corpus

`rustmodem-replay` (`tools/replay`) gives the frames a captured call
received to a new engine and prints its states, its events and the data it
receives; `--expect` compares that with a file and `--tx-out` writes what
the engine sends as sound. Three captures are in
`modem/engine/tests/corpus/`, each with its expectation and a note, and a
test replays them. The test peer has the modem as a kind of media, in the
program and in its scripts (`call ... as modem originate v21`).

### 9. Control API, CLI and manager

All additive: the protocol is still 1. `LineInfo.modem` and
`CallInfo.modem`; `lines.diag` and the `line.diag` event, which a client
is sent for one line, at most four times a second, and only when it asks
by name; `selftest.run`; `[modem]` in `config.get` and `config.set`, and
`modem` in `lines.add` and `lines.edit`. `rustmodemctl diag` and
`selftest`. In the manager: Rate cap and modulations, Line diagnostics and
Loopback self-test, the modem among the engines of a line with its own
settings, and the Retr. column and "Softmodem engine" live. Nothing is
marked `BLOCKED(phase3)` any more.

The scope of the diagnostics is the one thing in the manager that is not a
stock control, there being none for it: a static control drawn by its
owner, on a bitmap in memory that is copied to the screen in one piece
(`ui/win32/src/canvas.rs`).

### 10. This record

With `docs/MODEM.md`, and the modem's parts of `docs/AT.md`,
`docs/CAPTURE.md`, `docs/CONTROL_API.md`, `docs/TELEPHONY.md`, `README.md`,
`CHANGELOG.md` and `CLAUDE.md`. The version is 0.0.5.

## Bit error rate

Modulator into demodulator through the simulator and both G.711 laws, with
noise added, 400,000 bits a point, the receiver given the bit timing. The
figure is the error rate, and after it how many decibels better that is
than the curve for non-coherent detection of orthogonal tones,
`0.5 exp(-Eb/2N0)`.

| | Eb/N0 8 dB | 10 dB | 12 dB |
|---|---|---|---|
| V.21, caller's band | 1.058e-2, +0.87 dB | 1.345e-3, +0.73 dB | 4.500e-5, +0.70 dB |
| V.21, answerer's band | 1.027e-2, +0.90 dB | 1.195e-3, +0.82 dB | 5.501e-5, +0.61 dB |
| Bell 103, caller's band | 1.074e-2, +0.85 dB | 1.403e-3, +0.70 dB | 6.001e-5, +0.57 dB |
| Bell 103, answerer's band | 1.035e-2, +0.90 dB | 1.260e-3, +0.78 dB | 2.750e-5, +0.93 dB |

The demodulator is better than that curve, by 0.57 dB at the least. The
handoff asks for an implementation loss against "the theoretical
non-coherent FSK curve" and a loss is not what there is: that curve is for
tones that are orthogonal over a bit and a detector that looks at one bit,
and V.21's tones are 200 Hz apart at 300 bit/s, with continuous phase and a
band filter before the detector. A probe written apart from the crate, in
`f64`, reproduced the curve with an orthogonal pair, so the measurement is
not what is off. The test asserts a margin of more than 0.3 dB over the
curve, and that the error rate is above the bound for coherent detection,
which nothing can beat.

With no error at all, 64 KiB or more each way: at levels from -10 to
-42.5 dBm0; with the carrier 20 Hz either way off, where V.21 asks for 12;
with the far end's clock half a percent either way off; with an echo 15 dB
louder than the far end's signal; and at Eb/N0 of 20 dB.

The engine, in process through the simulator, 64 KiB each way with no
error: at Eb/N0 of 18 dB; at -10, -25 and -40 dBm0; 7 Hz either way off;
200 parts in a million either way; an echo 12 dB louder than the far end;
and all of these at once in A-law with a second coding and a sloped loop. A
packet lost during data costs five characters at the most and the carrier
is kept; a packet lost during V.8 makes the call connect later.

## Tick times

A tick is one 10 ms frame through the engine. The soak has 24 calls for 60
seconds, eight of them between modems; each figure is the greatest of its
lines'.

| Run | Engines | Median | 99th percentile | Longest |
|---|---|---|---|---|
| Phase 2, the development machine | echo and null | 4 us | 26 us | 347 us |
| Phase 3, the development machine | echo and null | 5 us | 25 us | 306 us |
| Phase 3, the development machine | modem | 17 us | 43 us | 272 us |

A modem's tick is about 12 microseconds more than the path's own, of ten
thousand it has. The soak asserts that the 99th percentile of every modem's
ticks is under a millisecond, and prints the figures, so the CI log of any
run has the runner's.

The self-test runs two modems against each other for 64 KiB each way,
which is more than 36 minutes of a call, in about six seconds.

## Markers

Every `BLOCKED` and `NEEDS-CODY` in the tree. Every `BLOCKED(phase3)` is
gone.

| Marker | Location | What is deferred |
|---|---|---|
| `BLOCKED(phase4)` | `modem/fsk/src/lib.rs` | Character formats other than 8N1 |
| `BLOCKED(phase4)` | `ui/model/src/lines.rs` | From phase 2: the Err column counts the RTP's errors; the error-control protocol's are added to them |
| `BLOCKED(phase5)` | `service/media/src/os.rs` | From phase 2: RTP packets are not marked with a DSCP |
| `BLOCKED(phase5)` | `service/svc/src/service.rs`, `service/svc/src/eventlog.rs` | From phase 0: the service account and the event message file |
| `BLOCKED(phase7)` | `dsp/chansim/src/lib.rs` | The simulator has no group delay and no nonlinear distortion: V.34 needs them |
| `BLOCKED(phase8)` | `dsp/chansim/src/lib.rs` | The simulator has no robbed-bit signalling: V.90 needs it |
| `BLOCKED(phase9)` | `ui/manager/src/dialogs/connect.rs`, `service/ctlsrv/src/lib.rs` | From phases 0 and 1: another computer's service, and TLS for it |
| `NEEDS-CODY` | `modem/engine/tests/corpus/README.md` | Captures of calls with real modems |
| `NEEDS-CODY` | `docs/GATEWAYS.md`, `docs/TELEPHONY.md`, `docs/RUNTIME.md` | From phase 2 and the retrofit |
| `NEEDS-CODY` | `service/svc/src/ports.rs`; `dte/serial/src/com0com.rs` (three), `docs/COM0COM.md` | From phases 0 and 1 |

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| Work packages in the order 1 to 10 | The simulator, package 6, was built before the modem and V.8, packages 4 and 5 | Their tests go through it |
| Each work package is one commit | Fourteen commits. Package 7 is two, the engine and then its place in the service; package 9 is two, the control API with the CLI and then the manager; and there are `fix(fsk)` and a `test` commit | Each half stands by itself and was checked by itself. The fix is the fuzz finding above, and the test is the second of the red commits below |
| Encoding a value gives the nearest code | It follows the decision values of G.711, and just above the first decision value of a segment that is the code below the nearest | The Recommendation defines the encoder so. The test asserts it, and that those are the only places the two differ |
| The idle code of A-law decodes to silence | It decodes to 1/4096: A-law has no zero | The Recommendation's table. u-law's `0x7F` decodes to negative zero, so that decoding and encoding again is the identity on all 256 codes |
| The answer tone detector's thresholds are V.25's | They are those V.21 clause 8.3 gives a line signal detector, -43 and -48 dBm0 | V.25 gives none |
| A loop slope of up to 9 dB | Up to 7 dB | The simulator's loop is one pole, which at 8000 Hz can make no more between 1 and 3 kHz |
| The demodulator meets an implementation loss against theory | It asserts a margin over the curve | See "Bit error rate" |
| Zero errors with the clock up to 2% off | Up to 0.5% for the modem; the framing alone takes 3% | A clock 2% off moves the carriers themselves out of the tolerance V.21 gives them |
| `unsafe` is forbidden in every new crate | It is in `tools/alloccount`, the counting allocator | See "What needs Cody" |
| `S9` is a register of V.250 | It is the Hayes register; V.250 (07/2003) has `S10` and no `S9` | The handoff asks for it, and the Recommendation does not have it to cite |
| `+MS` reports the carrier names `V21` and `BELL103` | `V21` and `B103` | V.250 Table 13 keeps names that begin with `V` for ITU-T modulations, and the others are for the manufacturer to choose: they are not to begin with `V`, and `B103` is the name modems of that family use. `CONNECT` and the control API say `BELL103` |
| A line's settings have the time given to each modulation | `fallback_ms`, in `[modem]`, 3000 by default | The handoff asks for the time and names no key |
| The manager has a rate cap | It has none | There is one rate. The menu item keeps its name |
| The 64 KiB over SIP and RTP is a long test | It is in the ordinary run | Two ends that tick on what they receive have no clock between them: three calls of 36 minutes each take five seconds |
| The corpus has the engine calling itself | It has the engine answering itself: three captures, of a caller with V.8, with V.21 and no V.8, and with Bell 103 only | The bench that captures has the peer call the line. A capture of the line calling is not there |
| Replay takes `--answer`, `--originate`, `--law`, `--expect` and `--tx-out` | And `--modulations` and `--no-v8`, and it prints `sent-as-captured` | A capture does not say what the modem's settings were, and a replay with others is not that call. The line says for how many frames the replay sends what the call sent |
| `rustmodemctl` has "modem settings on `line`" | `line` shows them | The program has no command that changes a line |
| The demo service names the engine it relied on | Its lines are modem lines, except in the `sip` script, and its calls are still played by the demo engine, which modulates nothing and makes up what a modem would show of itself | So that the manager's three new windows have something to work on in a demo and in the tests |
| Goldens are blessed on the CI runner image | The manager's commit went to a branch, `phase3-goldens`, first; CI took the pictures; the branch was deleted | As in phases 1 and 2 |
| Every commit on `main` is green | Two were red on the Linux job | `feat(engine)`: the soak failed with a wrong octet on a null-engine line, on a commit that touched neither the soak nor that engine. Each end of a call in the soak began to send the moment its own engine connected, which can be before the other end's has: what is sent to a modem that is not connected yet is lost, as it is between any two modems. The runner lost that race once, and again on the manager's branch, on a modem line. Each end waits for the other's now. `feat(ctl)`: `a_hundred_lines_on_memory_ports_are_served_as_the_first_is` read the carrier signal the moment the pool said the line was online, and the signal is raised by the port's own thread a moment later. The test waits for it now. Both repairs are the `test` commit, with a third: the CLI's test of the self-test gave the service 10 seconds, and the runner took a little more |

One thing was learned about the tools and not the modem. A test run whose
output went through a pipe did not report for an hour and forty minutes
after it had finished: a manager test that fails leaves its demo services
running, and they hold the pipe. The long runs write to a file since.

## Tests whose expectations changed

The handoff makes `modem` the default engine and `+MS` a checked command,
and says that tests which relied on the old default now name the engine
they want. These are all of them.

Named the null engine, and assert what they asserted:

- `service/pool/tests/common/mod.rs`, `service/ctlsrv/tests/common/mod.rs`,
  `service/pipe/tests/api.rs`, `service/svc/tests/lines.rs` and
  `tools/ctl/src/tests.rs`: the configuration each fixture makes.
- `service/svc/tests/transparency.rs`: its two tests of the path say
  `engine = "null"` and `"echo"` in the line's settings, as they said them
  in an argument before.

Assert something else, because what the service does changed:

- `service/config/tests/config.rs` and `telephony.rs`: a line with no
  `engine` key has the modem.
- `dte/at/tests/`: transcripts 35 and 53 and `table.rs` have `S09:006` in
  the register line; transcript 36, `table.rs`, `profile.rs` and
  `escape.rs` set `+MS` to values the modem has, `B103,0,300,300`, where
  they set `V34` or `V22`, and read back all six subparameters; transcript
  38 and `table.rs` use `Y0` as the unknown command where they used `B0`.
- `service/pool/tests/operator.rs`: the same `B0` to `Y0`.
- `service/svc/src/profiles.rs`: a stored `+MS` of `V22,1,1200,1200`
  became `B103,0,300,300,0,0`.
- `service/svc/tests/soak.rs`: the lines are thirds of echo, null and
  modem where they were halves of echo and null.
- `ui/model/` and `ui/manager/`: the three commands are no longer blocked;
  the engines of a line are three with the modem first, so the echo engine
  is the third in the list; "Softmodem engine" and the Retr. column show
  something.

## Decisions to confirm

Each of these is a choice the handoff left open. They are in the code and
the documentation as described; say if any should change.

**Configuration and AT**

- `fallback_ms`, as above. It is in `[modem]` only: a line cannot have its
  own.
- A line's `modem` table takes `modulations`, `v8` and `tx_level_dbm0`.
  `answer_delay_ms` is the service's.
- `+MS` chooses within what the configuration allows. A carrier the
  configuration does not allow is not refused: the configuration's list is
  used as it is. With automode off the modem uses the one carrier and no
  V.8.
- `B0` is V.21 and `B1` is Bell 103, and they set the carrier of `+MS` and
  nothing else.
- `S10` of 255 is 25.5 seconds, not "never" as V.250 clause 6.3.12 has it.
- `I4` begins `SOFT MODEM` on a modem line.
- A change of the modem's settings takes effect at the line's next call.

**The modem**

- The answering silence is 2150 ms, the middle of what V.25 clause 4.3
  allows, after `answer_delay_ms`.
- V.8: the caller waits 1 s after ANSam (Te); a signal is taken on eight
  ones before its synchronisation where ten are sent; "two identical" is
  any two of the last three; the answerer ends JM when CM has been gone for
  a second, for a CJ that was lost; and an exchange that does not finish in
  5 s is given up, a figure V.8 does not give. V.8 is used only where V.21
  is allowed, Bell 103 not being in it.
- Without V.8 the answerer offers each of its modulations for `fallback_ms`
  and listens for the caller's carrier of any of them the whole time.
- A modem that gives up before it connects is reported as the carrier
  lost, and logged with RMD7001 naming the state it reached.
- The modem's own deterministic maths, in place of the platform's.
- The DSP crates are compiled with optimisation in the development
  profile too: their tests took 55 seconds without and take 2 with.

**Control API**

- Every number the modem reports is whole: levels in tenths of a dBm0
  (`rx_level_tenth_dbm0`), the frequency error in millihertz
  (`frequency_error_mhz`), the quality in a hundred (`quality_percent`).
  The call's RTP is reported the same way, in microseconds, and the types
  can be compared for equality.
- `LineInfo.modem` is the settings the line's calls use, with `own`
  listing the keys the line sets for itself. In `lines.add` and
  `lines.edit`, `modem` is the line's own settings whole: a key left out
  is the service's from then on, and an empty object leaves the line none.
- `subscribe` takes `line` with `line.diag`, and a client is sent one
  line's diagnostics: naming another changes which. `line.diag` is sent at
  once, then at most four times a second while the call has a modem, and
  once more with none.
- A line in a self-test shows as `handshaking`: there is no new line
  state, so that a client of an earlier version reads every line. It takes
  no call, and its host's `ATD` is answered `BUSY`.
- A self-test that runs and fails is a result with `passed` false and the
  reason in `error` (RMD7005, RMD7006). Only a test that cannot be run is
  an error of the method (RMD7002 to RMD7004).

**Manager**

- A line's own modem settings are all of them or none, where the control
  API allows each alone.
- The line's dialog lists the modem first, so a new line has it.
- The diagnostics are of the line that was selected when the command was
  chosen; chosen again with another line selected, the same window shows
  that line.

## References consulted

| Reference | For | How |
|---|---|---|
| ITU-T G.711 (11/1988) | `dsp/g711` | From `refs/itu/`. Tables 1 to 4, cited by clause |
| ITU-T V.21 (11/1988) | `modem/fsk` | From `refs/itu/` |
| ITU-T V.25 (10/96) with Corrigendum 1 (07/2001) | `dsp/tones`, `modem/engine` | From `refs/itu/` |
| ITU-T V.8 (11/2000) | `dsp/tones`, `modem/v8`, `modem/engine` | From `refs/itu/` |
| ITU-T V.250 (07/2003) with Supplement 1 | `dte/at` | From `refs/itu/` |
| The Audio EQ Cookbook's biquad formulas | `dsp/dspkit` | From memory |
| The textbook error rates of binary FSK | `modem/fsk/tests/` | From memory: `0.5 exp(-Eb/2N0)` and the coherent bound. Checked by a probe, as "Bit error rate" says |
| The WAVE format for non-PCM data | `tools/replay` | As in phase 2 |
| Microsoft's documentation of owner-drawn controls and GDI | `ui/win32/src/canvas.rs` | From memory of the function signatures, which the `windows` crate checks |
| Bell 103's frequencies | `modem/fsk` | The handoff's. No publication of them is in `refs/` |

No GPL or LGPL project was opened for any crate: not VoIPModem,
libv90modem, spandsp, linmodem or any other. Nothing in `dsp/` or `modem/`
was written from anything but the Recommendations and the handoff.

## Dependencies

Phase 3 adds no crate from outside the workspace. `cargo deny check`
passes.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check`, `ci/headers.sh` and all tests pass on Windows; every `dsp/*` and `modem/*` crate passes on Linux and depends on neither Tokio nor any OS crate | Run locally on Windows and, the whole Linux job, under WSL before every commit: 1,070 tests on Windows at the end, and the 29 pictures of the manager. `ci/pure.sh` lists the new crates and fails on Tokio in any; none of them has a dependency outside the workspace but the test crates. Two commits on `main` were red on CI: see "Where the build differs" |
| 2 | G.711 matches the Recommendation's tables exhaustively | `dsp/g711/tests/recommendation.rs`: every code of both laws, the decision values, and the conversion tables |
| 3 | V.21 and Bell 103 meet the stated bit-error-rate margin against theory and carry data with zero errors across the stated ranges | `modem/fsk/tests/modem.rs`. The margin is over the curve and not under it, and the clock range is 0.5%: see "Bit error rate" and "Where the build differs" |
| 4 | Two engines connect by V.8 to V.21, by fallback to V.21 and by fallback to Bell 103, and pass 64 KiB each way intact, in process and over SIP and RTP on loopback | In process: `modem/engine/tests/modem.rs` and `impaired.rs`. Over SIP and RTP: the three `two_modem_lines_connect_...` tests of `service/svc/tests/transparency.rs`, two whole services each with the router and the threaded runner, 64 KiB each way. And a line against the test peer's modem, clean, with its clock off and with packets lost: `service/svc/tests/peer.rs` |
| 5 | `process` allocates nothing and panics on no input; the fuzz targets `fsk_demod` and `v8_decode` run for 10 minutes each with no findings | `modem/engine/tests/alloc.rs` and `modem/fsk/tests/alloc.rs` count the allocations of whole calls; `modem/engine/tests/anything.rs` gives the engine arbitrary octets. Fuzzed under WSL: `fsk_demod` 770,273 runs and `v8_decode` 653,386, each in 601 seconds, clean, after the finding above was fixed. CI runs each for 10 minutes on every push |
| 6 | The corpus replays to its expectations | `tools/replay/tests/corpus.rs` |
| 7 | The soak with modem engines passes, with tick times recorded | `service/svc/tests/soak.rs`; "Tick times" above |
| 8 | Every addition in work package 9 has a round-trip test, and nothing in the manager is left marked `BLOCKED(phase3)` | `every_method_round_trips` in `service/ctlsrv/tests/api.rs` and `every_method_round_trips_over_a_real_pipe` in `service/pipe/tests/api.rs` fail on a method with no case; `service/ctlsrv/tests/modem.rs` has the settings, the diagnostics and who is sent them, and the self-test with its 64 KiB; `service/ctlproto/tests/api.rs` holds every new type to its documentation. `grep -r "BLOCKED(phase3)"` finds it only in the records of phases 1 and 2 |

### Manual checklist

`NEEDS-CODY`: these need the machine the service runs on, an elevated
prompt, the SPA112 set up as `docs/GATEWAYS.md` says with the US Robotics
Courier on its FXS port, and a terminal program on the Courier's serial
port. `10.10.20.11` stands for the gateway's address; line 1 is on a
com0com pair as in phase 0's checklist, with a terminal program on its
host port.

The commands for the Courier are from memory of US Robotics modems and
were checked against no manual: `&F1` loads the hardware flow control
template, `&N1` fixes the rate at 300 bit/s, `&M0` turns error control
off, `&K0` compression off, `B0` and `B1` choose ITU-T and Bell answer
sequences, and `S27` has the V.8 and V.21 bits. **Look each up in the
Courier's manual before relying on it**, and write what was used into this
checklist.

A configuration to start from:

```toml
[sip]
enabled = true
listen = "0.0.0.0:5060"
trusted_peers = ["10.10.20.11"]
outbound_peer = "10.10.20.11"

[modem]
modulations = ["v21", "bell103"]
v8 = true
tx_level_dbm0 = -12

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }

[[route]]
called = "5551000"
target = "line:1"
```

**9. Before anything else: the phase 2 checklist.**

- [ ] Criteria 8 to 14 of `docs/PHASE2.md` have been run: above all the
      trunk call and the ten-minute capture that is sample-exact. A modem
      cannot be debugged on a path that has not been proven.

**10. V.21 without error control.**

```
rustmodem-svc --console C:\rustmodem\rustmodem.toml
rustmodemctl capture on 1
rustmodemctl watch
```

- [ ] On line 1's host: `ATS0=1`. On the Courier: `AT&F1&N1&M0&K0B0`, then
      `ATDT5551000`.
- [ ] Both ends say `CONNECT 300`.
- [ ] `rustmodemctl line 1` shows the call with `300 bit/s` and `V.21`,
      and `rustmodemctl diag 1` the state `data`.
- [ ] Text typed at each end appears at the other.
- [ ] A known file sent each way for ten minutes arrives with no error:
      compare the two with `fc /b`.
- [ ] `rustmodemctl capture check <file>` on the call's capture says
      clean.
- [ ] If error control is left on (`&M4`), the Courier opens with the V.42
      detection pattern, which arrives as a few stray characters, and then
      falls back or hangs up. That is correct for this phase; note which.

**11. V.8.**

- [ ] The Courier at its defaults but for the rate: `AT&F1&N1`, with V.8
      on. It dials.
- [ ] In the manager, View, Line diagnostics on line 1: the history shows
      `answer-tone`, `v8-jm`, `gap`, `training`, `data`. `v8-jm` is CM
      received and JM sent.
- [ ] The call is `CONNECT 300` with `V.21`.

**12. Bell 103.**

- [ ] On the Courier: `AT&F1&N1&M0&K0B1`, then dial.
- [ ] The history shows `answer-tone`, then `fallback`, `training`,
      `data`, and the call is `CONNECT 300` with `BELL103`.
- [ ] Text crosses both ways.

**13. Captures for the corpus.**

- [ ] Each of the three calls above was captured (`rustmodemctl capture
      on 1`). For each: copy the file to
      `modem/engine/tests/corpus/courier-<what>.rmcap`, run

      ```
      cargo run -p replay -- modem/engine/tests/corpus/courier-<what>.rmcap > modem/engine/tests/corpus/courier-<what>.expect
      ```

      look at what it printed, and write `courier-<what>.md` saying what
      the call was. `cargo test -p replay` replays them from then on.
- [ ] If a replay does not connect where the call did, that is a finding:
      the capture is the test for it.

**14. A second gateway.**

- [ ] Criterion 10 again with the Courier on the MP-124 or the VG310 in
      place of the SPA112.

**15. Diagnostics.**

- [ ] During a call from the Courier, the receive level the manager shows
      is steady and somewhere between -10 and -30 dBm0.
- [ ] The frequency error is within a few hertz, and the quality is
      steady.
- [ ] `rustmodemctl selftest 1 2` passes on the machine, with two idle
      modem lines: it is the same modem with no network, and tells a fault
      of the path from a fault of the modem.

## Known limits of phase 3

- The modem has met no modem but itself.
- 300 bit/s, 8N1, no error control and no compression.
- A capture does not say which end the line was or what its modem's
  settings were; a replay is told them.
- A replay is exact only until the engine's transmission changes
  (`docs/CAPTURE.md`).
- The corpus has no capture of a call the line made.
- The simulator has no robbed-bit signalling, no group delay and no
  nonlinear distortion.
- In the demo service nothing is modulated: its calls, and what its
  diagnostics show, are scripted.
