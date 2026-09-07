//! Bytes the process holds on the heap, as a host's allocator reports them, so a frame's
//! memory growth can be charged to the system it happens in when no profiler can attach.

use std::sync::atomic::{AtomicIsize, Ordering};

/// Bytes allocated and not yet freed. A host that installs a counting allocator keeps it
/// current; without one it stays at zero and every growth reads as zero.
pub static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);

/// Bytes on the heap right now, as the host's allocator counts them.
pub fn live_bytes() -> isize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Adds `bytes` (negative when freed) to the count; a counting allocator calls this.
pub fn account(bytes: isize) {
    LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed);
}
