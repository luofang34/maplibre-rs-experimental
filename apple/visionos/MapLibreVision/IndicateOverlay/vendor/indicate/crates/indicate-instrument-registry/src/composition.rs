//! Screen composition: whole validated panel scenes, placed and stacked
//! by declaration on one logical screen (AIR-OUT-011).
//!
//! A composition is layout and nothing else. It names panels, gives
//! each a rectangle of the screen, and fixes the paint order — **slot
//! index is z**, exactly as [`indicate_instrument_scene::LayerId`]'s
//! discriminant is z within a scene, so later slots paint above earlier
//! ones and declaration order is paint order everywhere. It carries no
//! per-slot configuration: the descriptor declares layout, the shell
//! supplies data and configuration at draw time, so configuration never
//! joins the composition's identity.
//!
//! Nothing here resizes a scene. There is no `SCALE` opcode and a
//! compositor never rewrites commands, so a slot's dimensions *are* the
//! frame its panel is asked to emit — which is why a slot rect must be
//! a frame the panel declared it can lay out against.
//!
//! Overlap rides [`BackgroundCapability`] rather than any new alpha
//! mechanism: `Opaque` and `Cedeable` panels prove a full-frame opaque
//! cover at admission, so they occlude their whole rect, while a
//! `NotUsed` panel paints nothing in the background band and functions
//! as an overlay through which lower slots show.

use indicate_instrument_descriptor::{BackgroundCapability, CriticalityBands, DesignFrame, Region};

use crate::registry::Registry;

mod coverage;
mod digest;
mod error;

pub use digest::{COMPOSITION_DIGEST_DOMAIN, composition_digest};
pub use error::CompositionError;

/// What a fixed-size cover array holds outside the prefix it fills.
/// It covers nothing, so an entry read past that prefix by mistake
/// could only ever understate coverage.
const NOWHERE: Region = Region {
    x: 0.0,
    y: 0.0,
    width: 0.0,
    height: 0.0,
};

/// Most slots one screen may compose.
///
/// Every composed-frame ceiling is a sum over slots — total scene bytes
/// are at most this many times
/// [`indicate_instrument_scene::MAX_SCENE_BYTES`], and the composed
/// timing envelope is the sum of the slot envelopes — so this constant
/// is what makes those sums finite.
pub const MAX_COMPOSITION_SLOTS: usize = 8;

/// One placed panel. `rect` is in screen units and `occludes` names the
/// panels below whose ordinary symbology this slot is allowed to cover.
///
/// A name in `occludes` is a reviewed decision, in the same spirit as
/// the admission warning ratchet: growing obscuration is deliberate,
/// never drift. It does not reach criticality content — see
/// [`validate_composition`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    /// The registered panel this slot paints.
    pub panel: &'static str,
    /// Where on the screen it paints, in screen units.
    pub rect: Region,
    /// Panel ids below this slot whose ordinary symbology it may cover.
    pub occludes: &'static [&'static str],
}

/// A declared screen layout: the logical screen, and the slots in paint
/// order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompositionDescriptor {
    /// The logical screen space, under the same units discipline as a
    /// panel frame.
    pub screen: DesignFrame,
    /// The slots, bottom to top.
    pub slots: &'static [Slot],
}

