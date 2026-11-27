# Phase 4 record: V.22bis and V.42

Phase 4 makes the modem one that dial-up networking can use, and is the
first release, 0.1.0. A line connects at 2400 bit/s by V.22bis and at 1200
by V.22 or Bell 212A, as well as at 300 as before, and at the two faster
rates the call has the error control of V.42 and the compression of
V.42bis where the far end has them. Everything is in-house and written
from the Recommendations: the blocks of a synchronous modem in
`dsp/dspkit`, the modem in `modem/v22`, and three new crates under
`link/` for what is between the modem and the host.

All ten work packages are built and committed. The nine automated exit
criteria pass. The nine manual ones need the Courier, the gateways and a
machine with Routing and Remote Access; they are a checklist at the end.
None of them has been done, and phase 3's have not been reported done
either: **the modem has still connected only to itself.** Every claim here
about a real modem, about the Windows modem driver and about RRAS is
untested, and `docs/RRAS.md` was written without Windows' dialogs in front
of the writer.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| Manual exit criteria 10 to 18 | "Manual checklist" below | Run them, phase 3's first (criterion 10). Do not tag before they pass |
| The wording of Windows' dialogs | `docs/RRAS.md`, each `NEEDS-CODY: confirm wording` | The steps and what each must achieve are certain; the names of buttons, pages and list entries are from memory |
| The initialisation strings | `dte/at/tests/hostlogs/README.md` (`NEEDS-CODY`) | Criterion 15: paste the AT lines of the service's debug log from the first RRAS call into a file there. A test replays every such file and fails on a command answered `ERROR` |
| Captures of real calls | `modem/engine/tests/corpus/README.md` (`NEEDS-CODY`) | Criterion 16 |
| Decisions made without asking | "Decisions to confirm" below | Several are in the configuration, the AT command set and the control API. The first of them, the default `modulations`, changes what an existing configuration does |
| The zip's manager is `rustmodem-manager.exe` | `.github/workflows/release.yml` | The handoff names `rustmodem-gui.exe`. There is no program of that name: the manager has been `rustmodem-manager.exe` since phase 1 |
| The release workflow has never run | `.github/workflows/release.yml` | It runs on a tag, and nothing was tagged. Criterion 18 is its first run |
| The pictures of the manager | `ui/manager/tests/goldens/` | The main window, the modem's two dialogs, the diagnostics, the history and the self-test changed. They were taken by CI on a branch, as in the earlier phases. Look at them |
| Two red commits on `main` | "Where the build differs" below | `feat(v42): LAPM error control` and this record's own commit, both on the Linux job, both the soak with one wrong octet, and both a race in the test that the slower runner lost. It is mended in the `test(svc)` commit that follows this record |
| Still open from earlier phases | `docs/PHASE0.md` to `docs/PHASE3.md`, `docs/RUNTIME.md` | Their manual checklists and `NEEDS-CODY` items are as they were |

## What was built

### 1. `dsp/dspkit`

The blocks of a modem that sends symbols: complex numbers, the scrambler
and descrambler of V.22 with its rule against 64 ones, a pulse-shaping
filter that is the square root of a raised cosine, modulation to and from
a carrier, timing recovery, carrier recovery, an adaptive equaliser, a
slicer for the four and the sixteen points, and a measure of quality from
the slicer's error.

Timing is by Gardner's detector, chosen because it needs no decisions and
does not care about the carrier's phase, on two samples a symbol
interpolated by the phase-3 interpolator's taps. It pulls in from any of
sixteen starting phases inside 300 symbols and does not slip with the far
end's clock 500 parts in a million off for a minute. The carrier loop
pulls in 10 Hz either way inside a second. The equaliser is a normalised
LMS with a small leak, within 0.6 dB of the Wiener floor at 20 and 30 dB,
and does not adapt on a signal under a floor: a lost packet once left its
taps too loud for sixteen points, and that is why.

### 2. `modem/v22`, the modulator and demodulator

V.22, Bell 212A and V.22bis at 600 baud: the caller on a carrier of
1200 Hz and the answerer on 2400 Hz, with the guard tone of 1800 or 550 Hz
where it is asked for. The transmitted pulse is twelve symbols long and
puts what is sent 77 dB down in the other channel. The receiver is a
matched filter at baseband, gain after it, the timing loop, the
equaliser, the carrier loop and the slicer, all directed by decisions from
the first symbol: V.22bis has no training sequence of known symbols. "Bit
error rate" below has what it does against theory.

Not built: the 600 bit/s modes of V.22, its Alternative C, the fixed
compromise equaliser of clause 2.3 (the adaptive one does its work), and
the test loops of clause 7.

### 3. `modem/v22`, the handshake

Both ends of V.22bis clause 6.3: the unscrambled ones, S1, the scrambled
ones at 1200 and at 2400 bit/s, by their times, with the fallback to
1200 bit/s where either end is a V.22 modem; and the retrain of clause
6.5, asked for and answered. The signals are told apart by their spectral
lines. "Handshake timings" below has each time measured against its
clause. The rate change of clause 6.6 is not built: a retrain keeps the
rate.

### 4. `link/v14`

V.14: start-stop characters on a synchronous line, 8N1, 7E1 and 7O1, with
the deletion of one stop element in eight when the host is faster than
the line and its restoring at the far end, and the break. `+ICF` sets the
format, and the 300 bit/s modem takes it too, which closes the phase 3
marker on character formats.

