# The modem

rustmodem's modem is in-house, written from the ITU-T Recommendations. This
phase's is the 300 bit/s modem: V.21, Bell 103, and V.8 to agree between
them. It runs on the media thread of each call, one frame of 80 samples in
and one out, and is the engine of every line whose `engine` is `modem`,
which is the default.

| Crate | What it is | Written from |
|---|---|---|
| `dsp/g711` | The u-law and A-law codecs, and the conversions between them | G.711 |
| `dsp/dspkit` | Oscillators, filters, detectors, level control, resampling | |
| `dsp/tones` | Answer tone, ANSam and calling tone, and their detectors | V.25, V.8 |
| `dsp/chansim` | A telephone channel with its faults, for tests | |
| `modem/fsk` | The V.21 and Bell 103 modulator, demodulator and start-stop framing | V.21 |
| `modem/v8` | The V.8 signals and the two ends of the procedure | V.8 |
| `modem/engine` | The modem: the call from answer to carrier lost | V.25, V.8, V.21 |

Every sample is an `f32` at 8000 Hz, with ±1.0 the overload point of G.711.
Nothing in these crates reads a clock or a random number it was not given,
so a call given the same samples does the same thing on every machine.

## Settings

The `[modem]` table of the configuration file has the settings of every
line, and a line's own `modem` table has that line's where they differ.

```toml
[modem]
modulations = ["v21", "bell103"]
v8 = true
tx_level_dbm0 = -12
answer_delay_ms = 0
fallback_ms = 3000

[[line]]
id = 3
serial = { backend = "com0com", port = "CNCB2" }
modem = { modulations = ["bell103"], v8 = false }
```

| Key | Default | Range | What it is |
|---|---|---|---|
| `modulations` | `["v21", "bell103"]` | `v21`, `bell103` | The modulations the modem may use. The first is the one it prefers; answering without V.8, it offers them in this order |
| `v8` | `true` | | Whether the modem uses V.8. Bell 103 is not in V.8, so a line with only `bell103` never uses it |
| `tx_level_dbm0` | `-12` | -20 to -9 | The level the modem sends at |
| `answer_delay_ms` | `0` | 0 to 10000 | Silence before the 2150 ms of silence an answering modem begins with (V.25 clause 4.3) |
| `fallback_ms` | `3000` | 500 to 20000 | How long an answering modem offers each modulation before the next. `[modem]` only |

A line's `modem` table takes `modulations`, `v8` and `tx_level_dbm0`.

The host changes what a call uses, within what the line's configuration
allows, with `AT+MS`, `ATB0` and `ATB1`, and how long the carrier is waited
for and missed with `S9` and `S10`; `docs/AT.md` has them. A carrier the
line's configuration does not allow is not the host's to choose: the
configuration's list is used as it is.

A change to the settings takes effect at the next call.

## A call

An answering modem:

1. is silent for 2150 ms (V.25 clause 4.3), after `answer_delay_ms`;
2. sends answer tone at 2100 Hz with its phase reversed every 450 ms: with
   V.8, ANSam, which is that tone modulated at 15 Hz, for up to 5 s while
   it listens for the caller's CM (V.8 clause 8.2); without, ANS for 3.3 s
   (V.25 clause 4.4);
3. with V.8, on two identical CMs, sends JM until the caller's CJ, is
   silent for 75 ms, and sends the carrier of the modulation the two have
   in common;
4. without V.8, or when no CM came, is silent for 75 ms and offers each of
   its modulations in turn, sending its carrier for `fallback_ms` and
   listening for the caller's carrier of any modulation it may use;
5. connects when the caller's carrier has been there for the time in `S9`.

An originating modem listens. On ANSam, and with V.8, it is silent for 1 s
(V.8 clause 8.1.1), sends CM until two identical JMs, sends CJ, is silent
for 75 ms and sends its carrier. On anything else it waits for the answer
tone to end, tells the answerer's modulation by its carrier, and answers
with its own; if no carrier it knows comes within a second it sends the
carrier of the first modulation it may use (V.25 clause 3.10).

Connected, the modem carries the host's characters as start-stop
characters of eight data bits, no parity and one stop bit, the least
significant bit first. The carrier lost for the time in `S10` ends the
call. Formats other than 8N1 are not in this phase.

A modem that gives up before it connects is logged with RMD7001, which
names the state it was in.

## States

The control API, the manager and RMD7001 name the modem's state:

| State | The modem is |
|---|---|
| `idle` | Stopped |
| `answer-silence` | Answering: silent before the answer tone |
| `answer-tone` | Answering: sending answer tone, and with V.8 listening for CM |
| `v8-jm` | Answering with V.8: CM received, sending JM |
| `gap` | Silent between the answer tone or the V.8 signals and what follows |
| `fallback` | Answering without V.8, or after it came to nothing: sending a carrier and listening for the caller's |
| `listening` | Originating: listening for answer tone or a carrier |
| `v8-te` | Originating with V.8: ANSam detected, silent before CM |
| `v8-cm` | Originating with V.8: sending CM, listening for JM |
| `v8-cj` | Originating with V.8: JM received, sending CJ |
| `training` | Sending the chosen modulation's carrier and waiting for the far end's to have been there long enough |
| `data` | Connected |
| `ended` | The attempt failed, or the carrier was lost |

## The frequencies

| Modulation | Caller sends: mark, space | Answerer sends: mark, space |
|---|---|---|
| V.21 (clause 3) | 980 Hz, 1180 Hz | 1650 Hz, 1850 Hz |
| Bell 103 | 1270 Hz, 1070 Hz | 2225 Hz, 2025 Hz |

Mark is binary 1 and the idle line. The line signal detector turns on above
-43 dBm0 and off below -48 dBm0 (V.21 clause 8.3).

## On the tick

`ModemEngine::process` is called by the call's media thread for every 10 ms
of the call. It allocates nothing, takes no lock, logs nothing, makes no
system call and cannot panic: everything a call needs is made when the call
is answered or begun. A test counts the allocations of a whole call and
finds none, and the soak test holds the 99th percentile of the time a tick
takes under a millisecond.

Ten times a second the media thread takes the engine's snapshot of itself,
its state, the levels received and sent, the carrier, its frequency error,
the quality of the received signal, framing errors, its last changes of
state and a short trace of the demodulator's output, and leaves it where
the pool can read it without waiting.

## Replaying a call

`rustmodem-replay` gives the frames of a captured call to a new engine and
prints what it does; `docs/CAPTURE.md` has it.
