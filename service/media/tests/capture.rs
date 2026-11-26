// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Call capture: the tap in the tick loop, the writer thread, and the
//! files it writes, checked with the capture checker.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine::{EchoEngine, ModemEngine};
use media::{
    CAPTURE_RING, CaptureEvent, CaptureMeta, CaptureTap, CaptureWriter, RtpEndpoint, RtpParams,
    ThreadedRunner,
};
use pool::{MediaEndpoint, MediaRunner};
use rtp::capture::{Kind, Reader, Report, check};
use rtp::{HEADER_LEN, RtpHeader, RtpPacket};

/// A directory of this test's own, under the system's, removed at the end.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path =
            std::env::temp_dir().join(format!("rustmodem-capture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Scratch(path)
    }

    fn files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.0)
            .map(|entries| entries.filter_map(|e| Some(e.ok()?.path())).collect())
            .unwrap_or_default();
        files.sort();
        files
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rtp(n: u32) -> RtpHeader {
    RtpHeader {
        marker: n == 0,
        payload_type: 0,
        sequence: n as u16,
        timestamp: 160 * n,
        ssrc: 9,
    }
}

fn report(path: &Path) -> (rtp::capture::Header, Report) {
    let bytes = std::fs::read(path).unwrap();
    let mut reader = Reader::new(&bytes[..]).unwrap();
    let header = reader.header().clone();
    (header, check(&mut reader).unwrap())
}

fn writer(directory: &Path) -> (CaptureWriter, Receiver<String>) {
    let (said, events) = channel();
    let said = Mutex::new(said);
    let writer = CaptureWriter::new(
        directory.to_path_buf(),
        Box::new(move |event| {
            let text = match event {
                CaptureEvent::Started { line, path } => format!(
                    "started {line} {}",
                    path.file_name().unwrap().to_string_lossy()
                ),
                CaptureEvent::Failed { line, .. } => format!("failed {line}"),
            };
            let _ = said.lock().unwrap().send(text);
        }),
    )
    .unwrap();
    (writer, events)
}

#[test]
fn a_tap_that_is_full_drops_and_marks_the_hole() {
    let (mut tap, mut ring) = CaptureTap::standalone(4);
    for n in 0..10 {
        tap.offer(Kind::Received, u64::from(n), &rtp(n), &[n as u8; 160]);
    }
    // Four went in; six were dropped, and nothing waited.
    assert_eq!(tap.dropped(), 6);
    let held: Vec<u16> = std::iter::from_fn(|| ring.pop().ok())
        .map(|record| record.header.sequence)
        .collect();
    assert_eq!(held, [0, 1, 2, 3]);

    // The next record that goes in is preceded by one that says how many
    // are missing.
    tap.offer(Kind::Sent, 100, &rtp(10), &[0; 160]);
    let gap = ring.pop().unwrap();
    assert_eq!((gap.kind, gap.dropped(), gap.nanos), (Kind::Gap, 6, 100));
    let next = ring.pop().unwrap();
    assert_eq!((next.kind, next.header.sequence), (Kind::Sent, 10));
    assert!(ring.pop().is_err());
    assert_eq!(tap.dropped(), 6);

    // A ring with room for the mark and not for the record: the record is
    // owed with the rest.
    let (mut tap, mut ring) = CaptureTap::standalone(2);
    for n in 0..5 {
        tap.offer(Kind::Received, 0, &rtp(n), &[]);
    }
    assert_eq!(tap.dropped(), 3);
    ring.pop().unwrap();
    tap.offer(Kind::Received, 0, &rtp(5), &[]);
    assert_eq!(tap.dropped(), 4);
    ring.pop().unwrap();
    assert_eq!(ring.pop().unwrap().dropped(), 3);
    ring.pop().unwrap_err();
    tap.offer(Kind::Received, 0, &rtp(6), &[]);
    let gap = ring.pop().unwrap();
    assert_eq!((gap.kind, gap.dropped()), (Kind::Gap, 1));
    assert_eq!(ring.pop().unwrap().header.sequence, 6);
}