### 5. `link/v42`, framing and detection

HDLC framing with the 16-bit check sequence, bit stuffing and aborts, and
the detection phase of V.42 clause 7.2: the originator's and the
answerer's patterns, and the timer T400.

### 6. `link/v42`, LAPM

One end of the error-corrected connection as a machine given frames, the
host's octets and the time: negotiation by XID, set-up and release,
I frames modulo 128 with RR, RNR and REJ, the window, the timer-recovery
condition, flow control both ways, the break, and the ends of clause
8.4.9. "LAPM" below says what of it is covered by which test and what is
not built.

### 7. `link/v42bis`

The compressor and decompressor of V.42bis: the dictionary as a trie in
fixed arrays, both modes with the test that changes between them,
the escape character, flush, and the errors of clause 5.8. "Compression"
below has the ratios.

### 8. `modem/engine` and the service

`Modem` chooses the modulation, by V.8 or by what the far end sends, runs
the V.22bis modem or the 300 bit/s one, and at 1200 and 2400 bit/s runs
the data link: detection, then LAPM with or without V.42bis, or V.14, as
the line's policy and the far end allow. `Connect` is reported when the
link is up, with the protocol and the compression. `docs/MODEM.md` has it
step by step.

In the service: `error_control`, `compression` and `guard_tone` in
`[modem]` and on a line; `+ES`, `+DS`, the older commands and registers
mapped to them, `+MS` with the new carriers, and the reports `+MR`, `+ER`,
`+DR` and `+ILRR`; CTS lowered when the host sends faster than the line
carries; RMD8001 and RMD8002 for a link that ends a call.

### 9. Tools, control API and manager

`rustmodem-replay` says what the data link did and, with `--frames`,
prints every frame of LAPM both ways; the engine keeps the frames for it
in a tap that is off in a call of the service. The corpus has four new
captures. The control API, the CLI and the manager have the new settings,
the receiver's figures, the data link's counts and the constellation;
"What was built" in `CHANGELOG.md` lists them. Nothing is marked
`BLOCKED(phase4)` any more.

The constellation is drawn on the canvas the scope is drawn on, in the
same window: a line at 300 bit/s has the scope and one at 1200 or 2400
the constellation, with the ideal points marked.

### 10. This record

With `docs/RRAS.md`, `docs/INSTALL.md`, `docs/rustmodem.sample.toml`, the
release workflow, the host-log fixture of the AT interpreter, and the
phase's parts of `docs/MODEM.md`, `docs/AT.md`, `docs/CAPTURE.md`,
`docs/CONTROL_API.md`, `README.md`, `CHANGELOG.md` and `CLAUDE.md`. The
version is 0.1.0. Nothing is tagged.

## Bit error rate

Modulator into demodulator through the simulator and G.711, with noise
added, the receiver finding its own timing, carrier and equaliser. The
figure is the error rate of the line's bits, descrambled, and after it
how many decibels worse that is than nearest-neighbour detection of the
same points would be at that ratio.

| | Eb/N0 | Error rate | Theory | Loss |
|---|---|---|---|---|
| 1200 bit/s, four points | 5 dB | 1.284e-2 | 1.191e-2 | 0.09 dB |
| | 7 dB | 1.701e-3 | 1.545e-3 | 0.08 dB |
| | 9 dB | 9.167e-5 | 6.725e-5 | 0.16 dB |
| 2400 bit/s, sixteen points | 9 dB | 8.407e-3 | 7.317e-3 | 0.17 dB |
| | 11 dB | 1.149e-3 | 9.412e-4 | 0.16 dB |
| | 13 dB | 5.000e-5 | 4.039e-5 | 0.11 dB |

The loss is under 0.2 dB at every point: the receiver's loops and its
equaliser, adapting on noisy decisions, cost that and no more.

The handoff asks for less than 1 dB of implementation loss, and the tests
assert it at each point.

With no error at all in the line's bits, at 1200 and at 2400 bit/s, the
modulator and demodulator alone (`modem/v22/tests/modem.rs`): at every
level from -9 to -43 dBm0, the level read within a decibel; with the
carrier 7 Hz either way off, where V.22bis asks for that much, and the
offset read; with the two clocks 200 parts in a million apart for two
minutes, in which one end's symbols move through the other's by fourteen
whole symbols; under an echo 14 dB louder than the far end's signal;
through a loop with a slope of 3 and of 7 dB; in both laws and through a
second coding; and with a long sloped loop, a second coding in the other
law, the carrier and the clock off, an echo, the guard tone and noise
30 dB under the signal all at once.

The engine, in process through the simulator
(`modem/engine/tests/sync.rs`, `loss.rs`): 1 MiB each way, intact, in
every case that connects, with and without error control; and with
everything at once in A-law.

One finding belongs here. An echo of this end's own signal joins the far
end's before the G.711 coder of the receiving side, and sets that coder's
quantising noise: with the echo 24 dB louder than the far signal about
18 dB of signal-to-noise ratio is left, which carries four points and not
sixteen. With it 14 dB louder, 28 dB is left and both are clean. It is the
coder and not the filters, and nothing in a modem with no echo canceller
can mend it; `docs/GATEWAYS.md` already says to keep the gateway's levels
sensible.

## Handshake timings

Two modems through a clean simulated line, each time measured from the
signals themselves and held to its clause by
`modem/v22/tests/handshake.rs`.

