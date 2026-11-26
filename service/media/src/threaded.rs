// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The runner that gives each call a thread.
//!
//! The threads are made when the runner is, one for each line, and wait
//! for a call to carry. A thread that takes a call registers itself with
//! the Multimedia Class Scheduler Service under `Pro Audio`, runs the
//! tick loop until the pool stops the call, and gives the engine back.

#![forbid(unsafe_code)]

use std::io;
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine::ModemEngine;
use pool::{LineId, MediaCall, MediaEndpoint, MediaPort, MediaRunner, media_pair};

use crate::capture::{CaptureMeta, CaptureTap, CaptureWriter};
use crate::endpoint::RtpEndpoint;
use crate::os;
use crate::ticker::{Clock, MAX_DATAGRAM, Received, Ticker, Transport};

/// Wakes the pool's thread: it has an event to take.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// How long a media thread waits on its socket before it looks at its
/// clock and at whether it has been told to stop.
const WAIT: Duration = Duration::from_millis(10);

/// How long the pool waits for a media thread to give a call's engine
/// back.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// A call for a thread to carry.
struct Job {
    engine: Box<dyn ModemEngine>,
    endpoint: RtpEndpoint,
    /// Where the call's packets are offered when its capture is on.
    capture: Option<CaptureTap>,
    port: MediaPort,
    originate: bool,
    stop: Arc<AtomicBool>,
    done: SyncSender<Box<dyn ModemEngine>>,
}

struct Worker {
    jobs: Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

/// A call a thread is carrying.
struct Carried {
    line: LineId,
    worker: usize,
    stop: Arc<AtomicBool>,
    done: Receiver<Box<dyn ModemEngine>>,
}

/// The runner that carries each call on a thread of its own, ticked by
/// the RTP packets of the far end. It takes [`RtpEndpoint`]s.
pub struct ThreadedRunner {
    workers: Vec<Worker>,
    /// The workers that have no call.
    idle: Vec<usize>,
    calls: Vec<Carried>,
    waker: Waker,
    /// The thread that writes captures. It is dropped after the media
    /// threads have ended, when every capture is complete.
    capture: Option<CaptureWriter>,
}

impl ThreadedRunner {
    /// A runner with `threads` media threads, one for each line there is.
    /// More are made if more calls come than that. `waker` is called from
    /// a media thread when it has an event for the pool.
    pub fn new(threads: usize, waker: Waker) -> io::Result<ThreadedRunner> {
        ThreadedRunner::build(threads, waker, None)
    }

    /// A runner whose calls can be captured: `writer` writes the files.
    pub fn with_capture(
        threads: usize,
        waker: Waker,
        writer: CaptureWriter,
    ) -> io::Result<ThreadedRunner> {
        ThreadedRunner::build(threads, waker, Some(writer))
    }

    fn build(
        threads: usize,
        waker: Waker,
        capture: Option<CaptureWriter>,
    ) -> io::Result<ThreadedRunner> {
        let mut runner = ThreadedRunner {
            workers: Vec::with_capacity(threads),
            idle: Vec::with_capacity(threads),
            calls: Vec::with_capacity(threads),
            waker,
            capture,
        };
        for _ in 0..threads {
            let index = runner.spawn()?;
            runner.idle.push(index);
        }
        Ok(runner)
    }

    /// How many media threads there are.
    pub fn threads(&self) -> usize {
        self.workers.len()
    }

    /// How many calls are being carried.
    pub fn call_count(&self) -> usize {
        self.calls.len()
    }

    fn spawn(&mut self) -> io::Result<usize> {
        let index = self.workers.len();
        let (jobs, inbox) = channel::<Job>();
        let waker = Arc::clone(&self.waker);
        let thread = std::thread::Builder::new()
            .name(format!("media-{index}"))
            .spawn(move || {
                // Parked here until there is a call, and done when the
                // runner is dropped.
                while let Ok(job) = inbox.recv() {
                    let done = job.done.clone();
                    let engine = carry(job, &waker);
                    let _ = done.send(engine);
                }
            })?;
        self.workers.push(Worker {
            jobs,
            thread: Some(thread),
        });
        Ok(index)
    }
}

impl MediaRunner for ThreadedRunner {
    fn start(
        &mut self,
        line: LineId,
        engine: Box<dyn ModemEngine>,
        endpoint: MediaEndpoint,
        originate: bool,
    ) -> Result<MediaCall, Box<dyn ModemEngine>> {
        let Ok(endpoint) = endpoint.downcast::<RtpEndpoint>() else {
            return Err(engine);
        };
        let worker = match self.idle.pop() {
            Some(worker) => worker,
            None => match self.spawn() {
                Ok(worker) => worker,
                Err(_) => return Err(engine),
            },
        };
        let (call, port) = media_pair();
        let stop = Arc::new(AtomicBool::new(false));
        let (done, engine_back) = sync_channel(1);
        // The ring of the call's capture is made now, whether or not the
        // capture is on: it can be turned on in the middle of the call.
        let capture = self.capture.as_ref().map(|writer| {
            let negotiation = endpoint.params.get();
            writer.tap(CaptureMeta {
                line,
                call_id: endpoint.call_id.clone(),
                payload_type: negotiation.payload_type,
                ptime_ms: (negotiation.frames * 10) as u8,
            })
        });
        let job = Job {
            engine,
            endpoint,
            capture,
            port,
            originate,
            stop: Arc::clone(&stop),
            done,
        };
        if let Err(refused) = self.workers[worker].jobs.send(job) {
            // The thread is gone. It is not put back among the idle.
            return Err(refused.0.engine);
        }
        self.calls.push(Carried {
            line,
            worker,
            stop,
            done: engine_back,
        });
        Ok(call)
    }