/// Validates a composition against the registry it will be painted
/// from and the criticality bands measured for its panels.
///
/// Allocation-free and meant for init: a composition fault is refused
/// before first paint, never discovered as a rendering curiosity
/// (AIR-HAZ-004).
///
/// The occlusion rules are two, and the difference between them is the
/// whole of AIR-OUT-011's floor:
///
/// - **Ordinary symbology** — a lower panel's declared `group_regions`,
///   placed at its slot's origin — may be covered, but only where the
///   covering slot names the lower panel in `occludes`.
/// - **Criticality content** — the measured `Annunciation`/`Failure`
///   band — may not be covered at all. No declaration licences it,
///   because a declaration that could conceal a warning would be a
///   declaration that the warning does not matter.
///
/// The floor protects what is *in* those two bands, and nothing else.
/// It is therefore exactly as wide as the panels make it: content a
/// panel paints into a lower band is ordinary symbology as far as this
/// is concerned, whatever it says. A panel carrying the simulation
/// labelling AIR-BAS-001 and AIR-FLAG-007 require must paint it into a
/// criticality band for the floor to reach it; no shipped panel emits
/// that labelling today, so the obligation is the author's and this
/// does not pretend otherwise.
pub fn validate_composition(
    registry: &Registry,
    composition: &CompositionDescriptor,
    criticality: &CriticalityBands,
) -> Result<(), CompositionError> {
    validate_shape(composition)?;
    // Every slot's own declaration is judged before any rule that reads
    // another slot's rect, so a nonsense rectangle is named as one
    // rather than reported as whatever it did to its neighbours.
    for (index, slot) in composition.slots.iter().enumerate() {
        validate_slot(registry, composition, index, slot)?;
        validate_band_known(criticality, index, slot)?;
    }
    for index in 0..composition.slots.len() {
        validate_alive(composition, registry, index)?;
    }
    validate_occlusion(registry, composition, criticality)
}

fn validate_shape(composition: &CompositionDescriptor) -> Result<(), CompositionError> {
    let screen = composition.screen;
    if !Region::of(screen).is_sound() {
        return Err(CompositionError::BadScreen { screen });
    }
    if composition.slots.is_empty() {
        return Err(CompositionError::NoSlots);
    }
    if composition.slots.len() > MAX_COMPOSITION_SLOTS {
        return Err(CompositionError::TooManySlots {
            slots: composition.slots.len(),
            ceiling: MAX_COMPOSITION_SLOTS,
        });
    }
    Ok(())
}

fn validate_slot(
    registry: &Registry,
    composition: &CompositionDescriptor,
    index: usize,
    slot: &Slot,
) -> Result<(), CompositionError> {
    let Some(panel) = registry.by_id(slot.panel) else {
        return Err(CompositionError::UnknownPanel {
            slot: index,
            panel: slot.panel,
        });
    };
    if !slot.rect.is_sound() {
        return Err(CompositionError::SlotRectDegenerate {
            slot: index,
            rect: slot.rect,
        });
    }
    if !Region::of(composition.screen).contains(&slot.rect) {
        return Err(CompositionError::SlotOutsideScreen {
            slot: index,
            rect: slot.rect,
            screen: composition.screen,
        });
    }
    let frame = slot_frame(slot);
    if panel.accepts(frame).is_err() {
        return Err(CompositionError::SlotFrameUnsupported {
            slot: index,
            panel: slot.panel,
            frame,
        });
    }
    Ok(())
}

/// A slot is only validated against measured evidence, so a panel with
/// no band pinned at the size the slot asks for is refused rather than
/// treated as inking nothing.
fn validate_band_known(
    criticality: &CriticalityBands,
    index: usize,
    slot: &Slot,
) -> Result<(), CompositionError> {
    let frame = slot_frame(slot);
    if criticality.entry(slot.panel, frame).is_none() {
        return Err(CompositionError::CriticalityUnknown {
            slot: index,
            panel: slot.panel,
            frame,
        });
    }
    Ok(())
}

fn validate_alive(
    composition: &CompositionDescriptor,
    registry: &Registry,
    index: usize,
) -> Result<(), CompositionError> {
    let Some(slot) = composition.slots.get(index) else {
        return Ok(());
    };
    let mut covers = [NOWHERE; MAX_COMPOSITION_SLOTS];
    let mut count = 0;
    for above in composition.slots.iter().skip(index.wrapping_add(1)) {
        if !opaque(registry, above) {
            continue;
        }
        if let Some(entry) = covers.get_mut(count) {
            *entry = above.rect;
            count = count.wrapping_add(1);
        }
    }
    let covers = covers.get(..count).unwrap_or(&[]);
    if coverage::covered(&slot.rect, covers) {
        return Err(CompositionError::DeadSlot { slot: index });
    }
    Ok(())
}

