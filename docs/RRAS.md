# Answering dial-up calls with Routing and Remote Access

From a working RustModem service to an answered PPP call: a Windows client
behind a modem dials a number, the call arrives over SIP, a RustModem line
answers it, and Routing and Remote Access (RRAS) on the same machine runs
PPP over that line's COM port as it would over a hardware modem.

This is for lines on RustModem ports, which is what the setup program and
the manager make. Each such port comes with its modem, so nothing is
installed by hand: no modem is added in Phone and Modem, and no port is
wired. Lines on com0com pairs work as before, with one more step, which
is at the end. Nothing here is specific to a version of Windows Server,
but the names of the dialogs differ a little between versions.

**Not yet done on a real machine.** RustModem's modem has been installed
and listed by Windows on a test machine, and has answered every command
Windows is known to send it. Routing and Remote Access has not yet
answered a call on one: criteria 10 and 11 of `docs/PHASE5.md`.

> **NEEDS-CODY: confirm wording.** The steps in Windows below were written
> without the dialogs in front of the writer. Each place where the exact
> wording of a button, a list entry or a page is not certain is marked.
> The order of the steps and what each must achieve are certain.

## Before you start

- [ ] RustModem is installed and its service running, with a line for
      every modem RRAS is to have, each on a RustModem port.
      `docs/INSTALL.md`.
- [ ] `rustmodem-ctl ports` lists each line's port with its modem, and
      Device Manager shows each under "Modems" with no warning.
- [ ] Calls arrive: `[sip]` is set up, the gateway sends the pilot number to
      a hunt group of these lines, and `rustmodem-ctl watch` shows a line
      ring when the number is dialled. `docs/TELEPHONY.md` and
      `docs/GATEWAYS.md`.
- [ ] A call connects at 300 bit/s from a real modem, as the manual
      criteria of phase 3 ask. Do not debug RRAS on a path where a plain
      terminal call is unproven.

In this guide line 1 is on the port `COM10`, which is the first that
setup makes. Use the names of your own ports.

## 1. The lines' settings

RRAS and the Windows modem driver send their own commands, answer for
themselves, and want error control. The defaults do that. What matters:

```toml
[modem]
modulations = ["v22bis", "v22", "bell212a", "v21", "bell103"]
error_control = "auto"
compression = true

[[line]]
id = 1
serial = { backend = "vcom", port = "COM10" }
auto_answer_rings = 0
unknown_command = "ok"
```

| Key | Value | Why |
|---|---|---|
| `auto_answer_rings` | `0` | The modem driver answers with `ATA` when RRAS tells it to; a line that answered by itself would answer calls RRAS is not ready for |
| `unknown_command` | `ok` | A driver's initialisation has commands for hardware this modem has not got: speaker, pulse dialling, vendor registers. With `ok` they are skipped; with `error` the driver takes the modem for broken |
| `error_control` | `auto` | The driver sets what it wants with `\N` or `&Q`, and those work. `auto` is what the line does until it has |

## 2. The modems

They are there already. Each RustModem port was made with a modem,
"RustModem Softmodem", attached to it, and Windows was told by the
modem's own INF what to send it and what it answers.

1. Open **Phone and Modem** in Control Panel. If it asks for a location,
   give a country and an area code; they are not used.
2. The **Modems** tab lists a RustModem Softmodem for each port, the
   second and later ones with a number after the name.
3. Select one, choose **Properties**, and on the **Diagnostics** tab
   **Query Modem**. Windows sends the modem its identification commands
   and lists the answers: the manufacturer is Quartz Systems and the
   model RustModem Softmodem. NEEDS-CODY: confirm wording.

Leave each modem's properties alone. In particular leave **Extra
initialization commands** on the **Advanced** tab empty, and leave the
maximum port speed as it is: the port has no speed, and the rate of a call
is what the far end and the line's `modulations` make it.

Check: `rustmodem-ctl ports` lists each port with its modem, and
`rustmodem-ctl lines` shows each line's binding as its modem's name. The
lines are `not_ready` still: nothing has their ports open yet.

