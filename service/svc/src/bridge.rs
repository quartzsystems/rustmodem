// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The serial bridge: a thread for each port, and the pool's end of it.
//!
//! Overlapped I/O on a COM handle has no place in Tokio's reactor, so a
//! port is not a task. Each port has one thread, which owns the port's
//! [`SerialBackend`], waits on its handle, and exchanges messages with the
//! pool's task: what the host wrote and did goes one way, what is to be
//! written and the signals to set go the other. The pool sees a
//! [`SerialBackend`] over those channels, so neither the pool nor its
//! lines know there is a thread.
//!
//! No thread waits on more than its own port, so there is no limit on the
//! number of ports from the 64 handles one Windows wait can take.
//!
//! One thing differs from a port the pool holds itself. A write or a
//! change of signals is carried out a moment after the pool asked for it,
//! so the pool cannot be told of its failure where it asked. It is told
//! when it next reads the port's signals, which is at its next turn: the
//! port fails as it would have, and the message says the signals could not
//! be read.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use config::LineConfig;
use msg::Message;
use pool::SerialFactory;
use serial::{ModemSignals, SerialBackend, SerialEvent, SerialOperation, WaitHandle};
use tokio::sync::Notify;
use tokio::sync::mpsc;

/// How many octets a port's thread reads at a time.
const CHUNK: usize = 512;

/// How many chunks of what the host wrote may wait for the pool. A port
/// whose line is not being read fills this and is then not read itself,
/// so what the host writes waits in the port, as it did when the pool read
/// the port directly.
const CHUNKS: usize = 16;

/// How many of the things the host did may wait for the pool. They have a
/// channel of their own: a line that is not being read still hears that
/// its host has dropped DTR or closed the port.
const EVENTS: usize = 64;

/// How many octets may be on their way to a port's host. The pool is
/// offered no more room than this, so a host that does not read holds the
/// octets up in the pool, where the line's high-water mark is.
const WINDOW: usize = 4096;

/// How long a thread whose port has a handle waits on it before it looks
/// at the port anyway: the pool's tick, at which the pool used to look at
/// every port.
#[cfg(windows)]
const HANDLE_WAIT: Duration = Duration::from_millis(pool::TICK_MS);

/// How often a thread looks at a port that has nothing to wait on: a
/// memory port, in a test or in the demo service.
const POLL: Duration = Duration::from_millis(5);

/// From a port's thread to the pool, beside the octets the host wrote.
enum News {
    /// Something the host did.
    Event(SerialEvent),
    /// The port has failed. Nothing follows.
    Failed(SerialOperation, io::Error),
}

/// From the pool to a port's thread.
enum Order {
    /// Octets for the host.
    Write(Vec<u8>),
    /// The signals toward the host.
    Signals(ModemSignals),
}

/// Wakes a port's thread when the pool has given it an order.
#[derive(Clone)]
enum Kick {
    /// The thread waits on its orders: an order is its own wake.
    Orders,
    /// The thread waits on the port's handle and on this event.
    #[cfg(windows)]
    Event(Arc<pipe::Event>),
}

impl Kick {
    fn kick(&self) {
        match self {
            Kick::Orders => {}
            #[cfg(windows)]
            Kick::Event(event) => event.set(),
        }
    }
}

/// Opens ports through another factory and gives each a thread.
pub struct BridgedPorts {
    inner: Box<dyn SerialFactory>,
    /// Tells the pool's task that a port has something for it.
    news: Arc<Notify>,
}

impl BridgedPorts {
    /// Bridges the ports `inner` opens. `news` is notified whenever a
    /// port's thread has something for the pool; the pool's task waits on
    /// it.
    pub fn new(inner: Box<dyn SerialFactory>, news: Arc<Notify>) -> BridgedPorts {
        BridgedPorts { inner, news }
    }
}

impl SerialFactory for BridgedPorts {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let backend = self.inner.open(line)?;
        let port = &line.serial.port;
        bridge(backend, port, Arc::clone(&self.news))
            .map(|bridged| Box::new(bridged) as Box<dyn SerialBackend>)
            .map_err(|error| serial::error_message(port, SerialOperation::Open, &error))
    }
}

