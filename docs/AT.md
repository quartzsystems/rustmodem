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
it is not known, and the code is then plain `CONNECT`. The null engine of
phase 0 always reports rate 0.

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
| `A` | Answer: go off hook and enter handshaking. `ERROR` if already off hook. Ends the line | 6.3.5 |
| `D<string>` | Dial. See below. `ERROR` if already off hook. Ends the line | 6.3.1 |
| `E0`, `E1` | Echo off, on. Default `E1` | 6.2.4 |
| `H`, `H0` | Hang up, if off hook. `H1` returns `OK` and does nothing. Above 1 is an error | 6.3.6 |
| `I`, `I0` | Print the product code, `rustmodem` | 6.1.3 |
| `I3` | Print `rustmodem <version>` | |
| `I4` | Print the capability line, `NULL MODEM,V.250,+MS,+ES,+DS` | |
| `I1`, `I2`, `I5`..`I9` | Nothing; `OK`. Above 9 is an error | |
| `L0`..`L3` | Speaker volume. Stored, no effect | 6.3.13 |
| `M0`..`M3` | Speaker mode. Stored, no effect | 6.3.14 |
| `O`, `O0` | Return from online command mode to data, with `CONNECT`. `ERROR` if there is no connection. Ends the line | 6.3.7 |
| `Q0`, `Q1` | Result codes on, off. Default `Q0` | 6.2.5 |
| `S<n>=<v>` | Set register `n`. A missing value means 0. Above 255 is an error | 5.3.2 |
| `S<n>?` | Print register `n` as three digits | 5.3.2 |
| `V0`, `V1` | Numeric, verbose responses. Default `V1` | 6.2.6 |
| `X0`..`X4` | Result code set. Default `X4` | 6.2.7 |
| `Z`, `Z0` | Hang up if off hook, return every setting to the stored profile (in phase 0, the factory defaults), `OK`. Ends the line | 6.1.1 |
| `&C0`, `&C1` | DCD always on; DCD follows the carrier. Default `&C1` | 6.2.8 |
| `&D0`..`&D3` | What a DTR drop does. See below. Default `&D2` | 6.2.9 |
| `&F`, `&F0` | Return every setting to the factory defaults. Does not hang up; the rest of the line runs | 6.1.2 |
| `&K0`, `&K3` | Flow control: none, RTS/CTS. Stored, no effect. Default `&K3`. Other values are an error | |
| `&V` | Print the settings, the registers and the stored extended parameters | |
| `&W`, `&W0`, `&W1` | Accepted, `OK`. Nothing is stored (`BLOCKED(phase1)`) | |
| `+MS` | Modulation selection. Stored, no effect | 6.4.1 |
| `+ES` | Error control selection. Stored, no effect | 6.5.1 |
| `+DS` | Data compression. Stored, no effect | 6.6.1 |
| anything else | `OK` or `ERROR`, as the line's `unknown_command` setting says | |

A known command with a number outside its range is always an error, whatever
`unknown_command` says.

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

`+MS`, `+ES` and `+DS` each take three forms:

| Form | Example | Effect |
|---|---|---|
| Set | `AT+MS=V34,1,300,33600` | Stores the value. Letters, digits, `,` and `.`, up to 64 characters. An empty value restores the default |
| Read | `AT+MS?` | Prints `+MS: <value>` |
| Test | `AT+MS=?` | Prints `+MS: <ranges>` |

The name alone, with no `=` or `?`, is an error.

| Parameter | Default | Test form reports |
|---|---|---|
| `+MS` | `V21,1,0,0,0,0` | `(V21),(0,1),(0),(0),(0),(0)` |
| `+ES` | `3,0,2` | `(0-4),(0-4),(0-6)` |
| `+DS` | `3,0,2048,6` | `(0-3),(0),(512-2048),(6-250)` |

Phase 0 has no modulation, error control or compression. The values are
stored and reported and nothing reads them; the defaults and ranges are
placeholders for the phases that add each one.

### Unknown commands

Each line has an `unknown_command` setting, `ok` or `error`.

With `error`, a command that is not in the table above stops the line with
`ERROR`.

With `ok`, the command is skipped and the line carries on. How much is skipped:

| The command starts with | What is skipped |
|---|---|
| A letter | The letter, a number, then `?` or `=` and digits and commas |
| `&` | `&`, one more character, then the same |
| `\`, `%`, `*`, `-`, `$`, `^`, `)`, `:`, `@` | That character, one more character, then the same |
| `#` | `#`, a whole word of letters, then the same |
| `+` | Everything up to the next `;` |

A register that the interpreter does not have is treated the same way: with
`ok`, setting it does nothing and reading it prints `000`.

## Registers

| Register | Default | Meaning |
|---|---|---|
| S0 | 0 | Rings before auto-answer; 0 is off. The default comes from the line's `auto_answer_rings` |
| S1 | 0 | Ring count. Returns to 0 eight seconds after the last ring. A value written to it is not kept |
| S2 | 43 | Escape character. Above 127 disables the escape |
| S3 | 13 | Command line terminator. 0 to 127 |
| S4 | 10 | Response formatting character. 0 to 127 |
| S5 | 8 | Command line editing character. 0 to 127 |
| S6 | 2 | Wait before dialling, seconds. Stored, no effect |
| S7 | 50 | Seconds to wait for a connection |
| S10 | 14 | Carrier loss delay, tenths of a second. Stored, no effect |
| S12 | 50 | Escape guard time, units of 20 ms |

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
