// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A whole call under an allocator that counts: nothing a media thread
//! does to an engine in a tick allocates.

use engine::{
    CallSettings, EngineState, ErrorControl, Law, Modem, ModemConfig, ModemEngine, ModemEvent,
    Modulation,
};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

/// What came of a call.
#[derive(Default)]
struct Told {
    connected: [Option<String>; 2],
    lost: [bool; 2],
    received: [Vec<u8>; 2],
}

/// Runs a call between two engines for some ticks, counting every
/// allocation made inside what the media thread does in a tick: the
/// host's data in, the tick itself, the data and the events out, and the
/// snapshot. Returns the count.
fn run(
    engines: &mut [Box<dyn ModemEngine>; 2],
    frames: &mut [[u8; 80]; 2],
    ticks: usize,
    data: &mut [&[u8]; 2],
    told: &mut Told,
) -> u64 {
    let mut allocations = 0;
    // Room for what is received is made before the counting.
    for received in &mut told.received {
        received.reserve(ticks * 4 + 64);
    }
    for _ in 0..ticks {
        let rx = [frames[1], frames[0]];
        for n in 0..2 {
            let engine = &mut engines[n];
            let mut buffer = [0u8; 64];
            let mut event = None;
            let (count, (taken, read)) = alloccount::count(|| {
                let taken = engine.dte_write(&data[n][..data[n].len().min(8)]);
                engine.process(&rx[n], &mut frames[n]);
                let read = engine.dte_read(&mut buffer);
                event = engine.poll_event();
                let diagnostics = engine.diagnostics();
                std::hint::black_box(&diagnostics);
                (taken, read)
            });
            allocations += count;
            data[n] = &data[n][taken..];
            told.received[n].extend_from_slice(&buffer[..read]);
            match event {
                Some(ModemEvent::Connect(info)) => told.connected[n] = info.carrier,
                Some(ModemEvent::CarrierLost) => told.lost[n] = true,
                _ => {}
            }
        }
    }
    allocations
}

fn call(originator: ModemConfig, answerer: ModemConfig, law: Law, carrier: &str) {
    call_with(originator, answerer, law, carrier, 600);
}

/// A call that carries `len` octets each way: text one way, which a
/// compressor compresses, and octets it cannot the other.
fn call_with(originator: ModemConfig, answerer: ModemConfig, law: Law, carrier: &str, len: u32) {
    let mut engines: [Box<dyn ModemEngine>; 2] = [
        Box::new(Modem::new(originator)),
        Box::new(Modem::new(answerer)),
    ];
    for engine in &mut engines {
        engine.set_law(law);
        engine.set_call(&CallSettings::default());
    }
    // Starting a call may allocate: it is done by the pool, before the
    // media thread has the engine.
    engines[0].originate();
    engines[1].answer();

    let mut frames = [[law.silence(); 80]; 2];
    let mut told = Told::default();
    let from_a: Vec<u8> = (0..len)
        .map(|n| b"the modem answers a call and sends its tone "[(n % 44) as usize])
        .collect();
    let from_b: Vec<u8> = (0..len)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();

    // Twenty seconds for the answer sequence, V.8 or the fallback, the
    // carriers or the handshake, and the data link.
    let mut nothing: [&[u8]; 2] = [&[], &[]];
    let mut allocations = run(&mut engines, &mut frames, 2000, &mut nothing, &mut told);
    assert_eq!(told.connected[0].as_deref(), Some(carrier));
    assert_eq!(told.connected[1].as_deref(), Some(carrier));

    // Twenty-five seconds of data both ways.
    let mut data: [&[u8]; 2] = [&from_a, &from_b];
    allocations += run(&mut engines, &mut frames, 2500, &mut data, &mut told);
    assert!(told.received[0] == from_b && told.received[1] == from_a);

    // One end stops, and the other loses the carrier.
    engines[0].hangup();
    allocations += run(&mut engines, &mut frames, 300, &mut nothing, &mut told);
    assert!(told.lost[1]);
    assert_eq!(engines[1].diagnostics().unwrap().state, EngineState::Ended);

    assert_eq!(allocations, 0, "{carrier} in {law:?}");
}

#[test]
fn a_whole_call_allocates_nothing_in_a_tick() {
    let modem = |list: &[Modulation], v8: bool| ModemConfig {
        modulations: engine::modulations(list),
        v8,
        ..ModemConfig::default()
    };
    let both = &[Modulation::V21, Modulation::Bell103];
    // By V.8, by the fallback to V.21, and by the fallback to Bell 103.
    call(modem(both, true), modem(both, true), Law::Ulaw, "V.21");
    call(modem(both, true), modem(both, true), Law::Alaw, "V.21");
    call(
        modem(&[Modulation::V21], false),
        modem(both, true),
        Law::Ulaw,
        "V.21",
    );
    call(
        modem(&[Modulation::Bell103], false),
        modem(both, true),
        Law::Ulaw,
        "BELL103",
    );

    // At 2400 bit/s by V.8, with LAPM and V.42bis; at 1200 bit/s by the
    // fallback, with LAPM and with start-stop characters; and Bell 212A.
    let all = ModemConfig::default();
    call_with(all, all, Law::Ulaw, "V.22bis", 5000);
    call_with(all, all, Law::Alaw, "V.22bis", 5000);
    call_with(
        modem(&[Modulation::V22], false),
        all,
        Law::Ulaw,
        "V.22",
        2500,
    );
    let plain = ModemConfig {
        error_control: ErrorControl::Off,
        ..modem(&[Modulation::V22], false)
    };
    call_with(plain, all, Law::Ulaw, "V.22", 2500);
    let bell = modem(&[Modulation::Bell212A], false);
    call_with(bell, bell, Law::Ulaw, "BELL212A", 2500);

    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
