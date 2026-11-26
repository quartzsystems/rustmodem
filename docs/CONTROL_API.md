# The rustmodem control API

This file is generated from `service/ctlproto/src/doc.rs` and checked against
the API's types by that crate's tests. Do not edit it by hand: change the
tables there and run `UPDATE_DOCS=1 cargo test -p ctlproto`.

## Transport

The service listens on a Windows named pipe, `\\.\pipe\rustmodem-control` by
default; the name is `control.pipe` in the configuration. The pipe is open to
SYSTEM and Administrators only, so a client must run elevated. Remote clients
are refused.

Each message is one JSON object on one line, in UTF-8, ended by a line feed.
A message may be at most 1048576 bytes long.

## Hello

The first message each way is `hello`. The client sends its hello first:

```json
{"hello":{"protocol":1,"version":"0.0.1"}}
```

The service answers with its own, in the same form. This version of the
service speaks protocol 1. If the client's protocol is another, the service
answers with error `RMD2001` and closes the connection. If the first message
is not a hello, the service answers with error `RMD2005` and closes the
connection. Both errors are sent with a null `id`.

## Requests, responses and events

A request names a method. The `id` is chosen by the client and comes back in
the answer; `params` may be left out when the method takes none.

```json
{"id":7,"method":"lines.get","params":{"id":1}}
```

The answer is a result or an error:

```json
{"id":7,"result":{ ... }}
{"id":7,"error":{"msgid":"RMD1001","text":"Line 1 does not exist.","cause":"...","recovery":"..."}}
```

An error is always an `RMD` message: its id, its text, and its cause and
recovery. When a message cannot be read as a request at all, the error has a
null `id`.

Events are sent only to a client that has subscribed:

```json
{"event":"line.state","data":{ ... }}
```

Answers come in the order of their requests. Events may come between them.

A client that subscribes must keep reading. The service queues what a client
has not read yet. When the queue is full, the oldest `log` events are dropped
first; every other event reports state and is never dropped. A client that
falls further behind than that is disconnected (`RMD2006` in the service
log), and can connect again and ask for the current state.

## Compatibility

The protocol number changes only when an existing message changes its
meaning or its shape. Methods, events and optional fields may be added
without a protocol change, and fields may be added to results and to the
data of events. A client must ignore fields and events it does not know.
The service refuses a parameter it does not know with `RMD2003`, so a client
finds out at once that the service is older than it is.

## Methods