| Signal | Measured | Clause 6.3.1 asks |
|---|---|---|
| Caller: unscrambled 1 detected for 155 ms, then silence for 456 ms, to its S1 or scrambled 1 | 610.5 ms | 611 +-20 |
| S1 lasts, caller and answerer | 99.5 and 99.6 ms | 100 +-3 |
| Answerer: the end of the caller's S1 to its own S1 | 28.8 ms | 20 +-20 |
| The end of the other's S1 to scrambled 1 at 2400 bit/s, each end | 599.9 ms | 600 +-10 |
| Scrambled 1 at 2400 bit/s before data, each end | 198.5 ms | 200 +-10 |
| 32 ones at 2400 bit/s detected, caller and answerer | 35.6 and 37.2 ms | |
| At 1200 bit/s: scrambled 1 detected, to this end's own, answerer | 271.6 ms, 251.8 where the caller is V.22bis | 270 +-40 |
| At 1200 bit/s: scrambled 1 detected, caller | 271.9 ms | 270 +-40 |
| At 1200 bit/s: scrambled 1 before data, answerer | 765.0 ms | 765 +-10 |
| At 1200 bit/s: ready to send after, caller | 775.0 ms | 775 +-10 |
| Retrain: S1 lasts | 98.4 ms | 100 +-3 |
| Retrain: scrambled 1 at 2400 bit/s, and before data | 599.9 and 198.4 ms | 600 +-10, 200 +-10 |

From the start of a call, a line that answers a caller with V.8 is at
2400 bit/s with LAPM and V.42bis 7.2 seconds later: silence until 2150 ms,
answer tone, the joint menu of V.8 at 4184, the handshake of V.22bis from
4867, the data link from 6396, and data at 7160.

A lost packet in the handshake: of 480 calls, each with one packet of
20 ms lost at a different place, 479 connect. The one that does not loses
the last fifth of the answerer's S1 on its way to the caller and gives up
cleanly, with the carrier reported lost.

## LAPM

The edition of V.42 the link is written from, 03/2002, has no state
tables: the handoff expects them, and the tests are by clause instead.
`link/v42/tests/lapm.rs`:

| What | Test |
|---|---|
| The frames of Tables 6 and 8, octet for octet | `the_frames_of_a_connection_are_those_of_tables_6_and_8` |
| XID as clause 12.2 has it, and every offer of compression with its answer | `the_xid_frame_is_that_of_clause_12_2`, `every_offer_of_compression_gets_the_answer_of_v42bis_clause_5_1`, `an_answerer_keeps_n401_and_k_between_what_is_asked_and_the_default` |
| Set-up, N400 tries, a lost UA, a refusal, the collisions of clause 8.9 | `set_up_is_tried_n400_times_and_then_given_up`, `a_lost_ua_is_made_up_for`, `a_refusal_and_the_collisions_of_clause_8_9` |
| Data both ways, the window, RNR | `data_is_carried_both_ways_in_order`, `the_window_closes_at_k_and_the_host_is_held_off`, `a_host_that_does_not_read_stops_the_far_end_with_rnr` |
| REJ, the timer T401 with its poll, N400 polls, T403 | `a_lost_i_frame_is_asked_for_again_with_rej`, `a_lost_last_frame_is_found_by_the_timer`, `a_far_end_that_stops_answering_is_given_up_after_n400_polls`, `an_idle_connection_is_checked_every_t403` |
| What ends a connection (clause 8.4.9) | `what_cannot_be_right_ends_the_connection` |
| The break | `a_break_is_carried_and_given_once` |
| The property: whatever is lost, repeated, reordered or made up, what is delivered is a prefix of what was sent, and below the threshold of giving up all of it | `whatever_is_lost_what_is_delivered_is_what_was_sent`, `any_loss_below_the_threshold_loses_no_data`, `frames_repeated_and_out_of_order_never_put_wrong_data_through`, `any_frames_at_all_are_borne` |

The machine's states are five: disconnected, negotiating, establishing,
connected and releasing. All are reached by the tests. Within the
connected state the conditions of far end busy, own end busy, reject sent
and timer recovery are each reached, and in combination by the property
tests. What is not reached because it is not built:

- **Selective reject (SREJ) is not built.** It is optional, is not
  offered in XID, and an SREJ received is a frame that cannot be right.
  Recovery is by REJ: go back and send again from the frame that was
  lost.
- The loop-back test (TEST frames) and the 32-bit check sequence, both
  optional and neither offered.
- Re-establishment of a connection. The 2002 edition has none: where the
  earlier ones reset the link, clause 8.4.9 ends the connection, and so
  does this. A machine makes one connection; after its end SABME is
  answered DM.

Two things were found by running it, and both are tests now.

At 2400 bit/s a frame of 128 octets is on the line for half a second. A
REJ takes V(S) back, and the acknowledgement of a frame sent before the
REJ can arrive while the frame after it is still going out. By the letter
of clause 8.5.2 that N(R) is greater than V(S) and ends the connection,
and with one packet in a hundred lost a call ended so after about 200
seconds. N(R) is held to the number of the next frame never sent, and
V(S) goes on from such an acknowledgement.
(`a_frame_sent_before_going_back_may_be_acknowledged_after`.)

The fuzz target `lapm_frames` found, on its first run, that a SABME held
back on the line could make a connection again at one end after that end
had given up, while the other end still had the old one, and data of the
new was delivered as the old one's. It was found and mended before the
commit.
(`a_connection_that_has_ended_is_not_made_again`.)

