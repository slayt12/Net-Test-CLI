//! Fixed-capacity ring buffer for chart points. Oldest entries are overwritten.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    cap: usize,
    inner: VecDeque<T>,
}

impl<T: Clone> RingBuffer<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            inner: VecDeque::with_capacity(cap.max(1)),
        }
    }

    pub fn push(&mut self, v: T) {
        if self.inner.len() == self.cap {
            self.inner.pop_front();
        }
        self.inner.push_back(v);
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.inner.iter()
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.inner.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }
}
