// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The threaded runner over real sockets on the loopback interface.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use engine::{ConnectInfo, ModemEngine, ModemEvent, NullConfig, NullEngine};
use media::{RtpEndpoint, RtpParams, RtpPorts, ThreadedRunner, Waker};
use pool::{LocalEndpoint, MediaCall, MediaEndpoint, MediaEvent, MediaRunner};

fn loopback() -> (UdpSocket, SocketAddrV4) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback socket");
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        panic!("not an IPv4 socket");
    };
    (socket, address)
}

fn endpoint(
    socket: UdpSocket,
    remote: SocketAddrV4,
    ptime_ms: u32,
    timeout_ms: u64,
) -> MediaEndpoint {
    MediaEndpoint::new(RtpEndpoint {
        socket,
        params: Arc::new(RtpParams::new(remote, 0, ptime_ms)),
        timeout_ms,
        ssrc: u32::from(remote.port()),
        sequence: 7,
        timestamp: 9000,
        call_id: format!("call-{}", remote.port()),
    })
}

fn null_engine() -> Box<dyn ModemEngine> {
    Box::new(NullEngine::new(NullConfig::default()))
}

fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let waker: Waker = Arc::new(move || {
        seen.fetch_add(1, Ordering::Relaxed);
        true
    });
    (waker, count)
}

/// Waits for the next event of a call.
fn next_event(call: &mut MediaCall, within: Duration) -> Option<MediaEvent> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(event) = call.poll_event() {
            return Some(event);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Data for one direction: every octet value, in an order that does not
/// repeat with the frame.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect()
}

/// Exit criterion: a megabyte each way through two null engines over
/// loopback UDP arrives byte for byte.
#[test]
fn a_megabyte_each_way_through_two_null_engines_over_loopback() {
    const SIZE: usize = 1024 * 1024;
    let (waker, wakes) = counting_waker();
    let mut runner = ThreadedRunner::new(2, waker).unwrap();
    assert_eq!(runner.threads(), 2);

    let (socket_a, address_a) = loopback();
    let (socket_b, address_b) = loopback();
    let mut a = runner
        .start(
            1,
            null_engine(),
            endpoint(socket_a, address_b, 20, 10_000),
            true,
        )
        .ok()
        .expect("the runner takes an RTP endpoint");
    let mut b = runner
        .start(
            2,
            null_engine(),
            endpoint(socket_b, address_a, 20, 10_000),
            false,
        )
        .ok()
        .expect("the runner takes an RTP endpoint");
    assert_eq!(runner.call_count(), 2);

    // Both engines connect at once and say so.
    let connected = Some(MediaEvent::Modem(ModemEvent::Connect(
        ConnectInfo::unknown(),
    )));
    assert_eq!(next_event(&mut a, Duration::from_secs(5)), connected);
    assert_eq!(next_event(&mut b, Duration::from_secs(5)), connected);
    assert!(
        wakes.load(Ordering::Relaxed) >= 2,
        "the pool was woken for them"
    );

    let from_a = pattern(SIZE, 1);
    let from_b = pattern(SIZE, 2);
    let (mut sent_a, mut sent_b) = (0, 0);
    let mut at_a = Vec::with_capacity(SIZE);
    let mut at_b = Vec::with_capacity(SIZE);
    let mut buffer = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(240);
    while at_a.len() < SIZE || at_b.len() < SIZE {
        assert!(
            Instant::now() < deadline,
            "{} of {SIZE} at a, {} at b",
            at_a.len(),
            at_b.len()
        );
        sent_a += a.write(&from_a[sent_a..]);
        sent_b += b.write(&from_b[sent_b..]);
        let count = a.read(&mut buffer);
        at_a.extend_from_slice(&buffer[..count]);
        let count = b.read(&mut buffer);
        at_b.extend_from_slice(&buffer[..count]);
        std::thread::yield_now();
    }
    assert!(at_b == from_a, "what a sent is what b got");
    assert!(at_a == from_b, "what b sent is what a got");
    assert_eq!(a.poll_event(), None, "the carrier was never lost");
    assert_eq!(b.poll_event(), None);

    // The statistics the threads published: every frame that came in went
    // out, nothing was lost, and nothing slipped.
    std::thread::sleep(Duration::from_millis(50));
    for call in [&a, &b] {
        let stats = call.stats().expect("a media thread publishes statistics");
        assert_eq!(
            (stats.lost, stats.resync, stats.late, stats.duplicate),
            (0, 0, 0, 0)
        );
        assert_eq!((stats.stray, stats.ignored), (0, 0));
        assert!(stats.rx_packets > 6000, "{stats:?}");
        assert_eq!(stats.payload_type, 0);
        assert_eq!(stats.ptime_ms, 20);
        assert!(stats.tick_p99_us <= stats.tick_max_us);
    }

    // Both engines come back, stopped, and their threads are free again.
    let engine_a = runner.stop(1, a).expect("the engine comes back");
    let engine_b = runner.stop(2, b).expect("the engine comes back");
    drop((engine_a, engine_b));
    assert_eq!(runner.call_count(), 0);
    let (socket, address) = loopback();
    let again = runner.start(
        3,
        null_engine(),
        endpoint(socket, address, 20, 10_000),
        true,
    );
    assert!(again.is_ok());
    assert_eq!(
        runner.threads(),
        2,
        "a thread that finished a call takes the next"
    );
}

