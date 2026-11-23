//! Thread-local small-block magazine allocator for `wasm32`.
//!
//! Avoids repeated `dlmalloc` contention/overhead (and global spinlock contention under
//! `wasm32 + atomics`) by caching freed small blocks (`16..=2048` bytes, `align <= 8`) in
//! per-thread freelists and draining when workers or render frames go idle.
#![expect(
    unsafe_code,
    reason = "thread-local magazine allocator implements GlobalAlloc and raw freelist pointers"
)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::{Cell, UnsafeCell},
    ptr,
};

const NUM_CLASSES: usize = 8;
const MIN_CLASS_BYTES: usize = 16;
const MAX_CLASS_BYTES: usize = 2048;
const BASE_ALIGN: usize = 8;
const CLASS_CAPS: [u16; NUM_CLASSES] = [96, 96, 64, 48, 32, 24, 16, 8];

struct Magazine {
    heads: [*mut u8; NUM_CLASSES],
    counts: [u16; NUM_CLASSES],
}

impl Magazine {
    const fn new() -> Self {
        Self {
            heads: [ptr::null_mut(); NUM_CLASSES],
            counts: [0; NUM_CLASSES],
        }
    }

    fn pop(&mut self, class_idx: usize) -> Option<*mut u8> {
        let head_slot = self.heads.get_mut(class_idx)?;
        let head = *head_slot;
        if head.is_null() {
            return None;
        }
        // SAFETY: `head` points to a free `class_layout(class_idx)` block (size >= 16 >= ptr size)
        // whose first pointer-sized word stores the next node address.
        let next = unsafe { *head.cast::<*mut u8>() };
        *head_slot = next;
        if let Some(count) = self.counts.get_mut(class_idx) {
            *count = count.saturating_sub(1);
        }
        Some(head)
    }

    fn push(&mut self, class_idx: usize, ptr_raw: *mut u8) -> bool {
        let cap = CLASS_CAPS.get(class_idx).copied().unwrap_or(0);
        if self.counts.get(class_idx).copied().unwrap_or(0) >= cap {
            self.drain_half(class_idx);
        }
        let Some(head_slot) = self.heads.get_mut(class_idx) else {
            return false;
        };
        // SAFETY: `ptr_raw` is a uniquely owned block of `class_layout(class_idx)` (`>= 16` bytes,
        // 8-byte aligned), large and aligned enough to store a `*mut u8` freelist link at offset 0.
        unsafe { *ptr_raw.cast::<*mut u8>() = *head_slot };
        *head_slot = ptr_raw;
        if let Some(count) = self.counts.get_mut(class_idx) {
            *count = count.saturating_add(1);
        }
        true
    }

    fn drain_half(&mut self, class_idx: usize) {
        let count = self.counts.get(class_idx).copied().unwrap_or(0);
        let burst = (count / 2).max(1);
        let target = count.saturating_sub(burst);
        self.drain_to(class_idx, target);
    }

    fn drain_all(&mut self) {
        for class_idx in 0..NUM_CLASSES {
            self.drain_to(class_idx, 0);
        }
    }

    fn drain_to(&mut self, class_idx: usize, target: u16) {
        let layout = class_layout(class_idx);
        while self.counts.get(class_idx).copied().unwrap_or(0) > target {
            let Some(head_slot) = self.heads.get_mut(class_idx) else {
                break;
            };
            let head = *head_slot;
            if head.is_null() {
                if let Some(count) = self.counts.get_mut(class_idx) {
                    *count = 0;
                }
                break;
            }
            // SAFETY: `head` was allocated from `System` with `class_layout(class_idx)` and stores
            // its next freelist link in word 0.
            unsafe {
                *head_slot = *head.cast::<*mut u8>();
                if let Some(count) = self.counts.get_mut(class_idx) {
                    *count = count.saturating_sub(1);
                }
                System.dealloc(head, layout);
            }
        }
    }
}

pub(crate) fn drain_thread_magazine() {
    let Ok(_) = MAG.try_with(|tm| tm.with_mut(Magazine::drain_all)) else {
        return;
    };
}

struct ThreadMagazine {
    in_alloc: Cell<bool>,
    mag: UnsafeCell<Magazine>,
}

impl ThreadMagazine {
    const fn new() -> Self {
        Self {
            in_alloc: Cell::new(false),
            mag: UnsafeCell::new(Magazine::new()),
        }
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut Magazine) -> R) -> Option<R> {
        if self.in_alloc.get() {
            return None;
        }
        self.in_alloc.set(true);
        // SAFETY: `self` resides in thread-local storage and `in_alloc` prevents re-entrant
        // mutable access on this thread while the returned `&mut Magazine` is live.
        let res = f(unsafe { &mut *self.mag.get() });
        self.in_alloc.set(false);
        Some(res)
    }
}

thread_local! {
    static MAG: ThreadMagazine = const { ThreadMagazine::new() };
}

#[inline]
const fn class_index(layout: Layout) -> Option<usize> {
    let size = layout.size();
    if size == 0 || size > MAX_CLASS_BYTES || layout.align() > BASE_ALIGN {
        return None;
    }
    let normalized = if size < MIN_CLASS_BYTES {
        MIN_CLASS_BYTES
    } else {
        size.next_power_of_two()
    };
    Some((normalized.trailing_zeros() - 4) as usize)
}