/// Gives `backend` a thread and returns the pool's end.
fn bridge(
    backend: Box<dyn SerialBackend>,
    port: &str,
    news: Arc<Notify>,
) -> io::Result<BridgedPort> {
    let (to_pool, from_port) = mpsc::channel(EVENTS);
    let (bytes_to_pool, bytes_from_port) = mpsc::channel(CHUNKS);
    let (to_port, orders) = std_mpsc::channel();
    let outstanding = Arc::new(AtomicUsize::new(0));
    let kick = match backend.wait_handle().raw() {
        #[cfg(windows)]
        Some(_) => Kick::Event(Arc::new(pipe::Event::auto()?)),
        _ => Kick::Orders,
    };
    let thread = PortThread {
        backend,
        orders,
        to_pool,
        bytes_to_pool,
        news,
        outstanding: Arc::clone(&outstanding),
        #[cfg(windows)]
        kick: kick.clone(),
        out: VecDeque::new(),
        signals: None,
        events: VecDeque::new(),
    };
    let thread = std::thread::Builder::new()
        .name(format!("rm-port-{port}"))
        .spawn(move || thread.run())?;
    Ok(BridgedPort {
        from_port,
        bytes_from_port,
        to_port: Some(to_port),
        kick,
        outstanding,
        bytes: VecDeque::new(),
        events: VecDeque::new(),
        failed: None,
        thread: Some(thread),
    })
}

/// The pool's end of a bridged port.
struct BridgedPort {
    from_port: mpsc::Receiver<News>,
    bytes_from_port: mpsc::Receiver<Vec<u8>>,
    /// Taken when the port is dropped, which is how its thread is told.
    to_port: Option<std_mpsc::Sender<Order>>,
    kick: Kick,
    /// Octets handed to the thread and not yet taken by the host.
    outstanding: Arc<AtomicUsize>,
    /// What is left of the chunk being read.
    bytes: VecDeque<u8>,
    events: VecDeque<SerialEvent>,
    failed: Option<(SerialOperation, io::Error)>,
    thread: Option<JoinHandle<()>>,
}

impl BridgedPort {
    /// Takes in what the thread has said of the host and of the port.
    fn take_news(&mut self) {
        while let Ok(news) = self.from_port.try_recv() {
            match news {
                News::Event(event) => self.events.push_back(event),
                News::Failed(operation, error) => self.failed = Some((operation, error)),
            }
        }
    }

    fn order(&mut self, order: Order) -> io::Result<()> {
        let sent = match &self.to_port {
            Some(to_port) => to_port.send(order).is_ok(),
            None => false,
        };
        if !sent {
            // The thread has gone, which it does only after it has said
            // why; that reason is on its way through `poll`.
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.kick.kick();
        Ok(())
    }

    /// The failure the thread reported, if it is one that `asked` reports.
    /// What failed in the thread's own time, a write or a change of
    /// signals, is reported by `poll`.
    fn failure(&mut self, asked: SerialOperation) -> io::Result<()> {
        let reports = match self.failed.as_ref().map(|(operation, _)| *operation) {
            None => false,
            Some(SerialOperation::Read) => asked == SerialOperation::Read,
            Some(_) => asked == SerialOperation::Poll,
        };
        match self.failed.take_if(|_| reports) {
            Some((_, error)) => Err(error),
            None => Ok(()),
        }
    }
}

impl SerialBackend for BridgedPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // No more chunks are taken from the thread than are read here:
        // the rest wait in its channel, and behind that in the port.
        let mut filled = 0;
        while filled < buf.len() {
            if self.bytes.is_empty() {
                match self.bytes_from_port.try_recv() {
                    Ok(chunk) => self.bytes.extend(chunk),
                    Err(_) => break,
                }
            }
            let take = (buf.len() - filled).min(self.bytes.len());
            for (slot, byte) in buf[filled..].iter_mut().zip(self.bytes.drain(..take)) {
                *slot = byte;
            }
            filled += take;
        }
        if filled == 0 {
            self.take_news();
            self.failure(SerialOperation::Read)?;
        }
        Ok(filled)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = WINDOW.saturating_sub(self.outstanding.load(Ordering::Acquire));
        let take = data.len().min(room);
        if take == 0 {
            return Ok(0);
        }
        self.outstanding.fetch_add(take, Ordering::AcqRel);
        if self.order(Order::Write(data[..take].to_vec())).is_err() {
            // The octets go nowhere, and the pool hears why from `poll`.
            self.outstanding.fetch_sub(take, Ordering::AcqRel);
            return Ok(0);
        }
        Ok(take)
    }

    fn set_signals(&mut self, signals: ModemSignals) -> io::Result<()> {
        // A thread that has gone has said why, and `poll` reports it.
        let _ = self.order(Order::Signals(signals));
        Ok(())
    }

    fn poll(&mut self) -> io::Result<Option<SerialEvent>> {
        self.take_news();
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        self.failure(SerialOperation::Poll)?;
        Ok(None)
    }

    fn wait_handle(&self) -> WaitHandle {
        // The thread does the waiting.
        WaitHandle::NONE
    }
}