### Under loss

The test the handoff calls the one that matters,
`modem/engine/tests/loss.rs`: two engines at 2400 bit/s with LAPM and
V.42bis, one packet of 20 ms in a hundred lost at random each way, for
600 seconds of the call, random data both ways as fast as the link takes
it. About 80,000 octets cross each way, 133 a second against 282 on a
clean line; some 600 of 1,590 I frames are sent again, a frame of 128
octets spanning 22 packets; no retrain; the call holds; and every octet
delivered is the octet sent.

A burst of lost packets does not force a retrain, which the handoff
expected it to: the receiver holds its state through a fade of up to 1.2
seconds and finds the signal where it left it, and LAPM sends again what
was lost. Longer than `S10` and the carrier is lost, as it should be. So
the retrain is forced the way a line forces one, by changing: 6 dB more
loss and another slope in mid-call. The modems retrain, the retrain is
counted, and the data goes on intact.

The same over SIP and RTP on loopback, with the threaded runner and the
test peer losing one of its packets in a hundred for a minute:
`a_call_with_error_control_loses_no_data_when_the_peer_loses_packets` in
`service/svc/tests/peer.rs`.

## Compression

`link/v42bis/tests/v42bis.rs` compresses each of these and decompresses it
again, at each dictionary size and string length, comparing the two
dictionaries after every step. The ratio is octets in to octets out.

| Data | 512 codewords | 1024 | 2048 | 4096 |
|---|---|---|---|---|
| English text, 30,000 octets | 3.21 | 3.64 | 3.78 | 3.77 |
| Source code, 30,000 | 3.76 | 6.23 | 6.54 | 6.78 |
| Frames of PPP with binary in them, 30,000 | 1.09 | 1.09 | 1.14 | 1.23 |
| Random octets, 30,000 | 1.00 | 1.00 | 1.00 | 1.00 |
| One octet over and over, 20,000 | 20.77 | 20.77 | 20.77 | 20.77 |
| Every octet value in turn, 20,000 | 0.98 | 2.90 | 4.29 | 4.62 |

These are with strings of up to 32 characters, which is the factory value
of `+DS`; the test runs strings of 6, 20, 32 and 250 too, and the empty
input and the single octet. The default dictionary is 2048 codewords. The
random data is 30,134 octets out for 30,000 in, which is the cost of the
escape character in transparent mode; the one case under 1, every octet
in turn with the smallest dictionary, is 2% over, inside the bound.

On data that does not compress the compressor goes to transparent mode,
and the test holds the output to its bound.

## Tick times

A tick is one 10 ms frame through the engine. The soak has 24 calls for 60
seconds, eight of them between modems, four of those at 2400 bit/s with
LAPM and V.42bis; each figure is the greatest of its lines'.

| Run | Engines | Median | 99th percentile | Longest |
|---|---|---|---|---|
| Phase 3, the development machine | echo and null | 5 us | 25 us | 306 us |
| Phase 3, the development machine | modem at 300 bit/s | 17 us | 43 us | 272 us |
| Phase 4, the development machine | echo and null | 4 us | 35 us | 244 us |
| Phase 4, the development machine | modem at 300 bit/s | 15 us | 33 us | 130 us |
| Phase 4, the development machine | modem at 2400 bit/s, LAPM and V.42bis | 21 us | 37 us | 207 us |

The modem at 2400 bit/s with the whole stack over it takes about six
microseconds a tick more than the one at 300, and seventeen more than the
path's own, of ten thousand a tick has. A run beside a build on the same
machine gave 33, 75 and 236 us for the same row: the figures move with
what else the machine does more than the rows differ. The soak asserts
that the 99th percentile of every modem's ticks is under a millisecond and
prints the figures, so the CI log of any run has the runner's.

The work in one tick is bounded with the link up: the compressor is given
64 characters at the most, the decompressor is run 64 times on two octets
at the most, and LAPM does a bounded amount for each frame.
`modem/engine/tests/alloc.rs` counts the allocations of whole calls at
every rate with the link up and finds none.

## Markers

Every `BLOCKED` and `NEEDS-CODY` in the tree. Every `BLOCKED(phase4)` is
gone.

