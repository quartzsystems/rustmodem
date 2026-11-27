# Phase 1 record: the manager

Phase 1 adds the operator's window. `rustmodem-manager.exe`, RustModem
Manager, connects to the service over the control pipe, shows every line
live, and carries the operator's actions. It is built from the stock Win32
common controls and holds no state of its own. The phase also extends the
control API with what the window needs, adds a demo service so that the
window can be developed and tested with no com0com and no elevated prompt,
and closes phase 0's one `BLOCKED(phase1)` marker: stored profiles, `&W`.

All ten work packages are built and committed. The five automated exit
criteria pass in CI. The five manual ones need a person in front of the
window; they are a checklist at the end. Nothing of the manual five has been
done: no person has looked at the window beside a stock program, and the
manager has never met the real service, an elevation prompt or a real COM
port.

## What needs Cody

| Item | Where | What to do |
|---|---|---|
| Manual exit criteria 6 to 10 | "Manual checklist" below | Run them. Until then the look, scaling, the real service, elevation and the AT console are built and unseen |
| Port inventory from the registry | `service/svc/src/ports.rs` (`NEEDS-CODY`) | `ports.list` and the Binding column read the com0com pairs and the modem devices from the registry. The key and value names are from the com0com ReadMe and from memory, not from a machine that has them. Check them on one: criterion 8 does |
| Decisions made without asking | "Decisions to confirm" below | The handoff left these open and nobody was there to ask. Each was decided and is listed for review |
| The layout was built from the handoff's text | "References consulted" below | `docs/design/` did not exist and VoIPModem Manager was not looked at. If the arrangement is not what was meant, the screenshots in `ui/manager/tests/goldens/` show what was built |
| The goldens follow the CI runner image | "Exit criteria", criterion 3 | When GitHub changes the image and the rendering with it, the screenshot tests fail on CI and the goldens are taken again from the run's `screenshots` artifact. `CLAUDE.md` says how |
| Branches were used to bless the goldens | "Where the build differs" below | `phase1-goldens` and `phase1-goldens-2` were pushed, ran CI a few times each, and were deleted. Nothing of them is left but the runs in the Actions history |
| What Cody asked for while the phase was being built | "Changes Cody asked for" below | Six changes to the window and one to the service, made after the handoff's version had been built. Each replaces something the handoff says |
| Still open from phase 0 | `docs/PHASE0.md` | The com0com signal mapping, the V.250 clause numbers and phase 0's manual checklist are as they were |

## What was built

### 1. Control API additions

Five methods, all additions to protocol 1, which is still the only
protocol: `stats.get`, `log.tail`, `config.get`, `config.set` and
`ports.list`. `ServiceInfo` gained `config_loaded_ms` and `mode` (service,
console or demo). `LineInfo` gained `binding`, and its counters
`at_commands`. A client of phase 0 reads none of these and is not disturbed
by them; `docs/CONTROL_API.md` has a "Compatibility" section that says so.

- The pool keeps the statistics (`service/pool/src/stats.rs`): calls
  answered, connected, failed, rejected and abandoned, the average and the
  longest call, the peak of lines in use, bytes each way, AT commands, and
  the connect rates by rate and carrier.
- The server keeps the last 2000 log lines for `log.tail`.
- `config.set` writes `[service]` and `[control]` through the same
  comment-keeping editor as the rest, and says whether the service has to
  be started again for the change to take effect.
- `ports.list` is behind a `PortInventory` trait. The service's inventory
  (`service/svc/src/ports.rs`) puts together the serial ports Windows has,
  the com0com pairs and the modem devices; putting them together is pure
  and tested, reading them is not (see "What needs Cody").
- `rustmodem-ctl` has `stats`, `ports` and `log [--last N]`.

### 2. Stored profiles

`AT&W` stores the active profile. `ATZ`, `&D3`, an operator's reset and a
restart of the service restore it; `AT&F` goes to the factory defaults and
leaves the stored profile alone; `AT&V` shows it. The interpreter stays
pure: it hands the pool an `Action::StoreProfile`, and the pool keeps
profiles behind a `ProfileStore` trait. The service's store is
`profiles.toml` beside the configuration file. Transcripts 47 to 54 and
`dte/at/tests/profile.rs` cover it, and `docs/AT.md` has a "Stored
profiles" section.

