# Call capture

A call capture is every RTP packet a call received and sent, each with the
time it passed through the call's media thread. It is the evidence for the
question a modem call lives or dies by: did the line send exactly one sample
for every sample it took, for the whole call?

Captures are for diagnosis. They hold the audio of the call, which for a
modem call is the data of the call: treat them as you would the data.

## Taking a capture

Capture is on or off for a line. It is off when the service starts, and is
not kept in the configuration.

```
rustmodemctl capture on 3
rustmodemctl capture off 3
```

While it is on, each call on the line is written to a file of its own in
the capture directory: `media.capture_dir`, or `captures` under the log
directory when that is empty. The file is named for the line and the time
the capture began, in milliseconds since the Unix epoch:

```
line03-1800000000123.rmcap
```

The service logs RMD6006 when it begins a file and RMD6007 if the file
cannot be made or written. A capture that fails is given up; the call is not
affected. Turning capture on during a call captures the rest of that call.

A call at 20 ms packets writes about 1 MB a minute.

### What capture costs the call

Nothing that can be heard. The media thread never touches a file. It puts
each packet in a ring of 2,048 records, without waiting and without
allocating, and one writer thread for the whole service empties the rings
into the files every 20 ms. If a ring is full, because the disk has stalled,
the record is dropped and counted and the tick goes on. The hole is marked
in the file, so a capture with holes says so and is never mistaken for a
call with loss.

## Checking a capture

```
rustmodemctl capture check line03-1800000000123.rmcap
```

```
Capture:          line03-1800000000123.rmcap
Call:             3b1c9e@10.10.20.11 on line 3
Codec:            PCMU, 20 ms packets
Started:          2027-01-15 08:00:00 UTC
Received:         30000 packets
Sent:             30000 packets
Lost:             0
Late:             0
Duplicate:        0
Resync:           0
Ignored:          0
Largest gap:      21.3 ms between arrivals
Free run:         0 frames before the far end began
Slips:            0
Dropped records:  0
Result:           clean: nothing lost, nothing resynchronised, nothing slipped
```

The command reads the file and asks nothing of the service, so a capture
can be checked on another machine. With `--json` it prints the same counts
as one JSON object.

| Line | Meaning |
|---|---|
| Received | Packets from the far end that were delivered to the modem |
| Sent | Packets sent to the far end |
| Lost | Packets from the far end that never came in time. Each was replaced by fill frames, so the modem heard silence in its place |
| Late | Packets that came after their place had been filled. They were dropped |
| Duplicate | Packets that had been delivered already. They were dropped |
| Resync | Times the far end's stream started again: a new source identifier, or a jump of more than three seconds |
| Ignored | Packets of another payload type, or whose payload was not a whole number of 10 ms frames |
| Largest gap | The longest time between two arrivals from the far end. Much more than the packet time means the network, or the far end, held packets back |
| Free run | Frames sent on the local timer before the far end's first packet arrived |
| Slips | Points where the frames sent stopped matching the frames taken |
| Dropped records | Records the capture itself is missing |

The counts of the received stream are made by running the captured packets
through the same receiver the media thread used, so they are the counts the
thread made during the call.

**Slips** are the line's own fault, where the others are the network's. The
service has no jitter buffer and no clock of its own during a call: each
packet that arrives is so many frames, with fill frames for what was lost
before it, and for each of those frames the modem is run once and one frame
is sent. So between any two arrivals the frames sent must be the frames
taken, give or take the frames that are waiting to fill a packet. A slip is
a point where that does not hold: a frame was sent that no received frame
accounts for, or a received frame was never answered with one. A modem
loses synchronisation at a slip. There should never be one, whatever the
network does.

The exit code is 0 only when Lost, Resync, Slips and Dropped records are
all zero, and 1 otherwise, so the command can be the pass or fail of a test.
A capture with dropped records is not clean because what is counted around
a hole cannot be trusted. A file that cannot be read is RMD6003, and one
that is not a capture, or is cut off or damaged, is RMD6004; both exit
with 1.

## Listening to a capture

```
rustmodemctl capture wav line03-1800000000123.rmcap call.wav
```

This writes a two-channel WAV file at 8,000 samples a second, in the
call's own G.711 (format tag 7 for mu-law, 6 for A-law): nothing is
decoded, so the octets in the file are the octets of the call. What was
received is on the left and what was sent is on the right. Lost packets are
filled with silence, so the two channels stay aligned: a frame on the right
is the frame that was sent for the frame beside it on the left. Any audio
editor that reads G.711 WAV files opens it.

## Replaying a capture

A capture has every frame the call's modem was given. `rustmodem-replay`
gives them to a new modem engine, one at a time as the call's media thread
did, and prints what the engine does:

```
rustmodem-replay line03-1800000000123.rmcap
```

```
role answer
law ulaw
frames 1054
10 state answer-silence
2150 state answer-tone
4190 state v8-jm
4790 state gap
4870 state training
5490 state data
5490 connect 300 V.21
data 60 68549a0fb96d1595
framing-errors 0
sent-as-captured 554 of 1054
```

Each line with a time is something the engine did, at that many
milliseconds of the call's own samples: a state it entered
(`docs/MODEM.md` lists them), or `connect` with the rate, the carrier and
where there is any the error control and the compression, `carrier-lost`
or `retrain`. `data` is how many characters the engine received and a hash
of them, which is the same when the same characters were received.

A call at 1200 or 2400 bit/s has a data link, and the replay says what it
did:

```
6400 link detection
6400 state link
6720 link setup
6910 xid rx command n401 128/128 k 15/15 compression both codewords 2048 string 32
6910 xid tx response n401 128/128 k 15/15 compression both codewords 2048 string 32
7170 link lapm window 15 compression both
7170 state data
7170 connect 2400 V.22bis LAPM V.42bis
```

| Line | What it says |
|---|---|
| `link detection` | The detection phase of V.42 began |
| `link setup` | Both ends have error control: flags, XID and SABME follow |
| `link lapm window <k> compression <ways>` | The error-corrected connection is up, with the window this end sends in and V.42bis `both` ways, `out`, `in` or `none` |
| `link async` | No error control: start-stop characters |
| `link down <why>` | The link ended: `required`, `no-protocol`, `no-response`, `refused`, `remote`, `local`, `timed-out`, `protocol` or `compression`. The link has no re-establishment, so a fault that would reset one ends it, with `protocol` |
| `xid tx` or `rx`, `command` or `response` | An XID frame: the longest information field and the window, each as initiator/responder, and the compression asked for or agreed |
| `retransmissions <n>` | I frames sent again in the call so far, when the count changes |
| `frame-errors <n>` | Frames received that failed their check, when the count changes |

`--frames` adds every frame of LAPM the link sent and received, each where
it came:

```
7160 frame rx SABME command p=1
7160 frame tx UA response f=1
7170 link lapm window 15 compression both
7500 frame rx I command ns=0 nr=0 p=0 len=61
7500 frame tx RR response nr=1 f=0
```

A frame is its kind, whether it is a command or a response, N(S) and N(R)
where it has them, its poll or final bit, and the length of its
information field.

| Option | What it does |
|---|---|
| `--answer` | The engine answered the call. This is the default |
| `--originate` | The engine made the call |
| `--law ulaw` or `alaw` | The law of the call, when it is not the capture's |
| `--modulations <list>` | The modulations the modem may use, the first by choice: `v22bis`, `v22`, `bell212a`, `v21` and `bell103`, with commas between. All of them by default |
| `--no-v8` | The modem does not use V.8 |
| `--error-control auto`, `required` or `off` | The modem's error control. `auto` by default |
| `--no-compression` | The modem does not ask for V.42bis |
| `--frames` | Prints the frames of LAPM too. An expectation is compared without them |
| `--expect <file>` | Compares what the engine does with the file, which has what the program prints. Lines that begin with `#` are skipped. The program exits with 1 if they differ, and says where |
| `--tx-out <file.wav>` | Writes what the engine sends in the replay as sound |

A capture does not say which end the line was, or what its modem's settings
were: say them as they were in the call.

### What a replay is not

**A replay is exact only until the engine's transmission changes.** The far
end of the captured call answered what the engine of that day sent. Once
the engine of today sends something else, a different signal or the same
signal at another time, the frames that follow in the capture are the far
end's answer to a call that is no longer being made. The replay goes on,
and what it shows from there is how the engine takes those frames, which is
worth seeing and is not what a call would be.

`sent-as-captured` says where that point is: for how many frames from the
start the engine sent exactly what the capture says was sent. The data the
line's host sent is not in a capture, so a replayed engine sends none, and
in a capture of a call that carried data from the host the count stops
where that data began. Without error control, what was received does not
depend on it. With error control it does: the far end acknowledges the
frames the host's data went in, the replayed engine never sent them, and
to LAPM an acknowledgement of a frame that was not sent ends the
connection. Such a call replays as it went as far as the host's first
octet, and `link down protocol` is where it stops being the call.

The program is a developer's tool, built with `cargo build -p replay`, and
is not installed with the service.

### The corpus

`modem/engine/tests/corpus/` has captures the engine is replayed against in
the tests, each with the file of what is expected of it and a note of
where it came from. A capture of a call that went wrong, once the fault is
understood, belongs there.

## The file format

The extension is `.rmcap`. All numbers are little-endian. The file is a
header followed by records until the end of the file.

### Header

| Offset | Size | Field |
|---|---|---|
| 0 | 6 | Magic: `RMCAP` and a zero octet |
| 6 | 2 | Version: 1 |
| 8 | 1 | Codec: the RTP payload type of the call, 0 for PCMU or 8 for PCMA |
| 9 | 1 | Packet time in milliseconds |
| 10 | 2 | Line id |
| 12 | 8 | Start time, in milliseconds since the Unix epoch |
| 20 | 2 | Length of the call id in octets, at most 255 |
| 22 | n | The SIP Call-ID of the call, UTF-8 |

### Record

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | Kind: 0 received, 1 sent, 2 gap |
| 1 | 1 | The RTP marker bit: 0 or 1 |
| 2 | 1 | The RTP payload type |
| 3 | 1 | Reserved: zero |
| 4 | 8 | Time: nanoseconds on the media thread's monotonic clock since the call's media began |
| 12 | 2 | The RTP sequence number |
| 14 | 4 | The RTP timestamp |
| 18 | 4 | The RTP synchronisation source identifier |
| 22 | 2 | Length of the payload in octets, at most 240 |
| 24 | n | The payload |

Records are in the order the media thread handled the packets. A received
record is written for every datagram that parsed as RTP, before the receiver
decided what to do with it, so late, duplicate and ignored packets are in
the file.

A **gap** record marks a hole: records the media thread dropped because the
writer had fallen behind. Its timestamp field holds how many records are
missing, its payload is empty, and its other RTP fields are zero.

A file that ends inside a header or a record was cut off, which is what a
capture looks like while its call is still up or after the service was
killed. Everything before the cut is good.

A reader refuses a file whose version it does not know. A change to the
layout takes a new version number.
