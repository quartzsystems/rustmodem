# Installing RustModem from the release zip

A release is a zip of the three programs, a sample configuration and the
documentation. There is no installer yet: that, and the virtual COM driver
that makes com0com unnecessary, are phase 5. This is the install by hand.

It needs 64-bit Windows, an account that is an administrator, and
[com0com](https://com0com.sourceforge.net/), which is a separate product
under its own licence and is not in the zip.

## What is in the zip

| File | What it is |
|---|---|
| `rustmodem-svc.exe` | The service |
| `rustmodem-manager.exe` | The manager, "RustModem Manager" |
| `rustmodem-ctl.exe` | The command-line client |
| `rustmodem.sample.toml` | A configuration to start from |
| `LICENSE`, `README.md`, `CHANGELOG.md` | The licence, what RustModem is, and what each version changed |
| `docs\` | The documentation, this file among it |

## Install

1. **Unpack** the zip into a folder of its own, such as
   `C:\Program Files\rustmodem`. Keep the three programs together: the
   manager looks for `rustmodem-svc.exe` beside itself.
2. **Install com0com** and make a pair of ports for each line, wired as
   `docs\COM0COM.md` says.
3. **Write the configuration.** The service reads
   `%ProgramData%\Quartz Systems\RustModem\rustmodem.toml` unless it is
   told another path. Make the folder, copy `rustmodem.sample.toml` there
   as `rustmodem.toml`, and edit it: a `[[line]]` for each pair, naming
   RustModem's port of the pair, and `[sip]` for the network the calls
   come from. `docs\TELEPHONY.md` has `[sip]`, `docs\MODEM.md` has
   `[modem]`.
4. **Try it in the foreground**, from an elevated prompt in the folder of
   step 1:

   ```
   rustmodem-svc --console
   ```

   A configuration with a mistake in it is refused with a message that
   names the line and the key. In another elevated prompt:

   ```
   rustmodem-ctl info
   rustmodem-ctl lines
   ```

   Ctrl+C stops the service.
5. **Install the service** and start it:

   ```
   rustmodem-svc --install
   net start rustmodem
   ```

   To use a configuration somewhere else, give its path:
   `rustmodem-svc --install D:\modems\rustmodem.toml`. The service starts
   with Windows from then on.
6. **Start the manager**, `rustmodem-manager.exe`. The service's control
   pipe admits SYSTEM and Administrators only; started without
   administrator rights the manager says so and offers to start again
   with them.

Then, for dial-up networking, `docs\RRAS.md`.

## Check

- [ ] `rustmodem-ctl info` prints the version of the zip.
- [ ] `rustmodem-ctl lines` lists every line of the configuration.
- [ ] `rustmodem-ctl selftest 1 2` passes: the modems of lines 1 and 2 call
      each other inside the service and each sends the other 64 KiB.
- [ ] The checklist at the end of `docs\COM0COM.md` passes for one pair.

## Where things are

| What | Where |
|---|---|
| The configuration | `%ProgramData%\Quartz Systems\RustModem\rustmodem.toml`, or the path given to `--install` |
| Stored AT profiles (`AT&W`) | `profiles.toml` beside the configuration |
| The log | `%ProgramData%\Quartz Systems\RustModem\logs`, unless `[service] log_dir` names another; `rustmodem-ctl log` prints its end |
| Call captures | `captures` under the log folder, unless `[media] capture_dir` names another, when a line's capture is on |

## Upgrade

Stop the service, replace the three programs with those of the new zip,
and start it:

```
net stop rustmodem
net start rustmodem
```

The configuration and the stored profiles are kept: they are not in the
program folder. `CHANGELOG.md` says what a version changes in the
configuration, if anything.

## Remove

```
net stop rustmodem
rustmodem-svc --uninstall
```

Then delete the program folder, and the folder of the configuration if
nothing in it is wanted. com0com has its own uninstaller.