/// Whether a slot's panel occludes what it covers: an owned background
/// band is a proven full-frame opaque cover, and nothing else is.
fn opaque(registry: &Registry, slot: &Slot) -> bool {
    registry.by_id(slot.panel).is_some_and(|panel| {
        matches!(
            panel.background,
            BackgroundCapability::Opaque | BackgroundCapability::Cedeable
        )
    })
}

fn validate_occlusion(
    registry: &Registry,
    composition: &CompositionDescriptor,
    criticality: &CriticalityBands,
) -> Result<(), CompositionError> {
    for (upper, above) in composition.slots.iter().enumerate() {
        for (lower, below) in composition.slots.iter().enumerate().take(upper) {
            check_pair(registry, criticality, (upper, above), (lower, below))?;
        }
    }
    Ok(())
}

/// The whole rect of the slot above is what may cover, whatever its
/// background capability: an overlay is transparent only in the
/// background band, and its own symbology paints wherever it likes
/// inside its rect.
fn check_pair(
    registry: &Registry,
    criticality: &CriticalityBands,
    (upper, above): (usize, &Slot),
    (lower, below): (usize, &Slot),
) -> Result<(), CompositionError> {
    if !above.rect.intersects(&below.rect) {
        return Ok(());
    }
    check_criticality(criticality, (upper, above), (lower, below))?;
    if above.occludes.contains(&below.panel) {
        return Ok(());
    }
    check_ordinary(registry, (upper, above), (lower, below))
}

fn check_criticality(
    criticality: &CriticalityBands,
    (upper, above): (usize, &Slot),
    (lower, below): (usize, &Slot),
) -> Result<(), CompositionError> {
    // Unreachable in a validated run: every slot's band is required to
    // exist before any pair is compared, so a miss here would be a
    // caller that skipped that step rather than a permissive case.
    let Some(entry) = criticality.entry(below.panel, slot_frame(below)) else {
        return Ok(());
    };
    // A band nothing was ever witnessed in is *unknown*, not empty. It
    // does not say the panel puts no warnings anywhere; it says no case
    // that ran drew one. Reading it as empty would make an unexercised
    // failure cue the easiest thing on the screen to cover.
    let Some(band) = entry.band else {
        return Err(CompositionError::CriticalityUnwitnessed {
            upper,
            lower,
            panel: below.panel,
        });
    };
    let placed = band.translated(below.rect.x, below.rect.y);
    if above.rect.intersects(&placed) {
        return Err(CompositionError::CriticalityObscured {
            upper,
            lower,
            panel: below.panel,
            band: placed,
        });
    }
    Ok(())
}

/// `group_regions` describe the layout at the panel's readability floor
/// and nowhere else, so a slot that asks for a different frame has no
/// declaration this can place. That case treats the whole slot rect as
/// ordinary ink — the covering slot must then declare the obscuration,
/// which is the fail-closed direction.
fn check_ordinary(
    registry: &Registry,
    (upper, above): (usize, &Slot),
    (lower, below): (usize, &Slot),
) -> Result<(), CompositionError> {
    let refuse = |region| {
        Err(CompositionError::UndeclaredOcclusion {
            upper,
            lower,
            panel: below.panel,
            region,
        })
    };
    let placeable = registry
        .by_id(below.panel)
        .filter(|panel| panel.frame_min == slot_frame(below));
    let Some(panel) = placeable else {
        return refuse(below.rect);
    };
    for (_, region) in panel.group_regions {
        let placed = region.translated(below.rect.x, below.rect.y);
        if above.rect.intersects(&placed) {
            return refuse(placed);
        }
    }
    Ok(())
}

/// The frame a slot asks its panel for: composition is placement only,
/// so the slot's own dimensions are the emission frame.
fn slot_frame(slot: &Slot) -> DesignFrame {
    DesignFrame {
        width: slot.rect.width,
        height: slot.rect.height,
    }
}

#[cfg(test)]
mod tests;
