# answer-v22-lapm

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. The caller has V.22, no V.8, LAPM and no compression: the line's V.8 comes to
nothing, it offers the carrier of V.22bis and V.22, and the caller answers at
1200 bit/s. XID agrees no compression, and the link is LAPM. Sixty characters
come from the peer and the peer hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-v22-lapm.expect` is what a replay of the capture as the answerer does. The
line's host sends nothing in this call, so the replay sends every frame the
call sent, to its end: with error control the far end acknowledges what it is
sent, and a replay that sent less would be told of frames it never sent.
`rustmodem-replay --frames` shows the frames of LAPM, both ways.
