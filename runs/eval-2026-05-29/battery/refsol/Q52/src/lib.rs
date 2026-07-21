//! A fixed-capacity ring buffer.

/// A ring buffer holding at most `capacity` items. Pushing into a full buffer
/// overwrites the oldest item.
pub struct RingBuffer<T> {
    buf: Vec<T>,
    capacity: usize,
    start: usize,
    len: usize,
}

impl<T: Clone> RingBuffer<T> {
    /// Create a ring buffer that holds at most `capacity` items.
    pub fn new(capacity: usize) -> Self {
        RingBuffer {
            buf: Vec::with_capacity(capacity),
            capacity,
            start: 0,
            len: 0,
        }
    }

    /// Append an item. If the buffer is full, the oldest item is overwritten.
    pub fn push(&mut self, item: T) {
        if self.capacity == 0 {
            return;
        }
        if self.len < self.capacity {
            self.buf.push(item);
            self.len += 1;
        } else {
            self.buf[self.start] = item;
            self.start = (self.start + 1) % self.capacity;
        }
    }

    /// Number of items currently stored (never exceeds the capacity).
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when the buffer holds no items.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The stored items in order from oldest to newest.
    pub fn to_vec(&self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            out.push(self.buf[(self.start + i) % self.capacity].clone());
        }
        out
    }
}