| Method | Result | Summary |
|---|---|---|
| [`service.info`](#serviceinfo) | ServiceInfo | Reports the service's version, protocol, pipe name, configuration path, number of lines, how long it has been running and how it is being run. |
| [`lines.list`](#lineslist) | LineList | Lists every line, in id order. |
| [`lines.get`](#linesget) | LineInfo | Reports one line. |
| [`lines.answer`](#linesanswer) | LineInfo | Answers the call ringing on a line, as `ATA` from the host would. |
| [`lines.hangup`](#lineshangup) | LineInfo | Hangs up the call on a line, or turns away the call ringing on it. |
| [`lines.reset`](#linesreset) | LineInfo | Resets a line, as switching its modem off and on would: hangs up any call and returns the modem's settings to its stored profile, or to the factory defaults if it has none. |
| [`lines.enable`](#linesenable) | LineInfo | Puts a line in service. |
| [`lines.disable`](#linesdisable) | LineInfo | Takes a line out of service: any call on it is hung up and its port is released. |
| [`lines.add`](#linesadd) | LineInfo | Adds a line. |
| [`lines.edit`](#linesedit) | LineInfo | Changes the keys of a line that are given and leaves the others alone. |
| [`lines.remove`](#linesremove) | Done | Removes a line: any call on it is hung up and its port is released. |
| [`lines.capture`](#linescapture) | LineInfo | Turns the capture of a line's calls on or off. |
| [`calls.list`](#callslist) | CallList | Lists the calls that have ended, oldest first. |
| [`config.reload`](#configreload) | Reloaded | Reads the configuration file again and makes the lines match it. |
| [`config.get`](#configget) | Settings | Reports the settings that are not lines, as the configuration file has them, and the path of the file. |
| [`config.set`](#configset) | SettingsResult | Changes the settings that are given and leaves the others alone. |
| [`stats.get`](#statsget) | Stats | Reports the statistics of the service since it started. |
| [`log.tail`](#logtail) | LogTail | Returns the last lines the service logged, oldest first. |
| [`ports.list`](#portslist) | PortList | Lists the serial ports of the machine the service runs on, in name order, with the com0com pairs, the modem devices attached to them and the lines that use them. |
| [`sip.status`](#sipstatus) | SipStatus | Reports the SIP listener, the peers, and the registration of every line that registers. |
| [`sip.reregister`](#sipreregister) | Done | Registers a line again now, or every line that registers. |
| [`huntgroups.list`](#huntgroupslist) | HuntGroupList | Lists the hunt groups, in the order of the configuration file. |
| [`huntgroups.add`](#huntgroupsadd) | HuntGroupInfo | Adds a hunt group. |
| [`huntgroups.edit`](#huntgroupsedit) | HuntGroupInfo | Changes the keys of a hunt group that are given and leaves the others alone. |
| [`huntgroups.remove`](#huntgroupsremove) | Done | Removes a hunt group. |
| [`routes.list`](#routeslist) | RouteList | Lists the routes, in the order they are tried. |
| [`routes.add`](#routesadd) | RouteList | Adds a route. |
| [`routes.edit`](#routesedit) | RouteList | Changes the keys of a route that are given and leaves the others alone. |
| [`routes.remove`](#routesremove) | RouteList | Removes a route. |
| [`routes.move`](#routesmove) | RouteList | Moves a route to another place in the list. |
| [`subscribe`](#subscribe) | Subscription | Starts sending the client events. |
| [`unsubscribe`](#unsubscribe) | Subscription | Stops sending the client events. |

Every request can also get one of these errors: `RMD2002`, `RMD2003`, `RMD2004`, `RMD2010`, `RMD2014`.

### `service.info`

Reports the service's version, protocol, pipe name, configuration path, number of lines, how long it has been running and how it is being run.

Parameters: none.

Result: ServiceInfo.

```json
{"id":1,"method":"service.info"}
```

### `lines.list`

Lists every line, in id order.

Parameters: none.

Result: LineList.

```json
{"id":1,"method":"lines.list"}
```

### `lines.get`

Reports one line.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: LineInfo.

Errors: `RMD1001`.

```json
{"id":1,"method":"lines.get","params":{"id":1}}
```

### `lines.answer`

Answers the call ringing on a line, as `ATA` from the host would. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: LineInfo.

Errors: `RMD1001`, `RMD1002`, `RMD1003`.

```json
{"id":1,"method":"lines.answer","params":{"id":1}}
```

### `lines.hangup`

Hangs up the call on a line, or turns away the call ringing on it. The host is told `NO CARRIER`. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: LineInfo.

Errors: `RMD1001`, `RMD1004`.

```json
{"id":1,"method":"lines.hangup","params":{"id":1}}
```

### `lines.reset`

Resets a line, as switching its modem off and on would: hangs up any call and returns the modem's settings to its stored profile, or to the factory defaults if it has none. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: LineInfo.

Errors: `RMD1001`.

```json
{"id":1,"method":"lines.reset","params":{"id":1}}
```

### `lines.enable`

Puts a line in service. The change is written to the configuration file. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: LineInfo.

Errors: `RMD0011`, `RMD0019`.

```json
{"id":1,"method":"lines.enable","params":{"id":1}}
```

### `lines.disable`

Takes a line out of service: any call on it is hung up and its port is released. The change is written to the configuration file. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: LineInfo.

Errors: `RMD0011`, `RMD0019`.

```json
{"id":1,"method":"lines.disable","params":{"id":1}}
```

### `lines.add`

Adds a line. The change is written to the configuration file. The result is the new line.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id, 1 to 999, not in use. |
| `name` | string | no | A name for people. |
| `backend` | string | no | The serial backend. Default `com0com`. |
| `port` | string | yes | The serial port, for example `CNCB0`. |
| `enabled` | boolean | no | Whether the line is in service. Default true. |
| `auto_answer_rings` | integer | no | The initial S0, 0 to 255. Default 0. |
| `unknown_command` | string | no | What the line answers to an unknown AT command: `ok` or `error`. Default `ok`. |
| `engine` | string | no | The modem engine: `null` or `echo`. Default `null`. |
| `register` | RegisterSpec | no | The line's registration to a PBX. Default none: the line is reached by the trunk. |

Result: LineInfo.

Errors: `RMD0011`, `RMD0014`, `RMD0017`, `RMD0018`, `RMD0020`, `RMD0035`.

```json
{"id":1,"method":"lines.add","params":{"id":3,"name":"Line 3","backend":"com0com","port":"CNCB2","enabled":true,"auto_answer_rings":0,"unknown_command":"ok","engine":"null","register":{"server":"pbx.example.com","user":"203","password":"secret","auth_user":"line203","expires_s":300}}}
```

### `lines.edit`

Changes the keys of a line that are given and leaves the others alone. Changing the port, or disabling the line, hangs up any call on it. The change is written to the configuration file. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line to change. |
| `name` | string | no | A new name. An empty string removes the name. |
| `backend` | string | no | A new serial backend. |
| `port` | string | no | A new serial port. |
| `enabled` | boolean | no | Whether the line is in service. |
| `auto_answer_rings` | integer | no | A new initial S0, 0 to 255. |
| `unknown_command` | string | no | `ok` or `error`. |
| `engine` | string | no | A new modem engine: `null` or `echo`. It is used from the line's next call. |
| `register` | RegisterChange | no | Changes to the line's registration to a PBX. |

Result: LineInfo.

Errors: `RMD0011`, `RMD0014`, `RMD0017`, `RMD0018`, `RMD0019`, `RMD0035`.

```json
{"id":1,"method":"lines.edit","params":{"id":3,"name":"Front desk","backend":"com0com","port":"CNCB5","enabled":false,"auto_answer_rings":2,"unknown_command":"error","engine":"echo","register":{"server":"pbx.example.com:5060","user":"203","password":"secret","auth_user":"","expires_s":600}}}
```

### `lines.remove`

Removes a line: any call on it is hung up and its port is released. The change is written to the configuration file.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |

Result: Done.

Errors: `RMD0011`, `RMD0019`.

```json
{"id":1,"method":"lines.remove","params":{"id":3}}
```

### `lines.capture`

Turns the capture of a line's calls on or off. While it is on, every RTP packet of each call on the line is written to a file in the capture directory; see `docs/CAPTURE.md`. Turning it on during a call captures the rest of the call. The setting is not kept in the configuration: it is off when the service starts. The result is the line afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |
| `on` | boolean | yes | Whether the line's calls are captured. |

Result: LineInfo.

Errors: `RMD1001`.

```json
{"id":1,"method":"lines.capture","params":{"id":3,"on":true}}
```

### `calls.list`

Lists the calls that have ended, oldest first. The service remembers the last 1000.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `last` | integer | no | Return at most this many, the most recent. Default all. |

Result: CallList.

```json
{"id":1,"method":"calls.list","params":{"last":20}}
```

### `config.reload`

Reads the configuration file again and makes the lines match it. Lines whose configuration has not changed are not disturbed. Changes to `[service]` and `[control]` take effect when the service next starts. If the file is not valid nothing changes, and the error is the first problem found.

Parameters: none.

Result: Reloaded.

Errors: `RMD0010`, `RMD0012`, `RMD0013`, `RMD0014`, `RMD0015`, `RMD0016`, `RMD0017`, `RMD0018`.

```json
{"id":1,"method":"config.reload"}
```

### `config.get`

Reports the settings that are not lines, as the configuration file has them, and the path of the file.

Parameters: none.

Result: Settings.

```json
{"id":1,"method":"config.get"}
```

### `config.set`

Changes the settings that are given and leaves the others alone. The change is checked and written to the configuration file, comments kept. A change to `[service]` takes effect at once; a change to `[control]` takes effect when the service next starts. A change to `[sip]` or `[media]` takes effect at once for the calls that follow, except that turning SIP on or off, and changing the listener's address, the RTP ports or the capture directory, take effect when the service next starts. The result says whether a restart is needed.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `service` | ServiceChange | no | Changes to `[service]`. |
| `control` | ControlChange | no | Changes to `[control]`. |
| `sip` | SipChange | no | Changes to `[sip]`. |
| `media` | MediaChange | no | Changes to `[media]`. |

Result: SettingsResult.

Errors: `RMD0011`, `RMD0014`.

```json
{"id":1,"method":"config.set","params":{"service":{"log_level":"debug","log_dir":""},"control":{"pipe":"rustmodem-control"},"sip":{"enabled":true,"listen":"0.0.0.0:5060","address":"","user_agent":"","trusted_peers":["10.10.20.0/24"],"outbound_peer":"10.10.20.11","ring_timeout_s":60},"media":{"codecs":["pcmu","pcma"],"ptime_ms":20,"rtp_ports":"20000-20999","rtp_timeout_s":10,"capture_dir":""}}}
```

### `stats.get`

Reports the statistics of the service since it started.

Parameters: none.

Result: Stats.

```json
{"id":1,"method":"stats.get"}
```

### `log.tail`

Returns the last lines the service logged, oldest first. The service remembers the last 2000.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `last` | integer | no | Return at most this many, the most recent. Default all. |

Result: LogTail.

```json
{"id":1,"method":"log.tail","params":{"last":100}}
```

### `ports.list`

Lists the serial ports of the machine the service runs on, in name order, with the com0com pairs, the modem devices attached to them and the lines that use them.

Parameters: none.

Result: PortList.

```json
{"id":1,"method":"ports.list"}
```

### `sip.status`

Reports the SIP listener, the peers, and the registration of every line that registers.

Parameters: none.

Result: SipStatus.

```json
{"id":1,"method":"sip.status"}
```

### `sip.reregister`

Registers a line again now, or every line that registers. A line whose registration had failed does not wait out its back-off. The outcome comes as `sip.registration` events.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | no | The line to register again. Default every line that registers. |

Result: Done.

Errors: `RMD1001`.

```json
{"id":1,"method":"sip.reregister","params":{"id":3}}
```

### `huntgroups.list`

Lists the hunt groups, in the order of the configuration file.

Parameters: none.

Result: HuntGroupList.

```json
{"id":1,"method":"huntgroups.list"}
```

### `huntgroups.add`

Adds a hunt group. The change is written to the configuration file and takes effect for the calls that follow. The result is the group.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | The name, not in use. |
| `pilot` | string | yes | The number that calls the group, not the pilot of another. |
| `lines` | array of integer | yes | The lines of the group, in hunting order. |
| `strategy` | string | no | How the group chooses among its idle lines: `first-idle`, `round-robin` or `least-recent`. Default `first-idle`. |

Result: HuntGroupInfo.

Errors: `RMD0011`, `RMD0014`, `RMD0027`, `RMD0028`, `RMD0029`, `RMD0033`.

```json
{"id":1,"method":"huntgroups.add","params":{"name":"dialup","pilot":"5551000","lines":[1,2,3],"strategy":"round-robin"}}
```

### `huntgroups.edit`

Changes the keys of a hunt group that are given and leaves the others alone. When the group is renamed, the routes to it follow. The change is written to the configuration file. The result is the group afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | The group to change. |
| `new_name` | string | no | A new name. |
| `pilot` | string | no | A new pilot number. |
| `lines` | array of integer | no | The lines of the group, all of them, in hunting order. |
| `strategy` | string | no | `first-idle`, `round-robin` or `least-recent`. |

Result: HuntGroupInfo.

Errors: `RMD0011`, `RMD0014`, `RMD0027`, `RMD0028`, `RMD0029`, `RMD0032`.

```json
{"id":1,"method":"huntgroups.edit","params":{"name":"dialup","new_name":"modems","pilot":"5552000","lines":[3,2,1],"strategy":"least-recent"}}
```

### `huntgroups.remove`

Removes a hunt group. A group that a route sends calls to cannot be removed: change or remove the route first. The change is written to the configuration file.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | The name of the group. |

Result: Done.

Errors: `RMD0011`, `RMD0030`, `RMD0032`.

```json
{"id":1,"method":"huntgroups.remove","params":{"name":"dialup"}}
```

### `routes.list`

Lists the routes, in the order they are tried. A call to a number takes the first route that matches it.

Parameters: none.

Result: RouteList.

```json
{"id":1,"method":"routes.list"}
```

### `routes.add`

Adds a route. The change is written to the configuration file and takes effect for the calls that follow. The result is every route afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `called` | string | yes | The called number the route is for: exact, or a prefix ending in `*`. |
| `target` | string | yes | Where its calls go: `line:<id>` or `group:<name>`. |
| `index` | integer | no | Where in the list the route goes, from 0. Default at the end. |

Result: RouteList.

Errors: `RMD0011`, `RMD0014`, `RMD0030`, `RMD0034`.

```json
{"id":1,"method":"routes.add","params":{"called":"555*","target":"group:dialup","index":0}}
```

### `routes.edit`

Changes the keys of a route that are given and leaves the others alone. The change is written to the configuration file. The result is every route afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `index` | integer | yes | Where the route is in the list, from 0. |
| `called` | string | no | A new called number. |
| `target` | string | no | A new target. |

Result: RouteList.

Errors: `RMD0011`, `RMD0014`, `RMD0030`, `RMD0034`.

```json
{"id":1,"method":"routes.edit","params":{"index":0,"called":"5551234","target":"line:3"}}
```

### `routes.remove`

Removes a route. The change is written to the configuration file. The result is every route afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `index` | integer | yes | Where the route is in the list, from 0. |

Result: RouteList.

Errors: `RMD0011`, `RMD0034`.

```json
{"id":1,"method":"routes.remove","params":{"index":0}}
```

### `routes.move`

Moves a route to another place in the list. The change is written to the configuration file. The result is every route afterwards.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `index` | integer | yes | Where the route is in the list, from 0. |
| `to` | integer | yes | Where it goes: its index once it has been moved. |

Result: RouteList.

Errors: `RMD0011`, `RMD0034`.

```json
{"id":1,"method":"routes.move","params":{"index":2,"to":0}}
```

### `subscribe`

Starts sending the client events. The result is everything the client is now subscribed to.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `events` | array of string | no | The event names to add. Default every event. |

Result: Subscription.

```json
{"id":1,"method":"subscribe","params":{"events":["line.state","call.ended"]}}
```

### `unsubscribe`

Stops sending the client events. The result is everything the client is still subscribed to.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `events` | array of string | no | The event names to remove. Default every event. |

Result: Subscription.

```json
{"id":1,"method":"unsubscribe","params":{"events":["log"]}}
```

## Events

| Event | Data | When |
|---|---|---|
| `line.state` | LineInfo | A line changed state, or was added. The data is the whole line, with its counters and any call in progress. |
| `call.started` | CallEvent | A line dialled, or a call was offered to a line. |
| `call.connected` | CallEvent | The modems of a call connected. |
| `call.ended` | CallRecord | A call ended. |
| `log` | LogEvent | A line was written to the service log, at the service's log level. |
| `sip.registration` | SipRegistration | A line's registration changed state. |
| `sip.trace` | SipTrace | A SIP message was sent or received. Sent only to a client that names `sip.trace` in `subscribe`: subscribing to every event leaves it out. Like `log`, it may be dropped for a client that is slow to read. |

Removing a line sends no event of its own. A client that changes the lines
should list them again afterwards.

## Types

### Hello

The body of the `hello` message each side sends first.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `protocol` | integer | yes | The protocol number the sender speaks. |
| `version` | string | yes | The sender's version, for people. |

### Error

An `RMD` message.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `msgid` | string | yes | The message id: `RMD` and four digits. |
| `text` | string | yes | The first-level text. |
| `cause` | string | yes | The second-level text: the cause. |
| `recovery` | string | yes | The second-level text: the recovery. |

### ServiceInfo

The result of `service.info`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `version` | string | yes | The service version. |
| `protocol` | integer | yes | The protocol number the service speaks. |
| `pipe` | string | yes | The name of the control pipe. |
| `config_path` | string | yes | The path of the configuration file. |
| `lines` | integer | yes | How many lines the service has. |
| `started_ms` | integer | yes | When the service started, in milliseconds since the Unix epoch. |
| `uptime_ms` | integer | yes | How long the service has been running, in milliseconds. |
| `config_loaded_ms` | integer | yes | When the configuration was last loaded, in milliseconds since the Unix epoch. |
| `mode` | ServiceMode | yes | How the service is being run. |

### LineList

The result of `lines.list`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `lines` | array of LineInfo | yes | Every line, in id order. |

### LineInfo

Everything about one line.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The line id. |
| `name` | string | no | The line's name, if it has one. |
| `backend` | string | yes | The serial backend, for example `com0com`. |
| `port` | string | yes | The serial port, for example `CNCB0`. |
| `enabled` | boolean | yes | Whether the line is in service by configuration. |
| `state` | LineState | yes | The state of the line. |
| `auto_answer_rings` | integer | yes | The configured initial S0. |
| `unknown_command` | string | yes | What the line answers to an unknown AT command: `ok` or `error`. |
| `counters` | Counters | yes | The counters of the line. |
| `call` | CallInfo | no | The call in progress, if there is one. |
| `error` | Error | no | Why the serial port is not open, if it should be and is not. |
| `binding` | string | no | What the host side of the line's port is bound to, for example `Standard 56000 bps Modem on COM4`. Absent when unknown. |
| `engine` | string | yes | The modem engine of the line: `null` or `echo`. |
| `hunt_groups` | array of string | yes | The names of the hunt groups the line is in. |
| `sip` | LineSip | yes | The SIP side of the line. |
| `capture` | boolean | yes | Whether the line's calls are captured. |

### LineSip

The SIP side of a line.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `mode` | SipMode | yes | How the line is reached. |
| `extension` | string | no | The extension the line registers as. |
| `server` | string | no | The registrar, as `host` or `host:port`. |
| `auth_user` | string | no | The user name the line authenticates with, when it is not the extension. |
| `expires_s` | integer | no | The registration time the line asks for, in seconds. |
| `registration` | Registration | no | The registration, for a line that registers and is in service. |
| `has_password` | boolean | yes | Whether a password is configured. The password itself is never reported. |

### Registration

A line's registration as it stands.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `state` | RegistrationState | yes | The state. |
| `expires_in_s` | integer | no | The seconds left of the binding, when registered. |
| `error` | Error | no | Why the registration failed, when it has. |

### MediaInfo

The RTP of a call: what was agreed and what has happened. The counts of a call in progress are as of a moment ago.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `codec` | string | yes | The codec: `PCMU` or `PCMA`. |
| `ptime_ms` | integer | yes | The packet time in milliseconds. |
| `remote` | string | no | Where the far end receives RTP, as `address:port`. |
| `rx_packets` | integer | yes | Packets delivered from the far end. |
| `rx_octets` | integer | yes | Payload octets delivered from the far end. |
| `tx_packets` | integer | yes | Packets sent to the far end. |
| `tx_octets` | integer | yes | Payload octets sent to the far end. |
| `lost` | integer | yes | Packets from the far end that never arrived in time. |
| `late` | integer | yes | Packets that arrived after their place had been filled. |
| `duplicate` | integer | yes | Packets that had been delivered already. |
| `resync` | integer | yes | Times the far end's stream started again. |
| `ignored` | integer | yes | Packets of another payload type, or not of whole frames. |
| `free_run` | integer | yes | Frames sent on the local timer before the far end's first packet. |
| `slips` | integer | yes | Points where the frames sent stopped matching the frames taken. There should never be one. |
| `jitter_us_avg` | integer | yes | The interarrival jitter, smoothed as RFC 3550 has it, in microseconds. |
| `jitter_us_max` | integer | yes | The most the jitter has been, in microseconds. |
| `tick_p50_us` | integer | yes | The time a tick takes, in microseconds: the median. |
| `tick_p99_us` | integer | yes | The 99th percentile. |
| `tick_max_us` | integer | yes | The longest. |
| `overruns` | integer | yes | Ticks that took longer than the packet time. |

### Counters

The counters of a line since the service started.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `calls_out` | integer | yes | Calls the line dialled. |
| `calls_in` | integer | yes | Calls offered to the line. |
| `connects` | integer | yes | Calls that connected. |
| `bytes_tx` | integer | yes | Data bytes from the host to the far end. |
| `bytes_rx` | integer | yes | Data bytes from the far end to the host. |
| `at_commands` | integer | yes | AT command lines parsed on the line. |

### CallInfo

A call in progress.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The call id, unique since the service started. |
| `direction` | Direction | yes | Which way the call went. |
| `number` | string | yes | The number dialled, or for an incoming call the caller. |
| `started_ms` | integer | yes | When the call started, in milliseconds since the Unix epoch. |
| `connected_ms` | integer | no | When the call connected, if it has. |
| `connect` | ConnectInfo | no | What is known about the connection, once connected. |
| `bytes_tx` | integer | yes | Data bytes from the host to the far end so far. |
| `bytes_rx` | integer | yes | Data bytes from the far end to the host so far. |
| `media` | MediaInfo | no | The RTP of the call: what was agreed and what has happened. Absent for a call with no media. |

### CallRecord

A call that has ended. The same object is written, one a line, to `calls.jsonl` in the log directory.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | integer | yes | The call id, unique since the service started. |
| `line` | integer | yes | The line the call was on. |
| `direction` | Direction | yes | Which way the call went. |
| `number` | string | yes | The number dialled, or for an incoming call the caller. |
| `started_ms` | integer | yes | When the call started, in milliseconds since the Unix epoch. |
| `connected_ms` | integer | no | When the call connected, if it did. |
| `ended_ms` | integer | yes | When the call ended. |
| `reason` | EndReason | yes | Why the call ended. |
| `connect` | ConnectInfo | no | What is known about the connection, if the call connected. |
| `bytes_tx` | integer | yes | Data bytes from the host to the far end. |
| `bytes_rx` | integer | yes | Data bytes from the far end to the host. |
| `media` | MediaInfo | no | The RTP of the call: what was agreed and what has happened. Absent for a call with no media. |

### ConnectInfo

What is known about a connection. The null engine reports rate 0 and nothing else.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `rate` | integer | yes | The line rate in bit/s; 0 means not known. |
| `carrier` | string | no | The carrier (modulation) name. |
| `protocol` | string | no | The error-control protocol name. |
| `compression` | string | no | The data compression name. |

### CallList

The result of `calls.list`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `calls` | array of CallRecord | yes | The calls that have ended, oldest first. |

### CallEvent

The data of `call.started` and `call.connected`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `line` | integer | yes | The line the call is on. |
| `call` | CallInfo | yes | The call. |

### LogEvent

The data of `log`: one line of the service log.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `time_ms` | integer | yes | When, in milliseconds since the Unix epoch. |
| `level` | LogLevel | yes | How serious. |
| `source` | string | yes | Where from: `Line01`, `CTL`, `SVC`, `POOL`. |
| `text` | string | yes | The text. |

### Reloaded

The result of `config.reload`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `lines` | integer | yes | How many lines the configuration has. |

### Settings

The result of `config.get`: the settings that are not lines.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `service` | ServiceSettings | yes | The `[service]` settings. |
| `control` | ControlSettings | yes | The `[control]` settings. |
| `sip` | SipSettings | yes | The `[sip]` settings. |
| `media` | MediaSettings | yes | The `[media]` settings. |
| `path` | string | yes | The path of the configuration file. |

### SipSettings

The `[sip]` settings.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `enabled` | boolean | yes | Whether the service speaks SIP. |
| `listen` | string | yes | The address of the listener, as `address:port`. |
| `address` | string | yes | The address to advertise. Empty means the local address that routes to the peer. |
| `user_agent` | string | yes | The User-Agent. Empty means `rustmodem/<version>`. |
| `trusted_peers` | array of string | yes | The addresses, or CIDR blocks, that may send requests to the listener. |
| `outbound_peer` | string | yes | Where lines that are not registered send the calls they dial, as `host` or `host:port`. Empty means nowhere. |
| `ring_timeout_s` | integer | yes | How long a line may ring before the caller is turned away, in seconds. |

### MediaSettings

The `[media]` settings.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `codecs` | array of string | yes | The codecs, most preferred first: `pcmu`, `pcma`. |
| `ptime_ms` | integer | yes | The packet time in milliseconds. |
| `rtp_ports` | string | yes | The ports RTP may use, as `first-last`. |
| `rtp_timeout_s` | integer | yes | How long a call may go without a packet before it is cleared, in seconds. |
| `capture_dir` | string | yes | The capture directory. Empty means `captures` under the log directory. |

### SipChange

The `sip` part of `config.set`: the keys of `[sip]` to change.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `enabled` | boolean | no | Whether the service speaks SIP. |
| `listen` | string | no | A new address for the listener, as `address:port`. |
| `address` | string | no | A new address to advertise. An empty string means the local address that routes to the peer. |
| `user_agent` | string | no | A new User-Agent. An empty string means `rustmodem/<version>`. |
| `trusted_peers` | array of string | no | The trusted peers, all of them: addresses or CIDR blocks. |
| `outbound_peer` | string | no | A new outbound peer, as `host` or `host:port`. An empty string means none. |
| `ring_timeout_s` | integer | no | How long a line may ring, in seconds. |

### MediaChange

The `media` part of `config.set`: the keys of `[media]` to change.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `codecs` | array of string | no | The codecs, all of them, most preferred first: `pcmu`, `pcma`. |
| `ptime_ms` | integer | no | A new packet time in milliseconds: 10, 20 or 30. |
| `rtp_ports` | string | no | The ports RTP may use, as `first-last`. |
| `rtp_timeout_s` | integer | no | How long a call may go without a packet, in seconds. |
| `capture_dir` | string | no | A new capture directory. An empty string means the default one. |

### RegisterSpec

The `register` part of `lines.add`: the line is an extension of a PBX.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `server` | string | yes | The registrar, as `host` or `host:port`. |
| `user` | string | yes | The extension. |
| `password` | string | yes | The password. It is written to the configuration file and never reported. |
| `auth_user` | string | no | The user name to authenticate with. Default the extension. |
| `expires_s` | integer | no | The registration time to ask for, in seconds. Default 300. |

### RegisterChange

The `register` part of `lines.edit`: the keys of the line's registration to change. For a line that has no registration, `server`, `user` and `password` must all be given.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `server` | string | no | A new registrar. An empty string removes the registration: the line is reached by the trunk. |
| `user` | string | no | A new extension. |
| `password` | string | no | A new password. Left out, the password stays as it is. |
| `auth_user` | string | no | A new user name to authenticate with. An empty string means the extension. |
| `expires_s` | integer | no | A new registration time to ask for, in seconds. |

### ServiceSettings

The `[service]` settings.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `log_level` | LogLevel | yes | How much the service logs. |
| `log_dir` | string | yes | The log directory. Empty means the default one. |

### ControlSettings

The `[control]` settings.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `pipe` | string | yes | The control pipe name. |

### ServiceChange

The `service` part of `config.set`: the keys of `[service]` to change.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `log_level` | LogLevel | no | A new log level. |
| `log_dir` | string | no | A new log directory. An empty string means the default one. |

### ControlChange

The `control` part of `config.set`: the keys of `[control]` to change.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `pipe` | string | no | A new control pipe name. |

### SettingsResult

The result of `config.set`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `restart_required` | boolean | yes | Whether the service must be restarted for the change to take effect. |
| `config` | Settings | yes | The settings as the configuration file now has them. |

### Stats

The result of `stats.get`: the statistics of the service since it started. The call counts are of calls offered to lines.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `calls_answered` | integer | yes | Calls a line answered. |
| `calls_connected` | integer | yes | Calls whose modems connected. |
| `calls_failed` | integer | yes | Calls answered that never connected. |
| `calls_rejected` | integer | yes | Calls turned away: the line was busy, or no line was ready. |
| `calls_abandoned` | integer | yes | Calls the caller gave up on while the line was ringing. |
| `avg_call_ms` | integer | yes | The mean length of the connected calls that have ended, in milliseconds. |
| `longest_call_ms` | integer | yes | The length of the longest connected call that has ended, in milliseconds. |
| `peak_lines_in_use` | integer | yes | The most lines that had a call at one time. |
| `lines_in_use` | integer | yes | How many lines have a call now. |
| `bytes_in` | integer | yes | Data bytes from the far ends to the hosts. |
| `bytes_out` | integer | yes | Data bytes from the hosts to the far ends. |
| `at_commands` | integer | yes | AT command lines parsed. |
| `connects` | array of ConnectTally | yes | The connected calls by what they negotiated, largest count first. |
| `retrains` | integer | yes | Retrains and rate renegotiations. Zero until an engine reports them. |
| `protocols` | array of Tally | yes | The connected calls by error-control protocol, largest count first. Empty until an engine reports one. |
| `compressions` | array of Tally | yes | The connected calls by data compression, largest count first. Empty until an engine reports one. |
| `rtp_kbps` | integer | yes | RTP in both directions over the last second or so, headers included, in kbit/s. |
| `jitter_ms_avg` | number | yes | The mean interarrival jitter of the calls in progress, in milliseconds. |
| `jitter_ms_max` | number | yes | The most the jitter of a call in progress has been, in milliseconds. |
| `packet_loss_percent` | number | yes | The packets lost, in a hundred expected, over every call since the service started. |
| `codec` | string | yes | The codec the service prefers: `PCMU` or `PCMA`. Empty when SIP is off. |
| `ptime_ms` | integer | yes | The packet time the service asks for, in milliseconds. 0 when SIP is off. |
| `cpu_percent` | number | yes | How much of one processor the service process has used over the last second or so, in a hundred. |
| `sip` | SipSummary | yes | SIP in a line of numbers. |

### SipSummary

SIP in a line of numbers.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `listener` | ListenerState | yes | The state of the listener. |
| `registered` | integer | yes | How many lines are registered. |
| `registering` | integer | yes | How many lines in service have a registration configured. |
| `dropped_untrusted` | integer | yes | Requests dropped because they came from an address that is not trusted. |

### SipStatus

The result of `sip.status`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `listener` | ListenerState | yes | The state of the listener. |
| `listen` | string | yes | The address the listener is bound to, as `address:port`. Empty when SIP is off. |
| `trusted_peers` | array of string | yes | The addresses that may send requests to the listener. |
| `outbound_peer` | string | yes | Where lines that are not registered send the calls they dial. Empty means nowhere. |
| `registrations` | array of SipRegistration | yes | Every line that registers, in id order. |
| `dropped_untrusted` | integer | yes | Requests dropped because they came from an address that is not trusted. |
| `calls` | integer | yes | Calls the router has: ringing, connected or being cleared. |

### SipRegistration

One line's registration, with where it registers.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `line` | integer | yes | The line. |
| `server` | string | yes | The registrar, as the configuration names it. |
| `user` | string | yes | The extension. |
| `registration` | Registration | yes | The registration as it stands. |

### SipTrace

A SIP message that passed.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `time_ms` | integer | yes | When, in milliseconds since the Unix epoch. |
| `direction` | TraceDirection | yes | Which way it went. |
| `peer` | string | yes | The other end, as `address:port`. |
| `text` | string | yes | The message as it was on the wire. The response to a digest challenge is in it; the password is not. |

### HuntGroupList

The result of `huntgroups.list`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `groups` | array of HuntGroupInfo | yes | The hunt groups, in the order of the configuration file. |

### HuntGroupInfo

A hunt group: lines that share a number.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | The name, unique. |
| `pilot` | string | yes | The number that calls the group. |
| `lines` | array of integer | yes | The lines of the group, in hunting order. |
| `strategy` | string | yes | How the group chooses among its idle lines: `first-idle`, `round-robin` or `least-recent`. |

### RouteList

The result of `routes.list` and of every change to the routes.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `routes` | array of RouteInfo | yes | The routes, in the order they are tried. |

### RouteInfo

A route: where the calls to a number go.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `index` | integer | yes | Where the route is in the list, from 0. |
| `called` | string | yes | The called number the route is for: exact, or a prefix ending in `*`. |
| `target` | string | yes | Where its calls go: `line:<id>` or `group:<name>`. |

### ConnectTally

How many connected calls negotiated one rate and carrier.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `rate` | integer | yes | The line rate in bit/s; 0 means not known. |
| `carrier` | string | no | The carrier (modulation) name, if the engine reported one. |
| `count` | integer | yes | How many calls. |

### Tally

How many connected calls negotiated something with a name.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | The name. |
| `count` | integer | yes | How many calls. |

### LogTail

The result of `log.tail`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `lines` | array of LogEvent | yes | The lines, oldest first. |

### PortList

The result of `ports.list`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `ports` | array of PortInfo | yes | The serial ports of the machine, in name order. |

### PortInfo

One serial port of the machine the service runs on.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | The name the port is opened by, for example `CNCB0`. |
| `kind` | PortKind | yes | What kind of port it is. |
| `pair` | string | no | For com0com, the name of the other end of the pair. |
| `host_port` | string | no | The COM name applications see for the host side of the port. |
| `modem` | string | no | The friendly name of a modem device attached to the host port. |
| `line` | integer | no | The id of the line that uses the port. |

### Subscription

The result of `subscribe` and `unsubscribe`.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `events` | array of string | yes | The events the client is now subscribed to. |

### Done

The result of a method that has nothing to report: an empty object.

## Values

### LineState

The state of a line. A string, one of:

| Value | Meaning |
|---|---|
| `disabled` | The line is out of service by configuration. |
| `not_ready` | The serial port is closed, or the host has DTR off. |
| `idle` | In service, with no call. |
| `ringing` | A call is being offered to the line. |
| `handshaking` | A call is being set up. |
| `online` | A call is connected. |

### Direction

Which way a call went. A string, one of:

| Value | Meaning |
|---|---|
| `outgoing` | The line dialled out. |
| `incoming` | The call was offered to the line. |

### EndReason

Why a call ended. A string, one of:

| Value | Meaning |
|---|---|
| `hangup` | The host hung up with an `H` command. |
| `dtr` | The host dropped DTR. |
| `aborted` | The host sent a character while the call was being set up. |
| `no_answer` | Nothing answered in time. |
| `busy` | The number dialled was busy. |
| `no_dialtone` | There was no dial tone. |
| `remote` | The far end cleared the call, or the carrier was lost. |
| `abandoned` | The caller gave up before the line answered. |
| `failed` | The call could not be set up or carried on: the number is not a line in service, the line called went away, or the line's serial port failed. |
| `reset` | The host reset the modem with a `Z` command. |
| `operator` | The operator hung the line up, reset it, disabled it or removed it. |
| `shutdown` | The service stopped. |

### LogLevel

How serious a log line is. A string, one of:

| Value | Meaning |
|---|---|
| `error` | Something failed. |
| `warning` | Something is wrong but the service went on. |
| `info` | Something happened. |
| `debug` | Detail, including every AT command line and result code. |

### ServiceMode

How the service is being run. A string, one of:

| Value | Meaning |
|---|---|
| `service` | Under the Windows service control manager. |
| `console` | In the foreground, with `--console`. |
| `demo` | The demo service: memory ports and scripted calls. |

### PortKind

What kind of serial port a port is. A string, one of:

| Value | Meaning |
|---|---|
| `com0com` | One end of a com0com null-modem pair. |
| `serial` | A serial port with a `COM` name. |
| `other` | Anything else the system lists as a serial device. |

### SipMode

How a line is reached over SIP. A string, one of:

| Value | Meaning |
|---|---|
| `none` | Not at all: the service does not speak SIP. |
| `trunk` | By the trunk: calls come from the trusted peers and go to the outbound peer. |
| `registered` | As an extension of a PBX the line registers to. |

### RegistrationState

The state of a line's registration. A string, one of:

| Value | Meaning |
|---|---|
| `unregistered` | Not registered, and not trying. |
| `registering` | A REGISTER is out and there is no binding yet. |
| `registered` | The registrar holds a binding for the line. |
| `failed` | The last try failed. Another follows. |

### ListenerState

The state of the SIP listener. A string, one of:

| Value | Meaning |
|---|---|
| `off` | SIP is off. |
| `listening` | The listener is bound and takes requests. |
| `failed` | SIP is on in the configuration and the listener could not be bound. |

### TraceDirection

Which way a traced SIP message went. A string, one of:

| Value | Meaning |
|---|---|
| `sent` | The service sent it. |
| `received` | The service received it. |

## Errors

The errors the API returns, from the message catalogue
(`service/msg/messages.toml`). `&1`, `&2` and so on stand for the values the
service fills in.

| Id | Text | Cause | Recovery |
|---|---|---|---|
| `RMD0010` | Configuration file &1 could not be read: &2 | The file does not exist or the service is not allowed to read it. | Create the file or correct its permissions, then try again. |
| `RMD0011` | Configuration file &1 could not be written: &2 | The service is not allowed to write the file, or the disk is full. | Correct the permissions of the file and its directory, then repeat the change. |
| `RMD0012` | Configuration is not valid TOML at line &1: &2 | The configuration file could not be parsed. | Correct the file at the line given and try again. |
| `RMD0013` | Key &1 at line &2 has the wrong type; &3 expected. | The value given for the key is not of the type the key takes. | Correct the value and try again. |
| `RMD0014` | Key &1 at line &2 has value &3, which is not allowed. | The value is outside what the key accepts. Allowed: &4. | Correct the value and try again. |
| `RMD0015` | Required key &1 is missing from the table at line &2. | The table must have the key and it has no default. | Add the key and try again. |
| `RMD0016` | Line id &1 at line &2 is already used at line &3. | Two lines have the same id. Every line needs its own id. | Give one of the lines a different id and try again. |
| `RMD0017` | Serial port &1 at line &2 is already used at line &3. | Two lines name the same serial port. A port can serve one line only. | Give one of the lines a different port and try again. |
| `RMD0018` | Serial backend &1 at line &2 is not known. | The backend named for the line is not one this version of rustmodem has. Known backends: com0com. | Correct the backend name and try again. |
| `RMD0019` | Line &1 is not in the configuration. | The change names a line id that the configuration does not have. | List the lines to see which ids exist, then repeat the change with one of them. |
| `RMD0020` | Line &1 is already in the configuration. | A line cannot be added with an id that the configuration already has. | Choose an id that is not in use, or edit the existing line. |
| `RMD0027` | Hunt group &1 at line &2 names line &3, which does not exist. | A hunt group can only hold lines that the configuration has. | Add the line, or take it out of the hunt group, and try again. |
| `RMD0028` | Hunt group name &1 at line &2 is already used at line &3. | Two hunt groups have the same name. Routes name a group by its name, so every group needs its own. | Give one of the hunt groups a different name and try again. |
| `RMD0029` | Pilot number &1 at line &2 is already used at line &3. | Two hunt groups have the same pilot number. A call to the number can go to one group only. | Give one of the hunt groups a different pilot number and try again. |
| `RMD0030` | Route &1 at line &2 has target &3, which does not exist. | A route sends its calls to a line or to a hunt group that the configuration has. | Correct the target, or add the line or the hunt group, and try again. |
| `RMD0032` | Hunt group &1 is not in the configuration. | A change was asked for to a hunt group that the configuration does not have. | List the hunt groups with: rustmodemctl groups |
| `RMD0033` | Hunt group &1 is already in the configuration. | A hunt group was to be added, or renamed, with a name that another hunt group has. | Choose a name that is not in use, or change the group that has it. |
| `RMD0034` | There is no route at position &1: the configuration has &2 routes. | A route is named by its position in the list, from 0. The routes may have been changed since they were listed. | List the routes again with: rustmodemctl routes |
| `RMD0035` | The registration of line &1 is not complete. | A line that registers to a PBX needs the server, the user and the password. One of them was not given and the line has none. | Give the server, the user and the password together. |
| `RMD1001` | Line &1 does not exist. | The request names a line id that the service does not have. | List the lines to see which ids exist, then repeat the request with one of them. |
| `RMD1002` | Line &1 is disabled. | The request needs the line to be enabled. | Enable the line and repeat the request. |
| `RMD1003` | Line &1 is not ringing; its state is &2. | A line can be answered only while a call is ringing on it. | Wait for the line to ring, then repeat the request. |
| `RMD1004` | Line &1 has no call to hang up. | The line is neither connecting nor connected. | No action is needed. |
| `RMD2001` | Control protocol &1 is not supported; this service supports protocol &2. | The client and the service were built for different versions of the control protocol. | Use a client of the same version as the service. |
| `RMD2002` | Control method &1 is not known. | The request names a method that this version of the service does not have. | Correct the method name. The methods are listed in docs/CONTROL_API.md. |
| `RMD2003` | Parameters of control method &1 are not valid: &2 | A parameter is missing, has the wrong type, or is not one the method takes. | Correct the parameters. They are listed for each method in docs/CONTROL_API.md. |
| `RMD2004` | Control message is not valid: &1 | The message is not one JSON object on one line, or it is missing the fields a request needs. | Send each request as one JSON object with id and method, ended by a line feed. |
| `RMD2005` | The first control message must be hello. | A client sent something else before saying hello. | Send hello with the protocol number and client version first. |
| `RMD2010` | The service is stopping. | The request arrived while the service was shutting down, so it was not carried out. | Start the service and repeat the request. |
| `RMD2014` | Control message is longer than &1 bytes. | A control message must fit on one line within the size limit. | Send a shorter message. |
