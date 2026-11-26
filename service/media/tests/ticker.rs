// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The real tick loop with a scripted far end and a clock the test moves.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use engine::{ConnectInfo, ModemEngine, ModemEvent, NullConfig, NullEngine};
use media::{Clock, Received, RtpParams, Ticker, Transport, histogram_bucket};
use pool::{MediaCall, MediaEvent, MediaPort, media_pair};
use proptest::prelude::*;
use rtp::{HEADER_LEN, RtpHeader, RtpPacket};

const FAR: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 11), 16482);
const OTHER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 99), 16482);

/// What the loop sent, and to whom.
#[derive(Default)]
struct Wire {
    sent: Vec<(SocketAddrV4, Vec<u8>)>,
}

impl Transport for Wire {
    fn receive(&mut self, _buffer: &mut [u8]) -> Received {
        Received::Nothing
    }

    fn send(&mut self, datagram: &[u8], to: SocketAddrV4) {
        self.sent.push((to, datagram.to_vec()));
    }
}

/// A clock the test sets. Each reading moves it on by `step`, which is
/// how long a tick appears to take.
struct TestClock {
    now: u64,
    step: u64,
}

impl Clock for TestClock {
    fn now_us(&mut self) -> u64 {
        let now = self.now;
        self.now += self.step;
        now
    }
}

/// An engine that sends back what it receives and counts its ticks.
#[derive(Default)]
struct Echo {
    frames: u64,
    events: Vec<ModemEvent>,
}

impl ModemEngine for Echo {
    fn process(&mut self, rx: &[u8], tx: &mut [u8]) {
        assert_eq!(rx.len(), 80, "a tick is one frame of 80 octets");
        assert_eq!(tx.len(), 80);
        tx.copy_from_slice(rx);
        self.frames += 1;
    }
    fn dte_write(&mut self, data: &[u8]) -> usize {
        data.len()
    }
    fn dte_read(&mut self, _buf: &mut [u8]) -> usize {
        0
    }
    fn poll_event(&mut self) -> Option<ModemEvent> {
        self.events.pop()
    }
    fn answer(&mut self) {}
    fn originate(&mut self) {}
    fn hangup(&mut self) {}
}

/// A call under test: the loop, its far end, its clock and both ends of
/// the seam.
struct Call<E: ModemEngine> {
    ticker: Ticker,
    params: Arc<RtpParams>,
    wire: Wire,
    clock: TestClock,
    engine: E,
    port: MediaPort,
    pool: MediaCall,
}

impl<E: ModemEngine> Call<E> {
    fn new(engine: E, payload_type: u8, ptime_ms: u32, timeout_ms: u64) -> Call<E> {
        let params = Arc::new(RtpParams::new(FAR, payload_type, ptime_ms));
        let (pool, port) = media_pair();
        Call {
            ticker: Ticker::new(
                0,
                Arc::clone(&params),
                timeout_ms,
                0xABCD_0123,
                1000,
                50_000,
            ),
            params,
            wire: Wire::default(),
            clock: TestClock { now: 0, step: 0 },
            engine,
            port,
            pool,
        }
    }

    /// One turn of the loop at `now_us` with nothing received.
    fn idle(&mut self, now_us: u64) -> bool {
        self.clock.now = now_us;
        self.ticker.turn(
            Received::Nothing,
            &[],
            &mut self.engine,
            &mut self.wire,
            &mut self.clock,
            &mut self.port,
        )
    }

    /// One turn of the loop at `now_us` with this datagram received.
    fn datagram(&mut self, now_us: u64, from: SocketAddrV4, datagram: &[u8]) -> bool {
        self.clock.now = now_us;
        self.ticker.turn(
            Received::Datagram {
                len: datagram.len(),
                from,
            },
            datagram,
            &mut self.engine,
            &mut self.wire,
            &mut self.clock,
            &mut self.port,
        )
    }

    /// The packets sent since the last look: header and payload.
    fn sent(&mut self) -> Vec<(RtpHeader, Vec<u8>)> {
        self.wire
            .sent
            .drain(..)
            .map(|(to, datagram)| {
                assert_eq!(to, self.params.remote());
                let packet = RtpPacket::parse(&datagram).expect("what is sent is RTP");
                (packet.header, packet.payload.to_vec())
            })
            .collect()
    }
}

