//! Coalescing best-fit range suballocator for fixed-capacity GPU buffer pages.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// Greatest common divisor of two non-negative integers.
pub(super) const fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// Least common multiple of two positive integers.
pub(super) const fn lcm_u64(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        return 0;
    }
    (a / gcd_u64(a, b)) * b
}

/// Byte-alignment required in the vertex buffer so `first_vertex = byte_offset / vertex_stride`
/// simultaneously aligns `first_vertex * vertex_stride` and `first_vertex * record_stride` to
/// [`wgpu::COPY_BUFFER_ALIGNMENT`].
pub(super) const fn lockstep_vertex_alignment(vertex_stride: u64, record_stride: u64) -> u64 {
    if vertex_stride == 0 {
        return wgpu::COPY_BUFFER_ALIGNMENT;
    }
    let copy_align = wgpu::COPY_BUFFER_ALIGNMENT;
    let v_elems = copy_align / gcd_u64(copy_align, vertex_stride);
    let fm_elems = if record_stride == 0 {
        1
    } else {
        copy_align / gcd_u64(copy_align, record_stride)
    };
    lcm_u64(v_elems, fm_elems) * vertex_stride
}

/// Tracks free sub-ranges within `[0, capacity)` with `O(log F)` best-fit allocation and
/// immediate neighbour coalescing on deallocation (`F` = free interval count).
#[derive(Debug, Clone)]
pub(super) struct RangeAllocator {
    capacity: u64,
    used_bytes: u64,
    /// Disjoint, non-adjacent free intervals keyed by `start -> len`.
    free_by_offset: BTreeMap<u64, u64>,
    /// Identical free intervals ordered by `(len, start)` for `O(log F)` best-fit search.
    free_by_size: BTreeSet<(u64, u64)>,
}

impl RangeAllocator {
    /// Creates an allocator with `[0, capacity)` initially free.
    pub(super) fn new(capacity: u64) -> Self {
        let mut free_by_offset = BTreeMap::new();
        let mut free_by_size = BTreeSet::new();
        if capacity > 0 {
            free_by_offset.insert(0, capacity);
            free_by_size.insert((capacity, 0));
        }
        Self {
            capacity,
            used_bytes: 0,
            free_by_offset,
            free_by_size,
        }
    }

    /// Resets the allocator back to `[0, capacity)` completely free.
    pub(super) fn clear(&mut self) {
        self.used_bytes = 0;
        self.free_by_offset.clear();
        self.free_by_size.clear();
        if self.capacity > 0 {
            self.free_by_offset.insert(0, self.capacity);
            self.free_by_size.insert((self.capacity, 0));
        }
    }

    /// Size in bytes of the largest contiguous free block currently available.
    #[cfg(test)]
    #[inline]
    pub(super) fn largest_free_block(&self) -> u64 {
        self.free_by_size.last().map_or(0, |&(len, _)| len)
    }

    /// Returns `true` if a contiguous block of `size` bytes aligned to `align` fits.
    pub(super) fn can_allocate(&self, size: u64, align: u64) -> bool {
        if size == 0 {
            return true;
        }
        if size > self.capacity - self.used_bytes {
            return false;
        }
        self.find_best_fit(size, align.max(1)).is_some()
    }

    /// Allocates `size` bytes aligned to `align`, splitting any leftover prefix/suffix back into
    /// the free trees.
    pub(super) fn allocate(&mut self, size: u64, align: u64) -> Option<Range<u64>> {
        if size == 0 {
            return Some(0..0);
        }
        let align = align.max(1);
        let (block_start, block_len, aligned_start) = self.find_best_fit(size, align)?;
        let block_end = block_start + block_len;
        let alloc_end = aligned_start + size;

        self.free_by_offset.remove(&block_start);
        self.free_by_size.remove(&(block_len, block_start));

        if aligned_start > block_start {
            let prefix_len = aligned_start - block_start;
            self.free_by_offset.insert(block_start, prefix_len);
            self.free_by_size.insert((prefix_len, block_start));
        }
        if alloc_end < block_end {
            let suffix_len = block_end - alloc_end;
            self.free_by_offset.insert(alloc_end, suffix_len);
            self.free_by_size.insert((suffix_len, alloc_end));
        }

        self.used_bytes += size;
        Some(aligned_start..alloc_end)
    }

    /// Releases a previously allocated `range`, immediately coalescing with adjacent free blocks.
    pub(super) fn free(&mut self, range: Range<u64>) {
        if range.is_empty() {
            return;
        }
        let mut start = range.start;
        let mut end = range.end;
        let len = end - start;
        debug_assert!(end <= self.capacity && len <= self.used_bytes);
        self.used_bytes = self.used_bytes.saturating_sub(len);

        if let Some((&prev_start, &prev_len)) = self.free_by_offset.range(..=start).next_back() {
            debug_assert!(
                prev_start + prev_len <= start,
                "overlapping free in RangeAllocator"
            );
            if prev_start + prev_len == start {
                self.free_by_offset.remove(&prev_start);
                self.free_by_size.remove(&(prev_len, prev_start));
                start = prev_start;
            }
        }

        if let Some(next_len) = self.free_by_offset.remove(&end) {
            self.free_by_size.remove(&(next_len, end));
            end += next_len;
        }

        let merged_len = end - start;
        self.free_by_offset.insert(start, merged_len);
        self.free_by_size.insert((merged_len, start));
    }

    fn find_best_fit(&self, size: u64, align: u64) -> Option<(u64, u64, u64)> {
        for &(block_len, block_start) in self.free_by_size.range((size, 0)..) {
            let aligned_start = block_start.div_ceil(align) * align;
            if let Some(alloc_end) = aligned_start.checked_add(size) {
                if alloc_end <= block_start + block_len {
                    return Some((block_start, block_len, aligned_start));
                }
            }
        }
        None
    }
}