## 3. Routing and Remote Access

If the role is not installed:

1. In **Server Manager** choose **Add roles and features**.
2. Choose the role **Remote Access**, and in it the role service
   **DirectAccess and VPN (RAS)**. Accept the features it asks for.
   NEEDS-CODY: confirm wording; on older versions the role is **Network
   Policy and Access Services** with the role service **Routing and Remote
   Access Services**.

Then:

1. Open **Routing and Remote Access** from Tools in Server Manager.
2. Right-click the server and choose **Configure and Enable Routing and
   Remote Access**.
3. Choose **Remote access (dial-up or VPN)**, and on the next page tick
   **Dial-up** only.
4. For addresses choose **From a specified range of addresses**, and give
   a range that is not otherwise in use on the network the callers are to
   reach, with at least one address more than there are lines: the server
   takes the first.
5. Choose **No, use Routing and Remote Access to authenticate connection
   requests**, unless you have a RADIUS server.
6. Finish. The service starts.

If RRAS was already configured for something else, the same settings are
in the server's **Properties**: **Remote access server** on the **General**
tab, the address range on the **IPv4** tab under **Static address pool**.

## 4. The ports

RRAS does not answer on a modem until its port is enabled for inbound
connections.

1. In **Routing and Remote Access**, right-click **Ports** and choose
   **Properties**.
2. Select the RustModem Softmodem on `COM10` in the list and choose
   **Configure...**.
3. Tick **Remote access connections (inbound only)**. Leave demand-dial
   routing unticked.
4. Repeat for each modem, and **OK**.

**Ports** now lists each modem with the status **Inactive**. RRAS has
opened the port: `rustmodem-ctl lines` shows the line `idle` where it
was `not_ready`, and the service's log at `debug` shows the initialisation
the driver sent.

## 5. Who may call

1. In the server's **Properties**, on the **Security** tab, choose
   **Authentication Methods...** and tick what the callers' systems can
   do. **MS-CHAP v2** is what a client from Windows 98 on does. Older
   clients need **CHAP** or **MS-CHAP**, which newer servers offer only
   after a registry change or not at all. NEEDS-CODY: confirm which the
   server version in use offers, and what the test client needs.
2. Give a user the right to dial in: in the user's **Properties**, on the
   **Dial-in** tab, choose **Allow access**. On a domain where access is
   by policy, add a network policy in **Network Policy Server** that
   grants access to a group over the NAS port type **Async (Modem)**.
   NEEDS-CODY: confirm wording.

## 6. A client

On the client PC, with its modem on the line that reaches the pilot number:

1. Create a dial-up connection to the pilot number, with the user name and
   password of the user above.
2. For the first call, cap the client's modem at 2400 bit/s, so that the
   two modems agree on V.22bis without trying anything faster first. On a
   Courier that is an initialisation command. NEEDS-CODY: the command;
   `AT&N3` fixes the link rate at 2400 bit/s on a Courier of the V.34 era.
3. Dial.

What should happen, and where to see it:

| Step | Where |
|---|---|
| The call arrives and a line rings | `rustmodem-ctl watch`; the manager's grid; `RING` in the log at `debug` |
| RRAS answers | `ATA` in the log; the line goes to `handshaking` |
| The modems connect | The line goes to `online`, with `2400`, `V.22bis`, `LAPM` and `V.42bis`; `rustmodem-ctl diag 1` and the manager's Line diagnostics show a constellation of sixteen tight points |
| PPP comes up | **Ports** in RRAS shows the port **Active**; **Remote Access Clients** lists the user |
| The client has an address | `ipconfig` on the client; `ping` to the server's address in the pool |

Then transfer a file of a megabyte each way and compare checksums, and
leave the session up for half an hour: the manual criteria of
`docs/PHASE4.md` have the commands.

## The initialisation strings

The service logs every AT command line a host sends, and the modem's
answer, at `debug`. Set the log level in the manager's Settings, which
takes effect at once, or set `log_level = "debug"` under `[service]` in
the configuration file and restart the service. `rustmodem-ctl log` prints
the end of the log.

