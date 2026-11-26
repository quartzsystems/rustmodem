// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A counting allocator, for tests that assert a path allocates nothing.
//!
//! A modem runs on a real-time media thread, where an allocation is a lock
//! and a wait. The tests that hold the sample path to allocating nothing
//! run it under this allocator and look at the count.
//!
//! A test program installs it for itself:
//!
//! ```
//! #[global_allocator]
//! static ALLOCATOR: alloccount::Counting = alloccount::Counting;
//!
//! let (allocations, sum) = alloccount::count(|| 2 + 2);
//! assert_eq!((allocations, sum), (0, 4));
//! let (allocations, boxed) = alloccount::count(|| Box::new(7));
//! assert_eq!((allocations, *boxed), (1, 7));
//! ```
//!
//! Only the thread that calls [`count`] is counted, and only inside the
//! call, so the test harness and other tests may allocate as they please.
//!
//! This crate is for tests and is not shipped. It is the one place the
//! `unsafe` that an allocator cannot be written without is kept, so that
//! no modem crate and no test of one has any.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The system's allocator, counting the allocations of a thread that is
/// inside [`count`].
pub struct Counting;

thread_local! {
    // Whether this thread is counting, and what it has counted. Neither
    // has a destructor and both are made without allocating, so the
    // allocator may use them from inside an allocation.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    // A thread that is being torn down may have lost its thread-locals;
    // it is not inside `count`.
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
    }
}

// SAFETY: every request is passed to the system's allocator unchanged and
// its answer is returned unchanged, so this allocator meets the contract
// of `GlobalAlloc` exactly as `System` does. Counting touches only
// thread-locals of `Cell`s, which neither allocate nor unwind.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract,
        // which is `System.alloc`'s.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: as for `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator, which is to say from
        // `System`, with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: `ptr` came from `System` with this layout, and the
        // caller upholds the rest of `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Runs `f` and gives how many times it allocated, with what it returned.
/// A reallocation counts as an allocation; a deallocation does not count.
///
/// The count is zero unless the program has installed [`Counting`] as its
/// global allocator, so a test should also show that something that does
/// allocate is counted.
pub fn count<T>(f: impl FnOnce() -> T) -> (u64, T) {
    let before = ALLOCATIONS.with(Cell::get);
    let was = COUNTING.with(|counting| counting.replace(true));
    let value = f();
    COUNTING.with(|counting| counting.set(was));
    let after = ALLOCATIONS.with(Cell::get);
    (after - before, value)
}
