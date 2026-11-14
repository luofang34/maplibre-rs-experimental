//! Magic-header detection and OpenType `sfnt` table packing for `TTF`, `OTF`, `TTC`, `WOFF`, and `WOFF2`.

use std::borrow::Cow;

use super::FontError;

/// Maximum decompressed font size accepted when unpacking `WOFF` or `WOFF2` containers (128 MiB).
pub(super) const MAX_DECOMPRESSED_FONT_BYTES: usize = 128 * 1024 * 1024;

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
        FontContainerFormat::Woff => super::woff1::unpack_woff1(bytes).map(Cow::Owned),
        FontContainerFormat::Woff2 => super::woff2::unpack_woff2(bytes).map(Cow::Owned),
    }
}

/// One decoded OpenType table ready to be written into a reconstructed `sfnt` buffer.
pub(super) struct SfntTable {
    pub tag: [u8; 4],
    pub data: Vec<u8>,
}

/// Computes the OpenType 32-bit big-endian table checksum over `data` (zero-padded to 4 bytes).
pub(super) fn sfnt_checksum(data: &[u8]) -> u32 {
    let mut sum = 0_u32;
    let (chunks, rem) = data.as_chunks::<4>();
    for &chunk in chunks {
        sum = sum.wrapping_add(u32::from_be_bytes(chunk));
    }
    if !rem.is_empty() {
        let mut tail = [0_u8; 4];
        tail[..rem.len()].copy_from_slice(rem);
        sum = sum.wrapping_add(u32::from_be_bytes(tail));
    }
    sum
}

/// Assembles `tables` with `flavor` into a canonical OpenType `sfnt` byte vector, sorting the
/// table directory by tag and updating `head.checkSumAdjustment` (`0xB1B0AFBA - total_checksum`).
pub(super) fn assemble_sfnt(flavor: u32, mut tables: Vec<SfntTable>) -> Result<Vec<u8>, FontError> {
    if tables.is_empty() || tables.len() > u16::MAX as usize {
        return Err(FontError::InvalidContainer("invalid sfnt table count"));
    }
    tables.sort_by_key(|table| table.tag);

    let num_tables = tables.len() as u16;
    let max_pow2 = 1_u16 << num_tables.ilog2();
    let search_range = max_pow2.saturating_mul(16);
    let entry_selector = num_tables.ilog2() as u16;
    let range_shift = num_tables.saturating_mul(16).saturating_sub(search_range);

    let dir_size = 12usize + usize::from(num_tables) * 16;
    let body_size: usize = tables
        .iter()
        .map(|t| (t.data.len() + 3) & !3)
        .try_fold(0usize, usize::checked_add)
        .ok_or(FontError::InvalidContainer("sfnt size overflow"))?;
    let total_size = dir_size
        .checked_add(body_size)
        .filter(|&size| size <= MAX_DECOMPRESSED_FONT_BYTES)
        .ok_or(FontError::InvalidContainer("sfnt exceeds maximum size"))?;

    let mut out = vec![0_u8; total_size];
    out[0..4].copy_from_slice(&flavor.to_be_bytes());
    out[4..6].copy_from_slice(&num_tables.to_be_bytes());
    out[6..8].copy_from_slice(&search_range.to_be_bytes());
    out[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    out[10..12].copy_from_slice(&range_shift.to_be_bytes());

    let mut head_offset: Option<usize> = None;
    let mut data_cursor = dir_size;
    for (idx, mut table) in tables.into_iter().enumerate() {
        if &table.tag == b"head" && table.data.len() >= 12 {
            // Zero `checkSumAdjustment` before computing `head`'s table checksum.
            table.data[8..12].copy_from_slice(&[0, 0, 0, 0]);
            head_offset = Some(data_cursor);
        }
        let checksum = sfnt_checksum(&table.data);
        let offset_u32 = u32::try_from(data_cursor)
            .map_err(|_| FontError::InvalidContainer("table offset overflow"))?;
        let length_u32 = u32::try_from(table.data.len())
            .map_err(|_| FontError::InvalidContainer("table length overflow"))?;

        let rec = 12 + idx * 16;
        out[rec..rec + 4].copy_from_slice(&table.tag);
        out[rec + 4..rec + 8].copy_from_slice(&checksum.to_be_bytes());
        out[rec + 8..rec + 12].copy_from_slice(&offset_u32.to_be_bytes());
        out[rec + 12..rec + 16].copy_from_slice(&length_u32.to_be_bytes());

        out[data_cursor..data_cursor + table.data.len()].copy_from_slice(&table.data);
        data_cursor += (table.data.len() + 3) & !3;
    }

    if let Some(head_pos) = head_offset {
        let adjustment = 0xB1B0_AFBA_u32.wrapping_sub(sfnt_checksum(&out));
        out[head_pos + 8..head_pos + 12].copy_from_slice(&adjustment.to_be_bytes());
    }

    Ok(out)
}