#[test]
fn the_writer_puts_each_calls_packets_in_a_file_of_its_own() {
    let scratch = Scratch::new("writer");
    let (writer, events) = writer(&scratch.0);
    let meta = |line| CaptureMeta {
        line,
        call_id: format!("call-{line}@10.10.20.11"),
        payload_type: 8,
        ptime_ms: 20,
    };
    let mut first = writer.tap(meta(3));
    let mut second = writer.tap(meta(12));
    let untouched = writer.tap(meta(5));

    for n in 0..50 {
        first.offer(
            Kind::Received,
            u64::from(n) * 20_000_000,
            &rtp(n),
            &[1; 160],
        );
        first.offer(
            Kind::Sent,
            u64::from(n) * 20_000_000 + 1,
            &rtp(n),
            &[2; 160],
        );
    }
    second.offer(Kind::Sent, 0, &rtp(0), &[3; 160]);
    // The calls end, and the writer with them: everything is on disk when
    // it has been dropped.
    drop((first, second, untouched));
    drop(writer);

    let mut said: Vec<String> = events.try_iter().collect();
    said.sort();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[0].starts_with("started 12 line12-") && said[0].ends_with(".rmcap"));
    assert!(said[1].starts_with("started 3 line03-"));

    // A call nothing was captured of has no file.
    let files = scratch.files();
    assert_eq!(files.len(), 2);
    let (header, found) = report(&files[0]);
    assert_eq!(
        (
            header.line,
            header.payload_type,
            header.ptime_ms,
            header.call_id.as_str()
        ),
        (3, 8, 20, "call-3@10.10.20.11")
    );
    assert!(header.started_unix_ms > 1_700_000_000_000);
    assert_eq!(
        (found.received, found.sent),
        (0, 50),
        "payload type 0 in a PCMA call is ignored"
    );
    assert_eq!(found.ignored, 50);
    let (header, found) = report(&files[1]);
    assert_eq!(header.line, 12);
    assert_eq!(found.sent, 1);
}

#[test]
fn a_directory_that_cannot_be_made_fails_the_capture_and_nothing_else() {
    let scratch = Scratch::new("blocked");
    // A file where the directory should be.
    std::fs::write(&scratch.0, b"in the way").unwrap();
    let (writer, events) = writer(&scratch.0);
    let mut tap = writer.tap(CaptureMeta {
        line: 1,
        call_id: String::new(),
        payload_type: 0,
        ptime_ms: 20,
    });
    for n in 0..10 {
        tap.offer(Kind::Received, 0, &rtp(n), &[0; 160]);
    }
    drop(tap);
    drop(writer);
    let said: Vec<String> = events.try_iter().collect();
    assert_eq!(said, ["failed 1"], "said once, not for every record");
    std::fs::remove_file(&scratch.0).unwrap();
}

/// A far end with a clock of its own: sends its 20 ms packets `numbers`,
/// one every 20 ms, and counts what comes back.
fn far_end(
    socket: &UdpSocket,
    to: SocketAddrV4,
    numbers: std::ops::Range<u32>,
    skip: &[u32],
) -> u32 {
    // The wait is the one that loses no datagram: see `media::receive_within`.
    const WAIT: Duration = Duration::from_millis(1);
    media::prepare_socket(socket, WAIT).unwrap();
    let mut back = 0;
    let mut buffer = [0u8; 512];
    let began = Instant::now();
    let first = numbers.start;
    for n in numbers {
        let due = began + Duration::from_millis(20 * u64::from(n - first));
        while Instant::now() < due {
            if let Some((len, _)) = media::receive_within(socket, &mut buffer, WAIT)
                && RtpPacket::parse(&buffer[..len]).is_ok()
            {
                back += 1;
            }
        }
        if skip.contains(&n) {
            continue;
        }
        let mut packet = [0u8; HEADER_LEN + 160];
        rtp(n).write_packet(&[n as u8; 160], &mut packet).unwrap();
        socket.send_to(&packet, to).unwrap();
    }
    let until = Instant::now() + Duration::from_millis(100);
    while Instant::now() < until {
        if media::receive_within(socket, &mut buffer, WAIT).is_some() {
            back += 1;
        }
    }
    back
}

