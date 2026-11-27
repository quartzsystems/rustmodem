# The modem

rustmodem's modem is in-house, written from the ITU-T Recommendations. It
is the modem at 2400 bit/s, V.22bis, with the modems it falls back to: V.22
and Bell 212A at 1200 bit/s, and V.21 and Bell 103 at 300 bit/s. V.8 agrees
between them. At 1200 and 2400 bit/s it has the error control of V.42
(LAPM) and the data compression of V.42bis. It runs on the media thread of
each call, one frame of 80 samples in and one out, and is the engine of
every line whose `engine` is `modem`, which is the default.

| Crate | What it is | Written from |
|---|---|---|
| `dsp/g711` | The u-law and A-law codecs, and the conversions between them | G.711 |
| `dsp/dspkit` | Oscillators, filters, detectors, level control, resampling; the scrambler, pulse shaping, timing and carrier recovery and the equaliser | |
| `dsp/tones` | Answer tone, ANSam and calling tone, and their detectors | V.25, V.8 |
| `dsp/chansim` | A telephone channel with its faults, for tests | |
| `modem/fsk` | The V.21 and Bell 103 modulator, demodulator and start-stop framing | V.21 |
| `modem/v22` | The V.22, Bell 212A and V.22bis modulator and demodulator, the handshake, the fall back to 1200 bit/s and the retrain | V.22, V.22bis |
| `modem/v8` | The V.8 signals and the two ends of the procedure | V.8 |
| `link/v14` | Start-stop characters over a synchronous line | V.14 |
| `link/v42` | The detection phase, HDLC framing and LAPM | V.42 |
| `link/v42bis` | Data compression | V.42bis |
| `modem/engine` | The modem: the call from answer to carrier lost, and the data link over a synchronous modem | V.25, V.8, V.21, V.22bis, V.42 |

Every sample is an `f32` at 8000 Hz, with ±1.0 the overload point of G.711.
Nothing in these crates reads a clock or a random number it was not given,
so a call given the same samples does the same thing on every machine.

The editions are V.21, V.22 and V.22bis of 11/1988, V.14 of 03/93 with its
Corrigendum 1 of 09/98, V.42 of 03/2002 with its Corrigendum 1 of 07/2003,
V.42bis of 01/90, V.8 of 11/2000 and V.25 of 10/96 with its Corrigendum 1
of 07/2001.

## Settings

The `[modem]` table of the configuration file has the settings of every
line, and a line's own `modem` table has that line's where they differ.

```toml
[modem]
modulations = ["v22bis", "v22", "bell212a", "v21", "bell103"]
v8 = true
tx_level_dbm0 = -12
answer_delay_ms = 0
fallback_ms = 3000
error_control = "auto"
compression = true
guard_tone = "off"

[[line]]
id = 3
serial = { backend = "com0com", port = "CNCB2" }
modem = { modulations = ["v22", "bell103"], v8 = false, error_control = "required" }
```

| Key | Default | Range | What it is |
|---|---|---|---|
| `modulations` | all five | `v22bis`, `v22`, `bell212a`, `v21`, `bell103` | The modulations the modem may use. Answering a caller that has no V.8, it tries them in this order; `v22bis`, `v22` and `bell212a` are one kind of modem and are tried together, where the first of them is |
| `v8` | `true` | | Whether the modem uses V.8. The Bell modems are not in V.8, so a line with only those never uses it |
| `tx_level_dbm0` | `-12` | -20 to -9 | The level the modem sends at |
| `answer_delay_ms` | `0` | 0 to 10000 | Silence before the 2150 ms of silence an answering modem begins with (V.25 clause 4.3) |
| `fallback_ms` | `3000` | 500 to 20000 | How long an answering modem gives each kind of modem before the next. `[modem]` only |
| `error_control` | `"auto"` | `auto`, `required`, `off` | At 1200 and 2400 bit/s: LAPM if the far end has it; LAPM or the call is cleared; none |
| `compression` | `true` | | Whether V.42bis is asked for and agreed to, where there is LAPM |
| `guard_tone` | `"off"` | `off`, `1800`, `550` | The guard tone an answering modem sends with its data at 1200 and 2400 bit/s (V.22bis clause 2.2), for the networks whose signalling a 2400 Hz carrier alone would disturb |

A line's `modem` table takes `modulations`, `v8`, `tx_level_dbm0`,
`error_control`, `compression` and `guard_tone`.

The host changes what a call uses, within what the line's configuration
allows, with `AT+MS`, `ATB0` and `ATB1`, and how long the carrier is waited
for and missed with `S9` and `S10`. A carrier the line's configuration
does not allow is not the host's to choose: the configuration's list is
used as it is. The line's `error_control` and `compression` are the
factory values of `AT+ES` and `AT+DS` on that line, and the host's
commands, those and the older `\N`, `%C` and `&Q`, change them for its
calls. `docs/AT.md` has them all.

A change to the settings takes effect at the next call.

## A call

An answering modem:

1. is silent for 2150 ms (V.25 clause 4.3), after `answer_delay_ms`;
2. sends answer tone at 2100 Hz with its phase reversed every 450 ms: with
   V.8, ANSam, which is that tone modulated at 15 Hz (V.8 clause 7.2), for
   up to 5 s while it listens for the caller's CM (clause 8.2.2); without,
   ANS for 3.3 s (V.25 clause 2.2);
3. with V.8, on two identical CMs, sends JM until the caller's CJ (clause
   8.2.3), is silent for 75 ms, and begins the modulation the two have in
   common: the highest, V.22bis and V.22 before V.21;
4. without V.8, or when no CM came, is silent for 75 ms and takes each kind
   of modem it may be in turn, for `fallback_ms` each. For V.22bis, V.22
   and Bell 212A it sends unscrambled binary 1 at 1200 bit/s (V.22bis
   clause 6.3.1.1.2), or the 2225 Hz answering tone if Bell 212A is the
   only one of them it may be. For V.21 and for Bell 103 it sends the
   answer carrier. In every step it listens for the calling carrier of any
   300 bit/s modem it may be, and for the S1 or the scrambled binary 1 of
   a calling modem at 1200 or 2400 bit/s;
5. at 300 bit/s, connects when the caller's carrier has been there for the
   time in `S9`;
6. at 1200 and 2400 bit/s, runs the handshake: a caller that sends S1 is
   V.22bis and the call is at 2400 bit/s; one that sends scrambled binary
   1 without it is at 1200 bit/s. V.22 and Bell 212A send the same signal
   and cannot be told apart: the connection is called V.22 if the line may
   be V.22, and Bell 212A if not. Then the data link is made, and the
   modem connects when it is up.

An originating modem listens. On ANSam, and with V.8, it is silent for 1 s
(V.8 clause 8.1.1), sends CM until two identical JMs, sends CJ, is silent
for 75 ms and begins the modulation agreed (clause 8.1.2). Without:

- on unscrambled binary 1 in the high channel, or the 2225 Hz tone, a
  modem that may be V.22bis, V.22 or Bell 212A is the calling modem of
  V.22bis clause 6.3.1.1.1;
- on the answer carrier of V.21, a modem that may be V.21 sends its
  calling carrier;
- on the answer carrier of Bell 103, which at mark is the same 2225 Hz, a
  modem that may be Bell 103 and none of the faster ones sends its calling
  carrier. One that may be a faster one too tries that first, and if
  nothing answers its part of the handshake in a second and a half it is
  Bell 103. An answerer that does answer and has no rate in common with it
  ends the call;
- a modem that may be 300 bit/s modems only, and hears no carrier it knows
  within a second of the end of the answer tone, sends the calling carrier
  of the first of them (V.25 clause 3.10).

V.8 decides where both ends use it: two modems whose menus have nothing in
common do not connect, though both may be Bell modems, which are not in
it.

At 300 bit/s the modem carries the host's characters as start-stop
characters, the least significant bit first: of eight data bits, no
parity and one stop bit, or of seven data bits with even or odd parity,
as `AT+ICF` says (`docs/AT.md`). The carrier lost for the time in `S10`
ends the call, at any rate.

A modem that gives up before it connects is logged with RMD7001, which
names the state it was in.

## The data link

At 1200 and 2400 bit/s the line is synchronous: a bit every bit time,
scrambled, whether the host has anything to send or not. What the bits
carry is agreed when the modems have connected.

1. **Detection** (V.42 clause 7.2.1). The originator sends a pattern that a
   modem without error control passes to its host as two harmless
   characters, DC1 with its parity even and then odd. An answerer that
   has error control replies with its own pattern. If the reply has not
   come in 750 ms the answerer has none.
2. **LAPM** (V.42 clause 8), if both have it. The originator sends its
   parameters in an XID frame and the answerer replies: the window, the
   longest frame, and whether V.42bis compresses each direction and with
   what dictionary. Then SABME and UA make the connection. The host's data
   goes in numbered frames of up to 128 octets, each with a 16-bit check
   sequence; a frame that fails it is not acknowledged and is sent again,
   with every frame after it. The window is 15 frames.
3. **V.42bis**, if both agreed: the data in LAPM's frames is compressed,
   each direction by itself, with a dictionary of 2048 codewords and
   strings of up to 32 characters unless `AT+DS` says otherwise. Data that
   does not compress is sent as it is. What the compressor holds is sent
   when the host stops: nothing waits for more data.
4. **Start-stop characters** (V.14), if the far end has no error control,
   or the line's policy has none: the host's characters with a start and
   a stop element, in the format of `AT+ICF`, with nothing to protect
   them.

The modem connects, and the host has `CONNECT`, when the link is up. With
`error_control = "required"` and a far end that has none, the call is
cleared and the host has `NO CARRIER`.

Flow control is end to end. A host that does not read fills the modem's
queue, the modem tells the far end it is busy (RNR), the far end's window
closes, and its host is held off; a host that writes faster than the line
carries is held off with CTS (`AT&K3`). Nothing is dropped.

