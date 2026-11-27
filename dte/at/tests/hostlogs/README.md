# Host command logs

What a host sent a line and what the line answered, as the service's own
log has it. Each `.txt` file here is replayed by `dte/at/tests/hostlogs.rs`:
every command is given to a new AT interpreter, the result codes it issues
must be the ones the log has, and none of them may be `ERROR`.

It is how the initialisation a real host sends, the Windows modem driver
and Routing and Remote Access above all, is kept as a test: nothing is
transcribed, the lines of the log are pasted.

## The format

The service logs every command line and every result code of a line at
`debug`:

```
2027-01-15T08:00:02.120Z DEBUG [Line01] > ATE0V1
2027-01-15T08:00:02.121Z DEBUG [Line01] < OK
```

A file here is such lines. Of each line only `[LineNN]`, the `>` or `<`
and what follows are read, so the time and the level may be anything, and
a line that is neither a command nor a result code is skipped, as is one
that begins with `#`. All the lines of a file are of one line of the
service. The files are named `.txt`: the repository ignores
`.log`.

The first line may say how the service's line was configured, in the
words of the transcripts beside this folder:

```
# config unknown_command=ok auto_answer_rings=0
```

Without it the line is as the configuration's defaults make it.

A result code that no command accounts for is taken for what happened on
the line then: `RING` is a ring, `CONNECT` with its rate is the modems
connecting, and `NO CARRIER` is the far end clearing the call. The
reports before a `CONNECT` (`+MCR:`, `+MRR:`, `+ER:`, `+DR:`, `+ILRR:`)
say what the call connected with.

## Adding one

1. Set the log level to `debug`: in the manager's Settings, or
   `log_level = "debug"` under `[service]`.
2. Let the host do what it does: open the port, initialise, answer a
   call.
3. `rustmodemctl log --last 500 > host.log`, and copy the lines of the
   one line into a file here, named for the host: `rras-server-2022.txt`.
4. `cargo test -p at --test hostlogs`.

A command the interpreter answers otherwise than the log says is a
finding: either the interpreter changed since the log was taken, which
the test is there to catch, or the file's `# config` line does not say
how the line was configured.

## What is here

| File | What it is |
|---|---|
| `standard-modem.txt` | The initialisation of a Windows standard modem driver and an answered call, **written from memory** in the log's format, with `unknown_command=error`: every command in it is one this modem has |

NEEDS-CODY: the real thing. After the first call RRAS answers
(`docs/RRAS.md`, criterion 15 of `docs/PHASE4.md`), add the log of that
line here, and one for each standard modem model tried.