### 3. `tools/demosvc`

`rustmodem-demosvc.exe --lines N --script idle|busy|frozen [--pipe NAME]`:
a real pool and the real control API on in-memory ports, with scripted
hosts on the other ends, on a pipe that admits the user who started it.
`service.info` reports the mode `demo`.

- `idle`: lines and no calls.
- `busy`: calls come and go on every line, for the smoke test.
- `frozen`: a fixed history, then nothing, with the clock stopped at
  2027-01-15 08:24:57 UTC. Every run gives the same lines, calls,
  statistics and log, which is what the screenshot tests need.

To give the demo service and the manager the pipe, the named pipe transport
moved out of `service/svc` into a crate of its own, `service/pipe`, and
gained a client.

### 4. `ui/glyph`

The icons, drawn in code: the application icon, seven toolbar images and
seven line state images, each at 16, 20, 24, 32, 48 and 256 pixels. The
crate writes `.ico` files and Windows `.res` files (icon, version
information, manifest) with no resource compiler, for build scripts to hand
to the linker. It has no dependencies; its PNG encoder is its own.
`rustmodem-svc.exe`, `rustmodem-ctl.exe` and `rustmodem-manager.exe` all get
their resources from it.

### 5. `ui/win32`

Safe wrappers over exactly as much of Win32 as the manager uses: the window
and its events, the list view (owner data), toolbar, status bar, rich edit,
menus and accelerators, the standard controls, dialogs from templates built
in memory, the property sheet, the task dialog, the file dialogs, fonts and
metrics from the system for the DPI of the window, a row and column layout,
a channel that wakes the UI thread, the per-user registry, a serial port,
elevation, the service control manager, and the capture of a window to
pixels. This is one of the four crates that may use `unsafe`; every block
is a Win32 call or the reading of a structure Windows passed in.

`cargo run -p win32 --example sampler` shows every wrapped control once, to
be put beside a stock Windows program.

### 6. `ui/model`

Everything the window knows, with no Win32: the connection state, the line
table with its filter and sort (which says exactly which row a change
touched, and tracks the selection by line id), the cell text of every
column, the statistics panels with percentages that add up to 100, the
event log bounded to 5000 lines, the call history, and every command with
whether it can be chosen now and why not. 53 tests, one of them the
property that any sequence of events leaves the table equal to a fresh
`lines.list`.

### 7. `ui/manager`: the main window

The window of the handoff: title, menu bar, toolbar with the filter, the
line grid in its group box, the three panels, the status bar. The menus and
the toolbar are built from the model's list of commands, and the model
decides what can be chosen.

- `link.rs`: one thread owns the pipe, says hello and writes requests; a
  second reads and forwards each decoded message through a `UiSender`. A
  failure to connect, or a lost connection, is reported with its `RMD`
  message and tried again every 3 seconds.
- `requests.rs`: answers are matched to requests by id, and a request with
  no answer in 10 seconds fails with `RMD4001`.
- `window.rs`: the state (model, connection, requests) is changed in short
  steps that touch no control, and the controls are brought up to date
  after each step. `view.rs` makes, places and fills the controls.
- `settings.rs`: column widths, sort, filter, the bars and the window
  placement, under `HKCU\Software\Quartz Systems\rustmodem\Manager`.

### 8. `ui/manager`: dialogs and actions

Connect to service, New line and Edit line, the confirmation for Remove
line, Settings (a property sheet with a Service and a Control page, built
as a list of pages for later phases to add to), Ports and drivers, Call
history with export, the AT console (modeless, on a COM port of the
machine, not through the service), About, the two exports, and the service
commands. A dialog that needs the service asks through `Manager::call`,
which waits for the answer while the window behind goes on taking events.

Thirteen messages, `RMD4001` to `RMD4013`, in the new range for the manager.
Everything the manager tells the user is one of the catalogue's messages in
a task dialog, except the two questions it asks (Remove line, Install
service), which are not errors.

### 9. Tests and goldens

- `ui/manager/tests/screenshots.rs` (a program, `harness = false`): 16
  views against the frozen demo service, each compared with its golden. See
  criterion 3.
- `tests/smoke.rs`: a minute against the busy demo service. See criterion 4.
- `tests/reconnect.rs`: the demo service is killed and started again under
  the manager. See criterion 4.
