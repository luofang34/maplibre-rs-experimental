//! The alert axis of the case matrix.
//!
//! A composed frame fans one `AlertOutput` to every slot, so a panel's
//! criticality band is only honest if it was measured with alerts fed.
//! Measuring with `None` alone would record a band that excludes the
//! shared alert stack entirely, and a composition would then be told it
//! may cover warning rows.
//!
//! One extra case per state is enough because the stack's extent is
//! bounded and [`saturated_stack`] reaches all of it.

use indicate_alerts::{
    AlertCondition, AlertContext, AlertEvent, AlertManager, AlertOutput, AlertProfile,
    MiscompareFault,
};

/// The alert output that drives the shared stack to its full extent.
///
/// Three things together bound it, and this fixture does all three:
///
/// - a **faulted** alerting path, which paints the `ALRT FAIL` marker
///   four rows above the stack base — the topmost row the stack can
///   reach, and, at nine characters, tied for the widest;
/// - **more alerts than the stack shows**, which fills every visible row
///   and raises the `MORE` marker below them;
/// - a **warning** among them, so the highest class is represented.
///
/// A band measured over this case therefore contains the stack of every
/// other alert output, which is what lets one extra case per state
/// stand in for the whole alert axis.
pub(super) fn saturated_stack() -> AlertOutput {
    let mut events =
        [AlertEvent::Assert(AlertCondition::Miscompare(MiscompareFault::Attitude)); 32];
    for (index, event) in events.iter_mut().enumerate().skip(1) {
        // Distinct identities: the manager keys on identity, so
        // repeating one condition would fill a single slot.
        *event = AlertEvent::Assert(AlertCondition::FrameMismatch { code: index as u8 });
    }
    let mut manager = AlertManager::new();
    manager.step(
        &AlertProfile::simulator(),
        &events,
        AlertContext {
            // A faulted path is the only way to reach the stack's top
            // row, and it is a real posture: the alerting path may
            // degrade while the panels keep drawing.
            alerting_path_healthy: false,
            ..AlertContext::default()
        },
        1_000,
    )
}
