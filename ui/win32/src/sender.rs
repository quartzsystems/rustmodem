// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Waking the UI thread from another thread.
//!
//! A [`UiSender`] puts a value on a queue and posts a private registered
//! message to a window, which arrives as [`crate::Event::Wake`]. The
//! window's handler then takes everything off the queue with its
//! [`UiReceiver`]. The value travels in the queue, not in the message, so
//! nothing is lost or leaked if the window has gone or a message is
//! dropped: the next wake finds whatever is still queued.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::window::{Hwnd, wake_message};

struct Queue<T> {
    items: Mutex<VecDeque<T>>,
}

impl<T> Queue<T> {
    fn lock(&self) -> MutexGuard<'_, VecDeque<T>> {
        // A thread that panicked while pushing or popping left the queue
        // whole; what is in it is still good.
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The sending end: any thread may hold one.
pub struct UiSender<T> {
    queue: Arc<Queue<T>>,
    /// The window to wake, as a number: a window handle is a number that
    /// any thread may post a message to.
    window: isize,
}

impl<T> Clone for UiSender<T> {
    fn clone(&self) -> Self {
        UiSender {
            queue: Arc::clone(&self.queue),
            window: self.window,
        }
    }
}

impl<T: Send> UiSender<T> {
    /// Queues a value and wakes the window. Returns `false` if the window
    /// has gone; the value is queued all the same.
    pub fn send(&self, value: T) -> bool {
        self.queue.lock().push_back(value);
        // SAFETY: posting a message is sound from any thread and for any
        // handle value; for one that is not a window it fails.
        unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut _)),
                wake_message(),
                WPARAM(0),
                LPARAM(0),
            )
        }
        .is_ok()
    }
}

/// The receiving end: it stays on the UI thread.
pub struct UiReceiver<T> {
    queue: Arc<Queue<T>>,
    /// A window handle is what makes this not `Send`.
    _window: Hwnd,
}

impl<T> UiReceiver<T> {
    /// Takes everything that is queued, oldest first.
    pub fn drain(&self) -> Vec<T> {
        self.queue.lock().drain(..).collect()
    }
}

/// Makes a channel that wakes `window`.
pub fn channel<T: Send>(window: Hwnd) -> (UiSender<T>, UiReceiver<T>) {
    let queue = Arc::new(Queue {
        items: Mutex::new(VecDeque::new()),
    });
    (
        UiSender {
            queue: Arc::clone(&queue),
            window: window.raw(),
        },
        UiReceiver {
            queue,
            _window: window,
        },
    )
}