/// Packet `n` of the far end's stream: `frames` frames of the octet `n`.
fn far_packet(n: u32, payload_type: u8, frames: usize) -> Vec<u8> {
    far_packet_at(n, 160 * n, payload_type, &vec![n as u8; 80 * frames])
}

fn far_packet_at(sequence: u32, timestamp: u32, payload_type: u8, payload: &[u8]) -> Vec<u8> {
    let header = RtpHeader {
        marker: sequence == 0,
        payload_type,
        sequence: sequence as u16,
        timestamp,
        ssrc: 0x5555_0001,
    };
    let mut out = vec![0u8; HEADER_LEN + payload.len()];
    header.write_packet(payload, &mut out).unwrap();
    out
}

#[test]
fn before_the_far_end_sends_the_loop_ticks_from_its_own_timer() {
    for (ptime_ms, frames) in [(10u32, 1usize), (20, 2), (30, 3)] {
        let mut call = Call::new(Echo::default(), 0, ptime_ms, 10_000);
        let packet_us = u64::from(ptime_ms) * 1000;

        // Nothing is due before a packet time has passed.
        call.idle(packet_us - 1);
        assert!(call.sent().is_empty());
        assert_eq!(call.engine.frames, 0);

        call.idle(packet_us);
        let sent = call.sent();
        assert_eq!(sent.len(), 1, "one packet a packet time");
        let (header, payload) = &sent[0];
        assert_eq!(
            *header,
            RtpHeader {
                marker: true,
                payload_type: 0,
                sequence: 1000,
                timestamp: 50_000,
                ssrc: 0xABCD_0123,
            }
        );
        // What the engine was given, and echoed, is silence.
        assert_eq!(*payload, vec![0xFF; 80 * frames]);
        assert_eq!(call.engine.frames, frames as u64);

        // And so on, a packet for each packet time, however the turns fall.
        call.idle(packet_us + 1);
        call.idle(2 * packet_us + 400);
        call.idle(4 * packet_us);
        let sent = call.sent();
        assert_eq!(sent.len(), 3);
        assert!(!sent[0].0.marker);
        assert_eq!(sent[2].0.sequence, 1003);
        assert_eq!(sent[2].0.timestamp, 50_000 + 3 * 80 * frames as u32);
        assert_eq!(call.ticker.stats().free_run, 4 * frames as u64);
    }
}

#[test]
fn a_loop_that_fell_far_behind_its_timer_does_not_make_it_all_up() {
    let mut call = Call::new(Echo::default(), 0, 20, 100_000);
    // Three seconds late: five packets are made up and the rest let go.
    call.idle(3_000_000);
    assert_eq!(call.sent().len(), 5);
    call.idle(3_000_001);
    assert!(call.sent().is_empty());
    call.idle(3_020_000);
    assert_eq!(call.sent().len(), 1);
}

#[test]
fn the_first_packet_switches_to_tick_on_receive_for_good() {
    let mut call = Call::new(Echo::default(), 0, 20, 10_000);
    call.idle(20_000);
    call.idle(40_000);
    assert_eq!(call.sent().len(), 2);
    assert_eq!(call.ticker.stats().free_run, 4);

    call.datagram(45_000, FAR, &far_packet(0, 0, 2));
    assert_eq!(call.sent().len(), 1);

    // From here on the local timer sends nothing, however long the far
    // end is quiet.
    for now in [60_000, 80_000, 500_000, 5_000_000] {
        call.idle(now);
    }
    assert!(call.sent().is_empty());
    assert_eq!(call.engine.frames, 6);
    assert_eq!(call.ticker.stats().free_run, 4);

    call.datagram(5_000_100, FAR, &far_packet(1, 0, 2));
    assert_eq!(call.sent().len(), 1);
}

