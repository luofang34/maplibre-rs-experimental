// Exported C names require an unsafe attribute. Values cross this ABI by copy;
// neither export dereferences a caller pointer or keeps shared mutable state.
#![allow(unsafe_code)]

use indicate_instrument_descriptor::{DesignFrame, EMPTY_CONFIG, PanelDescriptor};
use indicate_instrument_glyphs::PANEL_GLYPHS;
use indicate_instrument_hmd::{
    AngularScene, HMD_DESCRIPTOR, HMD_GLANCE_DESCRIPTOR, ViewReference, directions, view_reference,
};
use indicate_instrument_scene::SceneWriter;

use crate::telemetry::{ReplayTelemetry, resolve_replay};

/// Bounded, self-contained scene returned across the C ABI.
#[repr(C)]
pub struct OverlayScene {
    /// Encoded byte count. Zero means scene production failed; show a failure indication.
    pub length: u32,
    /// Storage for the complete scene. Bytes beyond `length` have no meaning.
    pub bytes: [u8; 8192],
}

/// Emits one independent SVS overlay from a resolved replay sample.
#[unsafe(no_mangle)]
pub extern "C" fn indicate_svs_render(input: ReplayTelemetry) -> OverlayScene {
    render(input, HMD_DESCRIPTOR)
}

/// Produces a compact panel for looking away from the aircraft axis.
#[unsafe(no_mangle)]
pub extern "C" fn indicate_svs_glance(input: ReplayTelemetry) -> OverlayScene {
    render(input, HMD_GLANCE_DESCRIPTOR)
}

fn render(input: ReplayTelemetry, descriptor: PanelDescriptor) -> OverlayScene {
    let mut output = OverlayScene {
        length: 0,
        bytes: [0; 8192],
    };
    let Ok(mut writer) = SceneWriter::new(&mut output.bytes) else {
        return output;
    };
    let frame = DesignFrame {
        width: 1200.0,
        height: 600.0,
    };
    if (descriptor.draw)(
        &resolve_replay(input),
        &EMPTY_CONFIG,
        None,
        frame,
        &mut writer,
    )
    .is_ok()
    {
        output.length = writer.finish() as u32;
    }
    output
}

/// Packs the verified five-by-seven Indicate glyph into seven low bytes.
/// Bit 63 indicates coverage; a zero result indicates an unavailable glyph.
#[unsafe(no_mangle)]
pub extern "C" fn indicate_svs_glyph(scalar: u32) -> u64 {
    let Some(glyph) = char::from_u32(scalar).and_then(|ch| PANEL_GLYPHS.lookup(ch)) else {
        return 0;
    };
    glyph
        .glyph
        .rows
        .iter()
        .enumerate()
        .fold(1_u64 << 63, |bits, (row, value)| {
            bits | (u64::from(*value) << (row * 8))
        })
}

/// Produces collimated true-world directions for either eye using the same resolved sample.
#[unsafe(no_mangle)]
pub extern "C" fn indicate_svs_directions(input: ReplayTelemetry) -> AngularScene {
    directions(&resolve_replay(input))
}

/// Resolves the instrument frame in true local coordinates, with an explicit fallback class.
#[unsafe(no_mangle)]
pub extern "C" fn indicate_svs_reference(input: ReplayTelemetry) -> ViewReference {
    view_reference(&resolve_replay(input))
}

/// Chooses compact instruments using the shared HWD visibility and attitude policy.
#[unsafe(no_mangle)]
pub extern "C" fn indicate_svs_compact(
    input: ReplayTelemetry,
    alignment_cosine: f32,
    was_compact: u32,
) -> u32 {
    u32::from(indicate_instrument_hmd::use_compact(
        &resolve_replay(input),
        alignment_cosine,
        was_compact != 0,
    ))
}
