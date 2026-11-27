# The rustmodem AT command set

This describes the Hayes-compatible command interpreter as it is built in
`dte/at`. It is written from the implementation: every command, register and
result code below is covered by a test in `dte/at/tests/`.

The interpreter is written from the phase 0 handoff and ITU-T V.250. Clause
numbers refer to V.250.

## Modes

| Mode | Meaning |
|---|---|
| Command | On hook, accepting commands |
| Ringing | An incoming call is ringing (S1 is not zero); commands are still accepted |
| Handshaking | A call is being set up after `A` or `D` |
| Online data | Connected; bytes from the host are data for the far end |
| Online command | Connected, but accepting commands |

In handshaking mode any byte from the host aborts the call with `NO CARRIER`
(clause 5.6.1). The byte is discarded. Bytes that arrive in the same write as
the command line that started the call, such as a line feed after the carriage
return, are discarded without aborting.

## Command lines

Clause 5.2.

- A command line starts with `AT` in either case, or mixed. Characters before
  the prefix are ignored (and echoed, with `E1`).
- `A/` runs the previous command line again at once. It needs no terminator.
  A line that was too long is not kept for `A/`.
- The line ends at the S3 character (carriage return by default).
- The S5 character (backspace by default) removes the previous character. It
  is echoed as S5, space, S5. It cannot remove the prefix.
- Other control characters (below 32) are ignored and not echoed.
- Spaces are ignored anywhere in the line. Commands are the same in either
  case.
- A line holds up to 255 characters after the prefix. A longer one returns
  `ERROR` and none of it is executed.
- Several commands on one line run left to right. The first error stops the
  line with `ERROR`; the commands before it have taken effect.
- `A`, `D`, `O` and `Z` end the line: nothing after them is processed.
- A basic command's number may be left out, which means 0.
- Extended commands (those that start with `+`) are separated from what
  follows by `;`.

## Responses

Clause 5.7.

| Setting | Result code | Information text |
|---|---|---|
| `V1` (default) | `<S3><S4>text<S3><S4>` | `<S3><S4>text<S3><S4>` |
| `V0` | `number<S3>` | `text<S3><S4>` |

`Q1` suppresses every result code, including `RING` and `CONNECT`. It does not
suppress information text or echo.

## Result codes

| Number | Text | When |
|---|---|---|
| 0 | `OK` | The command line was executed; the escape sequence was detected |
| 1 | `CONNECT` | The modems connected; `O` returned to data mode |
| 2 | `RING` | A ring-on interval of an incoming call began |
| 3 | `NO CARRIER` | The call ended, failed, or was aborted |
| 4 | `ERROR` | The command line was not accepted |
| 6 | `NO DIALTONE` | There is no dial tone |
| 7 | `BUSY` | The number dialled is busy |
| 8 | `NO ANSWER` | The number dialled did not answer within S7 seconds |

Which codes are used depends on `X`:

| Setting | `CONNECT` | `BUSY` | `NO DIALTONE` | `NO ANSWER` |
|---|---|---|---|---|
| `X0` | bare | `NO CARRIER` | `NO CARRIER` | `NO CARRIER` |
| `X1` | with rate | `NO CARRIER` | `NO CARRIER` | yes |
| `X2` | with rate | `NO CARRIER` | yes | yes |
| `X3` | with rate | yes | `NO CARRIER` | yes |
| `X4` (default) | with rate | yes | yes | yes |

"With rate" means `CONNECT <rate>` when the rate is known. A rate of 0 means
it is not known, and the code is then plain `CONNECT`. The null engine
always reports rate 0. The rate is the line's: 300, 1200 or 2400.

With `V0` and `X1` or above, a connection at a known rate reports one of these
numbers, which follow the common Hayes-compatible convention. Any other rate
reports 1.

| Rate | Number |
|---|---|
| 1200 | 5 |
| 2400 | 10 |
| 4800 | 11 |
| 9600 | 12 |
| 14400 | 13 |
| 19200 | 14 |

