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
