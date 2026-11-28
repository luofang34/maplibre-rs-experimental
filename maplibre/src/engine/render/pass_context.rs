//! Per-frame render inputs, pass hooks, references, tracked passes, ground helpers, GPU timer,
//! and tile-view pattern descriptors.

pub mod frame_input;
pub mod gpu_timer;
pub mod ground;
pub mod pass_hooks;
pub(crate) mod pass_refs;
pub mod placement_3d;
pub mod tile_view_pattern;
pub mod tracked_pass;