- `tests/window.rs` and `tests/dialogs.rs`: the menus against the model,
  the window without a service, the grid cell for cell against the model,
  the line commands, adding, editing and removing a line through the
  dialogs, a refusal from the service keeping the dialog open, settings
  sent and the restart offered, the exports written, and the walk of the
  menu tree.

The tests reach the dialogs through `Hooks` in the manager's options: a
test is told when a dialog is up, and can stand in for the task dialogs and
the file dialogs, which otherwise wait for a person.

### 10. This record

`CLAUDE.md`, `README.md` and `CHANGELOG.md` are brought up to date and the
version is 0.0.2.

### Changes Cody asked for

Cody saw the screenshots of work package 9 and asked for these. They are
in one commit after it, and where they differ from the handoff they are
what stands.

- **The title** is `RustModem Manager` and nothing else, connected or not.
  The handoff's title named the configuration file, the number of lines,
  the computer and the mode; the lines are counted over the grid and the
  connection is in the status bar.
- **The program** is `rustmodem-manager.exe`, not `rustmodem-gui.exe`.
- **The State column** has a colour for every state that has a coloured
  image: green for online, yellow for ringing and handshaking, blue for
  idle, red for a port error. Not ready and disabled are in the control's
  own colour. The yellow is a dark one, so that it can be read on white.
- **The event log** is coloured by level, not by source: an error in red,
  a warning in amber, information in the control's own colour, detail in
  the system's grey.
- **Answer now and Hang up** have new images: a desk telephone's handset
  from the side, lifted and green, or on the hook and red.
- **Reload config** is gone from the toolbar. File, Reload configuration
  (Ctrl+R) stays, for a configuration file edited by hand.
- **Settings take effect at once.** `config.set` now puts a changed log
  level or log directory into effect while the service runs, and says that
  a restart is needed only for what it could not apply, which is the
  control pipe. The manager offers the restart only then. The server asks
  the service to apply the settings through `ServerParts::apply_service`;
  the service's logger takes a new level and a new directory
  (`Logger::reconfigure`). The call records, `calls.jsonl`, stay in the
  directory the service started with until it starts again.

## Markers

Every `BLOCKED` and `NEEDS-CODY` in the tree. Phase 0's `BLOCKED(phase1)`
on `&W` is gone: work package 2 built it.

| Marker | Location | What is deferred |
|---|---|---|
| `BLOCKED(phase2)` | `ui/model/src/commands.rs`, `Command::blocked`; `ui/manager/src/window.rs`, `action` | Everything under the SIP menu (Re-register, Trunk peers, Hunt groups, Routes, SIP trace) and Call capture. In the menus, disabled, with the reason in the status bar |
| `BLOCKED(phase2)` | `ui/model/src/lines.rs`, the cell text | The SIP and Hunt group columns show an em dash |
| `BLOCKED(phase2)` | `ui/model/src/lib.rs`, `status_parts` | The status bar says `SIP: off` and `RTP: —` |
| `BLOCKED(phase2)` | `ui/model/src/stats.rs` | RTP codec, jitter and packet loss show an em dash |
| `BLOCKED(phase2)` | `service/pool/src/lib.rs`, `TICK_MS` | From phase 0: engine ticks move to RTP packet arrival |
| `BLOCKED(phase2)` | `service/pool/src/router.rs` | From phase 0: a SIP router replaces `NullSwitch` |
| `BLOCKED(phase3)` | `ui/model/src/commands.rs`; `ui/manager/src/window.rs` | Rate cap and modulations, Line diagnostics, Loopback self-test. Disabled, with the reason |
| `BLOCKED(phase3)` | `ui/model/src/lines.rs` | The Retr. column shows an em dash: retrains are counted by a real modem engine |
| `BLOCKED(phase3)` | `ui/model/src/stats.rs`, `ENGINE` | "Softmodem engine" says `null`, from the model, not from the service |
| `BLOCKED(phase4)` | `ui/model/src/lines.rs` | The Err column shows an em dash: errors are counted by the error-control protocol |
| `BLOCKED(phase5)` | `service/svc/src/service.rs`, `service/svc/src/eventlog.rs` | From phase 0: the service account and the event message file |
| `BLOCKED(phase9)` | `ui/manager/src/dialogs/connect.rs` | The Computer field of Connect to service is there and disabled |
| `BLOCKED(phase9)` | `ui/model/src/lib.rs`, `status_parts` | The status bar says `CPU: —`: the service does not report its processor time |
| `BLOCKED(phase9)` | `service/ctlsrv/src/lib.rs` | From phase 0: TLS for remote control |
| `NEEDS-CODY` | `service/svc/src/ports.rs` | Verify the registry keys of the port inventory on a machine with com0com pairs and a standard modem |
| `NEEDS-CODY` | `dte/serial/src/com0com.rs` (three), `docs/COM0COM.md` | From phase 0: the com0com signal mapping |

