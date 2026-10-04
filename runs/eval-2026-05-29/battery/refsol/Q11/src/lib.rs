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

    /// Add one to the counter. The read-modify-write happens under a single
    /// lock acquisition, so concurrent increments can't be lost.
    pub fn increment(&self) {
        let mut guard = self.value.lock().unwrap();
        *guard += 1;
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
