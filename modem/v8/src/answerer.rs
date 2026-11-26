// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The answering modem's side of V.8 (V.8 clause 8.2).

use crate::codec::{Menu, Recent, SequenceDecoder, SequenceEncoder, SignalKind};
use crate::{
    ANSAM_MS, CM_ABSENT_MS, Capabilities, EXCHANGE_TIMEOUT_MS, Event, GAP_MS, Modulations, Outcome,
    Sending, samples_of_ms,
};

#[derive(Clone, Copy, Debug)]
enum State {
    /// Sending ANSam and listening for CM (clause 8.2.2).
    AnswerTone,
    /// Sending JM and listening for CJ (clause 8.2.3).
    JointMenu,
    /// The silence before the modulation's own signal.
    Gap(Outcome),
    /// Over.
    Done,
}

/// The answering modem's side of the exchange.
///
/// It begins where the answering modem begins ANSam: the silence that
/// comes before, at least 0.2 s (V.8 clause 8.2), is the caller's to
/// keep. Give it every bit the V.21 channel No. 1 receiver recovers with
/// [`Answerer::receive_bit`], tell it of every sample that passes with
/// [`Answerer::advance`], send what [`Answerer::sending`] says, and take
/// what happened from [`Answerer::poll`].
#[derive(Clone, Debug)]
pub struct Answerer {
    capabilities: Capabilities,
    state: State,
    // Samples in the state.
    elapsed: u32,
    ansam_samples: u32,
    decoder: SequenceDecoder,
    // The last CM sequences, to compare the next with.
    recent: Recent,
    // The joint menu that is being sent.
    joint: Option<Menu>,
    // Samples since the last CM sequence was received.
    since_call_menu: u32,
    encoder: SequenceEncoder,
    // Samples between this side's ceasing to send and the line's going
    // quiet: the delay of the modulator's transmit filter.
    transmit_delay: u32,
    events: [Option<Event>; 4],
}

impl Answerer {
    /// An answering modem that can do this, at the moment it begins
    /// ANSam.
    pub fn new(capabilities: Capabilities) -> Answerer {
        Answerer {
            capabilities,
            state: State::AnswerTone,
            elapsed: 0,
            ansam_samples: samples_of_ms(ANSAM_MS),
            decoder: SequenceDecoder::new(),
            recent: Recent::default(),
            joint: None,
            since_call_menu: 0,
            encoder: SequenceEncoder::menu(Menu::from_octets(&[])),
            transmit_delay: 0,
            events: [None; 4],
        }
    }

    /// The same, for a modulator whose signal goes on for this many
    /// samples after it has been given its last bit. The 75 ms of silence
    /// after JM (V.8 clause 8.2.3) are counted from where the line goes
    /// quiet.
    pub fn with_transmit_delay(mut self, samples: u32) -> Answerer {
        self.transmit_delay = samples;
        self
    }

    /// The same, with ANSam lasting this long if no CM comes, and not the
    /// 5 s of V.8 clause 8.2.2: within 1 s of it is in tolerance.
    pub fn with_ansam_ms(capabilities: Capabilities, ansam_ms: u32) -> Answerer {
        let mut answerer = Answerer::new(capabilities);
        answerer.ansam_samples = samples_of_ms(ansam_ms);
        answerer
    }

    /// What is to be sent now.
    pub fn sending(&self) -> Sending {
        match self.state {
            State::AnswerTone => Sending::AnswerTone,
            State::JointMenu => Sending::Bits,
            State::Gap(_) | State::Done => Sending::Silence,
        }
    }

