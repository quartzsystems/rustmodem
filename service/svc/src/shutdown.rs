// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The signal that stops the service.
//!
//! One `tokio::sync::watch` channel. Whatever asks the service to stop,
//! Ctrl+C, Ctrl+Break, the console closing or the service control manager,
//! sends on it through a [`Shutdown`]; every task holds a [`Stopping`] and
//! selects on it. Once sent it stays sent: a task that looks late still
//! sees it.

use std::sync::Arc;

use tokio::sync::watch;

/// Asks the service to stop. It may be used from any thread, one of the
/// runtime's or not.
#[derive(Clone)]
pub struct Shutdown(Arc<watch::Sender<bool>>);

/// Tells a task that the service is stopping.
#[derive(Clone)]
pub struct Stopping(watch::Receiver<bool>);

/// The signal, not yet sent, and the first of its receivers.
pub fn channel() -> (Shutdown, Stopping) {
    let (sender, receiver) = watch::channel(false);
    (Shutdown(Arc::new(sender)), Stopping(receiver))
}

impl Shutdown {
    /// Asks the service to stop. Asking twice is harmless.
    pub fn trigger(&self) {
        // `send` would fail with no receiver left, and the value would
        // then not change: a receiver made afterwards must still see it.
        self.0.send_replace(true);
    }

    /// Another receiver of the signal.
    pub fn subscribe(&self) -> Stopping {
        Stopping(self.0.subscribe())
    }
}

impl Stopping {
    /// Whether the service has been asked to stop.
    pub fn is_set(&self) -> bool {
        *self.0.borrow()
    }

    /// The signal as the control server takes it.
    pub fn receiver(&self) -> watch::Receiver<bool> {
        self.0.clone()
    }

    /// Waits until the service is asked to stop. Returns at once if it
    /// already has been, and also when nothing is left that could ask.
    pub async fn wait(&mut self) {
        let _ = self.0.wait_for(|stopping| *stopping).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_receiver_sees_the_signal_whenever_it_looks() {
        let (shutdown, mut first) = channel();
        let mut second = shutdown.subscribe();
        assert!(!first.is_set());

        let waiting = tokio::spawn(async move {
            second.wait().await;
            second.is_set()
        });
        // From a thread that is not the runtime's, as the console control
        // handler's is.
        let asking = shutdown.clone();
        std::thread::spawn(move || asking.trigger()).join().unwrap();
        assert!(waiting.await.unwrap());

        first.wait().await;
        assert!(first.is_set());
        // One made after the signal was sent sees it too.
        let mut late = shutdown.subscribe();
        late.wait().await;
        shutdown.trigger();
        assert!(late.is_set());
    }

    #[tokio::test]
    async fn a_wait_ends_when_nothing_can_ask_any_more() {
        let (shutdown, mut stopping) = channel();
        drop(shutdown);
        stopping.wait().await;
        assert!(!stopping.is_set());
    }
}