## Commands

| Command | Behaviour | Clause |
|---|---|---|
| `B0`, `B1` | The family of the carrier at 300 and 1200 bit/s: the ITU's, V.21 and V.22, or Bell's, 103 and 212A. They change the first subparameter of `+MS` to the carrier of the same rate in the other family, and nothing else. With `V22B`, `B0` changes nothing and `B1` makes it `B212A`. Above 1 is an error | |
| `A` | Answer: go off hook and enter handshaking. `ERROR` if already off hook. Ends the line | 6.3.5 |
| `D<string>` | Dial. See below. `ERROR` if already off hook. Ends the line | 6.3.1 |
| `E0`, `E1` | Echo off, on. Default `E1` | 6.2.4 |
| `H`, `H0` | Hang up, if off hook. `H1` returns `OK` and does nothing. Above 1 is an error | 6.3.6 |
| `I`, `I0` | Print the product code, `rustmodem` | 6.1.3 |
| `I3` | Print `rustmodem <version>` | |
| `I4` | Print the capability line, `SOFT MODEM,V.250,+MS,+ES,+DS`. It starts with the name of the line's engine: `SOFT` for the modem, `NULL` for a line with `engine = "null"` and `ECHO` for one with `engine = "echo"` | |
| `I1` | Print `000`: a modem with a ROM prints its checksum, and this one has none | |
| `I5` | Print the manufacturer, `Quartz Systems` | |
| `I6` | Print the model, `RustModem Softmodem` | |
| `I7` | Print the model and the version, `RustModem Softmodem <version>` | |
| `I2`, `I8`, `I9` | Nothing; `OK`. Above 9 is an error | |
| `L0`..`L3` | Speaker volume. Stored, no effect | 6.3.13 |
| `M0`..`M3` | Speaker mode. Stored, no effect | 6.3.14 |
| `O`, `O0` | Return from online command mode to data, with `CONNECT`. `ERROR` if there is no connection. Ends the line | 6.3.7 |
| `Q0`, `Q1` | Result codes on, off. Default `Q0` | 6.2.5 |
| `S<n>=<v>` | Set register `n`. A missing value means 0. Above 255 is an error | 5.3.2 |
| `S<n>?` | Print register `n` as three digits | 5.3.2 |
| `V0`, `V1` | Numeric, verbose responses. Default `V1` | 6.2.6 |
| `X0`..`X4` | Result code set. Default `X4` | 6.2.7 |
| `Z`, `Z0` | Hang up if off hook, return every setting to the stored profile, or to the factory defaults if nothing is stored, `OK`. Ends the line | 6.1.1 |
| `&C0`, `&C1` | DCD always on; DCD follows the carrier. Default `&C1` | 6.2.8 |
| `&D0`..`&D3` | What a DTR drop does. See below. Default `&D2` | 6.2.9 |
| `&F`, `&F0` | Return every setting to the factory defaults. The stored profile is left as it is. Does not hang up; the rest of the line runs | 6.1.2 |
| `&K0`, `&K3` | Flow control: none, RTS/CTS. With `&K3` CTS goes off while the modem holds data of the host's that the line has not taken, and on again when it has; with `&K0` CTS stays on. Either way the modem stops reading the port while it holds too much. Default `&K3`. Other values are an error; `&K4`, XON/XOFF, is not built | |
| `&Q0`, `&Q5`, `&Q6` | The communications mode of modems before V.250: direct, error control, buffered. Each sets `+ES`. See "The older commands". `&Q1` to `&Q4`, the synchronous modes, are an error | |
| `&V` | Print the settings, the registers and the extended parameters of the active profile, and then of the stored profile if there is one | |
| `&W`, `&W0`, `&W1` | Store the active profile. There is one stored profile; `&W0` and `&W1` write the same one. See "Stored profiles" | |
| `+MS` | Modulation selection. See "Modulation selection" | 6.4.1 |
| `+ES` | Error control selection. See "Error control and compression" | 6.5.1 |
| `+DS` | Data compression. See "Error control and compression" | 6.6.1 |
| `+ICF` | Character framing. See "Character framing" | 6.2.11 |
| `+MR`, `+ER`, `+DR`, `+ILRR` | What is reported before `CONNECT`. See "Connection reports" | 6.4.3, 6.5.5, 6.6.2, 6.2.13 |
| `\N0`..`\N5` | Error control, of modems before V.250. Each sets `+ES`. See "The older commands". Above 5 is an error | |
| `%C0`..`%C3` | Compression, of the same modems. Each sets `+DS`. Above 3 is an error | |
| anything else | `OK` or `ERROR`, as the line's `unknown_command` setting says | |

