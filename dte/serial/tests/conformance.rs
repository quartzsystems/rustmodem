// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The serial conformance suite: one set of tests of what a port must do,
//! run on every backend there is to run it on.
//!
//! A port under test has two sides. The modem's side is a
//! [`SerialBackend`], which is what the service has. The host's side is a
//! [`Host`]: for the memory backend the handle the backend comes with, and
//! for a real port the ordinary Win32 serial API on its COM name, which is
//! what RRAS and a terminal have.
//!
//! | Backend | Runs when |
//! |---|---|
//! | memory | Always |
//! | vcom | `RM_VCOM_PORT` names a RustModem port, as `COM40`, that nothing else has open |
//! | com0com | `RM_COM0COM_PAIR` names a pair set up as `docs/COM0COM.md` says, the service's port and then the host's, as `CNCB0,COM8` |
//!
//! A backend that is not there is said to be skipped, and so is a test of
//! something a backend's host side cannot do: the memory port has no
//! timeouts, no wait mask and no purge, which belong to the driver.

use std::io;
use std::time::{Duration, Instant};

use serial::{ModemSignals, SerialBackend, SerialEvent, memory_pair};

/// What a host's read is told to do when nothing more comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct ReadTimeouts {
    interval: u32,
    multiplier: u32,
    constant: u32,
}

/// The host's side of a port under test.
#[cfg_attr(not(windows), allow(dead_code))]
trait Host {
    /// Opens the port as a program does. False if it is refused.
    fn open(&mut self) -> bool;
    fn close(&mut self);
    /// Writes all of `data`.
    fn write(&mut self, data: &[u8]);
    /// Reads what has arrived, up to `most` octets, without waiting.
    fn read(&mut self, most: usize) -> Vec<u8>;
    fn signals(&mut self) -> ModemSignals;
    fn set_dtr(&mut self, on: bool);
    fn set_rts(&mut self, on: bool);

    /// Whether this is a real serial port, with the Win32 serial API.
    fn is_win32(&self) -> bool {
        false
    }
    /// A second open of the same port while this one has it. True if it
    /// was let in.
    fn second_open_succeeds(&mut self) -> bool {
        unreachable!("not a Win32 port")
    }
    /// A read of `want` octets under these timeouts. Gives what it read
    /// and how long it took. A read that has not ended after `limit` is
    /// cancelled, and its time is then at least `limit`.
    fn timed_read(
        &mut self,
        _: ReadTimeouts,
        _want: usize,
        _limit: Duration,
    ) -> (Vec<u8>, Duration) {
        unreachable!("not a Win32 port")
    }
    /// Sets the wait mask, starts a wait, does `cause`, and gives the
    /// events the wait ended with, or `None` if it did not end.
    fn wait_event(&mut self, _mask: u32, _cause: &mut dyn FnMut()) -> Option<u32> {
        unreachable!("not a Win32 port")
    }
    /// Starts a wait and then sets a new mask. Gives what the wait ended
    /// with.
    fn wait_ended_by_mask(&mut self, _mask: u32) -> Option<u32> {
        unreachable!("not a Win32 port")
    }
    fn purge(&mut self, _flags: u32) {
        unreachable!("not a Win32 port")
    }
    fn set_break(&mut self, _on: bool) {
        unreachable!("not a Win32 port")
    }
    /// Writes and gives whether the write ended, and with how many
    /// octets, within the time.
    fn try_write(&mut self, _data: &[u8], _limit: Duration) -> Option<usize> {
        unreachable!("not a Win32 port")
    }
}

/// A port under test.
struct Bench {
    name: &'static str,
    backend: Option<Box<dyn SerialBackend>>,
    host: Box<dyn Host>,
    /// Opens the modem's side again after it was dropped.
    reopen: Box<dyn Fn() -> Option<Box<dyn SerialBackend>>>,
}

impl Bench {
    fn backend(&mut self) -> &mut dyn SerialBackend {
        self.backend
            .as_deref_mut()
            .expect("the modem's side is open")
    }

