use crossbeam::queue::ArrayQueue;
use tracing::debug;

pub(super) const BUFFER_POOL_SIZE: usize = 16;
pub(super) const DEFAULT_BUFFER_SIZE: usize = 256 * 1024;

pub(super) struct BufferPool {
    pool: ArrayQueue<Vec<u8>>,
}

impl BufferPool {
    pub(super) fn new(capacity: usize, buffer_size: usize) -> Self {
        let pool = ArrayQueue::new(capacity);
        for _ in 0..capacity {
            if let Err(e) = pool.push(Vec::with_capacity(buffer_size)) {
                debug!("push failed: {e:?}");
            }
        }
        Self { pool }
    }

    pub(super) fn acquire(&self) -> Vec<u8> {
        self.pool
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(DEFAULT_BUFFER_SIZE))
    }

    pub(super) fn release(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        if let Err(e) = self.pool.push(buffer) {
            debug!("push failed: {e:?}");
        }
    }

    /// Number of buffers currently available in the pool.
    ///
    /// Used by tests to assert that fallible load paths do not permanently
    /// deplete the pool by dropping an acquired buffer on an early return.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.pool.len()
    }
}