A known command with a number outside its range is always an error, whatever
`unknown_command` says.

### Identification

What Windows sends a modem to find out what it is: when it installs one,
and when Query Modem is pressed on the Diagnostics page of the modem's
properties. Windows' own list is `ATQ0V1E0`, `AT+GMM`, `AT+FCLASS=?`,
`AT#CLS=?`, `AT+GCI?`, `AT+GCI=?` and `ATI1` to `ATI7`; the other four are
V.250's and are here because a host may ask. None of them is answered
`ERROR`, whatever the line does with commands it does not know.

| Command | Behaviour | Clause |
|---|---|---|
| `+GMI` | Print the manufacturer, `Quartz Systems` | 6.1.4 |
| `+GMM` | Print the model, `RustModem Softmodem`. It is the name the modem has in Windows | 6.1.5 |
| `+GMR` | Print the version of rustmodem | 6.1.6 |
| `+GSN` | Print the serial number, `00000000`: a modem made of software has none | 6.1.7 |
| `+GCAP` | Print `+GCAP: +MS,+ES,+DS`: the modem has modulation control, error control and compression, and no fax or voice | 6.1.9 |
| `+GMI=?` and the same of the other four | `OK` | 5.4.3 |
| `+FCLASS=?`, `+FCLASS?` | Print `0`: the one service class is data | T.31 8.2 |
| `+FCLASS=0` | `OK`. Any other class is `ERROR` | T.31 8.2 |
| `#CLS=?`, `#CLS?`, `#CLS=0` | As `+FCLASS`: the command older modems had for it. Any other class is `ERROR` | |
| `+GCI?` | Print the country of installation, a T.35 code in two hexadecimal digits: `+GCI: B5` until it is set | 6.1.10 |
| `+GCI=<code>` | Set it. Kept until the service restarts, and read back; nothing the modem does differs by country | 6.1.10 |
| `+GCI=?` | Print `+GCI: (00-FF)` | 6.1.10 |

### Dial

The dial string is the rest of the line. Digits, `*`, `#`, `+`, `,` and `W`
are kept. Everything else is dropped, including the `T` and `P` modifiers and
any punctuation.

With no SIP in phase 0, the pool reads the string as a line id and calls that
line (the "null switch").

The call then ends in one of: `CONNECT`, `BUSY`, `NO DIALTONE`, `NO ANSWER`
(after S7 seconds, when the modem hangs up by itself), or `NO CARRIER`.

A dial string that ends in `;` returns `OK` at once and stays in command mode
while the call goes on. If the far end then connects, the modem is in online
command mode with no `CONNECT` reported, and `O` takes it to data. If the call
fails, the failure is reported as usual.

### DTR

What happens when the host drops DTR:

| Setting | On a DTR drop |
|---|---|
| `&D0` | Nothing |
| `&D1` | In online data mode, go to online command mode and report `OK` |
| `&D2` | Hang up if off hook, and report `OK` |
| `&D3` | As `&D2`, and return every setting to the factory defaults, on or off hook |

Whatever the setting, a DTR drop discards a half-typed command line. While DTR
is off and the setting is not `&D0`, the modem does not auto-answer.