/// With one end silent the other starts by itself, on its own timer.
#[test]
fn an_engine_starts_before_the_far_end_sends() {
    let (waker, _) = counting_waker();
    let mut runner = ThreadedRunner::new(1, waker).unwrap();
    let (socket, _) = loopback();
    let (far, far_address) = loopback();
    far.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let call = runner
        .start(
            1,
            null_engine(),
            endpoint(socket, far_address, 20, 10_000),
            false,
        )
        .ok()
        .unwrap();

    // The far end hears us though it has said nothing: 20 ms packets of
    // the idle pattern, numbered from where the endpoint said.
    let mut buffer = [0u8; 512];
    let mut sequences = Vec::new();
    for _ in 0..5 {
        let (len, _) = far
            .recv_from(&mut buffer)
            .expect("a packet from the free run");
        let packet = rtp::RtpPacket::parse(&buffer[..len]).unwrap();
        assert_eq!(packet.payload, [0x7E; 160]);
        assert_eq!(packet.header.payload_type, 0);
        assert_eq!(packet.header.ssrc, u32::from(far_address.port()));
        sequences.push(packet.header.sequence);
    }
    assert_eq!(sequences, [7, 8, 9, 10, 11]);
    assert!(runner.stop(1, call).is_some());
}

#[test]
fn a_call_whose_far_end_goes_silent_is_reported_stalled() {
    let (waker, wakes) = counting_waker();
    let mut runner = ThreadedRunner::new(1, waker).unwrap();
    let (socket, _) = loopback();
    let (_far, far_address) = loopback();
    let mut call = runner
        .start(
            1,
            null_engine(),
            endpoint(socket, far_address, 20, 300),
            true,
        )
        .ok()
        .unwrap();
    assert!(matches!(
        next_event(&mut call, Duration::from_secs(5)),
        Some(MediaEvent::Modem(ModemEvent::Connect(_)))
    ));
    let before = wakes.load(Ordering::Relaxed);
    assert_eq!(
        next_event(&mut call, Duration::from_secs(5)),
        Some(MediaEvent::Stalled)
    );
    // The media thread puts the event in the ring and then wakes the
    // pool, so the event can be seen here a moment before the wake is.
    let waited = Instant::now();
    while wakes.load(Ordering::Relaxed) <= before && waited.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(wakes.load(Ordering::Relaxed) > before);
    let stats = call.stats().expect("published when it stalled");
    assert_eq!(stats.rx_packets, 0);
    assert!(
        stats.free_run >= 20,
        "it ticked from its own timer meanwhile: {stats:?}"
    );
    assert!(runner.stop(1, call).is_some());
}

#[test]
fn an_endpoint_that_is_not_rtp_is_refused_and_the_engine_comes_back() {
    let (waker, _) = counting_waker();
    let mut runner = ThreadedRunner::new(1, waker).unwrap();
    let (local, _) = LocalEndpoint::pair();
    let refused = runner.start(1, null_engine(), MediaEndpoint::new(local), true);
    assert!(refused.is_err());
    assert_eq!(runner.call_count(), 0);
    // The thread is still free for a call it can carry.
    let (socket, address) = loopback();
    assert!(
        runner
            .start(
                1,
                null_engine(),
                endpoint(socket, address, 20, 10_000),
                true
            )
            .is_ok()
    );
}

#[test]
fn more_calls_than_threads_make_more_threads() {
    let (waker, _) = counting_waker();
    let mut runner = ThreadedRunner::new(0, waker).unwrap();
    assert_eq!(runner.threads(), 0);
    let mut calls = Vec::new();
    for line in 1..=3 {
        let (socket, address) = loopback();
        let call = runner
            .start(
                line,
                null_engine(),
                endpoint(socket, address, 10, 10_000),
                true,
            )
            .ok()
            .unwrap();
        calls.push((line, call));
    }
    assert_eq!((runner.threads(), runner.call_count()), (3, 3));
    for (line, call) in calls {
        assert!(runner.stop(line, call).is_some());
    }
    assert_eq!(runner.call_count(), 0);
    // Dropping the runner ends its threads; a runner dropped with a call
    // still up ends that too.
    let (socket, address) = loopback();
    let _call = runner
        .start(
            9,
            null_engine(),
            endpoint(socket, address, 20, 10_000),
            true,
        )
        .ok()
        .unwrap();
    drop(runner);
}

