//! Fixed-capacity byte ring buffer.
//!
//! Sits between the network thread and the audio callback. The callback side
//! must never allocate, never lock and never block, so capacity is fixed at
//! construction and every operation is a copy into or out of storage that
//! already exists.
//!
//! The buffer deliberately exposes [`RingBuffer::with_contiguous_mut`], which
//! hands out a mutable slice of unread bytes. That is what will let a future
//! [`crate::processor::AudioProcessor`] run in place rather than copying
//! through a scratch buffer. See `docs/roadmap.md`.

/// A single-producer, single-consumer byte ring.
///
/// This type is not itself `Sync`; crossing threads is the transport layer's
/// job, and doing it here would drag in a synchronisation primitive that the
/// core has no business choosing.
#[derive(Debug)]
pub struct RingBuffer {
    storage: Box<[u8]>,
    read: usize,
    /// Bytes currently readable. Tracked explicitly rather than inferred from
    /// the two indices, which cannot distinguish full from empty.
    len: usize,
}

impl RingBuffer {
    /// Create a ring holding `capacity` bytes.
    ///
    /// # Panics
    /// Panics when `capacity` is zero, which no caller can use.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ring buffer capacity must be non-zero");
        Self {
            storage: vec![0; capacity].into_boxed_slice(),
            read: 0,
            len: 0,
        }
    }

    /// Total capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.storage.len()
    }

    /// Bytes available to read.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no bytes are available to read.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes that can be written before the buffer is full.
    #[must_use]
    pub fn free(&self) -> usize {
        self.capacity() - self.len
    }

    /// Discard everything currently buffered.
    pub fn clear(&mut self) {
        self.read = 0;
        self.len = 0;
    }

    /// Append `data`, returning how many bytes were taken.
    ///
    /// A short write means the buffer filled up. The caller decides what that
    /// means; the ring will not overwrite unread data on its own, because
    /// silently dropping the oldest audio is a policy the jitter buffer owns.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let take = data.len().min(self.free());
        if take == 0 {
            return 0;
        }

        let capacity = self.capacity();
        let write_at = (self.read + self.len) % capacity;
        let first = take.min(capacity - write_at);

        self.storage[write_at..write_at + first].copy_from_slice(&data[..first]);
        if first < take {
            self.storage[..take - first].copy_from_slice(&data[first..take]);
        }

        self.len += take;
        take
    }

    /// Fill `out` from the buffer, returning how many bytes were read.
    ///
    /// A short read means the buffer ran dry. The audio callback must treat
    /// the remainder as an underrun and emit silence rather than stale audio.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let take = out.len().min(self.len);
        if take == 0 {
            return 0;
        }

        let capacity = self.capacity();
        let first = take.min(capacity - self.read);

        out[..first].copy_from_slice(&self.storage[self.read..self.read + first]);
        if first < take {
            out[first..take].copy_from_slice(&self.storage[..take - first]);
        }

        self.read = (self.read + take) % capacity;
        self.len -= take;
        take
    }

    /// Read into `out`, filling any shortfall with zeroes.
    ///
    /// Returns the number of real bytes read, so the caller can count the
    /// underrun. This is the operation an audio callback wants: it always
    /// produces a full buffer, and never leaves the previous contents behind
    /// to be played twice.
    pub fn read_or_silence(&mut self, out: &mut [u8]) -> usize {
        let read = self.read(out);
        out[read..].fill(0);
        read
    }

    /// Drop up to `count` bytes without copying them anywhere.
    ///
    /// Returns how many were actually dropped. Used by drift correction to
    /// shed a small number of samples when the receiver is running behind.
    pub fn discard(&mut self, count: usize) -> usize {
        let take = count.min(self.len);
        self.read = (self.read + take) % self.capacity();
        self.len -= take;
        take
    }

    /// Run `f` over the longest contiguous run of unread bytes.
    ///
    /// Because the storage wraps, the readable region is at most two slices.
    /// This exposes the first, which is enough for in-place processing that
    /// works block by block. Returns the length passed to `f`.
    pub fn with_contiguous_mut<F: FnOnce(&mut [u8])>(&mut self, f: F) -> usize {
        if self.len == 0 {
            return 0;
        }
        let capacity = self.capacity();
        let run = self.len.min(capacity - self.read);
        f(&mut self.storage[self.read..self.read + run]);
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_returns_the_same_bytes() {
        let mut ring = RingBuffer::new(16);
        assert_eq!(ring.write(&[1, 2, 3, 4]), 4);
        assert_eq!(ring.len(), 4);

        let mut out = [0_u8; 4];
        assert_eq!(ring.read(&mut out), 4);
        assert_eq!(out, [1, 2, 3, 4]);
        assert!(ring.is_empty());
    }

    #[test]
    fn writes_stop_at_capacity_instead_of_overwriting() {
        let mut ring = RingBuffer::new(4);
        assert_eq!(ring.write(&[1, 2, 3, 4, 5, 6]), 4);
        assert_eq!(ring.free(), 0);
        assert_eq!(ring.write(&[9]), 0);

        let mut out = [0_u8; 4];
        ring.read(&mut out);
        assert_eq!(out, [1, 2, 3, 4], "oldest data must survive");
    }

    #[test]
    fn data_wrapping_the_end_is_reassembled_in_order() {
        let mut ring = RingBuffer::new(8);
        ring.write(&[1, 2, 3, 4, 5, 6]);

        let mut out = [0_u8; 4];
        ring.read(&mut out);
        assert_eq!(out, [1, 2, 3, 4]);

        // Now the write index is at 6 and the read index at 4, so this write
        // straddles the end of storage.
        assert_eq!(ring.write(&[7, 8, 9, 10, 11, 12]), 6);

        let mut out = [0_u8; 8];
        assert_eq!(ring.read(&mut out), 8);
        assert_eq!(out, [5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn a_long_sequence_of_partial_operations_stays_consistent() {
        let mut ring = RingBuffer::new(7);
        let mut expected: Vec<u8> = Vec::new();
        let mut next: u8 = 0;

        for step in 0..500 {
            if step % 3 == 0 {
                let chunk: Vec<u8> = (0..5)
                    .map(|_| {
                        let value = next;
                        next = next.wrapping_add(1);
                        value
                    })
                    .collect();
                let written = ring.write(&chunk);
                expected.extend_from_slice(&chunk[..written]);
                // Bytes the ring refused must not be counted as sent.
                next = next.wrapping_sub((chunk.len() - written) as u8);
            } else {
                let mut out = [0_u8; 3];
                let read = ring.read(&mut out);
                let head: Vec<u8> = expected.drain(..read).collect();
                assert_eq!(&out[..read], &head[..]);
            }
            assert_eq!(ring.len(), expected.len());
        }
    }

    #[test]
    fn read_or_silence_zero_fills_and_reports_the_shortfall() {
        let mut ring = RingBuffer::new(8);
        ring.write(&[1, 2, 3]);

        let mut out = [0xFF_u8; 6];
        assert_eq!(ring.read_or_silence(&mut out), 3);
        assert_eq!(out, [1, 2, 3, 0, 0, 0]);
    }

    #[test]
    fn discard_drops_from_the_front() {
        let mut ring = RingBuffer::new(8);
        ring.write(&[1, 2, 3, 4, 5]);
        assert_eq!(ring.discard(2), 2);

        let mut out = [0_u8; 3];
        ring.read(&mut out);
        assert_eq!(out, [3, 4, 5]);
    }

    #[test]
    fn discard_saturates_rather_than_underflowing() {
        let mut ring = RingBuffer::new(8);
        ring.write(&[1, 2]);
        assert_eq!(ring.discard(100), 2);
        assert!(ring.is_empty());
    }

    #[test]
    fn in_place_processing_is_visible_to_the_reader() {
        let mut ring = RingBuffer::new(8);
        ring.write(&[1, 2, 3, 4]);

        let touched = ring.with_contiguous_mut(|block| {
            for byte in block.iter_mut() {
                *byte *= 2;
            }
        });
        assert_eq!(touched, 4);

        let mut out = [0_u8; 4];
        ring.read(&mut out);
        assert_eq!(out, [2, 4, 6, 8]);
    }

    #[test]
    fn empty_operations_are_harmless() {
        let mut ring = RingBuffer::new(4);
        let mut out = [0_u8; 4];
        assert_eq!(ring.read(&mut out), 0);
        assert_eq!(ring.discard(3), 0);
        assert_eq!(ring.with_contiguous_mut(|_| unreachable!()), 0);
        assert_eq!(ring.write(&[]), 0);
    }

    #[test]
    #[should_panic(expected = "capacity must be non-zero")]
    fn zero_capacity_is_rejected() {
        let _ = RingBuffer::new(0);
    }
}