### Extended parameters

`+MS`, `+ES`, `+DS`, `+ICF`, `+MR`, `+ER`, `+DR` and `+ILRR` each take
three forms:

| Form | Example | Effect |
|---|---|---|
| Set | `AT+ES=3,2,4` | Sets the value. A subparameter left out stays as it is. An empty value restores the default. A value the modem has not got is `ERROR`, and the parameter stays as it was |
| Read | `AT+MS?` | Prints `+MS: <value>` |
| Test | `AT+MS=?` | Prints `+MS: <ranges>` |

The name alone, with no `=` or `?`, is an error.

| Parameter | Default | Test form reports |
|---|---|---|
| `+MS` | `V22B,1,0,0,0,0` | `(V21,V22,V22B,B103,B212A),(0,1),(0,300,1200,2400),(0,300,1200,2400),(0,300,1200,2400),(0,300,1200,2400)` |
| `+ES` | `3,0,2`, or what the line's `error_control` makes it | `(0-3),(0-3),(0-5)` |
| `+DS` | `3,0,2048,32`, or `0,0,2048,32` on a line with `compression = false` | `(0-3),(0),(512-4096),(6-250)` |
| `+ICF` | `3,3` | `(3,5),(0-3)` |
| `+MR`, `+ER`, `+DR`, `+ILRR` | `0` | `(0,1)` |

### Modulation selection

`+MS=<carrier>,<automode>,<min rate>,<max rate>,<min rx rate>,<max rx rate>`
(V.250 clause 6.4.1) says which modulation the line's next call uses.

| Subparameter | Values | Meaning |
|---|---|---|
| carrier | `V21`, `V22`, `V22B`, `B103`, `B212A` | The modulation to try first: V.21, V.22, V.22bis, Bell 103 or Bell 212A. Letters of either case |
| automode | `0`, `1` | With 1 the modem may use the line's other modulations too, and V.8; with 0 it uses this carrier and nothing else, and no V.8 |
| the four rates | `0`, `300`, `1200`, `2400` | 0 is no limit. With automode, a modulation whose rate is under a minimum or over a maximum is not used: `AT+MS=V22B,1,0,1200` keeps the call to 1200 bit/s and under. The modems are the same rate both ways, so the stricter of the two pairs is what counts. Without automode the carrier is used whatever the rates say |

A subparameter left out stays as it is: `AT+MS=,0` turns automode off and
leaves the carrier. The value is reported whole, `+MS: B103,0,0,0,0,0`. A
carrier, an automode or a rate the modem has not got is `ERROR`, and `+MS`
stays as it was.

`+MS` chooses within what the line's configuration allows (`modulations` in
`[modem]` or the line's own, `docs/MODEM.md`). A carrier the configuration
does not allow is not the host's to choose: the configuration's list is
used as it is.

The Bell modems are `B103` and `B212A` here because V.250 keeps the names
that begin with `V` for ITU-T modulations (Table 13); the control API
calls them `BELL103` and `BELL212A`.

V.22bis, V.22 and Bell 212A are one kind of modem, and an answering line
that is allowed any of them answers a caller of any of them it is allowed:
the order among the three changes nothing but what a connection at 1200
bit/s is called. `docs/MODEM.md` has how a call chooses its modulation.

### Error control and compression

`+ES=<orig_rqst>,<orig_fbk>,<ans_fbk>` (V.250 clause 6.5.1, Table 20) says
what error control a call has on a modulation that can have it, which is
every one but V.21 and Bell 103.

| Subparameter | Values | Meaning |
|---|---|---|
| orig_rqst | `0`, `1` | Originating: no error control. Direct mode and buffered mode are the same here: the port has no rate to change |
| | `2` | Originating: V.42 without the detection phase |
| | `3` | Originating: V.42 with the detection phase |
| orig_fbk | `0`, `1` | If the answerer has no error control, go on without |
| | `2`, `3` | If it has none, clear the call. The host gets `NO CARRIER` |
| ans_fbk | `0`, `1` | Answering: no error control |
| | `2`, `3` | Answering: error control if the caller has it |
| | `4`, `5` | Answering: error control or clear the call |

