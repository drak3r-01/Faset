//! Frame bookkeeping that needs no Vulkan: deferred destruction and the
//! uniform ring cursor.

/// Number of frames the CPU may record ahead of the GPU.
pub(crate) const FRAMES_IN_FLIGHT: usize = 2;

/// Resources retired while the GPU may still read them. An item queued while
/// frame `n` is current is released once frame `n + FRAMES_IN_FLIGHT` starts,
/// after that frame's fence wait proves frame `n` finished.
pub(crate) struct DeferredQueue<T> {
    items: Vec<(u64, T)>,
}

impl<T> DeferredQueue<T> {
    pub(crate) fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Queues `item`, retired while `frame` is the current frame index.
    pub(crate) fn push(&mut self, frame: u64, item: T) {
        self.items.push((frame, item));
    }

    /// Removes and returns the items that are safe to destroy at `frame`.
    pub(crate) fn take_ready(&mut self, frame: u64) -> Vec<T> {
        let (ready, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut self.items)
            .into_iter()
            .partition(|(queued, _)| frame >= queued + FRAMES_IN_FLIGHT as u64);
        self.items = pending;
        ready.into_iter().map(|(_, item)| item).collect()
    }

    /// Removes everything (the device is idle).
    pub(crate) fn take_all(&mut self) -> Vec<T> {
        std::mem::take(&mut self.items)
            .into_iter()
            .map(|(_, item)| item)
            .collect()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.items.len()
    }
}

/// Bump allocator over one frame's uniform buffer, reset when the frame is
/// reused.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RingCursor {
    capacity: u64,
    head: u64,
}

impl RingCursor {
    pub(crate) fn new(capacity: u64) -> Self {
        Self { capacity, head: 0 }
    }

    pub(crate) fn reset(&mut self) {
        self.head = 0;
    }

    /// Reserves `size` bytes aligned to `align` (a power of two); returns the
    /// offset, or `None` when the ring is full.
    // Used from lot 2.2 (per-draw uniforms); only tests call it today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn alloc(&mut self, size: u64, align: u64) -> Option<u64> {
        let offset = self.head.checked_next_multiple_of(align.max(1))?;
        let end = offset.checked_add(size)?;
        if end > self.capacity {
            return None;
        }
        self.head = end;
        Some(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_released_two_frames_later() {
        let mut q = DeferredQueue::new();
        q.push(5, "a");
        q.push(6, "b");
        assert!(q.take_ready(5).is_empty());
        assert!(q.take_ready(6).is_empty());
        assert_eq!(q.take_ready(7), vec!["a"]);
        assert_eq!(q.len(), 1);
        assert_eq!(q.take_ready(8), vec!["b"]);
        assert_eq!(q.len(), 0);
        q.push(9, "c");
        assert_eq!(q.take_all(), vec!["c"]);
    }

    #[test]
    fn ring_aligns_and_fills() {
        let mut ring = RingCursor::new(256);
        assert_eq!(ring.alloc(10, 64), Some(0));
        assert_eq!(ring.alloc(10, 64), Some(64));
        assert_eq!(ring.alloc(128, 64), Some(128));
        assert_eq!(ring.alloc(1, 64), None);
        ring.reset();
        assert_eq!(ring.alloc(256, 256), Some(0));
        assert_eq!(ring.alloc(0, 1), Some(256));
        assert_eq!(ring.alloc(u64::MAX, 1), None);
    }
}
