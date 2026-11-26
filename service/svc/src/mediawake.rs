// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! How a media thread tells the pool's task that a call has an event.
//!
//! The events themselves, a call connected, retrained, lost its carrier
//! or stalled, are in the call's own ring, where the pool reads them. What
//! crosses here is the notice that there is one: a message on a bounded
//! `tokio::sync::mpsc` channel, sent with `try_send`, which the pool's
//! task waits on. The media crate has no Tokio in it; it is given a
//! [`media::Waker`], and this is the one the service gives it.
//!
//! What `try_send` does, from Tokio's source (1.54): it takes a permit
//! with a compare-and-swap and returns at once if there is none, so it
//! never waits for room. Storing the message allocates a block for every
//! 32 messages sent, and waking the pool's task from a thread that is not
//! the runtime's takes the scheduler's lock for a moment. None of that is
//! on the path of a tick: a media thread calls its waker only when its
//! engine has reported something, which is a few times in a call, and
//! never for data.
//!
//! A channel that is full is counted, and the media thread, told that the
//! notice did not go, tries again at its next turn. It does not wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

/// How many notices may wait for the pool's task. One is enough to have
/// it look at every call, so this fills only if the task is not running
/// at all.
pub const NOTICES: usize = 256;

/// The pool's end: the notices, and how often there was no room for one.
pub struct MediaEvents {
    notices: mpsc::Receiver<()>,
    full: Arc<AtomicU64>,
}

/// The waker for the media threads and the pool's end of it.
pub fn channel() -> (media::Waker, MediaEvents) {
    let (sender, notices) = mpsc::channel(NOTICES);
    let full = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&full);
    let waker: media::Waker = Arc::new(move || match sender.try_send(()) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(())) => {
            counted.fetch_add(1, Ordering::Relaxed);
            false
        }
        // Nobody is left to tell: there is nothing to try again for.
        Err(mpsc::error::TrySendError::Closed(())) => true,
    });
    (waker, MediaEvents { notices, full })
}

impl MediaEvents {
    /// How many times a media thread found the channel full. The count
    /// can be read after the events have gone to the pool's task.
    pub fn full(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.full)
    }

    /// Waits for a notice. With no events, or once every media thread has
    /// gone, it waits for ever. The notices that have gathered are taken
    /// with the first: one turn of the pool looks at every call.
    pub async fn next(events: &mut Option<MediaEvents>) {
        let Some(waiting) = events else {
            return std::future::pending().await;
        };
        if waiting.notices.recv().await.is_none() {
            *events = None;
            return std::future::pending().await;
        }
        while waiting.notices.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_notice_wakes_the_task_and_the_rest_are_taken_with_it() {
        let (waker, events) = channel();
        let mut events = Some(events);
        // From a thread that is not the runtime's, as a media thread is.
        let waking = Arc::clone(&waker);
        std::thread::spawn(move || {
            assert!(waking());
            assert!(waking());
            assert!(waking());
        })
        .join()
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), MediaEvents::next(&mut events))
            .await
            .expect("the task is woken");
        // All three were taken: there is nothing to wake for now.
        let again = tokio::time::timeout(Duration::from_millis(50), MediaEvents::next(&mut events));
        assert!(again.await.is_err());
    }

    #[tokio::test]
    async fn a_full_channel_refuses_at_once_and_is_counted() {
        let (waker, events) = channel();
        let full = events.full();
        for _ in 0..NOTICES {
            assert!(waker());
        }
        let started = std::time::Instant::now();
        assert!(!waker(), "the notice did not go");
        assert!(!waker());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "and nothing waited"
        );
        assert_eq!(full.load(Ordering::Relaxed), 2);

        // Once the task has taken what was waiting there is room again.
        let mut events = Some(events);
        MediaEvents::next(&mut events).await;
        assert!(waker());
    }

    #[tokio::test]
    async fn with_nobody_to_tell_a_waker_does_not_ask_to_be_tried_again() {
        let (waker, events) = channel();
        drop(events);
        assert!(waker());
    }

    #[tokio::test]
    async fn with_no_media_threads_left_the_wait_never_ends() {
        let (waker, events) = channel();
        drop(waker);
        let mut events = Some(events);
        let waited =
            tokio::time::timeout(Duration::from_millis(50), MediaEvents::next(&mut events));
        assert!(waited.await.is_err());
        assert!(events.is_none());
    }
}
