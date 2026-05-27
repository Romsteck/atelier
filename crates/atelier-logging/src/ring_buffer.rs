use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::types::LogEntry;

pub struct RingBuffer {
    inner: Mutex<VecDeque<LogEntry>>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Push one entry; drop oldest if full.
    pub fn push(&self, entry: LogEntry) {
        let mut q = self.inner.lock();
        if q.len() >= self.capacity {
            q.pop_front();
        }
        q.push_back(entry);
    }

    /// Drain up to `n` oldest entries.
    pub fn drain(&self, n: usize) -> Vec<LogEntry> {
        let mut q = self.inner.lock();
        let take = n.min(q.len());
        q.drain(..take).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}