## Where the build differs from the handoff

| Handoff | Built | Why |
|---|---|---|
| New crates: `ui/model`, `ui/win32`, `ui/glyph`, `ui/manager`, `tools/demosvc` | Also `service/pipe` | The demo service listens on a pipe and the manager connects to one. The transport was in `service/svc`, which neither can depend on. It moved, with its `unsafe`, into a crate of its own and gained a client. `CLAUDE.md` rule 8 now names it |
| The `#` column is 30 pixels wide | 48 | The state image is at the start of the row and takes 16 of them. At 30 the number did not fit |
| A reader thread "subscribes to every event" | The window sends `subscribe` as a request, first of those it sends after `service.info` | One path for every request: matched by id, timed out like the rest. The thread owns the pipe and the hello only |
| The grid is kept current by events | Also by `lines.list` every 10 seconds | Removing a line sends no event (a phase 0 decision), so a line another client removed would stay in the grid |
| Command line: `--pipe NAME`, `--demo` | Also `start`, `stop`, `restart`, `install`, `uninstall` | The handoff asks for "a command-line argument naming the action" for the relaunch as administrator; these are the arguments |
| The screenshot tests fail on a difference | They fail on CI, or with `RM_SCREENSHOTS=strict`. Elsewhere they report the differences and pass | The goldens are blessed on the CI runner image and do not match another machine's fonts: on the development machine all 16 views differed by 4 to 63 per mille. A check that every developer's `cargo test --workspace` fails would be skipped, not run |
| A view is compared whole | The title bar, the top 32 rows, is left out | Its colours say whether the window is the active one, which a test behind other windows does not decide. The title's text is asserted by the other tests |
| The capture is "the `ui/win32` capture function" of work package 5 | The function was rebuilt in work package 9 | `PrintWindow` with the compositor's rendering has the window draw all of itself again and does not wait: of the main window's sixty controls a varying number came out. The client area is now copied from the window's own surface, which is exact, and `PrintWindow` gives the frame only |
| The smoke test asserts "the model equals a fresh `lines.list`" | It compares who each line is and what it is doing (id, port, enabled, state, call), retrying for a moment when they agree | With calls running, byte counts move between the two readings. The reconnect test, against the frozen service, compares the whole of every line |
| The smoke test opens "every dialog by command id" | Every dialog but the service commands | Start, stop, restart, install and uninstall would ask for elevation on a developer's machine and act on the real service on a CI runner, which is an administrator |
| Each work package is one commit | Two more commits: `test(ctlsrv): wait for the port to reopen…` after work package 5, and the goldens' branch | A phase 0 test turned out to be flaky on CI and was fixed in a commit of its own. For the branch see the next row |
| "Goldens are blessed on the CI runner image" | A branch was pushed to run CI before each commit that changed the pictures went to `main`, then deleted | The pictures can only be had from a run, and a run needs a push. `main` never had a failing commit |
| The title, the toolbar, the colours of the State column and of the event log, and "`[service]` takes effect at the next start" | As Cody asked afterwards | See "Changes Cody asked for" above |
| Version 0.0.2 is set in work package 10 | It is. The About golden was taken at 0.0.1 | The digit changes 39 of the dialog's 91,935 pixels, 0.4 per mille, measured. The golden was not taken again |

## Decisions to confirm

Each of these is a choice the handoff left open. They are in the code and
the documentation as described; say if any should change.

**Control API and pool**

- The statistics count incoming calls: a call between two lines is one
  call, counted on the line that was called.
- "Rejected (line busy)" counts calls that met a busy line, and calls to a
  line that does not exist or cannot take one.
- `restart_required` compares the file's `[service]` and `[control]` with
  what is in effect, not with what they were before the change.
