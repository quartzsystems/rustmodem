# answer-v22bis

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. The caller has V.22bis, no V.8 and no error control: the line's V.8 comes to
nothing, the modems train at 2400 bit/s, and the line's detection phase is sent
no pattern and ends in start-stop characters with no error control. Sixty
characters go each way and the peer hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-v22bis.expect` is what a replay of the capture as the answerer does. The
replay sends what the call sent until the line's host begins to send its
data, which is not in a capture.