| Marker | Location | What is deferred |
|---|---|---|
| `BLOCKED(phase5)` | `service/media/src/os.rs` | From phase 2: RTP packets are not marked with a DSCP |
| `BLOCKED(phase5)` | `service/svc/src/service.rs`, `service/svc/src/eventlog.rs` | From phase 0: the service account and the event message file |
| `BLOCKED(phase7)` | `dsp/chansim/src/lib.rs` | The simulator has no group delay and no nonlinear distortion: V.34 needs them |
| `BLOCKED(phase8)` | `dsp/chansim/src/lib.rs` | The simulator has no robbed-bit signalling: V.90 needs it |
| `BLOCKED(phase9)` | `ui/manager/src/dialogs/connect.rs`, `service/ctlsrv/src/lib.rs` | From phases 0 and 1: another computer's service, and TLS for it |
| `NEEDS-CODY` | `docs/RRAS.md` (seven) | The wording of Windows' dialogs, the standard modem model, the authentication methods, the Courier's command for 2400 bit/s, and the log of the first call |
| `NEEDS-CODY` | `dte/at/tests/hostlogs/README.md`, `dte/at/tests/hostlogs/standard-modem.txt` | The real initialisation strings, in place of the one written from memory |
| `NEEDS-CODY` | `modem/engine/tests/corpus/README.md` | Captures of calls with real modems, in each of the modes |
| `NEEDS-CODY` | This record | `fallback_ms` as one setting; a Bell publication for Bell 212A; the manual checklist |
| `NEEDS-CODY` | `docs/GATEWAYS.md`, `docs/TELEPHONY.md`, `docs/RUNTIME.md` | From phase 2 and the retrofit |
| `NEEDS-CODY` | `service/svc/src/ports.rs`; `dte/serial/src/com0com.rs` (three), `docs/COM0COM.md` | From phases 0 and 1 |

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| Each work package is one commit | Ten, and an eleventh, `test(svc)`, after this record | The soak's race, below, was found from the CI run of the record's own commit |
| The Recommendation wins | The 2002 edition of V.42 is the one in `refs/itu/`, with Corrigendum 1 | It has no state tables and no re-establishment of a connection, both of which the handoff assumes. See "LAPM" |
| SREJ if time allows | Not built | Go-back-N held the property and the loss test. It is the first thing to add if real lines want it |
| A band-pass filter and gain control before the demodulator | The matched filter at baseband is the channel filter, and the gain is after it | A gain before the filter would be set by this end's own echo |
| The timing loop uses the resampler of phase 3 | It uses that interpolator's taps on complex samples | The resampler is for real samples at a ratio; the loop wants one complex sample at a phase |
| A caller with no V.8 is answered by the V.22bis sequence, "which also serves V.22 and Bell 212A callers, told apart by what they send back" | V.22bis is told from the 1200 bit/s modems by S1. V.22 and Bell 212A are one signal: each end names the connection by its own modem, and a line that has both says V.22 | Nothing sent tells them apart |
| The fallback order is V.22bis, then V.21, then Bell 103 | The order of the line's `modulations`, in which the three of the V.22 family are one step | The setting was already the order in phase 3 |
| Each step's duration is a setting | One setting, `fallback_ms`, is the time of every step | See "Decisions to confirm" |
| A burst of loss long enough forces a retrain | A burst is ridden out; a line that changes forces the retrain | See "Under loss" |
| `required` against a far end with none "clears with the right result code" | `NO CARRIER`, and RMD8001 in the log | V.250 has no result code for it |
| The corpus gains the engine calling itself in each new mode | Four captures of the engine answering itself: V.8 to V.22bis with LAPM and V.42bis, V.22 with LAPM, V.22bis with no error control, Bell 212A with no error control | As in phase 3, the bench that captures has the peer call the line. In the two with LAPM the line's host sends nothing, so that a replay sends every frame the call sent |
| Replay prints resets | It prints `link down` with the reason | The link has no reset: see "LAPM" |
| The zip has `rustmodem-gui.exe` | `rustmodem-manager.exe` | See "What needs Cody" |
| The zip has `LICENSE`, `README.md` and `docs/` | And `CHANGELOG.md` | `docs/INSTALL.md` sends an upgrade to it |
| A fixture format for a host's command log | The lines of the service's own log, pasted: `dte/at/tests/hostlogs/` | So that criterion 15 is a paste and not a transcription. The transcripts of phase 0 stay for what a log cannot say |
| The control API changes are additive | They are, and `ModemInfo.link` is behind a pointer in the Rust type | The event type had grown past what clippy allows in an enum; the wire is the same |
| `lines.diag` gains the constellation ring | As a flat list of numbers, two for each point | The documentation generator of `service/ctlproto` has no array of arrays |
| Goldens are re-blessed | The manager's commit went to a branch first; CI took the pictures; the branch was deleted | As in the earlier phases |
| Every commit on `main` is green | Two were red on the Linux job | `feat(v42): LAPM error control` and `docs: phase 4 record and release preparation`. The soak failed with "an octet arrived that was not the one sent", from the peer to the host, on a modem line at 300 bit/s the first time and on a null-engine line the second: on commits that touched neither. The hosts of the soak begin by sending `ATE0Q1`, and went on when every line was idle, throwing away what they had been sent, which is the echo of the command. A line is idle as soon as its port is open, and that can be before the port's thread has given the modem the command: a host that went on then was sent the echo afterwards and took it for the first of its call's data. The slow runner lost that race twice in some ten runs; no run here did. The hosts wait for the echo itself now. Phase 3 had the same message from another race, each end sending before the other had connected, which was mended then and is not this |
| `S9`-style registers `S36`, `S46`, `S48` | Built, with the values hosts send | They are in no Recommendation; `docs/AT.md` has the mapping |

## Tests whose expectations changed

The default modem is now all five modulations with error control `auto`
and compression on, and `+MS`, `+ES` and `+DS` have new ranges and factory
values. Tests that relied on the old defaults either name the 300 bit/s
modem or assert the new behaviour.

Named the 300 bit/s modem, and assert what they asserted:

- `service/pool/tests/modem.rs`: the harness `modems()` sets
  `modulations` to V.21 and Bell 103; the tests of the faster modem use
  `fast_modems()`.
- `service/svc/tests/peer.rs`, `transparency.rs` and `soak.rs`: the calls
  that are meant to be at 300 bit/s say so at the peer.

Assert something else, because what the service does changed:

- `service/config/tests/`: the default `modulations` is the five, and the
  new keys have their defaults.
