# answer-bell103

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. The caller has Bell 103 and nothing else: the line sends ANSam, offers V.21 for a time, hears the caller's Bell 103 carrier and changes to it.
Sixty characters go each way and the peer hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-bell103.expect` is what a replay of the capture as the answerer does. The
replay sends what the call sent until the line's host begins to send its
data, which is not in a capture.
