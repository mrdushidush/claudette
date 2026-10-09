//! A fixed-capacity ring buffer.

use std::marker::PhantomData;

/// A ring buffer holding at most `capacity` items. Pushing into a full buffer
/// overwrites the oldest item.
pub struct RingBuffer<T> {
    _marker: PhantomData<T>,
}

impl<T: Clone> RingBuffer<T> {
    /// Create a ring buffer that holds at most `capacity` items.
    pub fn new(_capacity: usize) -> Self {
        unimplemented!()
    }

    /// Append an item. If the buffer is full, the oldest item is overwritten.
    pub fn push(&mut self, _item: T) {
        unimplemented!()
    }

    /// Number of items currently stored (never exceeds the capacity).
    pub fn len(&self) -> usize {
        unimplemented!()
    }

    /// True when the buffer holds no items.
    pub fn is_empty(&self) -> bool {
        unimplemented!()
    }

    /// The stored items in order from oldest to newest.
    pub fn to_vec(&self) -> Vec<T> {
        unimplemented!()
    }
}
