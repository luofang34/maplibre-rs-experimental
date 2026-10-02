//! Display choices stay separate from aircraft guidance and control state.
use indicate_alerts::{AlertClass, AlertOutput};
use indicate_instrument_scene::LayerId;
use indicate_instrument_state::{PanelData, Sig, SignalStatus, flight::presentation};

/// The HWD's responsibility for flight information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayRole {
    /// Declutter must retain the flight reference.
    #[default]
    Primary,
    /// A confirmed visible flight display can supply duplicate flight information.
    Supplemental,
}

/// The operator's requested information task; this never commands aircraft systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayTask {
    /// Flight and navigation information.
    #[default]
    Flight,
    /// Flight essentials with space reserved for mission status.
    Mission,
}

/// Operator-selected detail, independent of active guidance modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailLevel {
    /// Scales and secondary flight information.
    #[default]
    Normal,
    /// Flight essentials, targets, limits, modes, and alerts.
    Reduced,
}

/// The host's classified view region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewRegion {
    /// No duplicate flight display is in view.
    #[default]
    Outside,
    /// The observer looks through a conventional HUD.
    Hud,
    /// The observer looks at cockpit instruments.
    Cockpit,
}

/// Per-frame host input, independent of the aircraft state ABI.
#[derive(Debug, Clone, Copy)]
pub struct DisplayContext {
    /// Responsibility for flight information.
    pub role: DisplayRole,
    /// Requested information task.
    pub task: DisplayTask,
    /// Requested detail level.
    pub detail: DetailLevel,
    /// Classified view region; head angle alone cannot prove display visibility.
    pub region: ViewRegion,
    /// True only while another readable, functioning flight display is visible.
    /// The host must invalidate this signal when visibility evidence expires.
    pub alternate_flight_display: Sig<bool>,
}

impl Default for DisplayContext {
    fn default() -> Self {
        Self {
            role: DisplayRole::Primary,
            task: DisplayTask::Flight,
            detail: DetailLevel::Normal,
            region: ViewRegion::Outside,
            alternate_flight_display: Sig::with_status(false, SignalStatus::Missing),
        }
    }
}

/// Effective choices for one frame; recovery and failures override optional declutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationPlan {
    /// Unusual attitude takes priority over the requested task.
    pub recovery: bool,
    /// Duplicate flight geometry is suppressed; status and alerts remain visible.
    pub blank_flight: bool,
    /// Secondary flight detail is permitted.
    pub detailed: bool,
    /// The mission status reservation is available to a future task renderer.
    pub mission: bool,
}

/// Resolves declutter without changing aircraft state or inferring flight-display visibility.
pub fn presentation_plan(
    data: &PanelData,
    context: DisplayContext,
    alerts: Option<&AlertOutput>,
) -> PresentationPlan {
    let recovery = data.presentation.unusual;
    let urgent = alerts.is_some_and(|output| {
        output.health() != indicate_alerts::ManagerHealth::Nominal
            || output.active().iter().any(|alert| {
                !alert.inhibited && !alert.decluttered && alert.class >= AlertClass::Caution
            })
    });
    let alternate = context.alternate_flight_display;
    let blank_flight = !recovery
        && !urgent
        && flight_reference_valid(data)
        && context.role == DisplayRole::Supplemental
        && context.region != ViewRegion::Outside
        && alternate.status == SignalStatus::Valid
        && alternate.value;
    PresentationPlan {
        recovery,
        blank_flight,
        detailed: !recovery
            && context.detail == DetailLevel::Normal
            && context.task == DisplayTask::Flight,
        mission: !recovery
            && !urgent
            && flight_reference_valid(data)
            && context.task == DisplayTask::Mission,
    }
}

fn flight_reference_valid(data: &PanelData) -> bool {
    [
        presentation::current_speed(data).1,
        data.altitude.value_ft,
        data.roll_rad,
        data.pitch_rad,
        data.heading.value_rad,
        data.vsi_fpm,
    ]
    .into_iter()
    .all(crate::panel::live)
}

/// Projection frame for nonconformal instrument geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentReference {
    /// Aircraft-relative forward panel.
    Aircraft,
    /// Observer direction with roll stabilization; measured attitude remains independent.
    HeadLevel,
    /// Display-fixed status, unaffected by head motion or panel switching.
    Head,
}

/// Keeps nonconformal instruments in one display frame to preserve text reservations.
/// World geometry from [`crate::directions`] uses its own earth reference.
pub const fn layer_reference(_layer: LayerId, _compact: bool) -> InstrumentReference {
    InstrumentReference::Head
}

#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod tests;
