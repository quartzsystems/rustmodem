# answer-v21

A line with the modem engine answers a call from the test peer's modem over
SIP and RTP on the loopback interface, PCMU at 20 ms. The caller has V.21 and no V.8: the line sends ANSam for its 5 s, hears no CM, and begins with the answer sequence of V.22bis, which is first among its modulations. A second after the tone the caller sends its calling carrier by itself (V.25 clause 3.10); the line hears it and is V.21.
Sixty characters go each way and the peer hangs up.

The far end is this engine too: the capture shows the engine against
itself, on a path with no faults. It is made by
`service/svc/tests/peer.rs`, run with `RM_CORPUS` naming this directory.

`answer-v21.expect` is what a replay of the capture as the answerer does. The
replay sends what the call sent until the line's host begins to send its
data, which is not in a capture.
