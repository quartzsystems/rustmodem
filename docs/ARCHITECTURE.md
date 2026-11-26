# rustmodem architecture

rustmodem is a software modem pool for 64-bit Windows. A Windows service
terminates SIP/RTP, runs one in-house softmodem per call, and presents each
line to Windows as a modem on a COM port, so Routing and Remote Access answers
dial-up calls as if the modems were hardware. A separate manager GUI and a
command-line client control the service over a local, versioned API. The
service is built from small crates: a modem engine behind one trait, a pure AT
command interpreter, a serial backend behind one trait, and a pool that owns
the lines and joins those three per line.

The full architecture guide is supplied separately by Cody and replaces this
summary when it is dropped in. Nothing beyond the phase 0 handoff is described
here; see [PHASE0.md](PHASE0.md) for what is built.
