// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The media of calls over RTP: one thread a call, ticked by the far end.
//!
//! The pool hands a connected call's engine and its [`RtpEndpoint`] to the
//! [`ThreadedRunner`], which carries the call on a media thread until it
//! ends. The thread runs the tick loop ([`Ticker`]):
//!
//! ```text
//!   for each RTP datagram that arrives:
//!       parse it and give it to the RtpReceiver
//!       for each frame the receiver says to deliver:
//!           engine.process(rx, tx)          one frame in, one frame out
//!           when tx holds a packet's worth: send one RTP packet
//!       move data between the rings and the engine; forward events
//! ```
//!
//! **One received frame produces exactly one transmitted frame.** Once the
//! far end's packets arrive, nothing is paced by a local timer: the far
//! end's sample clock is the only clock, so no sample is ever slipped
//! against it. Before the first packet the loop ticks from a local timer
//! at the packet time, so that an engine can start (a modem answers by
//! sending tone); those frames are counted as `free_run`.
//!
//! On the tick path there is no heap allocation, no lock, no logging and
//! no system call but the socket's receive and send. Everything a call
//! needs is made before the loop starts. The pool is woken, which is a
//! system call, only when the engine reports an event or the call stalls,
//! not for data: the pool looks for data every turn of its own clock.
//!
//! `unsafe` is used in [`os`] only: for the two things `std` cannot do,
//! the size of a socket's buffers and the thread's scheduling class.

mod endpoint;
mod os;
mod threaded;
mod ticker;

pub use endpoint::{RtpEndpoint, RtpParams, RtpPorts};
pub use threaded::{ThreadedRunner, Waker};
pub use ticker::{Clock, MAX_DATAGRAM, Received, Ticker, Transport, histogram_bucket};