- A log directory that cannot be made or written refuses the change with
  `RMD0022`, and nothing is written to the configuration file.
- `config.reload` still applies the lines only: a `[service]` edited by
  hand takes effect at the next start, or when it is next set through the
  API.
- `ports.list` reads the registry only. It does not ask SetupAPI, so a port
  shows as Windows last recorded it.
- A line's `binding` is the modem and the host port of its pair, as
  `<modem> on <port>`, refreshed every 30 seconds.
- `log.tail` reaches back 2000 lines.

**Stored profiles (`dte/at`)**

- There is one stored profile a line: `&W0` and `&W1` store the same one.
- An operator's reset (`lines.reset`) restores the stored profile, as `Z`
  does.
- S1, the ring count, is stored as 0.
- Removing a line deletes its stored profile.

**Demo service**

- `--pipe NAME`, defaulting to `rustmodem-demo`, so that tests can run
  several at once.
- Its engine "connects" at 2400 V.22bis, 1200 V.22 and 300 V.21 in turn, so
  that the connect rates have something to show. Nothing is modulated.
- The frozen script drops the control API's own log lines (a client
  connected, a client left), which would differ from run to run.

**Model**

- Thousands are separated with a comma, whatever the user's locale.
- "Bytes TX/RX" in the grid is the line's total since the service started,
  not the current call's.
- More than five connect rates fold the smallest into "Other".

**Manager**

- The filter's label is `Show:` with the mnemonic on `w`: `S` opens the SIP
  menu.
- Without a service the grid keeps the lines it had, disabled, until it is
  connected again.
- Disconnect stops the retries. Connect to service starts them again.
- The access-denied message is shown once, not at every retry.
- New line offers the free ends of com0com pairs. Of a pair with one end
  under a `COM` name, it offers the other end.
- Edit line sends only what was changed: changing a port hangs up, so an
  untouched port is not sent.
- Uninstall service does not ask first; its menu item has no ellipsis.
  Install service asks.
- A dialog waits for the service with its window disabled, for up to the
  10 seconds any request gets.
- The AT console opens at 115200 8N1, which a com0com pair ignores, and
  shows the modem's signals as disabled check boxes.
- CSV exports start with a byte order mark, so that a spreadsheet reads the
  em dashes.
- The accelerators (Ctrl+R, Ctrl+N, F5) are the main window's: in the AT
  console they type. Enter and Del act on the grid when it has the focus.

**Tests**

- A view is taken to have finished drawing when three pictures of it,
  250 ms apart, are the same to within 64 pixels (a caret blinks).
- The test window is 1040 by 720: the CI runner's screen is 1024 by 768 and
  a window is not let be larger than its screen.
- The handle margin of the smoke test is 48 GDI and 48 USER objects.

## References consulted

| Reference | For | How |
|---|---|---|
| `quartz-systems-access`: `qsa-glyph`, `qsa-icons` | `ui/glyph` | Quartz's own, MIT. The approach (icons drawn in code, `.ico` and `.res` written at build time) was reused and its code adapted, with the rustmodem header. `qsa-ui` was not used |
| VoIPModem Manager | The layout | **Not looked at.** Neither its source, its resource scripts, its icons nor a screenshot. The layout is the handoff's text and nothing else |
| `docs/design/` | The layout | Did not exist. There were no screenshots to match |
| Microsoft's Win32 documentation | `ui/win32` | From memory: the dialog template formats, the property sheet, the task dialog, the list view's notifications. No sample code was copied |
| The Windows `.res`, `.ico` and manifest formats | `ui/glyph` | From memory of their documentation |
| com0com `ReadMe.txt` | `service/svc/src/ports.rs` | The names of the driver's parameters, from memory (see "What needs Cody") |
| ITU-T V.250 | `dte/at`, stored profiles | The ranges and defaults a stored profile is checked against are the ones phase 0 cited by clause, with the same caveat: the citations are from memory. `&W` itself is not in V.250; it is the Hayes command |

libv90modem, spandsp and linmodem were not opened. Phase 1 has no
modulation.

## Dependencies

| Crate | Used by | Licence |
|---|---|---|
| `png` (tests only) | `ui/manager`, to read the goldens | MIT OR Apache-2.0 |
| `windows`, more features | `ui/win32`, `service/pipe` | MIT OR Apache-2.0 |
| `serde`, `serde_json` | `ui/manager` | MIT OR Apache-2.0 |

