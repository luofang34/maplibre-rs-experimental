//! Fixed reservations in the 1200 by 600 logical frame.

/// A rectangular content reservation; bounds do not imply physical visual angles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayZone {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Horizontal extent.
    pub width: f32,
    /// Vertical extent.
    pub height: f32,
}

/// Active, armed, engagement, protection, and transition indications.
pub const AUTOMATION_ZONE: DisplayZone = DisplayZone {
    x: 60.0,
    y: 0.0,
    width: 390.0,
    height: 122.0,
};
/// Stable alert stack, including alert-manager health and overflow.
pub const ALERT_ZONE: DisplayZone = DisplayZone {
    x: 840.0,
    y: 0.0,
    width: 330.0,
    height: 122.0,
};
/// No routine status text or opaque task panel belongs in this forward-view reservation.
/// Conformal flight and task cues can cross it at their true angular positions.
pub const CENTRAL_VIEW_ZONE: DisplayZone = DisplayZone {
    x: 450.0,
    y: 160.0,
    width: 300.0,
    height: 175.0,
};
/// Mission status reservation when [`crate::PresentationPlan::mission`] is true.
/// Detailed radar, maps, and checklists need a separate, explicitly opened panel.
pub const MISSION_ZONE: DisplayZone = DisplayZone {
    x: 900.0,
    y: 480.0,
    width: 280.0,
    height: 110.0,
};
/// One fitted line for host playback or connection status, including its contrast halo.
pub const HOST_CONTEXT_ZONE: DisplayZone = DisplayZone {
    x: 450.0,
    y: 584.0,
    width: 380.0,
    height: 16.0,
};
