# The capture corpus

Captured calls that the modem engine is replayed against. Each has three
files:

| File | What it is |
|---|---|
| `<name>.rmcap` | The capture, as the service writes it (`docs/CAPTURE.md`) |
| `<name>.expect` | What `rustmodem-replay` prints for it: the engine's states, its events and the data it receives |
| `<name>.md` | Where the capture came from and what it shows |

`tools/replay/tests/corpus.rs` replays every capture here and fails if the
engine no longer does what the capture's `.expect` says. A change to the
engine that is meant to change that is followed by making the file again:

```
cargo run -p replay -- modem/engine/tests/corpus/<name>.rmcap
```

The captures here are of the engine calling itself. Captures of calls with
real modems are added as they are taken.

NEEDS-CODY: capture calls from the US Robotics Courier through the SPA112
(`rustmodemctl capture on <line>`), V.21 and Bell 103, answered and
originated, and add each here with its `.expect` and a note.