- `dte/at/tests/`: `+MS` reads back `V22B,1,0,0,0,0`; `+ES` and `+DS`
  have their new ranges; transcripts 56 to 60 are new.
- `service/ctlsrv/tests/`, `service/pipe/tests/api.rs`,
  `tools/ctl/src/tests.rs`, `ui/model/tests/` and `ui/manager/tests/`:
  the modem's settings have five modulations and three more fields; the
  demo service's calls are at 2400 and 1200 bit/s with LAPM, so the
  diagnostics window shows a constellation where it showed the scope,
  and quality 90% where it showed 70%.
- `service/svc/src/profiles.rs`: the stored profile has `mr`, `er`, `dr`
  and `ilrr`.
- `modem/engine/tests/corpus/`: the three captures of phase 3 were made
  again, because the default order of the fallback changed what the line
  sends; `answer-bell103` connects sooner.
- `tools/sippeer`: a bare `media modem` is all five modulations.

## Decisions to confirm

Each of these is a choice the handoff left open or a place where the
build went another way. They are in the code and the documentation as
described; say if any should change.

**Configuration and AT**

- **The default `modulations` is all five.** A configuration of 0.0.5
  that names none gets the faster modem. The handoff's example has the
  five as the value of the key and does not say that it is the default.
- `fallback_ms` stays one setting for every step of the fallback.
  NEEDS-CODY: confirm, or say that each step is to have its own.
- A line's policy is the factory value of `+ES` and `+DS` on that line. A
  host that has set neither follows a change of the policy at once; one
  that has, at `&F`.
- `+DS` has the factory value `3,0,2048,32`: 2048 codewords and strings
  of 32, where V.250 suggests strings of 6 as the least. The range of the
  dictionary is 512 to 4096.
- `+ES` takes `(0-3),(0-3),(0-5)`: no alternative protocol and no
  synchronous access. An originator's request of 2 is LAPM with no
  detection phase.
- `\N5`, which asked for MNP alone, is error control required; `%C1`,
  which asked for MNP 5, is no compression. There is no MNP.
- `+ILRR` reports the line's rate: the port has none of its own.
- `B0` and `B1` choose between the ITU-T and the Bell modem at the rate
  of the carrier `+MS` has.
- A stored profile of 0.0.5 with a value now out of range is left out
  with RMD0026, not mended.
- RTS from the host is not acted on: a host that leaves it low would be
  given nothing. CTS to the host is.
- The guard tone is the answerer's, as V.22 has it, and off by default.

**The modem**

- An originator that may be the faster modem tries that first; with no
  answer to it in 1.5 seconds it becomes the Bell 103 caller if it may;
  answered by a modem with which it has no rate in common, it ends the
  call. A failure of V.8 after the menus agreed is final.
- An answerer listens for the caller of every modulation it has in every
  step of the fallback.
- Hanging up drops the carrier at once: no DISC is sent first, and the
  far end sees the carrier go.
- A break from the far end is counted and is not given to the port.
- An answerer with error control off is sent the originator's detection
  pattern as two characters for up to 750 ms, and gives them to its host:
  V.42 clause 7.2.1 has it so.
- `link/v14`, `link/v42` and `link/v42bis` are compiled with optimisation
  in the development profile, as the DSP crates are.

**Control API and manager**

- The receiver's figures are whole numbers in tenths of a decibel and
  parts in a million, as the levels are.
- The data link's stage and end, and the compression's mode, are words:
  `lapm`, `async`, `down`; `none`, `transparent`, `compressed`.
- The manager's error control list says "Auto: if the far end has it",
  "Required" and "Off".
- The engine has a tap on the link's frames for the replay tool, off
  unless asked for, and nothing in the service asks.

## References consulted

| Reference | For | How |
|---|---|---|
| ITU-T V.22 (11/1988) | `dsp/dspkit`, `modem/v22` | From `refs/itu/`: the scrambler, the spectrum, the guard tones, the 1200 bit/s handshake |
| ITU-T V.22bis (11/1988) | `dsp/dspkit`, `modem/v22`, `modem/engine` | From `refs/itu/`. Its figures are pictures: the constellation of Figure 2 and the handshakes of Figures 5 to 9 were read from the page images |
| ITU-T V.14 (03/93) with Corrigendum 1 (09/98) | `link/v14` | From `refs/itu/` |
| ITU-T V.42 (03/2002) with Corrigendum 1 (07/2003) | `link/v42`, `modem/engine`, `tools/replay` | From `refs/itu/`. Tables 6 to 8 and clause 12.2 for the frames |
| ITU-T V.42bis (01/90) | `link/v42bis`, `modem/engine` | From `refs/itu/` |
| ITU-T V.8 (11/2000), V.25 (10/96) with Corrigendum 1 | `modem/engine` | As in phase 3 |
| ITU-T V.250 (07/2003) with Supplement 1 | `dte/at` | From `refs/itu/`: `+ES`, `+DS`, `+MS`, `+MR`, `+ER`, `+DR`, `+ILRR`, `+ICF` |
| Gardner's timing error detector; the normalised LMS equaliser; the root raised cosine pulse and the Kaiser window; the error rates of QPSK and 16-QAM | `dsp/dspkit`, `modem/v22/tests/` | From memory of the textbooks |
| The Hayes commands `\N`, `%C`, `&Q` and the registers `S36`, `S46`, `S48` | `dte/at` | From memory of modem manuals, and the handoff. No manual is in `refs/` |
| Bell 212A's answer tone and timing | `modem/v22` | The handoff asks for "the published specification", and none is in `refs/`: the answer tone of 2225 Hz and the handshake are from memory of the Bell modems and from V.22, which Bell 212A shares its signal with. NEEDS-CODY: a Bell System publication to check it against, if one can be had |
| What the Windows standard modem driver and RRAS send, and Windows' dialogs | `dte/at/tests/`, `docs/RRAS.md` | From memory. Criterion 15 replaces memory with a log |