/// A call on a media thread with the echo engine, against a far end with
/// a clock, captured.
fn captured_call(name: &str, packets: u32, skip: &[u32], on_from: Option<u32>) -> (Report, u32) {
    let scratch = Scratch::new(name);
    let (writer, _events) = writer(&scratch.0);
    let mut runner = ThreadedRunner::with_capture(1, Arc::new(|| {}), writer).unwrap();

    let ours = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let far = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (SocketAddr::V4(our_address), SocketAddr::V4(far_address)) =
        (ours.local_addr().unwrap(), far.local_addr().unwrap())
    else {
        panic!("not IPv4");
    };
    let engine: Box<dyn ModemEngine> = Box::new(EchoEngine::new());
    let endpoint = MediaEndpoint::new(RtpEndpoint {
        socket: ours,
        params: Arc::new(RtpParams::new(far_address, 0, 20)),
        timeout_ms: 10_000,
        ssrc: 1,
        sequence: 100,
        timestamp: 0,
        call_id: "captured@10.10.20.11".to_owned(),
    });
    let call = runner.start(4, engine, endpoint, false).ok().unwrap();
    call.set_capture(on_from.is_none());

    let back = match on_from {
        None => far_end(&far, our_address, 0..packets, skip),
        Some(from) => {
            // The capture is turned on in the middle of the call.
            let before = far_end(&far, our_address, 0..from, skip);
            call.set_capture(true);
            before + far_end(&far, our_address, from..packets, &[])
        }
    };
    let stats = call.stats();
    assert!(runner.stop(4, call).is_some());
    drop(runner);
    assert_eq!(stats.map(|s| s.capture_dropped), Some(0));

    let files = scratch.files();
    assert_eq!(files.len(), 1, "{files:?}");
    let (header, found) = report(&files[0]);
    assert_eq!(header.call_id, "captured@10.10.20.11");
    assert_eq!(
        (header.line, header.payload_type, header.ptime_ms),
        (4, 0, 20)
    );
    (found, back)
}

/// The checker's verdict on a real call with nothing lost: no loss, no
/// resynchronisation, no slip.
#[test]
fn a_clean_call_checks_clean() {
    let (found, back) = captured_call("clean", 100, &[], None);
    assert!(found.is_clean(), "{found:?}");
    assert_eq!(found.received, 100);
    assert_eq!(
        (
            found.lost,
            found.late,
            found.duplicate,
            found.resync,
            found.slips
        ),
        (0, 0, 0, 0, 0)
    );
    // One packet out for each packet in, and what the local timer sent
    // before the far end began.
    assert_eq!(found.sent, 100 + found.free_run / 2);
    assert_eq!(
        u64::from(back),
        found.sent,
        "the far end got every one of them"
    );
    // The far end's clock is 20 ms; the gaps between arrivals say so,
    // give or take the scheduler.
    assert!(
        (15_000..200_000).contains(&found.largest_gap_us),
        "{found:?}"
    );
}

#[test]
fn a_call_with_loss_checks_as_lossy_and_still_without_a_slip() {
    let (found, _) = captured_call("lossy", 100, &[20, 21, 50], None);
    assert_eq!(found.lost, 3);
    assert_eq!(found.received, 97);
    assert_eq!(found.slips, 0, "what was lost was filled: {found:?}");
    assert!(!found.is_clean());
    // The fill was answered too: a packet out for every packet the far
    // end's clock produced.
    assert_eq!(found.sent, 100 + found.free_run / 2);
}

#[test]
fn a_capture_turned_on_in_the_middle_of_a_call_starts_there() {
    let (found, _) = captured_call("late", 100, &[], Some(60));
    assert!(found.is_clean(), "{found:?}");
    assert_eq!(found.received, 40);
    // An arrival and what is sent for it are captured together or not at
    // all, so there is no slip at the seam and nothing before it.
    assert_eq!((found.sent, found.free_run), (40, 0));
}

#[test]
fn the_ring_holds_seconds_of_packets() {
    // Both directions at the shortest packet time are 200 records a
    // second.
    const { assert!(CAPTURE_RING >= 5 * 200) };
}
