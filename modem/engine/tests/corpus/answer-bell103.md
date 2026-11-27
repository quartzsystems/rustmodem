# answer-bell103

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. The caller has Bell 103 and nothing else: the line sends ANSam for its 5 s, hears no CM, and begins with the answer sequence of V.22bis, which is first among its modulations. Its unscrambled binary 1, at 2250 Hz, the caller takes for the answer carrier of Bell 103 at mark, which is 2225 Hz; the line hears the caller's Bell 103 carrier and is Bell 103.
Sixty characters go each way and the peer hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-bell103.expect` is what a replay of the capture as the answerer does. The
replay sends what the call sent until the line's host begins to send its
data, which is not in a capture.