    /// Everything the host has done, as the modem's side is told it,
    /// until nothing more comes for a moment.
    fn events(&mut self) -> Vec<SerialEvent> {
        let mut seen = Vec::new();
        let mut quiet_since = Instant::now();
        while quiet_since.elapsed() < Duration::from_millis(150) {
            match self.backend().poll().expect("poll") {
                Some(event) => {
                    seen.push(event);
                    quiet_since = Instant::now();
                }
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        seen
    }

    /// Reads from the modem's side until `want` octets have come, or two
    /// seconds have passed with nothing.
    fn modem_reads(&mut self, want: usize) -> Vec<u8> {
        let mut got = Vec::new();
        let mut buffer = [0u8; 4096];
        let mut quiet_since = Instant::now();
        while got.len() < want && quiet_since.elapsed() < Duration::from_secs(2) {
            let most = buffer.len().min(want - got.len());
            let count = self.backend().read(&mut buffer[..most]).expect("read");
            if count == 0 {
                std::thread::sleep(Duration::from_millis(1));
            } else {
                got.extend(&buffer[..count]);
                quiet_since = Instant::now();
            }
        }
        got
    }

    /// Writes all of `data` from the modem's side, the host reading as
    /// it goes. Gives what the host read.
    fn modem_writes(&mut self, data: &[u8]) -> Vec<u8> {
        let mut sent = 0;
        let mut got = Vec::new();
        let mut quiet_since = Instant::now();
        while got.len() < data.len() && quiet_since.elapsed() < Duration::from_secs(2) {
            let mut moved = false;
            if sent < data.len() {
                let end = data.len().min(sent + 4096);
                let taken = self.backend().write(&data[sent..end]).expect("write");
                sent += taken;
                moved |= taken > 0;
            }
            let read = self.host.read(8192);
            moved |= !read.is_empty();
            got.extend(read);
            if moved {
                quiet_since = Instant::now();
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        got
    }
}

struct MemoryPort(serial::MemoryHost);

impl Host for MemoryPort {
    fn open(&mut self) -> bool {
        self.0.open();
        true
    }
    fn close(&mut self) {
        self.0.close();
    }
    fn write(&mut self, data: &[u8]) {
        self.0.write(data);
    }
    fn read(&mut self, most: usize) -> Vec<u8> {
        self.0.read_some(most)
    }
    fn signals(&mut self) -> ModemSignals {
        self.0.signals()
    }
    fn set_dtr(&mut self, on: bool) {
        self.0.set_dtr(on);
    }
    fn set_rts(&mut self, on: bool) {
        self.0.set_rts(on);
    }
}

/// The ports there are to test, each with its host's side closed.
fn benches() -> Vec<Bench> {
    let mut all = Vec::new();
    let (backend, host) = memory_pair();
    all.push(Bench {
        name: "memory",
        backend: Some(Box::new(backend)),
        host: Box::new(MemoryPort(host)),
        // A memory port's two ends are made together and not again.
        reopen: Box::new(|| None),
    });
    #[cfg(windows)]
    win32::add(&mut all);
    #[cfg(not(windows))]
    println!("vcom and com0com: skipped, they are ports of Windows");
    all
}

/// The benches whose host side is a real serial port. Says so when a
/// test has none to run on.
fn win32_benches(what: &str) -> Vec<Bench> {
    let all: Vec<Bench> = benches()
        .into_iter()
        .filter(|b| b.host.is_win32())
        .collect();
    if all.is_empty() {
        println!("{what}: skipped, no vcom or com0com port to test (see the top of this file)");
    }
    all
}

/// Octets that are not the same anywhere a mistake could move them to.
fn pattern(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as u8
        })
        .collect()
}

// The tests share real ports, which one test at a time can have.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn alone() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn the_modem_is_told_of_the_host_opening_and_closing() {
    let _alone = alone();
    for mut bench in benches() {
        let name = bench.name;
        assert_eq!(bench.events(), [], "{name}: nothing before the open");
        assert!(bench.host.open(), "{name}");
        let events = bench.events();
        // The open, and DTR and RTS coming on, in the order they come.
        assert_eq!(
            events.first(),
            Some(&SerialEvent::HostOpened),
            "{name}: {events:?}"
        );
        assert!(
            events.contains(&SerialEvent::Dtr(true)),
            "{name}: {events:?}"
        );
        assert!(
            events.contains(&SerialEvent::Rts(true)),
            "{name}: {events:?}"
        );
        bench.host.close();
        let events = bench.events();
        assert_eq!(
            events.last(),
            Some(&SerialEvent::HostClosed),
            "{name}: {events:?}"
        );
        assert!(
            events.contains(&SerialEvent::Dtr(false)),
            "{name}: {events:?}"
        );
        // And again: a port is opened many times in its life.
        assert!(bench.host.open(), "{name}");
        assert_eq!(
            bench.events().first(),
            Some(&SerialEvent::HostOpened),
            "{name}"
        );
        bench.host.close();
        assert_eq!(
            bench.events().last(),
            Some(&SerialEvent::HostClosed),
            "{name}"
        );
    }
}

#[test]
fn the_host_has_the_port_to_itself() {
    let _alone = alone();
    for mut bench in win32_benches("exclusivity") {
        assert!(bench.host.open(), "{}", bench.name);
        assert!(
            !bench.host.second_open_succeeds(),
            "{}: a second program opened the port",
            bench.name
        );
        bench.host.close();
    }
}

#[test]
fn ten_mebibytes_cross_each_way_intact() {
    let _alone = alone();
    const LENGTH: usize = 10 * 1024 * 1024;
    for mut bench in benches() {
        let name = bench.name;
        assert!(bench.host.open(), "{name}");
        bench.events();

        let to_host = pattern(LENGTH, 0x1234_5678);
        let got = bench.modem_writes(&to_host);
        assert_eq!(got.len(), to_host.len(), "{name}: to the host");
        assert!(got == to_host, "{name}: to the host, an octet changed");

        let to_modem = pattern(LENGTH, 0x8765_4321);
        let mut got = Vec::with_capacity(LENGTH);
        for chunk in to_modem.chunks(2048) {
            bench.host.write(chunk);
            got.extend(bench.modem_reads(chunk.len()));
        }
        assert_eq!(got.len(), to_modem.len(), "{name}: to the modem");
        assert!(got == to_modem, "{name}: to the modem, an octet changed");
        bench.host.close();
    }
}

#[test]
fn each_line_is_seen_at_the_other_side() {
    let _alone = alone();
    for mut bench in benches() {
        let name = bench.name;
        assert!(bench.host.open(), "{name}");
        bench.events();
        let off = ModemSignals::default();
        let cases = [
            ModemSignals { dcd: true, ..off },
            ModemSignals { dsr: true, ..off },
            ModemSignals { cts: true, ..off },
            ModemSignals { ri: true, ..off },
            ModemSignals {
                dcd: true,
                dsr: true,
                cts: true,
                ri: true,
            },
            off,
        ];
        for lines in cases {
            bench.backend().set_signals(lines).expect("set_signals");
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(bench.host.signals(), lines, "{name}");
        }
        // And the host's two lines, each by itself.
        bench.host.set_dtr(false);
        assert_eq!(bench.events(), [SerialEvent::Dtr(false)], "{name}");
        bench.host.set_rts(false);
        assert_eq!(bench.events(), [SerialEvent::Rts(false)], "{name}");
        bench.host.set_rts(true);
        assert_eq!(bench.events(), [SerialEvent::Rts(true)], "{name}");
        bench.host.set_dtr(true);
        assert_eq!(bench.events(), [SerialEvent::Dtr(true)], "{name}");
        bench.host.close();
    }
}

#[test]
fn a_read_ends_as_the_timeouts_say() {
    let _alone = alone();
    const MAXDWORD: u32 = u32::MAX;
    let limit = Duration::from_millis(1500);
    for mut bench in win32_benches("timeouts") {
        let name = bench.name;
        assert!(bench.host.open(), "{name}");
        bench.events();
        let timeouts = |interval, multiplier, constant| ReadTimeouts {
            interval,
            multiplier,
            constant,
        };
        let soon = Duration::from_millis(150);

        // Interval MAXDWORD, no totals: at once, with nothing.
        let (data, took) = bench.host.timed_read(timeouts(MAXDWORD, 0, 0), 8, limit);
        assert!(
            data.is_empty() && took < soon,
            "{name}: immediate, empty: {took:?}"
        );

        // The same, with what is there.
        bench.modem_writes_quietly(b"abc");
        let (data, took) = bench.host.timed_read(timeouts(MAXDWORD, 0, 0), 8, limit);
        assert!(
            data == b"abc" && took < soon,
            "{name}: immediate: {data:?} {took:?}"
        );

        // Interval and multiplier MAXDWORD: what is there, at once.
        bench.modem_writes_quietly(b"de");
        let (data, took) = bench
            .host
            .timed_read(timeouts(MAXDWORD, MAXDWORD, 400), 8, limit);
        assert!(
            data == b"de" && took < soon,
            "{name}: first byte, waiting: {data:?} {took:?}"
        );

        // And with nothing there, the constant.
        let (data, took) = bench
            .host
            .timed_read(timeouts(MAXDWORD, MAXDWORD, 400), 8, limit);
        assert!(data.is_empty(), "{name}: first byte, none: {data:?}");
        assert!(
            took >= Duration::from_millis(350) && took < Duration::from_millis(900),
            "{name}: first byte, none: {took:?}"
        );

        // A total constant: what came by then.
        bench.modem_writes_quietly(b"fg");
        let (data, took) = bench.host.timed_read(timeouts(0, 0, 300), 8, limit);
        assert!(data == b"fg", "{name}: total: {data:?}");
        assert!(
            took >= Duration::from_millis(250) && took < Duration::from_millis(800),
            "{name}: total: {took:?}"
        );

        // A total of multiplier times octets plus constant.
        let (data, took) = bench.host.timed_read(timeouts(0, 50, 100), 4, limit);
        assert!(data.is_empty(), "{name}: multiplier: {data:?}");
        assert!(
            took >= Duration::from_millis(250) && took < Duration::from_millis(800),
            "{name}: multiplier: {took:?}"
        );

        // A read that fills ends at once, whatever its timeouts.
        bench.modem_writes_quietly(b"hijk");
        let (data, took) = bench.host.timed_read(timeouts(0, 0, 1000), 4, limit);
        assert!(
            data == b"hijk" && took < soon,
            "{name}: full: {data:?} {took:?}"
        );

        // An interval is timed from the first octet: with one there, the
        // read ends an interval later and not at the total.
        bench.modem_writes_quietly(b"l");
        let (data, took) = bench.host.timed_read(timeouts(200, 0, 1200), 4, limit);
        assert!(data == b"l", "{name}: interval: {data:?}");
        assert!(
            took >= Duration::from_millis(150) && took < Duration::from_millis(700),
            "{name}: interval: {took:?}"
        );

        // No timeouts at all: the read waits for as long as it is let.
        let (data, took) = bench
            .host
            .timed_read(timeouts(0, 0, 0), 4, Duration::from_millis(400));
        assert!(
            data.is_empty() && took >= Duration::from_millis(400),
            "{name}: none: {took:?}"
        );
        bench.host.close();
    }
}

impl Bench {
    /// Writes from the modem's side and gives the octets a moment to be
    /// at the host's.
    fn modem_writes_quietly(&mut self, data: &[u8]) {
        let taken = self.backend().write(data).expect("write");
        assert_eq!(taken, data.len(), "{}", self.name);
        std::thread::sleep(Duration::from_millis(60));
    }
}

/// The events of the Win32 serial API.
#[cfg_attr(not(windows), allow(dead_code))]
mod ev {
    pub const RXCHAR: u32 = 0x0001;
    pub const TXEMPTY: u32 = 0x0004;
    pub const CTS: u32 = 0x0008;
    pub const DSR: u32 = 0x0010;
    pub const RLSD: u32 = 0x0020;
    pub const RING: u32 = 0x0100;
}

#[test]
fn each_event_ends_a_wait() {
    let _alone = alone();
    for mut bench in win32_benches("wait-mask events") {
        let name = bench.name;
        assert!(bench.host.open(), "{name}");
        bench.events();
        let off = ModemSignals::default();
        let lines: [(&str, u32, ModemSignals); 4] = [
            ("EV_CTS", ev::CTS, ModemSignals { cts: true, ..off }),
            ("EV_DSR", ev::DSR, ModemSignals { dsr: true, ..off }),
            ("EV_RLSD", ev::RLSD, ModemSignals { dcd: true, ..off }),
            ("EV_RING", ev::RING, ModemSignals { ri: true, ..off }),
        ];
        let Bench { backend, host, .. } = &mut bench;
        let backend = backend.as_deref_mut().expect("open");
        for (event, bit, signals) in lines {
            backend.set_signals(off).expect("set_signals");
            let seen = host.wait_event(bit, &mut || {
                backend.set_signals(signals).expect("set_signals");
            });
            assert_eq!(seen.map(|e| e & bit), Some(bit), "{name}: {event}");
        }
        backend.set_signals(off).expect("set_signals");

        let seen = host.wait_event(ev::RXCHAR, &mut || {
            backend.write(b"a").expect("write");
        });
        assert_eq!(seen, Some(ev::RXCHAR), "{name}: EV_RXCHAR");
        host.read(16);

        // The output queue empties when the modem's side has read it.
        let mut wrote = false;
        let seen = host.wait_event(ev::TXEMPTY, &mut || {
            if !wrote {
                wrote = true;
            }
        });
        // Nothing was sent, so nothing emptied.
        assert_eq!(seen, None, "{name}: EV_TXEMPTY with nothing sent");
        host.write(b"AT\r");
        let seen = host.wait_event(ev::TXEMPTY, &mut || {
            let mut buffer = [0u8; 16];
            let deadline = Instant::now() + Duration::from_secs(1);
            let mut got = 0;
            while got < 3 && Instant::now() < deadline {
                got += backend.read(&mut buffer).expect("read");
            }
        });
        // A port may say so the moment the write ends, before the wait
        // begins: then there is nothing left to wait for.
        assert!(
            seen == Some(ev::TXEMPTY) || seen.is_none(),
            "{name}: EV_TXEMPTY: {seen:?}"
        );

        // A new mask ends a wait with no events.
        assert_eq!(
            host.wait_ended_by_mask(ev::RXCHAR),
            Some(0),
            "{name}: a new mask"
        );
        bench.host.close();
    }
}

#[test]
fn a_purge_empties_what_it_says_and_a_break_harms_nothing() {
    let _alone = alone();
    // PURGE_RXCLEAR.
    const RXCLEAR: u32 = 0x0008;
    for mut bench in win32_benches("purge and break") {
        let name = bench.name;
        assert!(bench.host.open(), "{name}");
        bench.events();
        bench.modem_writes_quietly(b"to be thrown away");
        bench.host.purge(RXCLEAR);
        assert_eq!(bench.host.read(64), b"", "{name}: after the purge");
        bench.modem_writes_quietly(b"kept");
        assert_eq!(bench.host.read(64), b"kept", "{name}: after the purge");

        bench.host.set_break(true);
        bench.host.set_break(false);
        // A break is not something a modem's side is told as an event of
        // its own, and it must not look like one of the others.
        assert_eq!(bench.events(), [], "{name}: a break");
        bench.host.write(b"AT\r");
        assert_eq!(bench.modem_reads(3), b"AT\r", "{name}: after the break");
        bench.host.close();
    }
}

#[test]
fn a_port_whose_modem_side_has_gone_is_dead_and_comes_back() {
    let _alone = alone();
    for mut bench in win32_benches("the other side going away") {
        let name = bench.name;
        assert!(bench.host.open(), "{name}");
        bench.events();
        let up = ModemSignals {
            dcd: true,
            dsr: true,
            cts: true,
            ri: false,
        };
        bench.backend().set_signals(up).expect("set_signals");
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(bench.host.signals(), up, "{name}");

        // The service lets go of the port with the host still on it.
        bench.backend = None;
        std::thread::sleep(Duration::from_millis(100));
        let seen = bench.host.signals();
        println!("{name}: with the modem's side gone the host sees {seen:?}");
        if name == "vcom" {
            // A RustModem port says so: every line is low.
            assert_eq!(seen, ModemSignals::default(), "{name}");
            // And what the host writes goes nowhere, without holding it.
            assert_eq!(
                bench.host.try_write(b"AT\r", Duration::from_secs(2)),
                Some(3),
                "{name}"
            );
        }

        // The service comes back, and finds the host as it is.
        bench.backend = (bench.reopen)();
        assert!(
            bench.backend.is_some(),
            "{name}: the modem's side opens again"
        );
        let events = bench.events();
        println!("{name}: on coming back the modem's side is told {events:?}");
        if name == "vcom" {
            assert_eq!(
                events,
                [
                    SerialEvent::HostOpened,
                    SerialEvent::Dtr(true),
                    SerialEvent::Rts(true)
                ],
                "{name}"
            );
        }
        bench.backend().set_signals(up).expect("set_signals");
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(bench.host.signals(), up, "{name}");
        bench.host.write(b"AT\r");
        assert_eq!(bench.modem_reads(3), b"AT\r", "{name}");
        bench.host.close();
    }
}

#[test]
fn a_failing_port_is_an_error_and_not_a_panic() {
    // Only the memory port can be made to fail on purpose.
    let (mut backend, host) = memory_pair();
    host.inject_failure(Some(io::ErrorKind::BrokenPipe));
    assert!(backend.read(&mut [0; 4]).is_err());
    assert!(backend.write(b"a").is_err());
    assert!(backend.poll().is_err());
    assert!(backend.set_signals(ModemSignals::default()).is_err());
}

/// The host's side of a real port: the Win32 serial API.
#[cfg(windows)]
mod win32 {
    use std::time::{Duration, Instant};

