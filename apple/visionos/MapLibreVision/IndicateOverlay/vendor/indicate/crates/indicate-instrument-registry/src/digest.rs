//! The cross-shell scene digest (ADR-0033): one number that proves two
//! shells show the same instruments.
//!
//! The digest streams, per registered panel: the role-tagged,
//! length-prefixed panel id and the contract-relevant descriptor
//! fields, then per corpus state the role-tagged state id, and within
//! it per canonical frame the role-tagged frame and the emitted scene
//! bytes — drawn with the empty config and no alerts, so it is
//! invariant to SVS by construction (theme independence holds because
//! panels take no theme parameter at this boundary). Shells report the same digest or they are
//! not showing the same panels; pixel hashes stay per-backend
//! rasterizer regression tests, not the cross-shell contract. The
//! digest moves exactly once per deliberate contract change, re-pinned
//! with a review note saying why.

use indicate_instrument_descriptor::{
    BackgroundCapability, CANONICAL_STATES, DesignFrame, EMPTY_CONFIG, PanelDescriptor,
    PanelDrawError,
};
use indicate_instrument_scene::{SCENE_FORMAT_VERSION, SceneWriter};
use indicate_instrument_state::{FreshnessPolicy, abi::v8, resolve};
use indicate_sha256::Sha256Ctx;

use crate::registry::Registry;

/// Domain separator; a new value is a deliberate contract break.
///
/// The string is an identifier, not a name: it is hashed into every
/// composition digest consumers pin, so it does not track what the
/// crates are called. Rewriting it to match a crate rename would move
/// [`crate::scene_digest`] for zero paint change and redden every pin.
pub const SCENE_DIGEST_DOMAIN: &[u8] = b"pilotage-scene-digest-v1";

/// Why a digest run failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DigestError {
    /// A panel refused to draw a corpus state.
    #[error("panel {panel} failed to draw corpus state {state}")]
    Draw {
        /// The refusing panel.
        panel: &'static str,
        /// The corpus state being drawn.
        state: &'static str,
        /// The panel's own reason.
        #[source]
        source: PanelDrawError,
    },
    /// The caller's scratch buffer cannot hold a scene.
    #[error("scene scratch buffer of {len} bytes is too small")]
    Scratch {
        /// The offending buffer length.
        len: usize,
    },
}

/// Item-role tags framing the digest stream: every item carries its
/// role, so no cross-role collision can exist even in principle.
const ROLE_PANEL: u8 = 1;
const ROLE_STATE: u8 = 2;
const ROLE_SCENE: u8 = 3;
const ROLE_FRAME: u8 = 4;

/// Digests `registry` over the shared corpus plus each panel's own
/// extreme states, at every canonical frame each panel pins, drawing
/// into `scratch` (size it
/// [`indicate_instrument_scene::MAX_SCENE_BYTES`]).
pub fn scene_digest(registry: &Registry, scratch: &mut [u8]) -> Result<[u8; 32], DigestError> {
    let mut ctx = Sha256Ctx::new();
    ctx.update(SCENE_DIGEST_DOMAIN);
    ctx.update(&[SCENE_FORMAT_VERSION, v8::VERSION]);
    for panel in registry.panels() {
        digest_panel_contract(&mut ctx, panel);
        for state in CANONICAL_STATES {
            digest_state(&mut ctx, panel, state.id, (state.build)(), scratch)?;
        }
        for extreme in panel.extreme_states {
            digest_state(&mut ctx, panel, extreme.id, (extreme.build)(), scratch)?;
        }
    }
    Ok(ctx.finalize())
}

