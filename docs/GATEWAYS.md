# Gateways

rustmodem answers calls over SIP and carries the modem's audio as G.711 in
RTP. Between it and the telephone line there is a gateway: an analogue
telephone adapter with FXS or FXO ports, or a PBX with a digital trunk. A
gateway is built to make voice sound good, and almost everything it does to
that end damages a modem's signal. This is the list of what to turn off.

The service itself is described in `docs/TELEPHONY.md`.

## The checklist

Apply every item to the port, the dial peer or the trunk that carries modem
calls. A device that has a setting only for the whole unit needs a unit of
its own for modems.

| # | Setting | Set it to | Why |
|---|---|---|---|
| 1 | Codec | G.711 only: mu-law (PCMU) in North America and Japan, A-law (PCMA) elsewhere. No G.729, G.726, G.723 or wideband codec in the list at all | A modem's signal survives only a codec that carries every sample. A compressing codec that is merely allowed can still be chosen by the far end |
| 2 | Echo canceller | Off | It adapts to the signal and subtracts what it takes for echo, which for a full-duplex modem is the other direction of the call |
| 3 | Silence suppression (VAD) and comfort noise (CNG) | Both off | A gap in the packets is a gap in the samples, and noise made up at the far end is not what was sent. rustmodem says `a=silenceSupp:off` in its descriptions; many devices do not listen |
| 4 | Jitter buffer | Fixed, at its minimum | An adaptive buffer changes its depth during the call, which inserts and removes samples. A modem loses synchronisation at each |
| 5 | Gain, in both directions | 0 dB | A level the modems did not negotiate at is one they are no longer equalised for |
| 6 | In-band DTMF detection and DTMF relay (RFC 2833 or SIP INFO) | Off for the port | A detector that hears a tone in the modem's signal removes it from the audio and sends an event in its place |
| 7 | T.38 fax relay | Off | The gateway would hear the answer tone and try to switch the call to a fax relay with a re-INVITE. rustmodem refuses one with 488 and goes on with the call as it was; what the gateway does with its own end after the refusal is up to it |
| 8 | Modem or fax pass-through | On, if the device has it | It is the device's own switch for items 2 to 4: on hearing the answer tone it turns the echo canceller and silence suppression off and fixes the jitter buffer. Where it uses a payload type of its own (Cisco NSE), prefer the mode that does not, or leave it off and set items 2 to 4 by hand |
| 9 | Media path | Direct between the gateway and rustmodem | A PBX or a session border controller that relays the RTP adds a jitter buffer and a clock of its own, and may transcode |
| 10 | Packet time | 20 ms, or 10 ms if the device allows it | It must be the one rustmodem is set to (`media.ptime_ms`). A shorter packet time is a shorter delay |
| 11 | Clock | Nothing to set; know which it is | rustmodem has no clock of its own during a call. It takes each packet as it arrives and sends one back, so the gateway's clock is the call's clock, as the line's would be |

After the gateway is set up, make a call with capture on for the line and
check it (`docs/CAPTURE.md`):

```
rustmodem-ctl capture on 1
rustmodem-ctl capture check <file>
```

Lost, Late, Duplicate and Resync are the network's and the gateway's doing.
A clean capture of a ten-minute call, at both 10 ms and 20 ms, is the
evidence that the path is fit for a modem.

## What rustmodem needs from the gateway

- SIP over UDP, IPv4. No TCP, no TLS, no SRTP.
- The gateway's address in `sip.trusted_peers`, for a gateway that sends
  calls to the trunk listener; or a registration for each line
  (`register` in the line's table), for a PBX that the lines are extensions
  of.
- One audio stream of G.711 in each call, in both directions once the call
  is answered. Early media is not listened to. A call the gateway puts on
  hold is held, and a modem does not survive that: nothing should hold a
  modem call.
- The called number in the request URI. Routes and hunt groups match on it.

## Devices

A section for each device rustmodem has been set up against. The exact
names of the settings differ from one firmware to the next and are filled
in from the device as it is configured.

`NEEDS-CODY`: each table below has the checklist's items and no setting
names yet. Fill in the page or command, the name of the setting, and the
value, as each device is set up; note the firmware version.

### Cisco SPA112

A two-port FXS adapter, configured in its web pages (Voice, then Line 1 or
Line 2; the advanced view shows every setting). Firmware:

| # | Item | Page | Setting | Value |
|---|---|---|---|---|
| 1 | G.711 only | | | |
| 2 | Echo canceller off | | | |
| 3 | Silence suppression and comfort noise off | | | |
| 4 | Jitter buffer fixed at its minimum | | | |
| 5 | Gains at 0 dB | | | |
| 6 | DTMF detection and relay off | | | |
| 7 | T.38 off | | | |
| 8 | Modem or fax pass-through | | | |
| 9 | Media direct | | | |
| 10 | Packet time | | | |

### AudioCodes MP-124

A 24-port FXS gateway, configured in its web pages or by an `ini` file.
Firmware:

| # | Item | Page or `ini` parameter | Setting | Value |
|---|---|---|---|---|
| 1 | G.711 only | | | |
| 2 | Echo canceller off | | | |
| 3 | Silence suppression and comfort noise off | | | |
| 4 | Jitter buffer fixed at its minimum | | | |
| 5 | Gains at 0 dB | | | |
| 6 | DTMF detection and relay off | | | |
| 7 | T.38 off | | | |
| 8 | Modem or fax pass-through | | | |
| 9 | Media direct | | | |
| 10 | Packet time | | | |

### Cisco VG310

A 24-port FXS voice gateway running IOS, configured at the command line:
the settings are on the `voice-port` and on the `dial-peer voice` that
points at rustmodem. IOS version:

| # | Item | Where (`voice-port`, `dial-peer`, global) | Command | Value |
|---|---|---|---|---|
| 1 | G.711 only | | | |
| 2 | Echo canceller off | | | |
| 3 | Silence suppression and comfort noise off | | | |
| 4 | Jitter buffer fixed at its minimum | | | |
| 5 | Gains at 0 dB | | | |
| 6 | DTMF detection and relay off | | | |
| 7 | T.38 off | | | |
| 8 | Modem or fax pass-through | | | |
| 9 | Media direct | | | |
| 10 | Packet time | | | |