The values that ask for the alternative protocol of V.42 Annex A (MNP),
`orig_rqst` 4, `orig_fbk` 4 and `ans_fbk` 6, are `ERROR`: the modem has
LAPM and nothing else. So is the synchronous access mode, `orig_rqst` 6.

`+DS=<direction>,<compression_negotiation>,<max_dict>,<max_string>` (V.250
clause 6.6.1, Table 27) says what V.42bis compression a call with error
control asks for and agrees to.

| Subparameter | Values | Meaning |
|---|---|---|
| direction | `0` | None |
| | `1`, `2`, `3` | What the host sends, what it receives, both |
| compression_negotiation | `0` | The call goes on if compression is not agreed. 1, clear the call, is `ERROR` |
| max_dict | `512` to `4096` | The most codewords to agree to |
| max_string | `6` to `250` | The longest string to agree to |

The factory values are the line's policy: `error_control` and `compression`
in `[modem]` or the line's own (`docs/MODEM.md`).

| `error_control` | `+ES` |
|---|---|
| `auto` | `3,0,2` |
| `required` | `3,2,4` |
| `off` | `1,0,1` |

With `compression = false` the direction of `+DS` is 0. The dictionary of
2048 and the string length of 32 are this modem's choice; V.250 leaves the
first to the manufacturer and recommends 6 for the second, which gives up
most of what compression is for. What the host sets is what the next call
uses, and `&F` returns to the line's.

### The older commands

Hosts send the commands that modems had for error control and compression
before V.250, and the standard modem drivers of Windows do. Each sets the
`+ES` or the `+DS` that it is, and that is all it does: `AT\N3` and
`AT+ES=3,0,2` leave the modem the same.

| Command | Meaning where it comes from | Sets |
|---|---|---|
| `\N0` | Normal mode: buffered, no error control | `+ES=1,0,1` |
| `\N1` | Direct mode | `+ES=0,0,0` |
| `\N2` | Reliable mode: error control or no call | `+ES=3,2,4` |
| `\N3` | Auto-reliable: error control if the far end has it | `+ES=3,0,2` |
| `\N4` | LAPM or no call | `+ES=3,3,5` |
| `\N5` | MNP or no call. There is no MNP: error control or no call | `+ES=3,2,4` |
| `&Q0` | Direct asynchronous mode | `+ES=0,0,0` |
| `&Q5` | Error control | `+ES=3,0,2` |
| `&Q6` | Asynchronous mode with buffering | `+ES=1,0,1` |
| `%C0` | No compression | direction 0 of `+DS` |
| `%C1` | MNP 5. There is none: no compression | direction 0 of `+DS` |
| `%C2`, `%C3` | V.42bis; either | direction 3 of `+DS` |
| `S36=0`, `4` | If error control is not agreed, hang up | `orig_fbk` 2, and `ans_fbk` 4 unless it is 0 or 1 |
| `S36=1`, `3`, `5`, `7` | If it is not agreed, go on without | `orig_fbk` 0, and `ans_fbk` 2 unless it is 0 or 1 |
| `S46=136` | LAPM without compression | direction 0 of `+DS` |
| `S46=138` | LAPM with compression | direction 3 of `+DS` |
| `S48=7` | Negotiate error control | `orig_rqst` 3 if it was 2 |
| `S48=0` | Go to LAPM without negotiating | `orig_rqst` 2 if it was 3 |
| `S48=128` | Do not negotiate: no error control | `orig_rqst` 1 and `ans_fbk` 1 |

