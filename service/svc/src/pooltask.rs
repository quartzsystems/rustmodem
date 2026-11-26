// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The task that owns the pool.
//!
//! The pool is synchronous and pure, and this task is the only code that
//! touches it. The task sleeps until the pool's next deadline and wakes
//! sooner for whatever arrives: a request from the control server, news
//! from a serial port's thread, or the signal to stop. After each it gives
//! the pool a turn of its loop.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ctlsrv::PoolRequests;
use pool::{Now, Pool, TICK_MS};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::shutdown::Stopping;

/// How long the task goes on after the lines are down, for the calls to
/// be cleared and the registrations given up on the network. It is well
/// inside the time the service control manager allows a stop.
pub const LINGER: Duration = Duration::from_secs(3);

/// Tells the time since the Unix epoch, in milliseconds, for the records
/// and log lines of the pool.
pub type UnixClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The clock of the machine.
pub fn system_unix_clock() -> UnixClock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    })
}

/// The time the pool is told.
#[derive(Clone)]
pub enum PoolClock {
    /// Time passes: the monotonic clock is the runtime's, from when the
    /// task started, and the wall clock is this.
    Running(UnixClock),
    /// The pool is served at this one time for ever: what is asked of it
    /// is done, and nothing moves on by itself. The demo service's frozen
    /// script uses it.
    Frozen(Now),
}

/// What the pool's task is made from.
pub struct PoolTask {
    /// The pool. Its lines have been applied.
    pub pool: Pool,
    /// What is asked of the pool.
    pub requests: PoolRequests,
    /// Notified when something outside the task has something for the
    /// pool: a serial port's thread, a media thread, the SIP socket.
    pub wake: Arc<Notify>,
    /// The time the pool is told.
    pub clock: PoolClock,
    /// How long the router is given, once the lines are down, to finish
    /// what it has in flight on the network.
    pub linger: Duration,
}

impl PoolTask {
    /// Runs the pool until the service stops or nothing can ask anything
    /// of it any more, then shuts it down: every call is hung up, every
    /// port is released and every registration is given up.
    pub async fn run(self, mut stopping: Stopping) {
        let PoolTask {
            mut pool,
            mut requests,
            wake,
            clock,
            linger,
        } = self;
        let started = Instant::now();
        let now = || match &clock {
            PoolClock::Running(unix) => Now {
                mono_ms: started.elapsed().as_millis() as u64,
                unix_ms: unix(),
            },
            PoolClock::Frozen(at) => *at,
        };
        // When the pool's deadline is, on the runtime's clock. A pool
        // whose time stands still has no deadline that comes: it is
        // served every tick, as a pool that is behind would be.
        let due = |pool: &Pool| match &clock {
            PoolClock::Running(_) => started + Duration::from_millis(pool.next_deadline()),
            PoolClock::Frozen(_) => Instant::now() + Duration::from_millis(TICK_MS),
        };

        pool.service(now());
        loop {
            let deadline = due(&pool);
            tokio::select! {
                request = requests.recv() => match request {
                    Some(request) => request.run(&mut pool),
                    None => break,
                },
                () = wake.notified() => {}
                () = tokio::time::sleep_until(deadline) => {}
                () = stopping.wait() => break,
            }
            // Whatever else was asked meanwhile is done in the same turn.
            while let Ok(request) = requests.try_recv() {
                request.run(&mut pool);
            }
            pool.service(now());
        }

        pool.shutdown(now());
        // The lines are down. What the router has sent to say so, a BYE
        // for each call and the end of each registration, is answered by
        // the far ends in their own time; it is given a little.
        let until = Instant::now() + linger;
        while pool.linger(now()) && Instant::now() < until {
            tokio::select! {
                () = wake.notified() => {}
                () = tokio::time::sleep(Duration::from_millis(TICK_MS)) => {}
            }
        }
    }
}