impl Drop for BridgedPort {
    fn drop(&mut self) {
        // The thread is told by its orders ending, and is not left blocked
        // on a message nobody will take. It writes what it was last given,
        // sets the last signals and releases the port; the port is closed
        // when this returns, so it can be opened again at once.
        self.from_port.close();
        self.bytes_from_port.close();
        self.to_port = None;
        self.kick.kick();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A port's thread: what it owns and what it has in hand.
struct PortThread {
    backend: Box<dyn SerialBackend>,
    orders: std_mpsc::Receiver<Order>,
    to_pool: mpsc::Sender<News>,
    bytes_to_pool: mpsc::Sender<Vec<u8>>,
    news: Arc<Notify>,
    outstanding: Arc<AtomicUsize>,
    /// What ends this thread's wait on its port's handle.
    #[cfg(windows)]
    kick: Kick,
    /// Octets for the host that it has not taken yet.
    out: VecDeque<u8>,
    /// The signals to set, when they have not been yet.
    signals: Option<ModemSignals>,
    /// What the host did that the pool has not been sent yet.
    events: VecDeque<SerialEvent>,
}

/// Why a port's thread ends.
enum End {
    /// The pool dropped its end.
    Released,
    /// The port failed.
    Failed(SerialOperation, io::Error),
}

impl PortThread {
    fn run(mut self) {
        let end = loop {
            match self.turn() {
                Ok(true) => {}
                Ok(false) => self.wait(),
                Err(end) => break end,
            }
        };
        match end {
            End::Released => {
                // What the pool said last is done before the port closes:
                // the host sees its NO CARRIER and its signals go off.
                let _ = self.write();
                let _ = self.set_signals();
            }
            End::Failed(operation, error) => {
                // The pool is told, and the port is kept from it until it
                // lets go: nothing more is done with a port that failed.
                let _ = self.to_pool.blocking_send(News::Failed(operation, error));
                self.news.notify_one();
                while self.orders.recv().is_ok() {}
            }
        }
    }

    /// One look at the port and at the orders. Returns whether anything
    /// moved: if so there may be more.
    fn turn(&mut self) -> Result<bool, End> {
        let mut moved = false;
        let mut told = false;

        loop {
            match self.orders.try_recv() {
                Ok(Order::Write(bytes)) => self.out.extend(bytes),
                Ok(Order::Signals(signals)) => self.signals = Some(signals),
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => return Err(End::Released),
            }
            moved = true;
        }

        // What the host did.
        loop {
            match self.backend.poll() {
                Ok(Some(event)) => {
                    if event == SerialEvent::HostClosed {
                        // Nobody is there to read what was waiting.
                        self.outstanding.fetch_sub(self.out.len(), Ordering::AcqRel);
                        self.out.clear();
                    }
                    self.events.push_back(event);
                }
                Ok(None) => break,
                Err(error) => return Err(End::Failed(SerialOperation::Poll, error)),
            }
        }
        while let Some(event) = self.events.pop_front() {
            // This waits only while the pool is not being served at all.
            if self.to_pool.blocking_send(News::Event(event)).is_err() {
                return Err(End::Released);
            }
            moved = true;
            told = true;
        }

        // What the host wrote, for as long as the pool has room for it.
        loop {
            let room = match self.bytes_to_pool.try_reserve() {
                Ok(room) => room,
                Err(mpsc::error::TrySendError::Full(())) => break,
                Err(mpsc::error::TrySendError::Closed(())) => return Err(End::Released),
            };
            let mut chunk = vec![0u8; CHUNK];
            match self.backend.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    chunk.truncate(count);
                    room.send(chunk);
                    moved = true;
                    told = true;
                }
                Err(error) => return Err(End::Failed(SerialOperation::Read, error)),
            }
        }

        // What the pool has for the host.
        match self.write() {
            Ok(wrote) => {
                // The pool may have more waiting for the room this made.
                moved |= wrote;
                told |= wrote;
            }
            Err(error) => return Err(End::Failed(SerialOperation::Write, error)),
        }
        if let Err(error) = self.set_signals() {
            return Err(End::Failed(SerialOperation::SetSignals, error));
        }

        if told {
            self.news.notify_one();
        }
        Ok(moved)
    }