The three registers keep the value written and report it; any other value
is `ERROR`. The last command given wins: `AT\N0S36=7` has no error
control, because S36 leaves an `+ES` that has it off as it is, and
`ATS48=128&Q5` has it.

### Connection reports

`+MR=1`, `+ER=1`, `+DR=1` and `+ILRR=1` each ask for a report when a call
connects, before its `CONNECT` and in this order:

| Parameter | Report | Values |
|---|---|---|
| `+MR` | `+MCR: <carrier>` and then `+MRR: <rate>` | The carrier as `+MS` spells it; the line rate in bit/s |
| `+ER` | `+ER: <type>` | `LAPM`, `NONE` |
| `+DR` | `+DR: <type>` | `V42B` for both directions, `V42B TD` for what the host sends only, `V42B RD` for what it receives only, `NONE` |
| `+ILRR` | `+ILRR: <rate>` | The line rate in bit/s |

`+ILRR` is the rate between the host and the modem in V.250. Here the port
is no wire and has no rate of its own: the modem takes and gives data as
fast as the line carries it, so the line rate is what is reported.

The reports are intermediate result codes. With `V1` each is
`<S3><S4>text<S3><S4>`; with `V0` each is `text<S3><S4>`; with `Q1` there
are none. `O` reports `CONNECT` and no more.

`CONNECT <rate>` is always the line rate: 300, 1200 or 2400.

### Character framing

`+ICF=<format>,<parity>` (V.250 clause 6.2.11) is the format of the
characters on the line, where the line carries them as start-stop
characters: on a V.21 or Bell 103 call, and on a faster call that has no
error control (`docs/MODEM.md`). Under error control the line carries
octets and this changes nothing. The port itself has no format: it is not
a wire.

| `+ICF` | Format | A character on the line |
|---|---|---|
| `3` with any parity | 8N1 | A start bit, eight data bits, a stop bit |
| `5,1` | 7E1 | A start bit, seven data bits, a parity bit that makes the ones even, a stop bit |
| `5,0` | 7O1 | The same with the ones odd |

With seven data bits the eighth bit of what the host sends is not
carried, and what it receives has it clear. A character received with
the wrong parity is given all the same, and counted with the framing
errors.

A subparameter left out stays as it is: `AT+ICF=,0` changes the parity
alone. The other formats of Table 5/V.250, automatic detection (0), two
stop bits (1 and 4), eight data bits with parity (2) and seven with none
(6), and mark and space parity with format 5, are `ERROR`, and `+ICF`
stays as it was. The value takes effect at the line's next call.

### Unknown commands

Each line has an `unknown_command` setting, `ok` or `error`.

With `error`, a command that is not in the table above stops the line with
`ERROR`.

With `ok`, the command is skipped and the line carries on. How much is skipped:

| The command starts with | What is skipped |
|---|---|
| A letter | The letter, a number, then `?` or `=` and digits and commas |
| `&` | `&`, one more character, then the same |
| `\`, `%`, `*`, `-`, `$`, `^`, `)`, `:`, `@` | That character, one more character, then the same. `\N` and `%C` are not unknown |
| `#` | `#`, a whole word of letters, then the same |
| `+` | Everything up to the next `;` |

A register that the interpreter does not have is treated the same way: with
`ok`, setting it does nothing and reading it prints `000`.

## Stored profiles

A profile is every setting the commands above change (`E`, `Q`, `V`, `X`,
`L`, `M`, `&C`, `&D`, `&K`, `+MS`, `+ES`, `+DS`, `+ICF`, `+MR`, `+ER`,
`+DR`, `+ILRR`) and every S-register.
Each line has an active profile, which the commands change, and may have a
stored one.

- `&W` stores the active profile. The rest of the command line runs, and a
  call in progress is not disturbed. S1 counts the rings of a call in
  progress and is not a setting: it is stored as 0.
- `Z` returns to the stored profile. So does a DTR drop with `&D3`, and so
  does a reset of the line through the control API (`lines.reset`), which
  stands for switching the modem off and on.
