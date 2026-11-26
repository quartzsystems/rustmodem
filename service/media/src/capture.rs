// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Call capture: every packet of a call, written to a `.rmcap` file.
//!
//! The tick loop never touches a file. It offers each packet it receives
//! and sends to a [`CaptureTap`], which is a ring; a [`CaptureWriter`]
//! thread, one for the whole service, empties the rings into files. When a
//! ring is full the record is dropped and counted, and the tick goes on:
//! a capture may have holes, a call never waits for one. A hole is marked
//! in the file by a record that says how many records are missing.
//!
//! The format is `rtp::capture`, described in `docs/CAPTURE.md`.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pool::LineId;
use rtp::RtpHeader;
use rtp::capture::{Header, Kind, Record, Writer};
pub use rtrb::Consumer;
use rtrb::{Producer, RingBuffer};

/// The records a call's ring holds: several seconds of packets both ways
/// at the shortest packet time.
pub const CAPTURE_RING: usize = 2048;

/// How often the writer empties the rings.
const WRITE_EVERY: Duration = Duration::from_millis(20);

/// What a capture file says of its call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureMeta {
    pub line: LineId,
    /// The SIP Call-ID.
    pub call_id: String,
    pub payload_type: u8,
    pub ptime_ms: u8,
}

/// Something the writer has to say.
#[derive(Debug)]
pub enum CaptureEvent {
    /// A capture file was begun for a line's call.
    Started { line: LineId, path: PathBuf },
    /// A capture file could not be made or written. The call's capture is
    /// given up.
    Failed {
        line: LineId,
        path: PathBuf,
        error: io::Error,
    },
}

/// The media thread's end of a call's capture: where it offers packets.
pub struct CaptureTap {
    ring: Producer<Record>,
    /// Records dropped since the last one that went in.
    owed: u32,
    /// Records dropped in all.
    dropped: u64,
}

impl CaptureTap {
    /// A tap with no writer behind it: its ring, of `capacity` records,
    /// is the caller's to empty. For anything that wants a call's packets
    /// as they pass, and for tests.
    pub fn standalone(capacity: usize) -> (CaptureTap, Consumer<Record>) {
        let (producer, consumer) = RingBuffer::new(capacity);
        (
            CaptureTap {
                ring: producer,
                owed: 0,
                dropped: 0,
            },
            consumer,
        )
    }

    /// Offers a packet. Never waits and never allocates: if the ring is
    /// full the record is dropped and counted.
    pub fn offer(&mut self, kind: Kind, nanos: u64, header: &RtpHeader, payload: &[u8]) {
        // A hole is marked before the first record after it.
        if self.owed > 0 {
            if self.ring.push(Record::gap(nanos, self.owed)).is_err() {
                self.owed = self.owed.saturating_add(1);
                self.dropped += 1;
                return;
            }
            self.owed = 0;
        }
        if self
            .ring
            .push(Record::new(kind, nanos, *header, payload))
            .is_err()
        {
            self.owed = 1;
            self.dropped += 1;
        }
    }

    /// How many records have been dropped.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// A call's capture, on the writer's side.
struct Capture {
    meta: CaptureMeta,
    ring: Consumer<Record>,
    /// The file, once there has been something to put in it.
    file: Option<Writer<BufWriter<File>>>,
    /// The file could not be made or written: what arrives is thrown away.
    failed: bool,
}

/// The thread that writes captures. One serves every call.
pub struct CaptureWriter {
    register: Option<Sender<Capture>>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureWriter {
    /// Starts the writer. Files go in `directory`, which is made when the
    /// first capture is written. `events` is called from the writer's
    /// thread.
    pub fn new(
        directory: PathBuf,
        events: Box<dyn Fn(CaptureEvent) + Send>,
    ) -> io::Result<CaptureWriter> {
        let (register, registrations) = channel();
        let thread = std::thread::Builder::new()
            .name("capture".to_string())
            .spawn(move || write(&directory, &registrations, &*events))?;
        Ok(CaptureWriter {
            register: Some(register),
            thread: Some(thread),
        })
    }

    /// The tap for a new call. Its ring is made here, before the call's
    /// media begins. Nothing is written until the tap is offered a packet.
    pub fn tap(&self, meta: CaptureMeta) -> CaptureTap {
        let (tap, ring) = CaptureTap::standalone(CAPTURE_RING);
        if let Some(register) = &self.register {
            // A writer that has ended takes no more captures; the tap
            // then fills and drops, which harms nothing.
            let _ = register.send(Capture {
                meta,
                ring,
                file: None,
                failed: false,
            });
        }
        tap
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        // With no way to register more, the thread ends when the calls it
        // has are over.
        self.register = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The name of a capture file: the line, and when it began.
fn file_name(line: LineId, started_unix_ms: u64) -> String {
    format!("line{line:02}-{started_unix_ms}.rmcap")
}

fn open(directory: &Path, meta: &CaptureMeta) -> (PathBuf, io::Result<Writer<BufWriter<File>>>) {
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let path = directory.join(file_name(meta.line, started_unix_ms));
    let header = Header {
        payload_type: meta.payload_type,
        ptime_ms: meta.ptime_ms,
        line: meta.line,
        started_unix_ms,
        call_id: meta.call_id.clone(),
    };
    let writer = std::fs::create_dir_all(directory)
        .and_then(|()| File::create_new(&path))
        .and_then(|file| Writer::new(BufWriter::new(file), &header));
    (path, writer)
}

/// The writer's thread.
fn write(directory: &Path, registrations: &Receiver<Capture>, events: &dyn Fn(CaptureEvent)) {
    let mut captures: Vec<Capture> = Vec::new();
    let mut accepting = true;
    loop {
        while accepting {
            match registrations.try_recv() {
                Ok(capture) => captures.push(capture),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => accepting = false,
            }
        }
        captures.retain_mut(|capture| {
            let mut wrote = false;
            while let Ok(record) = capture.ring.pop() {
                if capture.failed {
                    continue;
                }
                if capture.file.is_none() {
                    let (path, opened) = open(directory, &capture.meta);
                    match opened {
                        Ok(file) => {
                            capture.file = Some(file);
                            events(CaptureEvent::Started {
                                line: capture.meta.line,
                                path,
                            });
                        }
                        Err(error) => {
                            capture.failed = true;
                            events(CaptureEvent::Failed {
                                line: capture.meta.line,
                                path,
                                error,
                            });
                            continue;
                        }
                    }
                }
                if let Some(file) = &mut capture.file
                    && let Err(error) = file.record(&record)
                {
                    capture.failed = true;
                    capture.file = None;
                    events(CaptureEvent::Failed {
                        line: capture.meta.line,
                        path: directory.to_path_buf(),
                        error,
                    });
                    continue;
                }
                wrote = true;
            }
            if wrote && let Some(file) = &mut capture.file {
                let _ = file.flush();
            }
            // A ring whose other end is gone and that is empty belongs to
            // a call that is over.
            !capture.ring.is_abandoned() || !capture.ring.is_empty()
        });
        if !accepting && captures.is_empty() {
            return;
        }
        std::thread::sleep(WRITE_EVERY);
    }
}
