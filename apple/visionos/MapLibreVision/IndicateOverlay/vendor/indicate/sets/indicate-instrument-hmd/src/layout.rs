//! Host-independent selection between the aircraft panel and compact off-axis instruments.
use indicate_instrument_state::PanelData;

/// Off-axis angle that selects compact instruments, in degrees.
pub const COMPACT_ENTRY_DEGREES: f32 = 35.0;
/// Return cone for the aircraft panel, in degrees.
pub const COMPACT_EXIT_DEGREES: f32 = 28.0;

/// Chooses a layout from measured aircraft state and observer-to-panel alignment.
///
/// `alignment_cosine` is the dot product of normalized head and panel forward directions.
/// Separate entry and exit cones prevent tracking noise from switching layouts repeatedly.
/// Unusual attitude uses the source's airframe presentation policy.
pub fn use_compact(data: &PanelData, alignment_cosine: f32, was_compact: bool) -> bool {
    let threshold = if was_compact {
        COMPACT_EXIT_DEGREES
    } else {
        COMPACT_ENTRY_DEGREES
    };
    crate::view_reference(data).kind == 0
        || data.presentation.unusual
        || !alignment_cosine.is_finite()
        || alignment_cosine < libm::cosf(threshold.to_radians())
}

#[cfg(test)]
mod tests;