    use serial::{Com0comBackend, ModemSignals, SerialBackend, VcomBackend};
    use windows::Win32::Devices::Communication::{
        CLRBREAK, CLRDTR, CLRRTS, COMM_EVENT_MASK, COMMTIMEOUTS, ESCAPE_COMM_FUNCTION,
        EscapeCommFunction, GetCommModemStatus, MODEM_STATUS_FLAGS, MS_CTS_ON, MS_DSR_ON,
        MS_RING_ON, MS_RLSD_ON, PURGE_COMM_FLAGS, PurgeComm, SETBREAK, SETDTR, SETRTS, SetCommMask,
        SetCommTimeouts, WaitCommEvent,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING, ReadFile, WriteFile,
    };
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::core::{HSTRING, PCWSTR};

    use super::{Bench, Host, ReadTimeouts};

    pub fn add(all: &mut Vec<Bench>) {
        match std::env::var("RM_VCOM_PORT") {
            Ok(port) => {
                let open = {
                    let port = port.clone();
                    move || -> Option<Box<dyn SerialBackend>> {
                        match VcomBackend::open(&port) {
                            Ok(backend) => Some(Box::new(backend)),
                            Err(message) => panic!("vcom: {} {}", message.id, message.text),
                        }
                    }
                };
                all.push(Bench {
                    name: "vcom",
                    backend: open(),
                    host: Box::new(ComPort::new(&port)),
                    reopen: Box::new(open),
                });
            }
            Err(_) => println!("vcom: skipped, RM_VCOM_PORT names no RustModem port"),
        }
        match std::env::var("RM_COM0COM_PAIR") {
            Ok(pair) => {
                let (service, host) = pair
                    .split_once(',')
                    .expect("RM_COM0COM_PAIR is the service's port, a comma, the host's port");
                let service = service.to_string();
                let open = move || -> Option<Box<dyn SerialBackend>> {
                    match Com0comBackend::open(&service) {
                        Ok(backend) => Some(Box::new(backend)),
                        Err(message) => panic!("com0com: {} {}", message.id, message.text),
                    }
                };
                all.push(Bench {
                    name: "com0com",
                    backend: open(),
                    host: Box::new(ComPort::new(host)),
                    reopen: Box::new(open),
                });
            }
            Err(_) => println!("com0com: skipped, RM_COM0COM_PAIR names no pair"),
        }
    }

