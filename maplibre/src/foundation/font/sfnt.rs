//! OpenType `sfnt` table assembly and checksum helpers shared by WOFF1/WOFF2 unpackers.

use super::FontError;

/// Maximum decompressed font size accepted when unpacking `WOFF` or `WOFF2` containers (128 MiB).
pub(super) const MAX_DECOMPRESSED_FONT_BYTES: usize = 128 * 1024 * 1024;

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
        if let Some(dst) = tail.get_mut(..rem.len()) {
            dst.copy_from_slice(rem);
        }
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

    let mut out = Vec::with_capacity(total_size);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&num_tables.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let mut head_offset: Option<usize> = None;
    let mut data_cursor = dir_size;
    let mut body = Vec::with_capacity(body_size);
    for mut table in tables {
        if &table.tag == b"head" {
            if let Some(slot) = table.data.get_mut(8..12) {
                // Zero `checkSumAdjustment` before computing `head`'s table checksum.
                slot.copy_from_slice(&[0, 0, 0, 0]);
                head_offset = Some(data_cursor);
            }
        }
        let checksum = sfnt_checksum(&table.data);
        let offset_u32 = u32::try_from(data_cursor)
            .map_err(|_| FontError::InvalidContainer("table offset overflow"))?;
        let length_u32 = u32::try_from(table.data.len())
            .map_err(|_| FontError::InvalidContainer("table length overflow"))?;

        out.extend_from_slice(&table.tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&offset_u32.to_be_bytes());
        out.extend_from_slice(&length_u32.to_be_bytes());

        body.extend_from_slice(&table.data);
        let padded_len = (body.len() + 3) & !3;
        body.resize(padded_len, 0);
        data_cursor += (table.data.len() + 3) & !3;
    }
    out.extend_from_slice(&body);

    if let Some(head_pos) = head_offset {
        let adjustment = 0xB1B0_AFBA_u32.wrapping_sub(sfnt_checksum(&out));
        if let Some(slot) = out.get_mut(head_pos + 8..head_pos + 12) {
            slot.copy_from_slice(&adjustment.to_be_bytes());
        }
    }

    Ok(out)
}