#[test]
fn one_received_frame_produces_exactly_one_transmitted_frame() {
    let mut call = Call::new(Echo::default(), 0, 20, 10_000);
    for n in 0..50u32 {
        call.datagram(u64::from(n) * 20_000, FAR, &far_packet(n, 0, 2));
    }
    let sent = call.sent();
    assert_eq!(sent.len(), 50);
    for (n, (header, payload)) in sent.iter().enumerate() {
        // The echo of packet n, in order and whole.
        assert_eq!(*payload, vec![n as u8; 160], "packet {n}");
        assert_eq!(header.sequence, 1000 + n as u16);
        assert_eq!(header.timestamp, 50_000 + 160 * n as u32);
        assert_eq!(header.marker, n == 0);
    }
    let stats = call.ticker.stats();
    assert_eq!((stats.rx_packets, stats.rx_octets), (50, 8000));
    assert_eq!((stats.tx_packets, stats.tx_octets), (50, 8000));
    assert_eq!(stats.free_run, 0);
    assert_eq!(
        (
            stats.lost,
            stats.late,
            stats.duplicate,
            stats.resync,
            stats.stray
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!((stats.payload_type, stats.ptime_ms), (0, 20));
    assert_eq!(stats.remote, Some(FAR));
}

#[test]
fn packets_of_another_size_are_ticked_frame_by_frame() {
    // The far end sends 30 ms packets and we send 20 ms ones: six frames
    // in, six frames out, as three packets.
    let mut call = Call::new(Echo::default(), 0, 20, 10_000);
    call.datagram(0, FAR, &far_packet_at(0, 0, 0, &[1; 240]));
    assert_eq!(call.sent().len(), 1);
    call.datagram(30_000, FAR, &far_packet_at(1, 240, 0, &[2; 240]));
    let sent = call.sent();
    assert_eq!(sent.len(), 2);
    // The frame left over from the first packet leads the next.
    assert_eq!(sent[0].1[..80], [1; 80]);
    assert_eq!(sent[0].1[80..], [2; 80]);
    assert_eq!(sent[1].1, vec![2; 160]);
    assert_eq!(call.engine.frames, 6);
}

#[test]
fn what_was_lost_is_filled_with_silence_before_the_packet() {
    for (payload_type, silence) in [(0u8, 0xFFu8), (8, 0xD5)] {
        let mut call = Call::new(Echo::default(), payload_type, 20, 10_000);
        call.datagram(0, FAR, &far_packet(0, payload_type, 2));
        // Packets 1 and 2 are lost.
        call.datagram(60_000, FAR, &far_packet(3, payload_type, 2));
        let sent = call.sent();
        assert_eq!(
            sent.len(),
            4,
            "a frame out for every frame of the far end's clock"
        );
        assert_eq!(sent[1].1, vec![silence; 160]);
        assert_eq!(sent[2].1, vec![silence; 160]);
        assert_eq!(sent[3].1, vec![3; 160]);
        assert_eq!(sent[0].0.payload_type, payload_type);
        let stats = call.ticker.stats();
        assert_eq!((stats.lost, stats.rx_packets, stats.tx_packets), (2, 2, 4));

        // The lost ones turn up late and a copy comes twice: nothing more
        // is ticked.
        call.datagram(61_000, FAR, &far_packet(1, payload_type, 2));
        call.datagram(62_000, FAR, &far_packet(3, payload_type, 2));
        assert!(call.sent().is_empty());
        let stats = call.ticker.stats();
        assert_eq!((stats.late, stats.duplicate), (1, 1));
    }
}

#[test]
fn only_the_far_end_of_the_call_is_listened_to() {
    let mut call = Call::new(Echo::default(), 0, 20, 10_000);
    call.datagram(0, OTHER, &far_packet(0, 0, 2));
    call.datagram(
        1000,
        SocketAddrV4::new(*FAR.ip(), FAR.port() + 2),
        &far_packet(0, 0, 2),
    );
    // From the far end, and not RTP.
    call.datagram(2000, FAR, b"OPTIONS sip:x SIP/2.0\r\n\r\n");
    call.datagram(3000, FAR, &[0x80; 5]);
    assert!(call.sent().is_empty());
    assert_eq!(call.engine.frames, 0);
    assert_eq!(call.ticker.stats().stray, 4);

    // Another payload type from the far end is counted apart.
    call.datagram(4000, FAR, &far_packet(0, 101, 2));
    assert_eq!(call.ticker.stats().ignored, 1);
    assert_eq!(call.engine.frames, 0);

    // A stranger does not stop the timer from starting the engine.
    call.idle(20_000);
    assert_eq!(call.sent().len(), 1);
}

#[test]
fn a_call_that_hears_nothing_for_the_timeout_is_given_up_once() {
    let mut call = Call::new(Echo::default(), 0, 20, 2000);
    call.datagram(0, FAR, &far_packet(0, 0, 2));
    assert!(!call.idle(2_000_000), "not yet: the timeout has not passed");
    assert_eq!(call.pool.poll_event(), None);

    assert!(call.idle(2_000_001), "the pool is woken");
    assert_eq!(call.pool.poll_event(), Some(MediaEvent::Stalled));
    assert!(call.ticker.is_stalled());
    assert!(call.pool.stats().is_some(), "the statistics are published");

    // It stays given up: no second report, and nothing is ticked.
    assert!(!call.idle(3_000_000));
    assert!(!call.datagram(3_000_100, FAR, &far_packet(1, 0, 2)));
    assert_eq!(call.pool.poll_event(), None);
    assert_eq!(call.sent().len(), 1);
}

#[test]
fn a_call_whose_far_end_never_sends_is_given_up_too() {
    let mut call = Call::new(Echo::default(), 0, 20, 1000);
    for now in (20_000..=1_000_000).step_by(20_000) {
        assert!(!call.idle(now));
    }
    assert_eq!(call.sent().len(), 50, "it sent all the while");
    assert!(call.idle(1_020_000));
    assert_eq!(call.pool.poll_event(), Some(MediaEvent::Stalled));
    // A stranger's packets do not keep it alive.
    let mut call = Call::new(Echo::default(), 0, 20, 1000);
    call.datagram(900_000, OTHER, &far_packet(0, 0, 2));
    assert!(call.datagram(1_000_001, OTHER, &far_packet(1, 0, 2)));
}

#[test]
fn a_renegotiation_takes_effect_on_the_next_turn() {
    let mut call = Call::new(Echo::default(), 0, 20, 10_000);
    call.datagram(0, FAR, &far_packet(0, 0, 2));
    assert_eq!(call.sent()[0].0.payload_type, 0);

    // The far end moves, changes codec and asks for 10 ms packets.
    call.params.set(OTHER, 8, 10, false);
    call.datagram(20_000, FAR, &far_packet(1, 0, 2));
    assert!(call.sent().is_empty(), "the old address is a stranger now");
    assert_eq!(call.ticker.stats().stray, 1);

    call.datagram(40_000, OTHER, &far_packet_at(2, 320, 8, &[7; 160]));
    let sent = call.sent();
    // The packet that went to the old address is a packet lost: its two
    // frames are filled, in the new codec's silence, and then come the two
    // of this one. Four frames, four packets of one frame.
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[0].0.payload_type, 8);
    assert_eq!(sent[0].1, vec![0xD5; 80]);
    assert_eq!(sent[1].1, vec![0xD5; 80]);
    assert_eq!(sent[2].1, vec![7; 80]);
    assert_eq!(sent[3].1, vec![7; 80]);
    // Our own numbering goes on where it was.
    assert_eq!(sent[0].0.sequence, 1001);
    assert_eq!(sent[0].0.timestamp, 50_160);
    assert_eq!(sent[1].0.timestamp, 50_240);
    let stats = call.ticker.stats();
    assert_eq!(
        (stats.payload_type, stats.ptime_ms, stats.remote),
        (8, 10, Some(OTHER))
    );
}

#[test]
fn a_far_end_that_holds_the_call_is_sent_nothing_and_the_engine_ticks_on() {
    let mut call = Call::new(Echo::default(), 0, 20, 1000);
    call.datagram(0, FAR, &far_packet(0, 0, 2));
    assert_eq!(call.sent().len(), 1);

    // The far end puts the call on hold the old way: its address is
    // 0.0.0.0.
    let nowhere = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    call.params.set(nowhere, 0, 20, true);
    call.idle(10_000);
    // The engine goes on ticking, from the local timer and on silence;
    // nothing is sent; and the silence of the far end, far longer than the
    // timeout, is not taken for a call that has gone.
    for now in (30_000..3_000_000).step_by(20_000) {
        assert!(!call.idle(now));
    }
    assert!(call.wire.sent.is_empty());
    assert_eq!(call.engine.frames, 2 + 2 * 149);
    assert!(!call.ticker.is_stalled());
    assert_eq!(call.pool.poll_event(), None);

    // Off hold, the far end starts a new stream. Its first packet is a
    // first packet: nothing is filled and nothing is counted against it.
    call.params.set(FAR, 0, 20, false);
    call.datagram(3_000_500, FAR, &far_packet_at(500, 999_999, 0, &[9; 160]));
    let sent = call.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, vec![9; 160]);
    let stats = call.ticker.stats();
    assert_eq!((stats.resync, stats.lost), (0, 0));

    // A far end that holds the call and goes on sending, music perhaps,
    // is listened to and not sent to.
    call.params.set(FAR, 0, 20, true);
    call.datagram(3_020_000, FAR, &far_packet_at(900, 5, 0, &[3; 160]));
    assert!(call.wire.sent.is_empty());
    assert_eq!(call.engine.frames, 2 + 2 * 149 + 4);
    assert_eq!(call.ticker.stats().resync, 0);

    // Off hold with nothing arriving, the call is given up after the
    // timeout, counted from when the hold ended.
    call.params.set(FAR, 0, 20, false);
    assert!(!call.idle(4_000_000));
    assert!(!call.idle(5_000_000));
    assert!(call.idle(5_000_001));
    assert_eq!(call.pool.poll_event(), Some(MediaEvent::Stalled));
}

