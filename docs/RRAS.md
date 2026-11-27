# Answering dial-up calls with Routing and Remote Access

From a working RustModem service to an answered PPP call: a Windows client
behind a modem dials a number, the call arrives over SIP, a RustModem line
answers it, and Routing and Remote Access (RRAS) on the same machine runs
PPP over that line's COM port as it would over a hardware modem.

This is for the com0com backend, which is the one there is until the
virtual COM driver of phase 5. Nothing here is specific to a version of
Windows Server, but the names of the dialogs differ a little between
versions.

> **NEEDS-CODY: confirm wording.** The steps in Windows below were written
> without the dialogs in front of the writer. Each place where the exact
> wording of a button, a list entry or a page is not certain is marked.
> The order of the steps and what each must achieve are certain.

## Before you start

- [ ] The service is installed and running, with a configuration that has a
      line for every modem RRAS is to have. `docs/INSTALL.md`.
- [ ] Every line has its com0com pair, wired as `docs/COM0COM.md` says, and
      the checklist at the end of that document passes for it.
- [ ] Calls arrive: `[sip]` is set up, the gateway sends the pilot number to
      a hunt group of these lines, and `rustmodem-ctl watch` shows a line
      ring when the number is dialled. `docs/TELEPHONY.md` and
      `docs/GATEWAYS.md`.
- [ ] A call connects at 300 bit/s from a real modem, as the manual
      criteria of phase 3 ask. Do not debug RRAS on a path where a plain
      terminal call is unproven.

In this guide line 1 is on the pair `CNCA0` and `CNCB0`: RustModem has
`CNCB0`, and the host's port `CNCA0` has been given the name `COM8`. Use the
names of your own pairs.

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
serial = { backend = "com0com", port = "CNCB0" }
auto_answer_rings = 0
unknown_command = "ok"
```

| Key | Value | Why |
|---|---|---|
| `auto_answer_rings` | `0` | The modem driver answers with `ATA` when RRAS tells it to; a line that answered by itself would answer calls RRAS is not ready for |
| `unknown_command` | `ok` | A driver's initialisation has commands for hardware this modem has not got: speaker, pulse dialling, vendor registers. With `ok` they are skipped; with `error` the driver takes the modem for broken |
| `error_control` | `auto` | The driver sets what it wants with `\N` or `&Q`, and those work. `auto` is what the line does until it has |

## 2. A modem on each host port

Windows must have a modem device on each host-side port, so that RRAS can
list it.

1. Open **Phone and Modem** in Control Panel. If it asks for a location,
   give a country and an area code; they are not used.
2. On the **Modems** tab choose **Add...**.
3. Tick **Don't detect my modem; I will select it from a list**, and
   **Next**.
4. Under **Manufacturer** choose **(Standard Modem Types)**. Under
   **Models** choose **Standard 56000 bps Modem**.
   NEEDS-CODY: confirm wording, and which standard model works best. Any
   standard model should do: the port has no rate of its own, and the line
   rate is what the far end and the line's `modulations` make it. The
   models differ in the initialisation they send.
5. Choose **Selected ports**, select `COM8`, and **Next**, **Finish**.
6. Repeat for the host port of every line, or select several ports at
   step 5.

Leave each modem's properties alone. In particular leave **Extra
initialization commands** on the **Advanced** tab empty, and leave the
maximum port speed as it is.

Check: `rustmodem-ctl ports` lists each host port with the modem Windows
has on it. `rustmodem-ctl lines` shows the lines `not_ready` still: nothing
has their host ports open yet.

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
2. Select the modem on `COM8` in the list and choose **Configure...**.
3. Tick **Remote access connections (inbound only)**. Leave demand-dial
   routing unticked.
4. Repeat for each modem, and **OK**.

**Ports** now lists each modem with the status **Inactive**. RRAS has
opened the host port: `rustmodem-ctl lines` shows the line `idle` where it
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

`standard-modem.txt` there, and transcripts 02, 03 and 43 in
`dte/at/tests/transcripts/`, are the strings this was written against,
from memory of what the standard modem drivers send. Every command in
them has defined behaviour in `docs/AT.md`, and the log is replayed with
unknown commands refused, so that none of them passes by being skipped.

## When it does not work

| What is seen | Likely cause |
|---|---|
| The modem is not in the list of **Ports** | No modem device on the host port: step 2. Or RRAS was configured before the modems were added: restart the Routing and Remote Access service |
| The line shows `not_ready` | Nothing has the host port open. RRAS opens it when the port is enabled for inbound connections: step 4 |
| The line rings and nobody answers | The port is not enabled for inbound connections; or the driver's initialisation failed: look for `ERROR` in the log, and set `unknown_command = "ok"` |
| `NO CARRIER` after some seconds of tones | The modems did not agree. `rustmodem-ctl diag` during the call shows where it stopped; RMD7001 in the log names the state. Try the client capped at 1200 bit/s, and then at 300 |
| `NO CARRIER` straight after the modems connect | `error_control = "required"`, or the driver's `\N2` or `\N4`, and a client with error control off. RMD8001 in the log says so |
| `CONNECT`, and then PPP fails | Authentication: step 5. The RRAS log, and the client's error number, say which side refused |
| The call drops under load | Look at the RTP statistics of the line's call, `rustmodem-ctl line 1`: lost packets, and slips. LAPM rides out lost packets, but a path that loses many of them or changes the audio is the gateway's to fix: `docs/GATEWAYS.md` |