    fn stop(&mut self, line: LineId, _call: MediaCall) -> Option<Box<dyn ModemEngine>> {
        let index = self.calls.iter().position(|call| call.line == line)?;
        let carried = self.calls.swap_remove(index);
        carried.stop.store(true, Ordering::Relaxed);
        match carried.done.recv_timeout(STOP_TIMEOUT) {
            Ok(engine) => {
                self.idle.push(carried.worker);
                Some(engine)
            }
            // The thread did not come back in time. It keeps its engine
            // and is not given another call.
            Err(_) => None,
        }
    }
}

impl Drop for ThreadedRunner {
    fn drop(&mut self) {
        for call in &self.calls {
            call.stop.store(true, Ordering::Relaxed);
        }
        for worker in self.workers.drain(..) {
            // With its sender gone the thread's wait for a call ends.
            drop(worker.jobs);
            if let Some(thread) = worker.thread {
                let _ = thread.join();
            }
        }
    }
}

/// Carries one call on the calling thread until it is told to stop, and
/// gives the engine back, stopped.
fn carry(job: Job, waker: &Waker) -> Box<dyn ModemEngine> {
    let Job {
        mut engine,
        endpoint,
        capture,
        mut port,
        originate,
        stop,
        done: _,
    } = job;

    // Everything the loop needs is made here, before the first tick. The
    // socket's buffers were sized when it was bound.
    let _ = os::prepare_socket(&endpoint.socket, WAIT);
    let mut transport = UdpTransport {
        socket: &endpoint.socket,
    };
    let mut clock = MonotonicClock(Instant::now());
    let mut buffer = [0u8; MAX_DATAGRAM];
    let mut ticker = Ticker::new(
        clock.now_us(),
        Arc::clone(&endpoint.params),
        endpoint.timeout_ms,
        endpoint.ssrc,
        endpoint.sequence,
        endpoint.timestamp,
        capture,
    );
    let _scheduling = os::ProAudio::enter();

    if originate {
        engine.originate();
    } else {
        engine.answer();
    }
    // An engine may connect the moment it is started.
    if port.drain(engine.as_mut()) {
        waker();
    }

    while !stop.load(Ordering::Relaxed) {
        let received = transport.receive(&mut buffer);
        let wake = ticker.turn(
            received,
            &buffer,
            engine.as_mut(),
            &mut transport,
            &mut clock,
            &mut port,
        );
        if wake {
            waker();
        }
    }

    ticker.publish(port.shared());
    engine.hangup();
    engine
}

/// A call's socket.
struct UdpTransport<'a> {
    socket: &'a UdpSocket,
}

impl Transport for UdpTransport<'_> {
    fn receive(&mut self, buffer: &mut [u8]) -> Received {
        // The wait must not be the socket's read timeout: on Windows a
        // datagram that arrives as that runs out is lost. See `os`.
        match os::receive_within(self.socket, buffer, WAIT) {
            Some((len, SocketAddr::V4(from))) => Received::Datagram { len, from },
            // Not IPv4, the wait ran out, or the system passed on an ICMP
            // error for something we sent: none of them is a datagram.
            Some(_) | None => Received::Nothing,
        }
    }

    fn send(&mut self, datagram: &[u8], to: SocketAddrV4) {
        // A datagram that cannot be sent is a datagram lost: the far end
        // fills it, as it would one the network dropped.
        let _ = self.socket.send_to(datagram, to);
    }
}

/// The system's monotonic clock.
struct MonotonicClock(Instant);

impl Clock for MonotonicClock {
    fn now_us(&mut self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}
