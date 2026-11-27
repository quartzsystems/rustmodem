# The capture corpus

Captured calls that the modem engine is replayed against. Each has three
files:

| File | What it is |
|---|---|
| `<name>.rmcap` | The capture, as the service writes it (`docs/CAPTURE.md`) |
| `<name>.expect` | What `rustmodem-replay` prints for it: the engine's states, its events, what its data link does and the data it receives |
| `<name>.md` | Where the capture came from and what it shows |

`tools/replay/tests/corpus.rs` replays every capture here and fails if the
engine no longer does what the capture's `.expect` says. A change to the
engine that is meant to change that is followed by making the file again:

```
cargo run -p replay -- modem/engine/tests/corpus/<name>.rmcap
```

The captures here are of the engine calling itself, one for each way a
call can go:

| Capture | The call |
|---|---|
| `answer-v8-v21` | V.8 chooses V.21 |
| `answer-v21` | V.21, the caller having no V.8 |
| `answer-bell103` | Bell 103 |
| `answer-v8-v22bis-lapm-v42bis` | V.8 chooses V.22bis; LAPM with V.42bis |
| `answer-v22-lapm` | V.22, the caller having no V.8; LAPM with no compression |
| `answer-v22bis` | V.22bis with no error control |
| `answer-bell212a` | Bell 212A with no error control |

In the two with LAPM the line's host sends nothing, which is what lets a
replay send every frame the call sent: `tools/replay` says why.

Captures of calls with real modems are added as they are taken.

NEEDS-CODY: capture calls from the US Robotics Courier through the SPA112
(`rustmodem-ctl capture on <line>`), in each of the modes above, answered
and originated, and add each here with its `.expect` and a note.
