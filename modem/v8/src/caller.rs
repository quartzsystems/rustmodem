// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The calling modem's side of V.8 (V.8 clause 8.1).

use crate::codec::{CJ_BITS, Menu, Recent, SequenceDecoder, SequenceEncoder, SignalKind, cj_bits};
use crate::{
    Capabilities, EXCHANGE_TIMEOUT_MS, Event, GAP_MS, Outcome, Sending, TE_MS, samples_of_ms,
};

#[derive(Clone, Copy, Debug)]
enum State {
    /// The silence Te after ANSam was detected (clause 8.1.1).
    Te,
    /// Sending CM and listening for JM (clause 8.1.2).
    CallMenu,
    /// Two JM alike have come: CM is finishing the octet it is in.
    Finishing(Outcome),
    /// Sending CJ: the number of its bits that have been sent.
    Terminator(Outcome, u8),
    /// The silence before the modulation's own signal.
    Gap(Outcome),
    /// Over.
    Done,
}

/// The calling modem's side of the exchange.
///
/// It begins where the calling modem has detected ANSam: a calling modem
/// sends CM only if it has (V.8 clause 7.2), and one that hears plain
/// answer tone goes on without V.8 (clause 8.1.1). No call signal is
/// sent before: CI is optional, and silence is allowed.
///
/// Give it every bit the V.21 channel No. 2 receiver recovers with
/// [`Caller::receive_bit`], tell it of every sample that passes with
/// [`Caller::advance`], send what [`Caller::sending`] says, and take what
/// happened from [`Caller::poll`].
#[derive(Clone, Debug)]
pub struct Caller {
    state: State,
    elapsed: u32,
    decoder: SequenceDecoder,
    recent: Recent,
    encoder: SequenceEncoder,
    // Samples between this side's ceasing to send and the line's going
    // quiet: the delay of the modulator's transmit filter.
    transmit_delay: u32,
    events: [Option<Event>; 4],
}

impl Caller {
    /// A calling modem that can do this, at the moment it has detected
    /// ANSam.
    pub fn new(capabilities: Capabilities) -> Caller {
        Caller {
            state: State::Te,
            elapsed: 0,
            decoder: SequenceDecoder::new(),
            recent: Recent::default(),
            encoder: SequenceEncoder::menu(Menu::from_capabilities(&capabilities, 0)),
            transmit_delay: 0,
            events: [None; 4],
        }
    }

    /// The same, for a modulator whose signal goes on for this many
    /// samples after it has been given its last bit. The 75 ms of silence
    /// after CJ (V.8 clause 8.1.2) are counted from where the line goes
    /// quiet.
    pub fn with_transmit_delay(mut self, samples: u32) -> Caller {
        self.transmit_delay = samples;
        self
    }

    /// What is to be sent now.
    pub fn sending(&self) -> Sending {
        match self.state {
            State::CallMenu | State::Finishing(_) | State::Terminator(..) => Sending::Bits,
            State::Te | State::Gap(_) | State::Done => Sending::Silence,
        }
    }

    /// The next bit to send, while [`Caller::sending`] is
    /// [`Sending::Bits`]: CM and then CJ, on V.21 channel No. 1. At any
    /// other time it is a ONE.
    ///
    /// The caller is still sending while the last bit of CJ goes out. It
    /// learns that the bit has ended by being asked for the next, and
    /// from then it is silent: of the ONE it gives to that last asking,
    /// nothing is to be sent.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        if let State::Finishing(outcome) = self.state
            && self.encoder.at_unit_boundary()
        {
            // The octet CM was in, with its start and stop bits, is
            // complete: CJ follows (clause 8.1.2).
            self.state = State::Terminator(outcome, 0);
        }
        match self.state {
            State::CallMenu | State::Finishing(_) => self.encoder.next_bit(),
            State::Terminator(outcome, sent) => {
                if sent as usize == CJ_BITS {
                    // The last bit of CJ has been sent to its end, which
                    // is what being asked for another means. After CJ, no
                    // signal for 75 ms (clause 8.1.2).
                    self.enter(State::Gap(outcome));
                    return true;
                }
                self.state = State::Terminator(outcome, sent + 1);
                cj_bits(sent as usize)
            }
            _ => true,
        }
    }

    /// Takes a bit received on V.21 channel No. 2.
    #[inline]
    pub fn receive_bit(&mut self, bit: bool) {
        let decoded = self.decoder.push(bit);
        if !matches!(self.state, State::CallMenu) {
            return;
        }
        let Some(signal) = decoded.signal else {
            return;
        };
        if signal.kind != SignalKind::Menu {
            return;
        }
        // CJ is sent after a minimum of two identical JM sequences have
        // been received (clause 8.1.2).
        if !self.recent.seen(signal.menu) {
            return;
        }
        let Some((joint, _)) = signal.menu.capabilities() else {
            return;
        };
        // Of the modes in JM, the one with the lowest item number is
        // used (clause 7.4). If JM shows zeros for them all, the calling
        // modem may disconnect after CJ (clause 8.1.2).
        let outcome = match joint.modulations.first() {
            Some(modulation) => Outcome::Agreed {
                modulation,
                call_function: joint.call_function,
                lapm: joint.lapm,
            },
            None => Outcome::NothingInCommon,
        };
        self.push(Event::JointMenu(joint));
        self.state = State::Finishing(outcome);
    }

    /// Tells of samples that have passed.
    pub fn advance(&mut self, samples: u32) {
        self.elapsed = self.elapsed.saturating_add(samples);
        match self.state {
            State::Te if self.elapsed >= samples_of_ms(TE_MS) => self.enter(State::CallMenu),
            // No JM. V.8 does not say how long to go on; see
            // EXCHANGE_TIMEOUT_MS.
            State::CallMenu if self.elapsed >= samples_of_ms(EXCHANGE_TIMEOUT_MS) => {
                self.push(Event::Done(Outcome::NoResponse));
                self.enter(State::Done);
            }
            State::Gap(outcome) if self.elapsed >= samples_of_ms(GAP_MS) + self.transmit_delay => {
                self.push(Event::Done(outcome));
                self.enter(State::Done);
            }
            _ => {}
        }
    }

    /// Whether the exchange is over.
    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Done)
    }

    /// The next thing that happened, if anything has.
    pub fn poll(&mut self) -> Option<Event> {
        let event = self.events[0].take()?;
        self.events.rotate_left(1);
        Some(event)
    }

    fn push(&mut self, event: Event) {
        if let Some(slot) = self.events.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(event);
        }
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.elapsed = 0;
    }
}