No GPL or LGPL project was opened for any crate: not VoIPModem,
libv90modem, spandsp, linmodem or any other. Nothing in `dsp/`, `modem/`
or `link/` was written from anything but the Recommendations and the
handoff.

## Dependencies

Phase 4 adds no crate from outside the workspace. `cargo deny check`
passes.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check`, `ci/headers.sh`, `ci/pure.sh` and all tests pass on Windows; every `dsp/*`, `modem/*` and `link/*` crate passes on Linux | Run locally on Windows and, the whole Linux job, under WSL before every commit: 1,244 tests on Windows at the end and 983 in the Linux job, and the 29 pictures of the manager. Two commits on `main` were red on CI: see "Where the build differs" |
| 2 | V.22 and V.22bis meet their bit-error-rate margins and carry data error-free across the stated ranges | `modem/v22/tests/modem.rs`; "Bit error rate" above |
| 3 | Every pairing of modulation capabilities reaches the right mode, and every error-control policy against every kind of peer gives the specified outcome | `modem/engine/tests/sync.rs`: 243 calls, every list of modulations at one end against every list at the other, each held to the mode the rules give; and the three policies against a far end with LAPM, with none and with it required, both ways round |
| 4 | LAPM's data-integrity property holds under arbitrary loss, duplication and corruption below the give-up threshold | The four property tests of `link/v42/tests/lapm.rs`; "LAPM" above |
| 5 | V.42bis round-trips the corpus at every supported parameter set, with the two dictionaries identical at every step | `link/v42bis/tests/v42bis.rs`; "Compression" above |
| 6 | With LAPM up and 1 percent packet loss for 10 minutes, zero data errors and the call holds | `modem/engine/tests/loss.rs`; "Under loss" above |
| 7 | `process` allocates nothing and panics on no input with the whole stack running; the five fuzz targets each run for 10 minutes with no findings | `modem/engine/tests/alloc.rs` and `anything.rs`, and the `alloc.rs` of `modem/v22`, `link/v14`, `link/v42` and `link/v42bis`. Fuzzed under WSL, 600 seconds each, clean: `v22_demod` 601,709 runs, `hdlc_deframe` 1,540,000, `lapm_frames` 496,000 after its finding was fixed, `v42bis_decode` 3,570,000, `v42bis_roundtrip` 1,590,000. CI runs each for 10 minutes on every push |
| 8 | The corpus replays to its expectations; the soak passes with tick times recorded | `tools/replay/tests/corpus.rs`, seven captures; `service/svc/tests/soak.rs`; "Tick times" above |
| 9 | Nothing in the tree is left marked `BLOCKED(phase4)` | `grep -r "BLOCKED(phase4)"` finds it only in the records of phases 1 to 3 and in this one |

### Manual checklist

`NEEDS-CODY`: these need the machine the service runs on, an elevated
prompt, the SPA112 set up as `docs/GATEWAYS.md` says with the US Robotics
Courier on its FXS port, a terminal program on the Courier's serial port,
and for criterion 14 a Windows Server with Routing and Remote Access and
a Windows client PC on the Courier. `10.10.20.11` stands for the gateway's
address; lines 1 and 2 are on com0com pairs as in phase 0's checklist.

The commands for the Courier are from memory of US Robotics modems and
were checked against no manual: `&F1` loads the hardware flow control
template; `&N3` fixes the link rate at 2400 bit/s and `&N2` at 1200;
`&M4` is error control if the far end has it, `&M5` error control or no
call, `&M0` none; `&K1` is compression where it helps and `&K0` none;
`B0` and `B1` choose the ITU-T and the Bell answer sequences; `ATI6`
prints the diagnostics of the last call and `ATO1` returns to data with a
retrain. **Look each up in the Courier's manual before relying on it**,
and write what was used into this checklist.

A configuration to start from is `docs/rustmodem.sample.toml`, with the
gateway's address in `trusted_peers` and `outbound_peer`, `CNCB0` and
`CNCB1` as the lines' ports, and `log_level = "debug"`.

**10. Before anything else: phase 3's manual criteria.**

- [ ] Criteria 9 to 15 of `docs/PHASE3.md` have been run and pass: above
      all a V.21 call from the Courier that carries a file with no error.
      For those, set `modulations = ["v21", "bell103"]` in `[modem]`, as
      that checklist's configuration has it. Do not debug V.22bis on a
      path where 300 bit/s is unproven.

**11. V.22bis with LAPM and V.42bis.**

```
rustmodem-svc --console C:\rustmodem\rustmodem.toml
rustmodem-ctl capture on 1
rustmodem-ctl watch
```

- [ ] On line 1's host: `ATS0=1`. On the Courier: `AT&F1&N3`, then
      `ATDT5551000`.
- [ ] Line 1's host says `CONNECT 2400`. The Courier says `CONNECT 2400`
      with its words for LAPM and V.42bis, and `ATI6` after the call
      names them.
- [ ] `rustmodem-ctl line 1` shows the call with `2400 bit/s`, `V.22bis`,
      `LAPM` and `V.42bis`.
- [ ] In the manager, View, Line diagnostics on line 1: the constellation
      is sixteen tight points on the sixteen marks; the signal-to-noise
      ratio is above 25 dB and steady; Link says `LAPM`, and its frames
      count up while data flows.
- [ ] A known file of 1 MB sent each way (ZMODEM in the terminal
      programs, or a plain capture of a text file) arrives with matching
      checksums: `certutil -hashfile <file> SHA256` at each end.
- [ ] The call holds for 30 minutes with data now and then, and
      `rustmodem-ctl line 1` shows no retrain, or few.
- [ ] `rustmodem-ctl capture check <file>` on the call's capture says
      clean.

**12. Fallbacks.** Each with `rustmodem-ctl capture on 1` first.

- [ ] V.22: on the Courier `AT&F1&N2`, dial. `CONNECT 1200` at both
      ends; `rustmodem-ctl line 1` shows `1200 bit/s`, `V.22` and `LAPM`;
      the constellation is four points. Text crosses both ways.
- [ ] Bell 212A: `AT&F1&N2B1`, dial. `CONNECT 1200`; the line says
      `V.22`, which is the same signal (see "Where the build differs");
      text crosses both ways.
- [ ] No error control: `AT&F1&N3&M0&K0`, dial. `CONNECT 2400`;
      `rustmodem-ctl line 1` shows no protocol; Link in the diagnostics
      says start-stop characters with no error control; text crosses
      both ways.
- [ ] Required against none: on line 1's host `AT+ES=3,2,4` (or set
      `error_control = "required"` on the line and restart nothing: it
      takes effect at the next call), and on the Courier
      `AT&F1&N3&M0`, dial. The modems train and the call clears: line
      1's host has `NO CARRIER` and no `CONNECT`, and `rustmodem-ctl log`
      shows RMD8001. Then `AT&F` on line 1's host.

**13. Retrain.**

- [ ] During a call as in criterion 11, on the Courier: `+++`, wait for
      `OK`, then `ATO1`. The data stops for a second or two and goes on;
      Retrains in the diagnostics and Retr. in the grid count one.
- [ ] During a call, disturb the line briefly: lift and replace a
      telephone on the same line, if the SPA112's port can be bridged
      so, or pull the Courier's line cord for under a second. The call
      survives, by a retrain or without needing one, and a file transfer
      in progress finishes with a matching checksum.

**14. RRAS end to end.**

- [ ] Follow `docs/RRAS.md` from "Before you start" to "A client". Mark
      each `NEEDS-CODY: confirm wording` there with what the dialog
      says.
- [ ] The client dials the pilot number; RRAS answers through
      rustmodem; the client authenticates and is given an address.
- [ ] From the client: `ping <the server's address in the pool>`.
- [ ] A file of 1 MB copied each way over the connection (`copy` to and
      from a share on the server) has matching checksums.
- [ ] The session holds for 30 minutes.

**15. Initialisation strings.**

- [ ] With `log_level = "debug"`, after criterion 14:
      `rustmodem-ctl log --last 500 > rras.log`. Copy the lines of line
      1 from the first `> AT` to the `< CONNECT` into
      `dte/at/tests/hostlogs/rras-<windows version>.txt`, as
      `dte/at/tests/hostlogs/README.md` says, and run
      `cargo test -p at --test hostlogs`.
- [ ] None of the commands was answered `ERROR`: the test fails on one,
      and names it.

**16. Captures.**

- [ ] The capture of each call of criteria 11 and 12 is added to
      `modem/engine/tests/corpus/` as `courier-<what>.rmcap`, with

      ```
      cargo run -p replay -- modem/engine/tests/corpus/courier-<what>.rmcap > modem/engine/tests/corpus/courier-<what>.expect
      ```

      and a `courier-<what>.md`. Add `--frames` to see the Courier's
      XID and what it asked for.
- [ ] A capture of a call with LAPM in which line 1's host sent data
      replays only as far as that data: `docs/CAPTURE.md` says why. For
      the corpus, capture a call in which only the Courier sends.

**17. A second gateway.**

- [ ] Criterion 11 again with the Courier on the MP-124 or the VG310.

**18. Tag.**

- [ ] With the above passed: `git tag v0.1.0` and `git push origin
      v0.1.0`. The release workflow runs the tests, builds, and attaches
      the zip to a release.
- [ ] On a second machine, install from the zip as `docs/INSTALL.md`
      says, and run its "Check".

## Known limits of phase 4

- The modem has met no modem but itself, the AT interpreter no Windows
  modem driver, and the service no RRAS.
- 2400 bit/s is the fastest. V.32 and above are phases 6 to 8.
- Recovery from loss is go-back-N: no selective reject.
- No MNP, no V.44, no 32-bit check sequence, no synchronous modes for the
  host, no XON/XOFF.
- A retrain keeps the rate: no rate change by clause 6.6 of V.22bis.
- A capture of a call with LAPM replays only until the line's host sent
  data.
- The corpus has no capture of a call the line made.
- com0com is still needed: the virtual COM driver and the installer are
  phase 5.
- In the demo service nothing is modulated: its calls, and what its
  diagnostics show, are scripted.
