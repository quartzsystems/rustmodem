# answer-v8-v22bis-lapm-v42bis

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. Both modems are as they come: V.8 chooses V.22bis, the detection phase of V.42
finds error control at both ends, XID agrees a window of 15 and V.42bis both
ways, and the link is LAPM. Sixty characters come from the peer and the peer
hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-v8-v22bis-lapm-v42bis.expect` is what a replay of the capture as the answerer does. The
line's host sends nothing in this call, so the replay sends every frame the
call sent, to its end: with error control the far end acknowledges what it is
sent, and a replay that sent less would be told of frames it never sent.
`rustmodem-replay --frames` shows the frames of LAPM, both ways.
