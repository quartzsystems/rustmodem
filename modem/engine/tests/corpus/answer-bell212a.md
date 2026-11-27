# answer-bell212a

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. The caller has Bell 212A and nothing else, and no error control. Its signal is
that of V.22 at 1200 bit/s, which the line has too, so V.22 is what the line
says it connected with. The detection phase is sent no pattern and ends in
start-stop characters. Sixty characters go each way and the peer hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-bell212a.expect` is what a replay of the capture as the answerer does. The
replay sends what the call sent until the line's host begins to send its
data, which is not in a capture.
