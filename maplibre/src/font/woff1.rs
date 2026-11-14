//! Pure-Rust WOFF 1.0 (`wOFF`) container decompression and `sfnt` reconstruction.

use std::io::Read;

use flate2::read::ZlibDecoder;

use super::{
    container::{assemble_sfnt, SfntTable, MAX_DECOMPRESSED_FONT_BYTES},
    FontError,
};

const WOFF1_HEADER_SIZE: usize = 44;
const WOFF1_TABLE_DIR_ENTRY_SIZE: usize = 20;

fn read_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Unpacks a WOFF 1.0 (`wOFF`) buffer into a standard OpenType/TrueType `sfnt` byte buffer.
pub(super) fn unpack_woff1(bytes: &[u8]) -> Result<Vec<u8>, FontError> {
    if bytes.len() < WOFF1_HEADER_SIZE || bytes.get(..4) != Some(b"wOFF") {
        return Err(FontError::InvalidContainer(
            "truncated or invalid WOFF1 header",
        ));
    }
    let flavor = read_u32_be(bytes, 4).ok_or(FontError::InvalidContainer("bad WOFF1 flavor"))?;
    let declared_len =
        read_u32_be(bytes, 8).ok_or(FontError::InvalidContainer("bad WOFF1 length"))? as usize;
    if declared_len > bytes.len() {
        return Err(FontError::InvalidContainer("WOFF1 length exceeds buffer"));
    }
    let num_tables =
        read_u16_be(bytes, 12).ok_or(FontError::InvalidContainer("bad WOFF1 numTables"))? as usize;
    let reserved =
        read_u16_be(bytes, 14).ok_or(FontError::InvalidContainer("bad WOFF1 reserved"))?;
    if num_tables == 0 || reserved != 0 {
        return Err(FontError::InvalidContainer(
            "WOFF1 header has zero tables or non-zero reserved field",
        ));
    }
    let total_sfnt_size = read_u32_be(bytes, 16)
        .ok_or(FontError::InvalidContainer("bad WOFF1 totalSfntSize"))?
        as usize;
    if total_sfnt_size > MAX_DECOMPRESSED_FONT_BYTES {
        return Err(FontError::InvalidContainer("WOFF1 totalSfntSize too large"));
    }

    let dir_end = WOFF1_HEADER_SIZE
        .checked_add(num_tables * WOFF1_TABLE_DIR_ENTRY_SIZE)
        .filter(|&end| end <= bytes.len())
        .ok_or(FontError::InvalidContainer(
            "WOFF1 table directory truncated",
        ))?;

    let mut tables = Vec::with_capacity(num_tables);
    let mut cumulative_uncompressed = 0usize;

    for idx in 0..num_tables {
        let base = WOFF1_HEADER_SIZE + idx * WOFF1_TABLE_DIR_ENTRY_SIZE;
        let tag_slice = bytes
            .get(base..base + 4)
            .ok_or(FontError::InvalidContainer("WOFF1 table tag truncated"))?;
        let tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
        let offset = read_u32_be(bytes, base + 4)
            .ok_or(FontError::InvalidContainer("WOFF1 table offset truncated"))?
            as usize;
        let comp_length = read_u32_be(bytes, base + 8)
            .ok_or(FontError::InvalidContainer("WOFF1 compLength truncated"))?
            as usize;
        let orig_length = read_u32_be(bytes, base + 12)
            .ok_or(FontError::InvalidContainer("WOFF1 origLength truncated"))?
            as usize;

        if comp_length > orig_length || offset < dir_end {
            return Err(FontError::InvalidContainer(
                "WOFF1 table has invalid compressed length or offset",
            ));
        }
        cumulative_uncompressed = cumulative_uncompressed
            .checked_add(orig_length)
            .filter(|&total| total <= MAX_DECOMPRESSED_FONT_BYTES)
            .ok_or(FontError::InvalidContainer(
                "WOFF1 uncompressed tables too large",
            ))?;

        let raw_end = offset
            .checked_add(comp_length)
            .filter(|&end| end <= bytes.len())
            .ok_or(FontError::InvalidContainer(
                "WOFF1 table payload out of bounds",
            ))?;
        let payload = &bytes[offset..raw_end];

        let data = if comp_length == orig_length {
            payload.to_vec()
        } else {
            let mut decoded = Vec::with_capacity(orig_length.min(65_536));
            let limit = u64::try_from(orig_length)
                .map_err(|_| FontError::InvalidContainer("WOFF1 table too large"))?
                .saturating_add(1);
            ZlibDecoder::new(payload)
                .take(limit)
                .read_to_end(&mut decoded)
                .map_err(|_| FontError::InvalidContainer("WOFF1 zlib decompression failed"))?;
            if decoded.len() != orig_length {
                return Err(FontError::InvalidContainer(
                    "WOFF1 decompressed table length mismatch",
                ));
            }
            decoded
        };
        tables.push(SfntTable { tag, data });
    }

    assemble_sfnt(flavor, tables)
}