Nothing else is new. `cargo deny check` passes. `png` brings `bitflags`,
`crc32fast`, `fdeflate`, `flate2`, `miniz_oxide`, `simd-adler32` and
`adler2`, all pure Rust; nothing compiles C. It is a dev-dependency: it is
in no binary.

## Exit criteria

### Automated

| # | Criterion | How it was shown |
|---|---|---|
| 1 | fmt, clippy with `-D warnings`, `cargo deny check`, `ci/headers.sh` and all tests pass on Windows; the pure crates, with `ui/model` and `ui/glyph`, pass on Linux | Run locally on Windows before every commit, and the pure crates under WSL. The GitHub Actions run for every commit pushed to `main` succeeded |
| 2 | Every addition of work package 1 is in `docs/CONTROL_API.md` and has a round-trip test over the in-memory transport and over a real pipe | The document is generated from the tables the tests check. `every_method_round_trips` in `service/ctlsrv/tests/api.rs` and `every_method_round_trips_over_a_real_pipe` in `service/pipe/tests/api.rs` go through the method list and fail on a method with no case; both lists have twenty methods |
| 3 | The screenshot tests pass against the committed goldens for the main window and every dialog | 16 views: the main window (connected, with a line selected, and without a service), Connect to service, New line, Edit line, Call history, both pages of Settings, Ports and drivers, About, the AT console, and four task dialogs (the Remove line question, a message from the service, the offer to restart, access denied). Blessed on the GitHub runner image `windows-2025-vs2026`, version 20261116.315.1 (Windows Server 2025), from the `screenshots` artifact of a run on that image. A pixel differs beyond 40 in any channel; a view fails beyond 2 per mille |
| 4 | The smoke test and the reconnect test pass | `a_minute_of_everything_leaks_nothing_and_ends_with_the_model_right`: about twenty rounds of every dialog, every filter and every column against twelve busy lines, no message shown, GDI and USER objects within the margin, the model as the service has it. `a_service_that_is_killed_and_started_again_is_lost_and_found`: disconnected with its `RMD` reason, grid and line commands disabled, then connected to the new service with every cell and every line right and the selection kept |
| 5 | Every menu item, toolbar button and panel exists; each works or is disabled with a marker and a reason; a test walks the menu tree | `the_menu_bar_is_the_one_the_model_lists` compares the window's real menu with the model's. `no_menu_item_is_enabled_without_a_handler` walks every item of every menu with no line, with each kind of line selected, and disconnected, and fails on one that is enabled and has no handler, or disabled with no reason. `every_command_that_is_built_has_something_that_carries_it_out` ties the handlers to the markers: a command has none exactly when the model marks it `BLOCKED` |

### Manual checklist

Build first:

```
cargo build --release -p svc -p ctl -p manager -p demosvc
```

The programs are in `target\release`.

**6. The look.**

- [ ] Start the demo service and the manager, unelevated:

  ```
  target\release\rustmodem-demosvc --script busy --lines 12
  target\release\rustmodem-manager --demo
  ```

- [ ] Put the window beside Event Viewer or Device Manager. Fonts, control
      heights, the list view's selection and hover, the menus, the toolbar,
      the status bar's grip and the focus rectangles are the system's.
- [ ] Keyboard: Alt opens the menus, each mnemonic works, Tab goes from the
      filter to the grid to the log, arrow keys move in the grid, Enter
      opens Edit line, Del asks about removing, the context-menu key opens
      the Lines menu under the selected row, F5, Ctrl+N and Ctrl+R work.
- [ ] Move the highlight through every menu: the status bar says what each
      command does, and for each disabled one why not.
- [ ] Click each column header twice: the rows sort both ways and the arrow
      follows. Select a line and watch it through a few calls: the
      selection stays on its line.
- [ ] Open every dialog. Each looks like a dialog of the system's.
- [ ] Close the manager and start it again: the column widths, the sort,
      the filter and the window's place are as they were left.
- [ ] The same for the sampler: `cargo run -p win32 --example sampler`.

**7. Scaling.**

