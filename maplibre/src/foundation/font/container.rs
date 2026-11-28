//! Magic-header detection and container dispatch for `TTF`, `OTF`, `TTC`, `WOFF`, and `WOFF2`.

use std::borrow::Cow;

use super::FontError;

/// Binary container format of a font file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontContainerFormat {
    /// Raw TrueType outlines (`0x00010000`, `true`, or `typ1`).
    Ttf,
    /// Raw OpenType CFF outlines (`OTTO`).
    Otf,
    /// TrueType/OpenType Font Collection (`ttcf`).
    Ttc,
    /// Web Open Font Format 1.0 (`wOFF`, zlib-compressed tables).
    Woff,
    /// Web Open Font Format 2.0 (`wOF2`, Brotli-compressed stream + transformed `glyf`/`loca`).
    Woff2,
}

/// Inspects the 4-byte magic signature at the start of `bytes`.
pub fn detect_container_format(bytes: &[u8]) -> Option<FontContainerFormat> {
    let magic: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    match &magic {
        [0x00, 0x01, 0x00, 0x00] | b"true" | b"typ1" => Some(FontContainerFormat::Ttf),
        b"OTTO" => Some(FontContainerFormat::Otf),
        b"ttcf" => Some(FontContainerFormat::Ttc),
        b"wOFF" => Some(FontContainerFormat::Woff),
        b"wOF2" => Some(FontContainerFormat::Woff2),
        _ => None,
    }
}

/// Unpacks `bytes` into an OpenType/TrueType `sfnt` or `ttcf` byte buffer readable by
/// `read_fonts`, `skrifa`, and `harfrust`.
///
/// Raw `TTF`, `OTF`, and `TTC` slices are returned borrowed without copying; `WOFF` and `WOFF2`
/// containers are decompressed and reconstructed into valid `sfnt` buffers.
pub fn unpack_font_container(bytes: &[u8]) -> Result<Cow<'_, [u8]>, FontError> {
    let format = detect_container_format(bytes).ok_or(FontError::UnrecognizedFormat)?;
    match format {
        FontContainerFormat::Ttf | FontContainerFormat::Otf | FontContainerFormat::Ttc => {
            if bytes.len() < 12 {
                return Err(FontError::InvalidContainer("sfnt header truncated"));
            }
            Ok(Cow::Borrowed(bytes))
        }
        #[cfg(any(
            test,
            not(target_arch = "wasm32"),
            feature = "font-shaping",
            feature = "thread-safe-futures"
        ))]
        FontContainerFormat::Woff => super::woff1::unpack_woff1(bytes).map(Cow::Owned),
        #[cfg(not(any(
            test,
            not(target_arch = "wasm32"),
            feature = "font-shaping",
            feature = "thread-safe-futures"
        )))]
        FontContainerFormat::Woff => Err(FontError::InvalidContainer(
            "WOFF1 unpacking on wasm32 requires the font-shaping feature",
        )),
        #[cfg(feature = "font-shaping")]
        FontContainerFormat::Woff2 => super::woff2::unpack_woff2(bytes).map(Cow::Owned),
        #[cfg(not(feature = "font-shaping"))]
        FontContainerFormat::Woff2 => Err(FontError::InvalidContainer(
            "WOFF2 requires the font-shaping feature",
        )),
    }
}