    /// Writes what the host will take. Returns whether it took anything.
    fn write(&mut self) -> io::Result<bool> {
        let mut wrote = false;
        while !self.out.is_empty() {
            let (front, _) = self.out.as_slices();
            let taken = self.backend.write(front)?;
            if taken == 0 {
                break;
            }
            self.out.drain(..taken);
            self.outstanding.fetch_sub(taken, Ordering::AcqRel);
            wrote = true;
        }
        Ok(wrote)
    }

    fn set_signals(&mut self) -> io::Result<()> {
        if let Some(signals) = self.signals {
            self.backend.set_signals(signals)?;
            self.signals = None;
        }
        Ok(())
    }

    /// Waits until the port may have something, the pool has given an
    /// order, or it is time to look anyway.
    fn wait(&mut self) {
        #[cfg(windows)]
        if let Kick::Event(event) = &self.kick {
            // Octets the pool could not be sent, or the host would not
            // take, are tried again soon, whatever the handle says.
            let held_up = !self.out.is_empty() || self.bytes_to_pool.capacity() == 0;
            match self.backend.wait_handle().raw() {
                Some(port) if !held_up => {
                    pipe::wait_any(&[port, event.raw()], HANDLE_WAIT);
                }
                _ => {
                    event.wait(POLL);
                }
            }
            return;
        }
        // A port with nothing to wait on is looked at every so often. An
        // order ends the wait, and is taken here.
        match self.orders.recv_timeout(POLL) {
            Ok(Order::Write(bytes)) => self.out.extend(bytes),
            Ok(Order::Signals(signals)) => self.signals = Some(signals),
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serial::{MemoryHost, memory_pair};

    use super::*;

    struct Fixture {
        port: BridgedPort,
        host: MemoryHost,
        news: Arc<Notify>,
    }

    fn fixture() -> Fixture {
        let (backend, host) = memory_pair();
        let news = Arc::new(Notify::new());
        let port = bridge(Box::new(backend), "P1", Arc::clone(&news)).unwrap();
        Fixture { port, host, news }
    }

    /// Waits until `check` is true, or fails the test.
    fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn read_all(port: &mut BridgedPort) -> Vec<u8> {
        let mut all = Vec::new();
        let mut buffer = [0u8; 100];
        loop {
            match port.read(&mut buffer).unwrap() {
                0 => return all,
                count => all.extend(&buffer[..count]),
            }
        }
    }

    #[test]
    fn what_the_host_does_and_writes_reaches_the_pool_and_wakes_it() {
        let mut fixture = fixture();
        fixture.host.open();
        fixture.host.write(b"ATZ\r");

        let mut events = Vec::new();
        eventually("the events", || {
            events.extend(fixture.port.poll().unwrap());
            events.len() == 3
        });
        assert_eq!(
            events,
            [
                SerialEvent::HostOpened,
                SerialEvent::Dtr(true),
                SerialEvent::Rts(true)
            ]
        );
        let mut read = Vec::new();
        eventually("the bytes", || {
            read.extend(read_all(&mut fixture.port));
            read == b"ATZ\r"
        });
        // The pool's task was told there was something to take.
        let woken = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(1), fixture.news.notified())
                    .await
                    .is_ok()
            });
        assert!(woken);
    }

    #[test]
    fn what_the_pool_writes_and_sets_reaches_the_host() {
        let mut fixture = fixture();
        fixture.host.open();
        assert_eq!(fixture.port.write(b"\r\nOK\r\n").unwrap(), 6);
        let signals = ModemSignals {
            dsr: true,
            cts: true,
            ..ModemSignals::default()
        };
        fixture.port.set_signals(signals).unwrap();
        let mut written = Vec::new();
        eventually("the bytes", || {
            written.extend(fixture.host.read());
            written == b"\r\nOK\r\n"
        });
        eventually("the signals", || fixture.host.signals() == signals);
    }

    #[test]
    fn a_host_that_does_not_read_holds_the_pool_up_and_loses_nothing() {
        let mut fixture = fixture();
        fixture.host.open();
        fixture.host.set_capacity(Some(100));
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();

        // The pool is offered the window and no more.
        let mut offered = 0;
        while offered < data.len() {
            match fixture.port.write(&data[offered..]).unwrap() {
                0 => break,
                taken => offered += taken,
            }
        }
        assert!(offered <= WINDOW + 100, "{offered} octets were taken");
        assert!(offered < data.len());

        // The host reads, slowly; the pool writes when there is room.
        let mut arrived = Vec::new();
        eventually("every octet", || {
            arrived.extend(fixture.host.read());
            if offered < data.len() {
                offered += fixture.port.write(&data[offered..]).unwrap();
            }
            arrived.len() == data.len()
        });
        assert_eq!(arrived, data);
    }

    #[test]
    fn a_pool_that_does_not_read_leaves_the_octets_in_the_port() {
        let mut fixture = fixture();
        fixture.host.open();
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 249) as u8).collect();
        fixture.host.write(&data);
        // The thread takes what the pool has room for and then stops.
        eventually("the thread to fill the pool's room", || {
            fixture.host.unread_by_modem() == data.len() - CHUNKS * CHUNK
        });
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(fixture.host.unread_by_modem(), data.len() - CHUNKS * CHUNK);
        // What the host does is still told while the octets wait.
        fixture.host.set_rts(false);
        let mut events = Vec::new();
        eventually("the event", || {
            events.extend(fixture.port.poll().unwrap());
            events.contains(&SerialEvent::Rts(false))
        });

        let mut read = Vec::new();
        eventually("every octet", || {
            read.extend(read_all(&mut fixture.port));
            read.len() == data.len()
        });
        assert_eq!(read, data);
    }

    #[test]
    fn a_port_that_fails_is_reported_once_with_what_failed() {
        let mut fixture = fixture();
        fixture.host.open();
        eventually("the port to be seen open", || {
            fixture.port.poll().unwrap().is_some()
        });
        while fixture.port.poll().unwrap().is_some() {}
        fixture
            .host
            .inject_failure(Some(io::ErrorKind::PermissionDenied));
        // The thread reads the signals first, as the pool did.
        let mut failure = None;
        eventually("the failure", || {
            failure = fixture.port.poll().err();
            failure.is_some()
        });
        assert_eq!(failure.unwrap().kind(), io::ErrorKind::PermissionDenied);
        // A port that failed is dropped by the pool; until then it is
        // quiet, and takes nothing.
        assert_eq!(fixture.port.poll().unwrap(), None);
        assert_eq!(fixture.port.read(&mut [0u8; 8]).unwrap(), 0);
        fixture.host.inject_failure(None);
        assert!(fixture.host.backend_alive());
        drop(fixture.port);
        assert!(!fixture.host.backend_alive());
    }

    #[test]
    fn dropping_the_port_writes_what_was_last_said_and_releases_it() {
        let mut fixture = fixture();
        fixture.host.open();
        let on = ModemSignals {
            dcd: true,
            dsr: true,
            cts: true,
            ..ModemSignals::default()
        };
        fixture.port.set_signals(on).unwrap();
        eventually("the signals", || fixture.host.signals() == on);

        // As the pool takes a line down: the last words, the signals off,
        // and the port let go, all at once.
        assert_eq!(fixture.port.write(b"\r\nNO CARRIER\r\n").unwrap(), 14);
        fixture.port.set_signals(ModemSignals::default()).unwrap();
        drop(fixture.port);
        assert!(!fixture.host.backend_alive(), "the port is released");
        assert_eq!(fixture.host.read(), b"\r\nNO CARRIER\r\n");
        assert_eq!(fixture.host.signals(), ModemSignals::default());
    }

    #[test]
    fn a_host_that_closes_is_not_sent_what_was_waiting_for_it() {
        let mut fixture = fixture();
        fixture.host.open();
        fixture.host.set_capacity(Some(0));
        assert_eq!(fixture.port.write(b"waiting").unwrap(), 7);
        std::thread::sleep(Duration::from_millis(30));
        fixture.host.close();
        let mut events = Vec::new();
        eventually("the close", || {
            events.extend(fixture.port.poll().unwrap());
            events.contains(&SerialEvent::HostClosed)
        });
        fixture.host.set_capacity(None);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(fixture.host.read(), b"");
        // The room is the pool's again.
        assert_eq!(fixture.port.write(&[0u8; WINDOW]).unwrap(), WINDOW);
    }
}
