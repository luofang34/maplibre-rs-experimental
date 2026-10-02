//! The screen-composition digest: one number that proves two shells lay
//! the same instruments out the same way on one surface.
//!
//! It covers the screen frame, the ordered slots — panel id, rect, and
//! `occludes` list — and the registry scene digest beneath, so it is
//! strictly stronger than the scene digest: two shells agreeing here
//! agree both about what their panels paint and about where.
//!
//! Slot rects are in the digest from day one. Relaxing which rects are
//! admissible therefore changes what validates, never what the digest
//! covers, and no such relaxation churns the format.
//!
//! Per-slot configuration is deliberately absent, because it is not
//! declared here: the descriptor declares layout and the shell supplies
//! configuration at draw time, so a shell that reconfigures a panel
//! still composes the same screen.

use indicate_sha256::Sha256Ctx;

use crate::composition::CompositionDescriptor;
use crate::digest::{DigestError, digest_frame, scene_digest, update_framed};
use crate::registry::Registry;

/// Domain separator; a new value is a deliberate contract break.
///
/// Like [`crate::SCENE_DIGEST_DOMAIN`] this is an identifier rather than
/// a name: it is hashed into every pin a consumer holds, so it does not
/// track what the crates are called.
pub const COMPOSITION_DIGEST_DOMAIN: &[u8] = b"pilotage-screen-composition-digest-v1";

/// Item-role tags framing the composition stream. They start above the
/// scene digest's tags so a reader of a hexdump cannot confuse the two
/// streams, though the domain separator already makes them disjoint.
const ROLE_SLOT: u8 = 0x11;
const ROLE_OCCLUDES: u8 = 0x12;

/// Digests `composition` over `registry`, drawing into `scratch` (size
/// it [`indicate_instrument_scene::MAX_SCENE_BYTES`]).
///
/// Deliberately independent of [`crate::validate_composition`]: a
/// digest states what a composition *is*, and a consumer comparing pins
/// must get the same answer whether or not its registry would admit it.
pub fn composition_digest(
    registry: &Registry,
    composition: &CompositionDescriptor,
    scratch: &mut [u8],
) -> Result<[u8; 32], DigestError> {
    let beneath = scene_digest(registry, scratch)?;
    let mut ctx = Sha256Ctx::new();
    ctx.update(COMPOSITION_DIGEST_DOMAIN);
    ctx.update(&beneath);
    digest_frame(&mut ctx, composition.screen);
    ctx.update(&(composition.slots.len() as u32).to_le_bytes());
    for slot in composition.slots {
        update_framed(&mut ctx, ROLE_SLOT, slot.panel.as_bytes());
        ctx.update(&slot.rect.x.to_le_bytes());
        ctx.update(&slot.rect.y.to_le_bytes());
        ctx.update(&slot.rect.width.to_le_bytes());
        ctx.update(&slot.rect.height.to_le_bytes());
        ctx.update(&(slot.occludes.len() as u32).to_le_bytes());
        for occluded in slot.occludes {
            update_framed(&mut ctx, ROLE_OCCLUDES, occluded.as_bytes());
        }
    }
    Ok(ctx.finalize())
}

#[cfg(test)]
mod tests;