A connection that ends, because the far end released it, stopped
answering for ten polls, or sent what the protocol does not allow, ends
the call. There is no second connection on a call: the edition of V.42
this is written from has none.

What is not built: selective reject (SREJ), the 32-bit check sequence, the
test frame and the alternative protocol of Annex A (MNP). A caller that
offers only MNP gets no error control.

### Retrains

At 2400 bit/s a modem whose received signal has gone bad, its error over
13 dB under the signal for a second, asks for a retrain (V.22bis clause
6.4), and answers the far end's. It takes about a second. The host is
told nothing: its data stops and goes on, and with LAPM none is lost. The
service counts it. A burst of lost packets does not need one: the receiver
holds what it has learned through the gap and finds the signal where it
left it. A line that comes back changed does.

At 1200 bit/s there is no retrain; the receiver finds the signal again by
itself.

## States

The control API, the manager and RMD7001 name the modem's state:

| State | The modem is |
|---|---|
| `idle` | Stopped |
| `answer-silence` | Answering: silent before the answer tone |
| `answer-tone` | Answering: sending answer tone, and with V.8 listening for CM |
| `v8-jm` | Answering with V.8: CM received, sending JM |
| `gap` | Silent between the answer tone or the V.8 signals and what follows |
| `fallback` | Answering without V.8, or after it came to nothing: sending a carrier or the answer sequence of V.22bis and listening for the caller |
| `listening` | Originating: listening for answer tone or a carrier |
| `v8-te` | Originating with V.8: ANSam detected, silent before CM |
| `v8-cm` | Originating with V.8: sending CM, listening for JM |
| `v8-cj` | Originating with V.8: JM received, sending CJ |
| `training` | At 300 bit/s, sending the carrier and waiting for the far end's to have been there long enough; at 1200 and 2400 bit/s, in the handshake |
| `link` | Connected at 1200 or 2400 bit/s and making the data link: detection, and the set-up of LAPM |
| `data` | Connected |
| `retrain` | Retraining: no data passes until it is over |
| `ended` | The attempt failed, or the carrier was lost |

## The frequencies

| Modulation | Caller sends | Answerer sends |
|---|---|---|
| V.21 | mark 980 Hz, space 1180 Hz | mark 1650 Hz, space 1850 Hz |
| Bell 103 | mark 1270 Hz, space 1070 Hz | mark 2225 Hz, space 2025 Hz |
| V.22, Bell 212A, V.22bis | a carrier of 1200 Hz at 600 baud | a carrier of 2400 Hz at 600 baud, and the guard tone if there is one |

Mark is binary 1 and the idle line. The line signal detector turns on above
-43 dBm0 and off below -48 dBm0 (V.21 clause 8.3, V.22bis clause 3.3).

At 600 baud a symbol of 1200 bit/s is one of four phase changes and
carries two bits; one of 2400 bit/s is one of sixteen points and carries
four (V.22bis clause 2.5, Figure 2). The spectrum is the square root of a
raised cosine with a roll-off of 75%.

## On the tick

`ModemEngine::process` is called by the call's media thread for every 10 ms
of the call. It allocates nothing, takes no lock, logs nothing, makes no
system call and cannot panic: everything a call needs is made when the call
is answered or begun, the data link and the two dictionaries of V.42bis
with it. A test counts the allocations of whole calls, at every rate and
with the link up, and finds none, and the soak test holds the 99th
percentile of the time a tick takes under a millisecond.

Ten times a second the media thread takes the engine's snapshot of itself
and leaves it where the pool can read it without waiting: its state, the
levels received and sent, the carrier, its frequency error, the quality of
the received signal, framing errors, retrains and its last changes of
state; at 300 bit/s a short trace of the demodulator's output; at 1200 and
2400 bit/s the last 64 received points of the constellation, the
signal-to-noise ratio, the equaliser's error and the timing offset, and
what the data link counts: its stage, frames each way, frames sent again,
frames that failed their check, the window, and the octets into and out of
the compressor each way.

## Replaying a call

`rustmodem-replay` gives the frames of a captured call to a new engine and
prints what it does: its states, and for a call at 1200 or 2400 bit/s what
the data link did, with what the XID frames agreed and, if asked, every
frame of LAPM both ways. `docs/CAPTURE.md` has it. The engine keeps the
frames for it in a tap that is off in a call of the service.

## Seeing and trying the modem

`rustmodem-ctl diag <line>` prints the modem of a line's call. The manager
shows the same, live, in View, Line diagnostics.

`rustmodem-ctl selftest <a> <b>`, and Tools, Loopback self-test in the
manager, join the modems of two idle lines with nothing between them: the
first calls the second, and each sends the other 64 KiB. The test says
whether the modems of this service work with the settings of those two
lines; the network and the hosts have no part in it. It takes some
seconds, and the two lines are busy meanwhile.

## What is not here

No echo canceller: these modems send and receive in separate bands and
need none. No V.32 and above. No V.23, no fax, no DTMF. No MNP. No
synchronous modes for the host. No XON/XOFF flow control.
