# Telephony

How RustModem behaves on a SIP network: what it listens to, how it routes a
call to a line, what it sends and answers method by method, and the
configuration that decides it. This describes the service as built in
phase 2. The code is `service/telephony` (the router), `voip/sip`,
`voip/sdp` and `voip/rtp` (the protocols), and `service/media` (the audio).

With `[sip]` absent or `enabled = false` none of this applies: the service
speaks no SIP, and `ATD<line>` calls another line of the same service, as
it did in phase 0.

## What it is and is not

RustModem is a SIP user agent over UDP on IPv4. It takes calls on one
listener, the trunk, and can also register any of its lines to a PBX as an
extension. Each call carries one stream of G.711 (PCMU or PCMA) over RTP.

It does not do: SIP over TCP or TLS, IPv6, SRTP, NAT traversal of any kind
(no STUN, no ICE, no rewriting of its own Contact from `received`), T.38,
DTMF in any form (`telephone-event` is never offered and never accepted),
comfort noise, session timers, PRACK, UPDATE, REFER, SUBSCRIBE, RTCP,
transfer, or a jitter or reorder buffer. A call rings one line; it is not
hunted again if that line does not answer.

Standards followed: RFC 3261 (SIP), RFC 6026 (the INVITE transaction's
Accepted state), RFC 3581 (`rport`), RFC 3264 and RFC 8866 (SDP and
offer/answer), RFC 3550 and RFC 3551 (RTP and its audio profile), RFC 7616
and RFC 2617 (digest authentication).

## Who may talk to it

Every request is checked against its source address before anything else
is done with it.

