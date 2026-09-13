//! Head-mounted flight symbology with separate world directions and numeric readouts.
#![no_std]

#[cfg(test)]
extern crate std;

mod angular;
mod layout;
mod panel;
pub use angular::{AngularScene, AngularStroke, ViewReference, directions, view_reference};
pub use layout::{COMPACT_ENTRY_DEGREES, COMPACT_EXIT_DEGREES, use_compact};
pub use panel::{HMD_DESCRIPTOR, HMD_GLANCE_DESCRIPTOR, HMD_SET};
