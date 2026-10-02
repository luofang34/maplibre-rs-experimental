//! Head-mounted flight symbology with separate world directions and numeric readouts.
#![no_std]

#[cfg(test)]
extern crate std;

mod angular;
mod layout;
mod panel;
mod presentation;
mod zones;
pub use angular::{AngularScene, AngularStroke, ViewReference, directions, view_reference};
pub use layout::{COMPACT_ENTRY_DEGREES, COMPACT_EXIT_DEGREES, use_compact};
pub use panel::{HMD_DESCRIPTOR, HMD_GLANCE_DESCRIPTOR, HMD_SET};

pub use panel::draw_hwd;
pub use presentation::{
    DetailLevel, DisplayContext, DisplayRole, DisplayTask, InstrumentReference, PresentationPlan,
    ViewRegion, layer_reference, presentation_plan,
};
pub use zones::{
    ALERT_ZONE, AUTOMATION_ZONE, CENTRAL_VIEW_ZONE, DisplayZone, HOST_CONTEXT_ZONE, MISSION_ZONE,
};