NEEDS-CODY: after the first call that works, take the lines of that line
from the log, from the driver's first `> AT` to the call's `< CONNECT`,
and add them to the repository as they are, in a file under
`dte/at/tests/hostlogs/`. `README.md` there has the format, which is the
log's own, and `cargo test -p at --test hostlogs` replays every file in
the folder: each command must be answered as the log says it was, and
none with `ERROR`.

What Windows sends a RustModem Softmodem is no longer a guess: it is in
the modem's INF, which is generated by `driver/modeminf` from the AT
interpreter's own tables. It initialises the modem with
`AT&FE0V1X4&C1&D2S0=0+MR=1;+ER=1;+DR=1`, so that the modem reports the
modulation, the error control and the compression of each call, and the
INF's table of responses has the line rates, so that RRAS shows the rate
the modems agreed on and not a port's. Transcript 61 in
`dte/at/tests/transcripts/` is what Query Modem sends.

`standard-modem.txt` in the folder of host logs, and transcripts 02, 03
and 43, are the strings of Windows' standard modem types, which a line on
a com0com pair still meets. They were written from memory. Every command
in them has defined behaviour in `docs/AT.md`, and the log is replayed
with unknown commands refused, so that none of them passes by being
skipped.

## When it does not work

| What is seen | Likely cause |
|---|---|
| The modem is not in the list of **Ports** | The port has no modem: Ports and drivers in the manager shows it as "No modem"; remove the port and add it again. Or RRAS was configured before the ports were made: restart the Routing and Remote Access service |
| The line shows `not_ready` | Nothing has the port open. RRAS opens it when the port is enabled for inbound connections: step 4 |
| RRAS cannot open the port | Something else has it: a terminal left open. A RustModem port has one host at a time. `docs/DRIVER.md`, "Recovering a stuck port" |
| RRAS shows a rate that is the port's and not the call's | The modem is not a RustModem Softmodem, or its INF's responses were not matched: send the line's log of the call at `debug` |
| The line rings and nobody answers | The port is not enabled for inbound connections; or the driver's initialisation failed: look for `ERROR` in the log, and set `unknown_command = "ok"` |
| `NO CARRIER` after some seconds of tones | The modems did not agree. `rustmodem-ctl diag` during the call shows where it stopped; RMD7001 in the log names the state. Try the client capped at 1200 bit/s, and then at 300 |
| `NO CARRIER` straight after the modems connect | `error_control = "required"`, or the driver's `\N2` or `\N4`, and a client with error control off. RMD8001 in the log says so |
| `CONNECT`, and then PPP fails | Authentication: step 5. The RRAS log, and the client's error number, say which side refused |
| The call drops under load | Look at the RTP statistics of the line's call, `rustmodem-ctl line 1`: lost packets, and slips. LAPM rides out lost packets, but a path that loses many of them or changes the audio is the gateway's to fix: `docs/GATEWAYS.md` |

## If the lines are on com0com pairs

A line on a com0com pair has no modem of its own. Windows must be given
one on the host's port of each pair, by hand, once, and then everything
above applies with that port's name in place of `COM10`.

1. Wire the pair as `docs/COM0COM.md` says.
2. Open **Phone and Modem** in Control Panel, and on the **Modems** tab
   choose **Add...**.
3. Tick **Don't detect my modem; I will select it from a list**, and
   **Next**.
4. Under **Manufacturer** choose **(Standard Modem Types)**. Under
   **Models** choose **Standard 56000 bps Modem**.
   NEEDS-CODY: confirm wording, and which standard model works best. The
   models differ in the initialisation they send.
5. Choose **Selected ports**, select the host's port of the pair, and
   **Next**, **Finish**.

`rustmodem-ctl ports` then lists the pair with the modem on its host
port. With a standard modem type RRAS shows the port's rate for a call,
not the line's: Windows' standard INFs do not know RustModem's responses.