#[inline]
const fn class_layout(class_idx: usize) -> Layout {
    let size = MIN_CLASS_BYTES << class_idx;
    // SAFETY: `size` is a non-zero power-of-two in `16..=2048` and `BASE_ALIGN` is `8`,
    // satisfying all `Layout::from_size_align` invariants without panic paths.
    unsafe { Layout::from_size_align_unchecked(size, BASE_ALIGN) }
}

pub struct MagazineAllocator;

unsafe impl GlobalAlloc for MagazineAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(class_idx) = class_index(layout) else {
            // SAFETY: Caller upholds `GlobalAlloc::alloc` preconditions for `layout`.
            return unsafe { System.alloc(layout) };
        };
        let popped = MAG
            .try_with(|tm| tm.with_mut(|m| m.pop(class_idx)).flatten())
            .ok()
            .flatten();
        if let Some(cached) = popped {
            return cached;
        }
        // SAFETY: Always back small-class allocations with `class_layout(class_idx)` so `dealloc`
        // and freelist recycling match identically across threads and re-entrancy guards.
        unsafe { System.alloc(class_layout(class_idx)) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let Some(class_idx) = class_index(layout) else {
            // SAFETY: Caller upholds `GlobalAlloc::alloc_zeroed` preconditions for `layout`.
            return unsafe { System.alloc_zeroed(layout) };
        };
        let popped = MAG
            .try_with(|tm| tm.with_mut(|m| m.pop(class_idx)).flatten())
            .ok()
            .flatten();
        if let Some(cached) = popped {
            // SAFETY: `cached` has at least `layout.size()` writable bytes.
            unsafe { cached.write_bytes(0, layout.size()) };
            return cached;
        }
        // SAFETY: Back with `class_layout(class_idx)` for uniform free/reuse layout matching.
        unsafe { System.alloc_zeroed(class_layout(class_idx)) }
    }

    unsafe fn dealloc(&self, ptr_raw: *mut u8, layout: Layout) {
        let Some(class_idx) = class_index(layout) else {
            // SAFETY: Uncached classes were allocated directly from `System` with `layout`.
            unsafe { System.dealloc(ptr_raw, layout) };
            return;
        };
        let pushed = MAG
            .try_with(|tm| tm.with_mut(|m| m.push(class_idx, ptr_raw)).unwrap_or(false))
            .unwrap_or(false);
        if !pushed {
            // SAFETY: Block was allocated from `System` with `class_layout(class_idx)`.
            unsafe { System.dealloc(ptr_raw, class_layout(class_idx)) };
        }
    }

    unsafe fn realloc(&self, ptr_raw: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old_class = class_index(layout);
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };
        let new_class = class_index(new_layout);
        if old_class.is_some() && old_class == new_class {
            return ptr_raw;
        }
        if old_class.is_none() && new_class.is_none() {
            // SAFETY: Neither size class uses the thread-local magazine.
            return unsafe { System.realloc(ptr_raw, layout, new_size) };
        }
        // SAFETY: Caller guarantees `ptr_raw` is valid for `layout`.
        let new_ptr = unsafe { self.alloc(new_layout) };
        if new_ptr.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: Both regions are valid for at least `min(layout.size(), new_size)` bytes and
        // do not overlap.
        unsafe {
            ptr::copy_nonoverlapping(ptr_raw, new_ptr, layout.size().min(new_size));
            self.dealloc(ptr_raw, layout);
        }
        new_ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_classes_cover_16_to_2048_bytes_and_drain_cleanly() {
        assert_eq!(
            class_index(
                Layout::from_size_align(1, 1)
                    .ok()
                    .unwrap_or(Layout::new::<u8>())
            ),
            Some(0)
        );
        assert_eq!(
            class_index(
                Layout::from_size_align(16, 8)
                    .ok()
                    .unwrap_or(Layout::new::<u8>())
            ),
            Some(0)
        );
        assert_eq!(
            class_index(
                Layout::from_size_align(2048, 8)
                    .ok()
                    .unwrap_or(Layout::new::<u8>())
            ),
            Some(7)
        );
        assert_eq!(class_layout(7).size(), 2048);
        assert_eq!(
            class_index(
                Layout::from_size_align(2049, 8)
                    .ok()
                    .unwrap_or(Layout::new::<u8>())
            ),
            None
        );
        assert_eq!(
            class_index(
                Layout::from_size_align(8192, 8)
                    .ok()
                    .unwrap_or(Layout::new::<u8>())
            ),
            None
        );
        assert_eq!(
            class_index(
                Layout::from_size_align(32, 16)
                    .ok()
                    .unwrap_or(Layout::new::<u8>())
            ),
            None
        );

        let layout = Layout::from_size_align(2048, 8)
            .ok()
            .unwrap_or(Layout::new::<u8>());
        let Some(class_idx) = class_index(layout) else {
            return;
        };
        // SAFETY: Allocates and deallocates a valid 2048-byte block through `MagazineAllocator`.
        unsafe {
            let ptr = MagazineAllocator.alloc(layout);
            assert!(!ptr.is_null());
            MagazineAllocator.dealloc(ptr, layout);
        }
        let before = MAG
            .try_with(|tm| tm.with_mut(|m| m.counts[class_idx]))
            .ok()
            .flatten()
            .unwrap_or(0);
        assert_eq!(before, 1);
        drain_thread_magazine();
        let after = MAG
            .try_with(|tm| tm.with_mut(|m| m.counts))
            .ok()
            .flatten()
            .unwrap_or([u16::MAX; NUM_CLASSES]);
        assert_eq!(after, [0; NUM_CLASSES]);
    }
}
