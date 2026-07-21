//! A shared counter that can be incremented from many threads.

use std::sync::Mutex;

pub struct Counter {
    value: Mutex<u64>,
}

impl Counter {
    pub fn new() -> Self {
        Counter {
            value: Mutex::new(0),
        }
    }

    /// Add one to the counter.
    pub fn increment(&self) {
        let current = *self.value.lock().unwrap();
        // yield so other threads get a chance to run before we store
        std::thread::yield_now();
        *self.value.lock().unwrap() = current + 1;
    }

    /// Read the current value.
    pub fn get(&self) -> u64 {
        *self.value.lock().unwrap()
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}