    /// The next bit to send, while [`Answerer::sending`] is
    /// [`Sending::Bits`]: JM, on V.21 channel No. 2. At any other time it
    /// is a ONE.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        match self.state {
            State::JointMenu => self.encoder.next_bit(),
            _ => true,
        }
    }

    /// Takes a bit received on V.21 channel No. 1.
    #[inline]
    pub fn receive_bit(&mut self, bit: bool) {
        let decoded = self.decoder.push(bit);
        match self.state {
            State::AnswerTone => {
                let Some(signal) = decoded.signal else {
                    return;
                };
                if signal.kind != SignalKind::Menu {
                    // CI: a modem that receives it is not to malfunction
                    // (V.8 clause 7.1). ANSam is being sent already.
                    return;
                }
                // JM is sent once a minimum of two identical CM sequences
                // have been received (V.8 clauses 7.4 and 8.2.2).
                if !self.recent.seen(signal.menu) {
                    return;
                }
                let Some((theirs, octets)) = signal.menu.capabilities() else {
                    // Two alike, and not a call menu: go on listening.
                    return;
                };
                let joint = self.joint_menu(&theirs);
                let joint = Menu::from_capabilities(&joint, octets);
                self.encoder = SequenceEncoder::menu(joint);
                self.joint = Some(joint);
                self.push(Event::CallMenu(theirs));
                self.enter(State::JointMenu);
                self.since_call_menu = 0;
            }
            State::JointMenu => {
                if decoded
                    .signal
                    .is_some_and(|signal| signal.kind == SignalKind::Menu)
                {
                    // The calling modem is still sending CM.
                    self.since_call_menu = 0;
                }
                // JM goes on until all three octets of CJ have been
                // received, and stops there, wherever in a sequence it
                // is (clause 8.2.3).
                if decoded.terminator {
                    let outcome = self.outcome();
                    self.push(Event::Terminator);
                    self.enter(State::Gap(outcome));
                }
            }
            State::Gap(_) | State::Done => {}
        }
    }

    /// The joint menu for a call menu (V.8 clauses 7.4 and 8.2.3).
    fn joint_menu(&self, theirs: &Capabilities) -> Capabilities {
        let ours = &self.capabilities;
        if theirs.call_function != ours.call_function {
            // The call function is not available: JM may indicate one
            // that is, and shows zeros for all modulation modes.
            return Capabilities {
                call_function: ours.call_function,
                modulations: Modulations::NONE,
                lapm: false,
                access: None,
                pcm: None,
            };
        }
        Capabilities {
            call_function: theirs.call_function,
            // All the modes that are both indicated in CM and available
            // here; none, shown as zeros, if there are none.
            modulations: theirs.modulations.common(ours.modulations),
            // The protocol octet is in JM if CM asks for LAPM and this
            // modem wishes to use it (clause 6.4).
            lapm: theirs.lapm && ours.lapm,
            // The PSTN access category is in JM if this modem wishes to
            // say where it is, or if CM has it with b5 set; b5 is as in
            // CM and b6 and b7 are this modem's own (clause 7.4).
            access: match (ours.access, theirs.access) {
                (Some(ours), theirs) => Some(crate::PstnAccess {
                    call_cellular: theirs.is_some_and(|theirs| theirs.call_cellular),
                    ..ours
                }),
                (None, Some(theirs)) if theirs.call_cellular => Some(crate::PstnAccess {
                    call_cellular: true,
                    answer_cellular: false,
                    digital: false,
                }),
                _ => None,
            },
            // PCM modem availability is in JM only if it is in CM and
            // this modem has it to offer (clause 7.4).
            pcm: match (ours.pcm, theirs.pcm) {
                (Some(ours), Some(_)) => Some(ours),
                _ => None,
            },
        }
    }

    /// How the exchange comes out, from the joint menu that was sent.
    fn outcome(&self) -> Outcome {
        let joint = self
            .joint
            .and_then(|menu| menu.capabilities())
            .map(|(capabilities, _)| capabilities);
        match joint {
            Some(joint) => match joint.modulations.first() {
                Some(modulation) => Outcome::Agreed {
                    modulation,
                    call_function: joint.call_function,
                    lapm: joint.lapm,
                },
                None => Outcome::NothingInCommon,
            },
            None => Outcome::Incomplete,
        }
    }

    /// Tells of samples that have passed.
    pub fn advance(&mut self, samples: u32) {
        self.elapsed = self.elapsed.saturating_add(samples);
        match self.state {
            // ANSam is sent for 5 s unless CM ends it, and then there is
            // 75 ms of silence before the modem goes on as it would have
            // without V.8 (clause 8.2.2).
            State::AnswerTone if self.elapsed >= self.ansam_samples => {
                self.enter(State::Gap(Outcome::NoResponse));
            }
            State::JointMenu => {
                self.since_call_menu = self.since_call_menu.saturating_add(samples);
                if self.since_call_menu >= samples_of_ms(CM_ABSENT_MS) {
                    // CM has stopped and CJ was not received: the calling
                    // modem has sent it and gone on. Clause 8.2.3 allows
                    // JM to be ended on the absence of CM, and the
                    // outcome is what the joint menu says.
                    let outcome = self.outcome();
                    self.enter(State::Gap(outcome));
                } else if self.elapsed >= samples_of_ms(EXCHANGE_TIMEOUT_MS) {
                    // CM goes on and on: the calling modem does not hear
                    // JM.
                    self.enter(State::Gap(Outcome::Incomplete));
                }
            }
            State::Gap(outcome) => {
                // After JM the modulator's signal goes on for its delay;
                // after ANSam, which is not the modulator's, it does not.
                let delay = match outcome {
                    Outcome::NoResponse => 0,
                    _ => self.transmit_delay,
                };
                if self.elapsed < samples_of_ms(GAP_MS) + delay {
                    return;
                }
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