- `&F` returns to the factory defaults and leaves the stored profile alone.
  To clear what is stored, give `AT&F&W`.
- A line starts from its stored profile. With nothing stored it starts from
  the factory defaults, where S0 is the line's `auto_answer_rings`. A
  stored profile has its own S0, and that one wins.

The interpreter itself keeps nothing: `&W` hands the profile to the service,
which keeps the profiles of all lines in `profiles.toml` beside the
configuration file, one `[line.<id>]` table for each line that has stored
one. The service reads the file when it starts and writes it whenever a
line gives `&W` or is removed. If the file cannot be written the host still
gets `OK`, the profile is in effect until the service stops, and the
service log has `RMD0025`. A profile in the file with a value that no
command could have stored is left out with `RMD0026`, and its line starts
from the factory defaults.

Removing a line from the configuration removes its stored profile. A line
added later under the same id starts from the factory defaults.

## Registers

| Register | Default | Meaning |
|---|---|---|
| S0 | 0 | Rings before auto-answer; 0 is off. The factory default comes from the line's `auto_answer_rings`; a stored profile has its own |
| S1 | 0 | Ring count. Returns to 0 eight seconds after the last ring. A value written to it is not kept |
| S2 | 43 | Escape character. Above 127 disables the escape |
| S3 | 13 | Command line terminator. 0 to 127 |
| S4 | 10 | Response formatting character. 0 to 127 |
| S5 | 8 | Command line editing character. 0 to 127 |
| S6 | 2 | Wait before dialling, seconds. Stored, no effect |
| S7 | 50 | Seconds to wait for a connection |
| S9 | 6 | Carrier response time, tenths of a second: how long the far end's carrier must be there before the modem connects. It is not in V.250; it is the register Hayes modems have |
| S10 | 14 | Carrier loss delay, tenths of a second: how long the carrier may be gone before the modem ends the call (V.250 clause 6.3.12). 255 is 25.5 seconds here; the Recommendation has it mean that the carrier is never taken to be lost |
| S12 | 50 | Escape guard time, units of 20 ms |
| S36 | 7 | What follows a failure to agree on error control. See "The older commands". 0, 1, 3, 4, 5 or 7 |
| S46 | 138 | Error control with compression, 138, or without, 136 |
| S48 | 7 | Whether error control is negotiated: 7, 0 or 128 |

S7 runs from the `A` or `D` command. When it expires the modem hangs up and
reports `NO ANSWER` if it was dialling, or `NO CARRIER` if it was answering.

## The escape sequence

In online data mode, the escape sequence is:

1. at least the guard time (S12) with nothing from the host, then
2. exactly three escape characters (S2), each less than the guard time after
   the one before, then
3. the guard time with nothing from the host again.

The three characters are sent to the far end as data when they arrive; data is
never held back. When the closing silence has passed, the modem enters online
command mode and reports `OK`.

An escape character that comes later than the guard time after the one before
opens a new sequence. A fourth escape character, or any other byte, inside the
closing silence cancels the sequence.

With S12 at 0 there is no timing: three escape characters in a row escape.

The interpreter reads no clock. Time comes only from the `Elapsed` inputs the
pool gives it, every 20 ms.

## Ringing and auto-answer

The pool generates the ring cadence. At the start of each ring-on interval
the interpreter reports `RING` and adds one to S1. If S0 is not zero and S1
has reached it, the modem answers as if `A` had been given.

Data received from the far end while the host is in online command mode is
held, up to 64 KiB, and delivered after the `CONNECT` when the host returns to
data mode with `O`. Data beyond that is dropped.

## Signals

| Signal | State |
|---|---|
| DSR | On while the line exists |
| DCD | With `&C1`, on while connected (online data or online command). With `&C0`, always on |
| RI | On during the ring-on interval |
| CTS | On. It goes off only when flow control back-pressure applies and `&K3` is set; phase 0 never applies it |