/// Binds the contract-relevant descriptor fields, not just the id: two
/// shells whose descriptors declare different required layers, groups,
/// frame ranges, background capability, or schemas are not showing the
/// same instruments even if their scene bytes agree.
///
/// Raster baselines stay out: they pin one backend's pixels, and a
/// deliberate re-pin there must not move cross-shell identity.
fn digest_panel_contract(ctx: &mut Sha256Ctx, panel: &PanelDescriptor) {
    update_framed(ctx, ROLE_PANEL, panel.id.as_bytes());
    ctx.update(&[panel.required_layers]);
    ctx.update(&panel.required_groups.bits().to_le_bytes());
    digest_frame(ctx, panel.frame_min);
    digest_frame(ctx, panel.frame_max);
    ctx.update(&panel.frame_step.0.to_le_bytes());
    ctx.update(&panel.frame_step.1.to_le_bytes());
    ctx.update(&panel.aspect_min.to_le_bytes());
    ctx.update(&panel.aspect_max.to_le_bytes());
    ctx.update(&(panel.canonical_frames.len() as u32).to_le_bytes());
    for frame in panel.canonical_frames {
        digest_frame(ctx, *frame);
    }
    ctx.update(&[match panel.background {
        BackgroundCapability::NotUsed => 0,
        BackgroundCapability::Opaque => 1,
        BackgroundCapability::Cedeable => 2,
    }]);
    ctx.update(&(panel.config_schema.len() as u32).to_le_bytes());
    for key in panel.config_schema {
        ctx.update(&key.0.to_le_bytes());
    }
}

fn digest_state(
    ctx: &mut Sha256Ctx,
    panel: &PanelDescriptor,
    state_id: &'static str,
    state: indicate_instrument_state::AircraftState,
    scratch: &mut [u8],
) -> Result<(), DigestError> {
    update_framed(ctx, ROLE_STATE, state_id.as_bytes());
    let data = resolve(&state, &FreshnessPolicy::default());
    for frame in panel.canonical_frames {
        digest_frame_scene(ctx, panel, state_id, &data, *frame, scratch)?;
    }
    Ok(())
}

fn digest_frame_scene(
    ctx: &mut Sha256Ctx,
    panel: &PanelDescriptor,
    state_id: &'static str,
    data: &indicate_instrument_state::PanelData,
    frame: DesignFrame,
    scratch: &mut [u8],
) -> Result<(), DigestError> {
    ctx.update(&[ROLE_FRAME]);
    digest_frame(ctx, frame);
    let scratch_len = scratch.len();
    let mut writer =
        SceneWriter::new(scratch).map_err(|_| DigestError::Scratch { len: scratch_len })?;
    (panel.draw)(data, &EMPTY_CONFIG, None, frame, &mut writer).map_err(|source| {
        DigestError::Draw {
            panel: panel.id,
            state: state_id,
            source,
        }
    })?;
    let used = writer.finish();
    let Some(scene) = scratch.get(..used) else {
        // A writer that reports more bytes than its buffer is broken;
        // digesting a truncated scene would silently misstate identity.
        return Err(DigestError::Scratch { len: scratch_len });
    };
    update_framed(ctx, ROLE_SCENE, scene);
    Ok(())
}

/// A frame is two fixed-width words, so it needs no length prefix; the
/// role tag that precedes a frame item keeps it from aliasing anything
/// else in the stream.
pub(crate) fn digest_frame(ctx: &mut Sha256Ctx, frame: DesignFrame) {
    ctx.update(&frame.width.to_le_bytes());
    ctx.update(&frame.height.to_le_bytes());
}

/// Role-tagged, length-prefixed (`u32` LE) update: framing keeps
/// adjacent fields and different item roles from aliasing each other.
pub(crate) fn update_framed(ctx: &mut Sha256Ctx, role: u8, bytes: &[u8]) {
    ctx.update(&[role]);
    ctx.update(&(bytes.len() as u32).to_le_bytes());
    ctx.update(bytes);
}

// The digest pin over the shipped panels lives in the panels crate
// (`descriptors/digest_tests.rs`): a dev-dependency back onto panels
// would duplicate this crate in the test graph and split its types.
#[cfg(test)]
mod tests;