    /// A COM port as a program has it.
    struct ComPort {
        name: String,
        handle: Option<HANDLE>,
        event: HANDLE,
    }

    impl ComPort {
        fn new(name: &str) -> ComPort {
            // SAFETY: no attributes and no name.
            let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.expect("event");
            ComPort {
                name: name.to_string(),
                handle: None,
                event,
            }
        }

        fn create(&self) -> windows::core::Result<HANDLE> {
            let path = HSTRING::from(format!(r"\\.\{}", self.name));
            // SAFETY: a valid wide string and plain values.
            unsafe {
                CreateFileW(
                    &path,
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
        }

        fn handle(&self) -> HANDLE {
            self.handle.expect("the port is open")
        }

        fn overlapped(&self) -> OVERLAPPED {
            OVERLAPPED {
                hEvent: self.event,
                ..OVERLAPPED::default()
            }
        }

        /// Waits for what was started on `overlapped`, cancelling it at
        /// the limit. Gives the octets moved, and whether it ended by
        /// itself.
        fn finish(
            &self,
            started: windows::core::Result<()>,
            overlapped: &mut OVERLAPPED,
            limit: Duration,
        ) -> (u32, bool) {
            let handle = self.handle();
            let mut moved = 0;
            // SAFETY: `overlapped` is the caller's, alive until this
            // returns, and nothing is left outstanding on it: a request
            // that has not ended is cancelled and then waited for.
            unsafe {
                match started {
                    Ok(()) => {}
                    Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
                    Err(e) => panic!("{}: {e}", self.name),
                }
                let ended =
                    WaitForSingleObject(self.event, limit.as_millis() as u32) == WAIT_OBJECT_0;
                if !ended {
                    let _ = CancelIoEx(handle, Some(&*overlapped));
                }
                let _ = GetOverlappedResult(handle, &*overlapped, &mut moved, true);
                (moved, ended)
            }
        }

        fn timeouts(&self, read: ReadTimeouts) {
            let timeouts = COMMTIMEOUTS {
                ReadIntervalTimeout: read.interval,
                ReadTotalTimeoutMultiplier: read.multiplier,
                ReadTotalTimeoutConstant: read.constant,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 0,
            };
            // SAFETY: an open port and a structure valid for the call.
            unsafe { SetCommTimeouts(self.handle(), &timeouts) }.expect("SetCommTimeouts");
        }

        fn escape(&self, function: ESCAPE_COMM_FUNCTION) {
            // SAFETY: an open port and a plain value.
            unsafe { EscapeCommFunction(self.handle(), function) }.expect("EscapeCommFunction");
        }
    }

    impl Drop for ComPort {
        fn drop(&mut self) {
            self.close();
            // SAFETY: the event is this port's and closed nowhere else.
            unsafe {
                let _ = CloseHandle(self.event);
            }
        }
    }

    impl Host for ComPort {
        fn open(&mut self) -> bool {
            match self.create() {
                Ok(handle) => {
                    self.handle = Some(handle);
                    // What the reads of the tests that do not say
                    // otherwise want: what is there, at once.
                    self.timeouts(ReadTimeouts {
                        interval: u32::MAX,
                        multiplier: 0,
                        constant: 0,
                    });
                    true
                }
                Err(_) => false,
            }
        }

        fn close(&mut self) {
            if let Some(handle) = self.handle.take() {
                // SAFETY: the handle is this port's and closed nowhere
                // else.
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
        }

        fn write(&mut self, data: &[u8]) {
            let written = self.try_write(data, Duration::from_secs(10));
            assert_eq!(
                written,
                Some(data.len()),
                "{}: a write did not end",
                self.name
            );
        }

        fn try_write(&mut self, data: &[u8], limit: Duration) -> Option<usize> {
            let mut overlapped = self.overlapped();
            // SAFETY: the buffer and the OVERLAPPED live until `finish`
            // has waited for the write to end.
            let started =
                unsafe { WriteFile(self.handle(), Some(data), None, Some(&mut overlapped)) };
            let (moved, ended) = self.finish(started, &mut overlapped, limit);
            ended.then_some(moved as usize)
        }

        fn read(&mut self, most: usize) -> Vec<u8> {
            let mut buffer = vec![0u8; most];
            let mut overlapped = self.overlapped();
            // SAFETY: as in `try_write`.
            let started = unsafe {
                ReadFile(
                    self.handle(),
                    Some(&mut buffer),
                    None,
                    Some(&mut overlapped),
                )
            };
            let (moved, _) = self.finish(started, &mut overlapped, Duration::from_secs(5));
            buffer.truncate(moved as usize);
            buffer
        }

        fn timed_read(
            &mut self,
            timeouts: ReadTimeouts,
            want: usize,
            limit: Duration,
        ) -> (Vec<u8>, Duration) {
            self.timeouts(timeouts);
            let mut buffer = vec![0u8; want];
            let mut overlapped = self.overlapped();
            let began = Instant::now();
            // SAFETY: as in `try_write`.
            let started = unsafe {
                ReadFile(
                    self.handle(),
                    Some(&mut buffer),
                    None,
                    Some(&mut overlapped),
                )
            };
            let (moved, _) = self.finish(started, &mut overlapped, limit);
            let took = began.elapsed();
            buffer.truncate(moved as usize);
            self.timeouts(ReadTimeouts {
                interval: u32::MAX,
                multiplier: 0,
                constant: 0,
            });
            (buffer, took)
        }

        fn signals(&mut self) -> ModemSignals {
            let mut status = MODEM_STATUS_FLAGS(0);
            // SAFETY: an open port and a value for the call to fill.
            unsafe { GetCommModemStatus(self.handle(), &mut status) }.expect("GetCommModemStatus");
            ModemSignals {
                dcd: status.0 & MS_RLSD_ON.0 != 0,
                dsr: status.0 & MS_DSR_ON.0 != 0,
                cts: status.0 & MS_CTS_ON.0 != 0,
                ri: status.0 & MS_RING_ON.0 != 0,
            }
        }

        fn set_dtr(&mut self, on: bool) {
            self.escape(if on { SETDTR } else { CLRDTR });
        }

        fn set_rts(&mut self, on: bool) {
            self.escape(if on { SETRTS } else { CLRRTS });
        }

        fn is_win32(&self) -> bool {
            true
        }

        fn second_open_succeeds(&mut self) -> bool {
            match self.create() {
                Ok(handle) => {
                    // SAFETY: a handle just made and held nowhere else.
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                    true
                }
                Err(_) => false,
            }
        }

        fn wait_event(&mut self, mask: u32, cause: &mut dyn FnMut()) -> Option<u32> {
            let handle = self.handle();
            let mut events = Box::new(COMM_EVENT_MASK(0));
            let mut overlapped = self.overlapped();
            // SAFETY: the mask and the OVERLAPPED live until `finish` has
            // waited for the wait to end or cancelled it.
            unsafe {
                SetCommMask(handle, COMM_EVENT_MASK(mask)).expect("SetCommMask");
                let started = WaitCommEvent(handle, &mut *events, Some(&mut overlapped));
                if started.is_ok() {
                    return Some(events.0);
                }
                cause();
                let (_, ended) = self.finish(started, &mut overlapped, Duration::from_millis(1500));
                ended.then_some(events.0)
            }
        }

        fn wait_ended_by_mask(&mut self, mask: u32) -> Option<u32> {
            let handle = self.handle();
            // Something other than nothing, to see it set to nothing.
            let mut events = Box::new(COMM_EVENT_MASK(u32::MAX));
            let mut overlapped = self.overlapped();
            // SAFETY: as in `wait_event`.
            unsafe {
                SetCommMask(handle, COMM_EVENT_MASK(mask)).expect("SetCommMask");
                let started = WaitCommEvent(handle, &mut *events, Some(&mut overlapped));
                if started.is_ok() {
                    return Some(events.0);
                }
                SetCommMask(handle, COMM_EVENT_MASK(mask)).expect("SetCommMask");
                let (_, ended) = self.finish(started, &mut overlapped, Duration::from_millis(1500));
                ended.then_some(events.0)
            }
        }

        fn purge(&mut self, flags: u32) {
            // SAFETY: an open port and a plain value.
            unsafe { PurgeComm(self.handle(), PURGE_COMM_FLAGS(flags)) }.expect("PurgeComm");
        }

        fn set_break(&mut self, on: bool) {
            self.escape(if on { SETBREAK } else { CLRBREAK });
        }
    }
}