#[test]
fn tick_times_are_measured_into_a_histogram() {
    let mut call = Call::new(Echo::default(), 0, 20, 100_000);
    // Each reading of the clock moves it 7 microseconds: a tick takes 7.
    call.clock.step = 7;
    for n in 0..100u32 {
        call.datagram(u64::from(n) * 20_000, FAR, &far_packet(n, 0, 2));
    }
    let stats = call.ticker.stats();
    assert_eq!(
        (stats.tick_p50_us, stats.tick_p99_us, stats.tick_max_us),
        (7, 7, 7)
    );
    assert_eq!(stats.overruns, 0);

    // Two ticks in two hundred take longer than a packet time: the 99th
    // percentile stays, the maximum and the overruns move.
    call.clock.step = 25_000;
    call.datagram(2_000_000, FAR, &far_packet(100, 0, 2));
    call.clock.step = 7;
    let stats = call.ticker.stats();
    assert_eq!(stats.tick_p50_us, 7);
    assert_eq!(stats.tick_p99_us, 7);
    assert_eq!(stats.tick_max_us, 25_000);
    assert_eq!(stats.overruns, 2);

    // Published for the pool about twice a second.
    let published = call.pool.stats().expect("published");
    assert!(published.rx_packets >= 75 && published.rx_packets <= 101);
    assert_eq!(published.remote, Some(FAR));
    assert_eq!(published.tick_p50_us, 7);
}