- [ ] At 100, 150 and 200 percent (Settings, Display, Scale; sign out and
      in between each), start the manager against the demo service. Nothing
      is clipped, overlaps or is blurred; the icons are sharp.
- [ ] With two monitors at different scaling, drag the window from one to
      the other: fonts, images, column widths and layout follow.
- [ ] Turn on a high-contrast theme (Settings, Accessibility, Contrast
      themes). Every colour is the theme's: "Online" and "Port error" are
      in the text colour, and so is the event log.

**8. Against the real service.**

- [ ] With the two com0com pairs and the configuration of phase 0's
      checklist, from an elevated prompt:

  ```
  target\release\rustmodem-svc --console
  target\release\rustmodem-manager
  ```

- [ ] The title is `RustModem Manager`. The group box over the grid counts
      the lines, and the status bar says `Service: connected`.
- [ ] Make phase 0's call by hand. The grid follows each state within a
      second: handshaking, ringing, online, idle. The selection and the
      scroll position stay.
- [ ] Ring a line and choose Answer now. Hang up a call with Hang up. Reset
      a line. Disable a line and enable it.
- [ ] The Binding column shows each line's host port, and the standard
      modem if one is installed on it. Tools, Ports and drivers shows both
      ends of each pair, with the lines. This is the check of the
      `NEEDS-CODY` in `service/svc/src/ports.rs`.
- [ ] In a terminal on a host port, `ATS0=2&W`, then Reset the line in the
      manager, then `ATS0?`: `002`. `profiles.toml` is beside the
      configuration file.
- [ ] Tools, Settings: change the log level to Debug, OK. Nothing is
      offered; the log has `DEBUG` lines from then on, and the
      configuration file has the change with its comments kept. Change the
      log directory: the log goes on in the new one.
- [ ] Tools, Settings, Control: change the pipe name, OK. The manager
      offers to restart the service. Change it back.
- [ ] Stop the service with Ctrl+C: the manager says why it is not
      connected within a few seconds, and finds the service again when it
      is started.

**9. Elevation.**

- [ ] With the service running, start `target\release\rustmodem-manager` from a
      prompt that is not elevated. The access-denied message (`RMD2008`)
      appears once, with "Restart as administrator".
- [ ] Choose it and accept the prompt: the first window closes and an
      elevated one opens, connected.
- [ ] Start it unelevated again and close the message. In the File menu the
      five service commands carry the shield.
- [ ] Copy `rustmodem-svc.exe` beside `rustmodem-manager.exe` if it is not
      there. File, Install service: it asks, then asks for elevation; the
      elevated manager reports `Service rustmodem installed`.
- [ ] File, Start service; Restart service; Stop service: each reports that
      the service was started, restarted or stopped, and `sc query
      rustmodem` agrees.
- [ ] File, Uninstall service: reported, and `sc query rustmodem` no longer
      finds it.
- [ ] Decline an elevation prompt: nothing happens and nothing is reported.

**10. The AT console.**

- [ ] With the service running on the pairs, Modem, AT console. Pick the
      host port of line 1 and choose Open.
- [ ] `AT` and Enter: `OK` appears.
- [ ] Clear DTR: the line goes to "Not ready" in the grid. Set it again:
      "Idle".
- [ ] From another line, dial line 1: RI follows the ring. `ATA`: DCD comes
      on with `CONNECT`. DSR and CTS are as the signal checklist at the end
      of `docs/COM0COM.md` says for the pair's wiring.
- [ ] Close the console window while the port is open: the port is
      released, and a terminal can open it.

## Known limits of phase 1

- Nothing here has been seen by a person. The automated tests show that the
  window holds the right things in the right controls and does not leak;
  they do not show that it looks right.
- The code that follows a change of DPI (`View::dpi_changed`) has never
  run: every test is at 96 DPI.
- The serial port wrapper has opened no port, the service control wrapper
  has controlled no service, and the relaunch as administrator has never
  been taken. They are small and are checked by criteria 9 and 10.
- The manager manages the computer it runs on. The Computer field is the
  place for the rest.
- The event log pane holds 5000 lines and the call history 1000.
- The themed progress bars of the connect rates glint from time to time. A
  screenshot can catch it: about a hundred pixels, well inside the limit.
- The manager's tests open real windows on the desktop of whoever runs
  them, and the screenshot tests move the pointer to the corner of the
  screen for as long as they run.
