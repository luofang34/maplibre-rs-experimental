//! Pure-Rust font container unpacking (`TTF`, `OTF`, `TTC`, `WOFF`, `WOFF2`), SDF glyph
//! rasterisation, OpenType complex text shaping (`harfrust`), `Style::font_faces` descriptors,
//! and system/in-memory font fallback.

use std::sync::{LazyLock, Mutex, MutexGuard};

mod cjk;
mod container;
mod sdf;
mod shaper;
mod spec;
mod system;
mod woff1;
#[cfg(feature = "font-shaping")]
mod woff2;

#[cfg(all(test, feature = "font-shaping"))]
pub(crate) mod tests;

pub use cjk::{is_complex_shaping_codepoint, is_local_cjk_codepoint};
pub use container::{detect_container_format, unpack_font_container, FontContainerFormat};
#[cfg(target_arch = "wasm32")]
pub(crate) use sdf::rasterize_canvas2d_chars_sdf;
pub use sdf::{
    rasterize_char_sdf, rasterize_glyph_sdf, RasterizedGlyph, SDF_BUFFER, SDF_CUTOFF, SDF_EM_SIZE,
    SDF_RADIUS,
};
pub use shaper::{face_glyph_atlas_key, GlyphKey, ShapedGlyph};
#[cfg(feature = "font-shaping")]
pub use shaper::{shape_slice_with_font, shape_text_runs};
pub use spec::{
    default_local_ideograph_font_family, deserialize_local_ideograph_font_family,
    is_default_local_ideograph_font_family, parse_font_stack_entry, FontFaceRule, FontFaces,
    FontStyle, FontWeight, UnicodeRange,
};
pub use system::{FontSystem, LoadedFace};

/// Errors returned while unpacking or registering font files.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FontError {
    /// Leading magic signature is none of `TTF`, `OTF`, `TTC`, `WOFF`, or `WOFF2`.
    #[error("unrecognized font container format")]
    UnrecognizedFormat,
    /// Structural or decompression failure inside a font container.
    #[error("invalid font container: {0}")]
    InvalidContainer(&'static str),
}

static GLOBAL_FONT_SYSTEM: LazyLock<Mutex<FontSystem>> =
    LazyLock::new(|| Mutex::new(FontSystem::new()));

/// Acquires the shared process-wide [`FontSystem`] used by symbol asset loading.
pub fn global_font_system() -> MutexGuard<'static, FontSystem> {
    crate::platform::lock_unpoisoned(&GLOBAL_FONT_SYSTEM)
}

/// Registers an in-memory `TTF`, `OTF`, `TTC`, `WOFF`, or `WOFF2` font into the shared
/// [`FontSystem`], optionally binding it under `family_override`.
pub fn register_font_bytes(
    family_override: Option<&str>,
    data: Vec<u8>,
) -> Result<Vec<u64>, FontError> {
    global_font_system().register_font_bytes(family_override, data)
}