| The request | Is taken from |
|---|---|
| Any request | An address in `sip.trusted_peers` |
| A request whose Request-URI is the contact a line registered (the line's extension and `;line=<id>`) | Also that line's registrar |
| Anything else | Nobody |

A request from any other address is **dropped without an answer**: no
response is sent, no transaction is made, and it is not traced. It is
counted (`dropped untrusted` in `sip.status`), and `RMD5010` is logged for
the first one and then for every hundredth. With `trusted_peers` empty and
no line registered, nothing is accepted at all.

Responses are matched to requests RustModem sent by the branch it put in
its Via; a response that matches nothing is ignored.

## Routing an incoming call

The called number is the user part of the Request-URI of the INVITE, with
its `%hh` escapes undone.

1. If the Request-URI is the contact a line registered, the call is for
   that line and for no other.
2. Otherwise the `[[route]]` entries are tried in file order. The first
   whose `called` matches decides, whatever the state of its target: the
   call does not fall through to a later route.
3. Then the `pilot` numbers of the `[[hunt_group]]` entries.
4. Nothing matches: `404 Not Found`.

A line can take a call when it is **idle**: in service, its port open,
the host's DTR on, and no call on it. The line is chosen once, when the
INVITE arrives.

| Strategy | The line chosen |
|---|---|
| `first-idle` | The first idle line, in the order the group lists them |
| `round-robin` | The first idle line after the one that took the group's last call |
| `least-recent` | The idle line whose last call ended longest ago; a line that has had no call comes before any that has |

When the target has no idle line:

| The lines of the target | Response |
|---|---|
| At least one is in a call (ringing, being set up or connected) | `486 Busy Here` |
| None is ready (all disabled, or not ready) | `480 Temporarily Unavailable` |

A line that the router has just offered a call to counts as in a call from
that moment, so two INVITEs that arrive together never get the same line.

Every call that is turned away before a line is rung is logged as
`RMD5011` with the caller, the called number and the status.

## An incoming call

| Event | What RustModem does |
|---|---|
| INVITE, routed to an idle line | `100 Trying`, then `180 Ringing` with its tag and Contact. The line rings: `RING` on the cadence of 2 s on, 4 s off |
| INVITE whose session description cannot be used (see "Session descriptions") | `488 Not Acceptable Here` |
| INVITE with a body that is not `application/sdp` | `415 Unsupported Media Type` |
| INVITE with no body (a delayed offer) | Accepted. RustModem's offer goes in the `200`, and the answer is taken from the ACK |
| INVITE whose Request-URI is not `sip:` | `416 Unsupported URI Scheme` |
| INVITE with no Contact that is a `sip:` URI | `400 Bad Request` |
| INVITE with `Require` | `420 Bad Extension`, with `Unsupported` naming what was required |
| No route / every line busy / no line ready | `404` / `486` / `480` |
| No RTP port free in `media.rtp_ports` | `503 Service Unavailable`, and `RMD5012` in the log |
| The host answers (`ATA`, or S0 rings) | `200 OK` with the session description. The `200` is sent again after 0.5 s, then at doubling intervals up to 4 s, until its ACK arrives |
| The ACK arrives | The call is connected: the line's modem starts as the answering modem and the media runs |
| The answer in the ACK of a delayed offer cannot be used, or there is none | BYE; the host gets `NO CARRIER` |
| No ACK in 32 s | BYE; the host gets `NO CARRIER`; `RMD5013` in the log |
| CANCEL before the answer | `200` to the CANCEL, `487 Request Terminated` to the INVITE; the line stops ringing |
| CANCEL that matches no INVITE still unanswered | `481 Call/Transaction Does Not Exist` |
| `sip.ring_timeout_s` passes with no answer | `480`; the line stops ringing |
| The host hangs up, or `lines.hangup`, while the line rings | `603 Decline` |
| The host hangs up after the `200` and before its ACK | The BYE is sent when the ACK arrives, or after the 32 s |

The Contact RustModem gives is `sip:<called user>@<address>:<port>`, and
for a call to a registered line `sip:<extension>@<address>:<port>;line=<id>`.

## An outgoing call

`ATD<number>` on a line places a call.

| The line | The INVITE goes to | As |
|---|---|---|
| Has `register` | Its registrar | Its extension: `From: <sip:<user>@<server>>` |
| Has no `register`, and `sip.outbound_peer` is set | `outbound_peer` | `From: <sip:<line id>@<peer>>` |
| Has no `register`, and `outbound_peer` is empty | Nowhere: the host gets `NO DIALTONE` | |

The Request-URI is `sip:<digits>@<host>[:<port>]`. Of the dial string, the
digits, `*` and `+` are kept, `#` is sent as `%23`, and everything else
(the pause `,`, the wait `W`) is left out. The INVITE carries RustModem's
offer.

| What comes back | What the host gets |
|---|---|
| `100`, `180`, `183` | Nothing yet. Early media is not listened to |
| `200` with an answer that can be used | The ACK is sent; `CONNECT` when the modems are through. The line's modem starts as the originating modem |
| `200` with an answer that cannot be used, or with none | The ACK, then BYE; `NO CARRIER` |
| `401` or `407`, on a registered line with a password | The INVITE again with credentials, once. A second challenge is answered only if it says `stale=true` and brings a new nonce |
| `401` or `407` otherwise, or the credentials refused | `NO CARRIER` |
| `486`, `600` | `BUSY` |
| `480`, `408`, or no response at all in 32 s | `NO ANSWER` |
| Any other final response | `NO CARRIER` |
| The caller's S7 runs out, or the host aborts | CANCEL, once the far end has sent any response; the `487` that follows is acknowledged |
| The far end answers `200` as the CANCEL is on its way | The ACK, then BYE |
| The `200` arrives again (the ACK was lost) | The same ACK again |

Which of `BUSY`, `NO ANSWER` and `NO DIALTONE` a host sees depends on its
`X` setting; see `docs/AT.md`.

## In a call

| Event | What RustModem does |
|---|---|
| BYE from the far end | `200`; the host gets `NO CARRIER` |
| BYE with a CSeq number lower than the last | `500 Server Internal Error`; the call goes on |
| The host hangs up, or `lines.hangup` | BYE, sent again until it is answered or 32 s have passed |
| No RTP packet for `media.rtp_timeout_s` | The call is cleared: BYE, `NO CARRIER`, `RMD6001` in the log |
| re-INVITE whose session description is still a G.711 stream | `200` with RustModem's description, its version one higher. The media is sent to the new address, in the new codec and at the new packet time, from then on |
| re-INVITE with `a=sendonly`, `a=inactive` or a `0.0.0.0` address (hold) | `200`. While the call is held RustModem sends no RTP, the line's modem goes on ticking on silence from a local timer, and the RTP timeout does not run |
| re-INVITE with no body | `200` with RustModem's offer |
| re-INVITE offering T.38, or no codec RustModem has | `488`; the call goes on as it was |
| OPTIONS, in a call or outside one | `200` with `Allow` and `Accept: application/sdp` |
| REGISTER, SUBSCRIBE, NOTIFY, REFER, UPDATE, PRACK, INFO, MESSAGE, PUBLISH | `405 Method Not Allowed` with `Allow` |
| Any other method | `501 Not Implemented` with `Allow` |
| Any request with `Require` | `420 Bad Extension` |
| BYE or re-INVITE for a dialog RustModem does not have | `481` |

`Allow` is always `INVITE, ACK, BYE, CANCEL, OPTIONS`. No `Supported`
header is sent: no extension is advertised.

## Registrations

A line with a `register` table is registered to its PBX, one registration
a line, for as long as the line is in service.

- The REGISTER goes to `sip:<server>`, for the address of record
  `sip:<user>@<server>`, with the contact
  `sip:<user>@<address>:<port>;line=<id>` and `Expires: <expires_s>`.
- It is refreshed at 90 percent of the time the registrar granted. The
  time granted is the `expires` of RustModem's contact in the `200`, or
  its `Expires` header, or what was asked for.
- `401` or `407`: answered once with the line's credentials, MD5 or
  SHA-256, with `qop=auth` when the registrar offers it. The user name is
  `auth_user`, or `user` when that is empty. The nonce is kept and used
  for the refreshes without being asked.
- `423 Interval Too Brief`: the registrar's `Min-Expires` is taken, once
  a try.
- Any other answer, or none in 32 s, is a failure. RustModem tries again
  after 5, 10, 20 and 40 seconds and then every 60.
- When the service stops, when the line is taken out of service, and when
  its `register` table is changed or removed, the binding is removed with
  `Expires: 0`.
- `sip.reregister` in the control API registers a line, or every line,
  again at once.

| State | Meaning |
|---|---|
| `registering` | The first REGISTER is out, or the registrar's name is being looked up |
| `registered` | The registrar accepted; `expires_in_s` says for how long |
| `failed` | The last try failed. The state stays `failed`, with its reason, while RustModem tries again, until a try succeeds |
| `unregistered` | The binding was removed |

| Reason | Message |
|---|---|
| The registrar did not answer | `RMD5004` |
| The user name or the password was refused, or the line has no password and was asked for one | `RMD5005` |
| The registrar offered no Digest challenge with MD5 or SHA-256 and `qop=auth` | `RMD5006` |
| The registrar answered with another status | `RMD5007` |
| The registrar's name did not resolve; it is looked up again every 60 s | `RMD5008` |

`RMD5003` is logged when a line registers and `RMD5009` when its binding
is removed. A failure is logged once, not at every try.

The password is used to compute the digest response and is never sent,
logged, traced or returned by the control API.

## Session descriptions

RustModem offers and answers one audio stream:

```
v=0
o=- <session id> <version> IN IP4 <address>
s=rustmodem
c=IN IP4 <address>
t=0 0
m=audio <port> RTP/AVP 0 8
a=rtpmap:0 PCMU/8000
a=rtpmap:8 PCMA/8000
a=ptime:20
a=sendrecv
a=silenceSupp:off - - - -
```

An offer lists the codecs of `media.codecs` in their order. An answer has
one codec: the first of `media.codecs` that the offer contains.

| In an offer | What RustModem does |
|---|---|
| No `m=audio` line over `RTP/AVP`, or its port is zero | Refused |
| Neither payload type 0 nor 8, or none that `media.codecs` has | Refused |
| An `m=image` line with a port (T.38) | Refused |
| No `c=` address for the stream, or one that is not IPv4 | Refused |
| `a=ptime` of 10, 20 or 30 | Used, both ways |
| Any other `a=ptime`, or none | `media.ptime_ms` is used and stated |
| Other streams beside the audio | Each is refused in the answer with port zero |
| `telephone-event`, comfort noise (13), dynamic payload types | Left out of the answer |

A dynamic payload type that an `a=rtpmap` maps to PCMU or PCMA is not
accepted: only the static payload types 0 and 8 are.

## Media

Each call has its own UDP socket on an even port of `media.rtp_ports`,
bound when its session description is built. RTP is sent to the address
and port of the far end's description. Packets from any other source are
dropped and counted (`stray`).

The media is **sample-exact**. There is no jitter buffer and no reorder
buffer. Each arriving packet is delivered to the line's modem as frames of
80 octets, and for every frame delivered exactly one frame is sent back:
the far end's sample clock is the only clock.

| Arrival | What happens |
|---|---|
| The next packet in order | Its payload is delivered |
| A packet after a gap | The missing samples, by the timestamp, are delivered first as silence (`0xFF` for PCMU, `0xD5` for PCMA), then the packet. Counted as `lost` |
| A packet delivered already | Dropped. Counted as `duplicate` |
| A packet older than the last delivered | Dropped; its place was filled. Counted as `late` |
| Another SSRC, or a timestamp more than 3 s from the one expected, or one that left the 10 ms grid | The stream starts again on this packet. Counted as `resync` |
| Another payload type, or a payload that is not a whole number of 10 ms frames | Dropped. Counted as `ignored` |

Before the far end's first packet, the modem is ticked from a local timer
at the packet time so that it can start; those frames are counted as
`free_run`. From the first packet on, nothing is paced locally.

DSCP marking is not done by the service: on Windows it is set by a QoS
policy, and the setup program makes one, "RustModem RTP", that marks the
UDP `rustmodem-svc.exe` sends with DSCP 46, expedited forwarding, on
every network profile. `Get-NetQosPolicy` shows it. The policy marks the
service's SIP as well as its RTP: both are UDP from the one program, and
the policy names the program and no ports, so that it holds whatever the
configuration's ports are.

NEEDS-CODY: a packet capture that shows the mark on the wire, on a
domain-joined machine and on a standalone one, is criterion 17 of
`docs/PHASE5.md`. Windows has applied such policies only to domain
networks on some editions; a value named `Do not use NLA` under
`HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\QoS` is said to lift that,
sources disagree on its type, and setup does not write it.

## Tracing

Every SIP message sent and received is offered to the control API as a
`sip.trace` event, with its direction, the other end's address and the
time. `sip.trace` is sent only to a client that subscribed to it by name,
and is not made at all while no client has: the manager's SIP trace window
asks for it while it is open. It is not written to the log. A request that
was dropped because its source is not trusted is not traced.

A call's audio is traced by capturing it: see `docs/CAPTURE.md`. What to
set on the gateway in front of the service is in `docs/GATEWAYS.md`.

## Configuration reference

Every section is optional. A file with none of them is a phase 0 file and
behaves as one.

### `[sip]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Whether the service speaks SIP. A `[sip]` table does not turn it on by being there |
| `listen` | `"0.0.0.0:5060"` | The UDP address of the trunk listener. RTP sockets are bound to the same address |
| `address` | `""` | The address to advertise in Via, Contact and session descriptions: an IPv4 address or a host name. Empty means the local address the system would send to the peer from |
| `user_agent` | `""` | The User-Agent and Server header. Empty means `rustmodem/<version>` |
| `trusted_peers` | `[]` | The addresses and CIDR blocks that may send requests to the trunk listener, for example `["10.10.20.11", "10.10.30.0/24"]` |
| `outbound_peer` | `""` | Where lines that are not registered send the calls they dial: `host` or `host:port`. Empty means such lines have no dial tone |
| `ring_timeout_s` | `60` | How long a line may ring before the caller gets `480`. 5 to 3600 |

A change to `listen` takes effect when the service next starts. The other
keys take effect at once.

### `[media]`

| Key | Default | Meaning |
|---|---|---|
| `codecs` | `["pcmu", "pcma"]` | The codecs, most preferred first. One or both of `pcmu` and `pcma` |
| `ptime_ms` | `20` | The packet time: 10, 20 or 30 |
| `rtp_ports` | `"20000-20999"` | The ports RTP may use, `first-last`, from 1024 up. Only the even ports are used: a range needs one even port for each call at a time |
| `rtp_timeout_s` | `10` | How long a call may go without an RTP packet before it is cleared. 1 to 3600 |
| `capture_dir` | `""` | Where call captures are written. Empty means `captures` under the log directory |

A change to `rtp_ports` takes effect for the next call.

### `[[line]]`, the keys phase 2 adds

| Key | Default | Meaning |
|---|---|---|
| `engine` | `"modem"` | The modem engine of the line: `modem`, or for tests of the path `null` or `echo` |
| `modem` | none | The line's own modem settings, over those of `[modem]`: see `docs/MODEM.md` |
| `register` | none | A table. With it the line is an extension of a PBX; without it, a trunk line |
| `register.server` | required | The registrar: `host` or `host:port` |
| `register.user` | required | The extension |
| `register.password` | `""` | The password. Write-only through the control API |
| `register.auth_user` | `""` | The user name to authenticate with. Empty means `user` |
| `register.expires_s` | `300` | The registration time to ask for. 30 to 86400 |

### `[[hunt_group]]`

| Key | Default | Meaning |
|---|---|---|
| `name` | required | The name, unique: 1 to 32 letters, digits, `_`, `-` or `.` |
| `pilot` | required | The number that calls the group, unique: digits, `#` or `+` |
| `lines` | `[]` | The ids of the lines of the group, in hunting order |
| `strategy` | `"first-idle"` | `first-idle`, `round-robin` or `least-recent` |

### `[[route]]`

| Key | Default | Meaning |
|---|---|---|
| `called` | required | The called number: exact (`"5551000"`), or a prefix ending in `*` (`"5552*"`). `"*"` matches every number |
| `target` | required | `"line:<id>"` or `"group:<name>"` |

Routes are tried in file order. A route that an earlier route keeps from
ever matching is reported as a warning, `RMD0031`, when the file is read.

### Validation

Everything wrong with a file is reported at once, each problem as an `RMD`
message with the key and the line number: a value that is not allowed
(`RMD0014`), a key of the wrong type (`RMD0013`), a required key that is
missing (`RMD0015`), a hunt group that names a line the file does not have
(`RMD0027`), two groups with one name (`RMD0028`) or one pilot (`RMD0029`),
and a route to a line or a group that does not exist (`RMD0030`).

## Windows Firewall

The setup program makes two inbound rules, for the service's program and
no other, on the SIP port and the RTP range of the configuration as it is
when setup runs:

```
netsh advfirewall firewall add rule name="RustModem SIP" dir=in action=allow protocol=UDP localport=5060 program="C:\Program Files\Quartz Systems\RustModem\rustmodem-svc.exe"
netsh advfirewall firewall add rule name="RustModem RTP" dir=in action=allow protocol=UDP localport=20000-20999 program="C:\Program Files\Quartz Systems\RustModem\rustmodem-svc.exe"
```

These are the commands it runs, with the ports of the default
configuration. After a change to `[sip] listen` or `[media] rtp_ports`,
`rustmodem-setup repair` makes the rules again for the new ports.
Uninstalling removes both.

The rules admit any address. The service itself checks every SIP request
against its source address, as "Who may talk to it" above describes; to have
the firewall refuse other addresses too, add `remoteip=` to the two rules
by hand, and know that `repair` will make them again without it.