#[test]
fn the_histogram_buckets() {
    for us in 0..16 {
        assert_eq!(histogram_bucket(us), us as usize);
    }
    // Sixteen buckets to each power of two, in order, never out of range.
    let mut last = 0;
    for us in (0..3_000_000u32).step_by(37) {
        let bucket = histogram_bucket(us);
        assert!(bucket >= last, "{us}");
        assert!(bucket < 16 * 18);
        last = bucket;
    }
    assert_eq!(histogram_bucket(16), 16);
    assert_eq!(histogram_bucket(17), 17);
    assert_eq!(histogram_bucket(32), 32);
    assert_eq!(histogram_bucket(33), 32);
    assert_eq!(histogram_bucket(34), 33);
    assert_eq!(histogram_bucket(u32::MAX), 16 * 18 - 1);
}

#[test]
fn data_and_events_cross_the_seam() {
    let mut call = Call::new(NullEngine::new(NullConfig::default()), 0, 20, 10_000);
    call.engine.originate();
    // The engine connected when it was started; the loop forwards it and
    // asks for the pool to be woken.
    assert!(call.idle(1000));
    assert_eq!(
        call.pool.poll_event(),
        Some(MediaEvent::Modem(ModemEvent::Connect(
            ConnectInfo::unknown()
        )))
    );
    assert!(!call.idle(2000), "nothing new to report");

    // Data from the host goes out in the next packet.
    assert_eq!(call.pool.write(b"hello"), 5);
    call.idle(3000);
    call.datagram(4000, FAR, &far_packet_at(0, 0, 0, &[0x7E; 160]));
    let sent = call.sent();
    assert_eq!(sent[0].1[..5], *b"hello");
    assert_eq!(sent[0].1[5..], [0x7E; 155]);

    // Data from the far end comes back for the host.
    let mut payload = [0x7E; 160];
    payload[..6].copy_from_slice(b"world!");
    call.datagram(24_000, FAR, &far_packet_at(1, 160, 0, &payload));
    let mut buffer = [0u8; 32];
    let count = call.pool.read(&mut buffer);
    assert_eq!(buffer[..count], *b"world!");
}