#[test]
fn sockets_come_from_the_even_ports_of_the_range() {
    // Ten ports well below the ones the system hands out by itself, which
    // the other tests are taking as this one runs.
    let first = 30_000 + (std::process::id() % 1000) as u16 * 10;
    let mut ports = RtpPorts::new(first, first + 9);

    let mut taken = Vec::new();
    let mut sockets = Vec::new();
    while let Ok(socket) = ports.bind(Ipv4Addr::LOCALHOST) {
        taken.push(socket.local_addr().unwrap().port());
        sockets.push(socket);
        assert!(sockets.len() <= 5, "there are five even ports in ten");
    }
    assert!(!taken.is_empty());
    for port in &taken {
        assert_eq!(port % 2, 0);
        assert!((first..=first + 9).contains(port));
    }
    let mut unique = taken.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), taken.len(), "no port is given twice");

    // A port given back can be had again, though the search has moved
    // on. (Another test may have let go of a port of the range meanwhile,
    // so it need not be the same one.)
    drop(sockets.remove(0));
    let again = ports.bind(Ipv4Addr::LOCALHOST).expect("a port is free");
    let port = again.local_addr().unwrap().port();
    assert_eq!(port % 2, 0);
    assert!((first..=first + 9).contains(&port));
    assert!(
        sockets
            .iter()
            .all(|s| s.local_addr().unwrap().port() != port)
    );
    // A range with no even port gives none.
    assert!(
        RtpPorts::new(first + 1, first + 1)
            .bind(Ipv4Addr::LOCALHOST)
            .is_err()
    );
}

/// The wait for a datagram loses none when it runs out. On Windows a
/// socket read timeout does: a datagram that arrives as the timeout fires
/// goes with the receive that was cancelled. Here the waits are far
/// shorter than the gaps between datagrams, so that every datagram is
/// preceded by a wait that ran out, some of them at the very moment it
/// arrives.
#[test]
fn a_wait_that_runs_out_loses_no_datagram() {
    const DATAGRAMS: u16 = 400;
    const WAIT: Duration = Duration::from_millis(2);
    let (receiving, at) = loopback();
    let (sending, _) = loopback();
    media::prepare_socket(&receiving, WAIT).unwrap();

    // Nothing has been sent: the wait runs out, and does not hang. How
    // short it may be is the system's timer's to say: on a machine whose
    // timer ticks coarsely a wait of 2 ms has come back inside one.
    let mut buffer = [0u8; 64];
    let began = Instant::now();
    assert_eq!(media::receive_within(&receiving, &mut buffer, WAIT), None);
    assert!(began.elapsed() < Duration::from_secs(1));

    let sender = std::thread::spawn(move || {
        for n in 0..DATAGRAMS {
            sending.send_to(&n.to_be_bytes(), at).unwrap();
            // A gap that drifts against the waits of the receiver.
            std::thread::sleep(Duration::from_micros(2500 + 37 * u64::from(n % 20)));
        }
    });
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while seen.len() < usize::from(DATAGRAMS) && Instant::now() < deadline {
        if let Some((len, _)) = media::receive_within(&receiving, &mut buffer, WAIT) {
            assert_eq!(len, 2);
            seen.push(u16::from_be_bytes([buffer[0], buffer[1]]));
        }
    }
    sender.join().unwrap();
    let expected: Vec<u16> = (0..DATAGRAMS).collect();
    assert_eq!(seen, expected, "every datagram, once, in order");
}

/// A waker that cannot deliver its wake says so, and the media thread
/// asks again at its next turns until it can: the pool is never left
/// without the wake for an event that is in the call's ring, and the
/// thread never waits for it.
#[test]
fn a_wake_that_is_refused_is_tried_again_until_it_is_taken() {
    const REFUSALS: usize = 5;
    let asked = Arc::new(AtomicUsize::new(0));
    let waker: Waker = {
        let asked = Arc::clone(&asked);
        // Refused the first few times, as by a channel that is full.
        Arc::new(move || asked.fetch_add(1, Ordering::Relaxed) >= REFUSALS)
    };
    let mut runner = ThreadedRunner::new(1, waker).unwrap();
    let (socket, _) = loopback();
    let (_far, far_address) = loopback();
    let mut call = runner
        .start(
            1,
            null_engine(),
            endpoint(socket, far_address, 20, 10_000),
            false,
        )
        .ok()
        .expect("the runner takes an RTP endpoint");

    // The engine connects at once: one event, and one wake owed for it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while asked.load(Ordering::Relaxed) <= REFUSALS {
        assert!(Instant::now() < deadline, "the wake was not tried again");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Once it is taken it is not asked for again: nothing else happened.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(asked.load(Ordering::Relaxed), REFUSALS + 1);
    // The event was in the ring all the while.
    assert_eq!(
        next_event(&mut call, Duration::from_secs(1)),
        Some(MediaEvent::Modem(ModemEvent::Connect(
            ConnectInfo::unknown()
        )))
    );
    runner.stop(1, call);
}
