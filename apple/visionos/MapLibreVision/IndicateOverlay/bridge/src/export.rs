// Exported C names require an unsafe attribute. Values cross this ABI by copy;
// neither export dereferences a caller pointer or keeps shared mutable state.
#![allow(unsafe_code)]

use indicate_instrument_descriptor::{DesignFrame, EMPTY_CONFIG};
use indicate_instrument_glyphs::PANEL_GLYPHS;
use indicate_instrument_scene::SceneWriter;
use indicate_instrument_svs::SVS_DESCRIPTOR;

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
    if (SVS_DESCRIPTOR.draw)(
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
