//! Head-mounted flight symbology with separate world directions and numeric readouts.
#![no_std]

#[cfg(test)]
extern crate std;

mod angular;
mod panel;
pub use angular::{AngularScene, AngularStroke, ViewReference, directions, view_reference};
pub use panel::{HMD_DESCRIPTOR, HMD_GLANCE_DESCRIPTOR, HMD_SET};
