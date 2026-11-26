// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The one runtime of the service.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::runtime::{Builder, Runtime};

/// How many threads run the tasks of the service. The work is the control
/// plane's: a pool that is served in microseconds, a few control clients
/// and one SIP socket. The audio of the calls is not on these threads.
pub const WORKERS: usize = 2;

/// What the runtime's threads are called, before their number.
pub const THREAD_NAME: &str = "rm-ctl";

/// Builds the runtime: [`WORKERS`] threads named `rm-ctl-N`, with timers
/// and I/O. The threads it makes for blocking work, such as looking up the
/// serial ports, take the numbers after the workers'.
pub fn build() -> io::Result<Runtime> {
    let next = AtomicUsize::new(0);
    Builder::new_multi_thread()
        .worker_threads(WORKERS)
        .thread_name_fn(move || format!("{THREAD_NAME}-{}", next.fetch_add(1, Ordering::Relaxed)))
        .enable_io()
        .enable_time()
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workers_are_named_and_tasks_run_on_them() {
        let runtime = build().unwrap();
        let name = runtime.block_on(async {
            tokio::spawn(async { std::thread::current().name().map(str::to_owned) })
                .await
                .unwrap()
        });
        let name = name.expect("a worker has a name");
        assert!(name.starts_with("rm-ctl-"), "{name}");
    }
}