// ---------------------------------------------------------------------
// The invariant of the receive session, through the real loop.

/// A far end's stream of at most 2.7 seconds: packets of one to three
/// frames, numbered from anywhere.
#[derive(Clone, Debug)]
struct Stream {
    packets: Vec<(Vec<u8>, u32, u32)>,
}

fn stream() -> impl Strategy<Value = Stream> {
    (
        any::<u16>(),
        any::<u32>(),
        proptest::collection::vec(1u32..=3, 1..90),
    )
        .prop_map(|(sequence, timestamp, sizes)| {
            let mut clock = 0u32;
            let packets = sizes
                .iter()
                .enumerate()
                .map(|(n, frames)| {
                    let header = RtpHeader {
                        marker: n == 0,
                        payload_type: 8,
                        sequence: sequence.wrapping_add(n as u16),
                        timestamp: timestamp.wrapping_add(80 * clock),
                        ssrc: 77,
                    };
                    let payload = vec![n as u8; 80 * *frames as usize];
                    let mut datagram = vec![0u8; HEADER_LEN + payload.len()];
                    header.write_packet(&payload, &mut datagram).unwrap();
                    let packet = (datagram, clock, *frames);
                    clock += frames;
                    packet
                })
                .collect();
            Stream { packets }
        })
}

fn arrivals() -> impl Strategy<Value = (Stream, Vec<usize>)> {
    stream().prop_flat_map(|stream| {
        let n = stream.packets.len();
        (Just(stream), proptest::collection::vec(0..n, 1..2 * n + 1))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Under any loss, duplication and reordering the engine is ticked
    /// exactly as many frames as the far end's sample clock produced
    /// between the first and the last packet delivered, and as many
    /// frames are sent as were ticked.
    #[test]
    fn the_loop_ticks_exactly_the_frames_the_far_end_produced(
        (stream, order) in arrivals(),
        ptime in prop::sample::select(vec![10u32, 20, 30]),
    ) {
        let mut call = Call::new(Echo::default(), 8, ptime, 100_000);
        let mut first = None;
        let mut last = 0;
        for (step, index) in order.iter().enumerate() {
            let before = call.engine.frames;
            // The turns come faster than the packet time, so the local
            // timer never fires before the first packet.
            call.datagram(step as u64 * 1000, FAR, &stream.packets[*index].0);
            if call.engine.frames != before {
                first.get_or_insert(*index);
                last = *index;
            }
        }
        let first = first.expect("the first arrival is delivered");
        let (_, last_clock, last_frames) = stream.packets[last];
        let produced = u64::from(last_clock + last_frames - stream.packets[first].1);
        prop_assert_eq!(call.engine.frames, produced);

        let stats = call.ticker.stats();
        prop_assert_eq!(stats.free_run, 0);
        prop_assert_eq!(stats.resync, 0);
        // Every frame ticked is sent, a packet's worth at a time; what is
        // left is less than a packet.
        let per_packet = u64::from(ptime / 10);
        prop_assert_eq!(stats.tx_packets, produced / per_packet);
        prop_assert_eq!(stats.tx_octets, 80 * per_packet * (produced / per_packet));
        prop_assert_eq!(call.wire.sent.len() as u64, produced / per_packet);
        prop_assert_eq!(
            stats.rx_packets + stats.late + stats.duplicate,
            order.len() as u64
        );
    }
}
