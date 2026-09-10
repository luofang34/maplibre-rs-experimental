//! Transparent, source-qualified flight readouts for synthetic terrain replay.
#![no_std]

#[cfg(test)]
extern crate std;

mod panel;

pub use panel::{SVS_DESCRIPTOR, SVS_SET};
